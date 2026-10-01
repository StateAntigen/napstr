//! The network's catalogue, held here so a search does not have to ask for it.
//!
//! Relays are the source of truth for what exists on the network, and asking them
//! is slow for a plain reason: a search is a fan-out over several relays with a
//! timeout on each, and the answer is only as fast as the slowest one that
//! answers. That is fine for a lookup nobody has made before and wrong for a
//! search box, where the same words are typed over and over and the network's
//! state barely changes between them.
//!
//! So this keeps a mirror of that state locally: one row per track, one row per
//! (track, seeder), and a full-text index over the words a person searches. A
//! query is answered here in about a millisecond, and the relays are asked to
//! *keep the mirror fresh* rather than to answer each question.
//!
//! Two things this deliberately is not:
//!
//! * **It is not the record of an announcement.** `remote_catalogue` keeps one row
//!   per (file, author) announcement — the event id, and the size a transfer
//!   verifies the bytes against. That stays exactly as it is, because the transfer
//!   path depends on it. This is the *searchable* projection of the same events:
//!   one row per file, which is what makes a query local.
//! * **It is not availability.** Who can serve a file is a heartbeat with an
//!   expiry, and heartbeats are written in as they arrive. A seeder row is only a
//!   seeder while its expiry is in the future, so a count from here is as live as
//!   the network can make it — better than a snapshot that is five seconds old and
//!   fetched with a query of its own.
//!
//! A search only ever answers with tracks somebody is holding *right now*: a file
//! nobody is seeding is a file nobody can play, and a result list is a promise
//! that something can be fetched. Records that stop being announced are pruned,
//! so the mirror is a window on the network rather than a growing archive of it.

use rusqlite::{params, Connection};

/// How long a track stays in the mirror after the last time it was announced.
///
/// Long enough that a file which comes and goes is not re-fetched constantly, and
/// short enough that the mirror stays a window on the network rather than an
/// archive of everything it has ever said.
const TRACK_LIFETIME_SECONDS: i64 = 30 * 24 * 60 * 60;

/// The most seeders named for one result. A result row shows who can serve it;
/// naming hundreds of pubkeys for one file is bytes nobody reads.
const MAX_NAMED_SEEDERS: usize = 32;

/// The most words of a query that are used for matching. The rest are ignored,
/// exactly as the relay-side search ignores them: past a handful of words the
/// extra ones only narrow a result set that is already narrow.
const MAX_QUERY_TERMS: usize = 8;

/// Every table and index the mirror needs. Idempotent, like the rest of the
/// schema in this app.
pub(crate) fn initialise_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS catalogue_track (
               file_id BLOB PRIMARY KEY,
               title TEXT NOT NULL DEFAULT '',
               artist TEXT NOT NULL DEFAULT '',
               album TEXT NOT NULL DEFAULT '',
               filename TEXT NOT NULL DEFAULT '',
               tags TEXT NOT NULL DEFAULT '',
               format TEXT NOT NULL DEFAULT '',
               mime TEXT NOT NULL DEFAULT '',
               size INTEGER NOT NULL DEFAULT 0,
               license TEXT NOT NULL DEFAULT '',
               service TEXT NOT NULL DEFAULT 'audio',
               first_seen INTEGER NOT NULL,
               last_seen INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS catalogue_track_last_seen ON catalogue_track(last_seen);
             -- One row per (file, holder). `expires_at` is the heartbeat's own
             -- expiry, which is what makes a seeder a fact with a lifetime rather
             -- than a flag that is set and never cleared.
             CREATE TABLE IF NOT EXISTS catalogue_seeder (
               file_id BLOB NOT NULL,
               pubkey BLOB NOT NULL,
               expires_at INTEGER NOT NULL,
               seen_at INTEGER NOT NULL,
               PRIMARY KEY(file_id, pubkey)
             );
             CREATE INDEX IF NOT EXISTS catalogue_seeder_pubkey ON catalogue_seeder(pubkey);
             CREATE INDEX IF NOT EXISTS catalogue_seeder_expiry ON catalogue_seeder(expires_at);
             -- External content: the index holds the words, the table holds the
             -- rows, and the triggers below keep the two in step. Nothing has to
             -- remember to update the index by hand.
             CREATE VIRTUAL TABLE IF NOT EXISTS catalogue_fts USING fts5(
               title, artist, album, filename, tags,
               content='catalogue_track', content_rowid='rowid',
               tokenize = 'unicode61 remove_diacritics 2'
             );
             CREATE TRIGGER IF NOT EXISTS catalogue_track_indexed
             AFTER INSERT ON catalogue_track BEGIN
               INSERT INTO catalogue_fts(rowid,title,artist,album,filename,tags)
               VALUES (new.rowid,new.title,new.artist,new.album,new.filename,new.tags);
             END;
             -- Reindexed only when a word that is searched actually changed: a
             -- re-announcement of the same track moves `last_seen` and nothing
             -- else, and that should cost one row write rather than two.
             CREATE TRIGGER IF NOT EXISTS catalogue_track_reindexed
             AFTER UPDATE ON catalogue_track
             WHEN old.title IS NOT new.title OR old.artist IS NOT new.artist
               OR old.album IS NOT new.album OR old.filename IS NOT new.filename
               OR old.tags IS NOT new.tags BEGIN
               INSERT INTO catalogue_fts(catalogue_fts,rowid,title,artist,album,filename,tags)
               VALUES ('delete',old.rowid,old.title,old.artist,old.album,old.filename,old.tags);
               INSERT INTO catalogue_fts(rowid,title,artist,album,filename,tags)
               VALUES (new.rowid,new.title,new.artist,new.album,new.filename,new.tags);
             END;
             CREATE TRIGGER IF NOT EXISTS catalogue_track_unindexed
             AFTER DELETE ON catalogue_track BEGIN
               INSERT INTO catalogue_fts(catalogue_fts,rowid,title,artist,album,filename,tags)
               VALUES ('delete',old.rowid,old.title,old.artist,old.album,old.filename,old.tags);
             END;",
        )
        .map_err(|error| error.to_string())
}

/// One track, as a catalogue event describes it.
///
/// Borrowed rather than owned because the only caller has the fields in hand and
/// passes straight through to the insert.
pub(crate) struct Track<'a> {
    /// The file id as hex, which is what the wire and the events use.
    pub file_id: &'a str,
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub filename: &'a str,
    pub tags: &'a str,
    pub format: &'a str,
    pub mime: &'a str,
    pub size: i64,
    pub license: &'a str,
    /// `audio` for a track, `audiobook` for a chapter.
    pub service: &'a str,
}

/// One search result: the track, and who is holding it right now.
pub(crate) struct Hit {
    pub file_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub filename: String,
    pub format: String,
    pub mime: String,
    pub size: i64,
    pub tags: String,
    pub license: String,
    /// The pubkeys of the live seeders, newest heartbeat first, capped.
    pub seeders: Vec<String>,
}

/// The 32 bytes a file id names, or `None` when it is not a hash at all.
///
/// The mirror stores ids as their own bytes rather than as hex: it is a third of
/// the size, it is what the id *is*, and the conversion has to happen somewhere
/// anyway.
fn id_bytes(file_id: &str) -> Option<Vec<u8>> {
    let trimmed = file_id.trim();
    if trimmed.len() != 64 || !trimmed.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    hex::decode(trimmed).ok()
}

fn id_hex(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

/// Write a track into the mirror, or refresh what is already known about it.
///
/// An upsert rather than `INSERT OR REPLACE` on purpose: REPLACE deletes the row
/// it replaces, and SQLite only fires the delete trigger for that when recursive
/// triggers are on — which would leave the words of the old row in the index for
/// ever. The upsert fires the update trigger, which is the one that reindexes.
pub(crate) fn remember_track(
    connection: &Connection,
    track: &Track<'_>,
    now: i64,
) -> Result<(), String> {
    let Some(file_id) = id_bytes(track.file_id) else {
        return Err("that is not a file id".into());
    };
    connection
        .execute(
            "INSERT INTO catalogue_track
               (file_id,title,artist,album,filename,tags,format,mime,size,license,service,first_seen,last_seen)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?12)
             ON CONFLICT(file_id) DO UPDATE SET
               title=excluded.title, artist=excluded.artist, album=excluded.album,
               filename=excluded.filename, tags=excluded.tags, format=excluded.format,
               mime=excluded.mime, size=excluded.size, license=excluded.license,
               service=excluded.service, last_seen=excluded.last_seen",
            params![
                file_id,
                track.title,
                track.artist,
                track.album,
                track.filename,
                track.tags,
                track.format,
                track.mime,
                track.size,
                track.license,
                track.service,
                now
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Write a heartbeat: this author is holding these files until `expires_at`.
///
/// Separate from the track write because the two arrive separately and neither
/// implies the other. A heartbeat names file ids and says nothing about what they
/// are; a catalogue event describes a file and says nothing about who holds it.
///
/// Returns how many files were written, which is what a caller logging a
/// heartbeat wants to say.
pub(crate) fn remember_seeders(
    connection: &Connection,
    file_ids: &[String],
    seeder: &str,
    expires_at: i64,
    now: i64,
) -> Result<usize, String> {
    let Some(pubkey) = id_bytes(seeder) else {
        return Err("that is not a seeder's public key".into());
    };
    let mut written = 0;
    {
        let mut statement = connection
            .prepare(
                "INSERT INTO catalogue_seeder(file_id,pubkey,expires_at,seen_at)
                 VALUES (?1,?2,?3,?4)
                 ON CONFLICT(file_id,pubkey) DO UPDATE SET
                   expires_at=excluded.expires_at, seen_at=excluded.seen_at",
            )
            .map_err(|error| error.to_string())?;
        for file_id in file_ids {
            let Some(file_id) = id_bytes(file_id) else {
                continue;
            };
            // A heartbeat that has already expired says nothing worth keeping.
            if expires_at <= now {
                continue;
            }
            statement
                .execute(params![file_id, pubkey, expires_at, now])
                .map_err(|error| error.to_string())?;
            written += 1;
        }
    }
    Ok(written)
}

/// The words a query is matched on, as one FTS5 expression.
///
/// Every word has to appear, which is what a search means; the last one is a
/// prefix, because the thing being searched is usually still being typed. The
/// terms are quoted so that words like `AND` or `NEAR` are words rather than
/// syntax, and they are alphanumeric-only by construction, so nothing in a query
/// can escape into the expression.
fn match_expression(query: &str) -> Option<String> {
    let terms = query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .take(MAX_QUERY_TERMS)
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    if terms.is_empty() {
        return None;
    }
    let last = terms.len() - 1;
    Some(
        terms
            .iter()
            .enumerate()
            .map(|(index, term)| {
                if index == last && term.chars().count() >= 2 {
                    format!("\"{term}\"*")
                } else {
                    format!("\"{term}\"")
                }
            })
            .collect::<Vec<_>>()
            .join(" AND "),
    )
}

/// What every result row needs, in one place so search and browse cannot drift.
const HIT_COLUMNS: &str = "t.file_id, t.title, t.artist, t.album, t.filename, t.format, t.mime,
       t.size, t.tags, t.license,
       (SELECT COUNT(*) FROM catalogue_seeder s
         WHERE s.file_id = t.file_id AND s.expires_at > ?1) AS seeders,
       (SELECT GROUP_CONCAT(hex(s.pubkey)) FROM catalogue_seeder s
         WHERE s.file_id = t.file_id AND s.expires_at > ?1
         ORDER BY s.seen_at DESC) AS sources";

fn read_hit(row: &rusqlite::Row<'_>) -> rusqlite::Result<Hit> {
    let file_id: Vec<u8> = row.get(0)?;
    let sources: Option<String> = row.get(11)?;
    Ok(Hit {
        file_id: id_hex(&file_id),
        title: row.get(1)?,
        artist: row.get(2)?,
        album: row.get(3)?,
        filename: row.get(4)?,
        format: row.get(5)?,
        mime: row.get(6)?,
        size: row.get(7)?,
        tags: row.get(8)?,
        license: row.get(9)?,
        seeders: sources
            .unwrap_or_default()
            .split(',')
            .filter(|pubkey| !pubkey.is_empty())
            .take(MAX_NAMED_SEEDERS)
            .map(str::to_string)
            .collect(),
    })
}

/// Search the mirror, best first.
///
/// Ranked by how many people can serve it and then by how well it matches, which
/// is the order that matters: the point of a result is that somebody can play it.
/// A file nobody is holding is not a result, whatever the mirror remembers about
/// it.
///
/// Whole words first (with a prefix on the last one, so a search finds things
/// while the word is still being typed), and only if that finds nothing, a
/// substring pass — because a person searching for `nights` means `Midnights`,
/// and a whole-word index does not know that.
pub(crate) fn search(
    connection: &Connection,
    query: &str,
    limit: usize,
    now: i64,
) -> Result<Vec<Hit>, String> {
    let limit = limit.clamp(1, 1_000) as i64;
    if let Some(expression) = match_expression(query) {
        let mut statement = connection
            .prepare(&format!(
                "SELECT {HIT_COLUMNS}
                   FROM catalogue_fts f
                   JOIN catalogue_track t ON t.rowid = f.rowid
                  WHERE catalogue_fts MATCH ?2
                    AND (SELECT COUNT(*) FROM catalogue_seeder s
                          WHERE s.file_id = t.file_id AND s.expires_at > ?1) > 0
                  ORDER BY seeders DESC, bm25(catalogue_fts), t.title
                  LIMIT ?3"
            ))
            .map_err(|error| error.to_string())?;
        let hits = statement
            .query_map(params![now, expression, limit], read_hit)
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        if !hits.is_empty() {
            return Ok(hits);
        }
    }
    partial_search(connection, query, limit, now)
}

/// Every word of the query has to appear somewhere in the row, as a substring.
///
/// The second chance for a query the word index cannot answer: half a word, a
/// middle of one, or a spelling that only matches part of a token.
fn partial_search(
    connection: &Connection,
    query: &str,
    limit: i64,
    now: i64,
) -> Result<Vec<Hit>, String> {
    let tokens = query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .take(MAX_QUERY_TERMS)
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let mut clause = String::new();
    let mut values = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if index > 0 {
            clause.push_str(" AND ");
        }
        // `?1` is the moment every subquery in `HIT_COLUMNS` compares against, so
        // the words start at `?2`.
        clause.push_str(&format!(
            "(lower(t.title) LIKE ?{n} OR lower(t.artist) LIKE ?{n} OR lower(t.album) LIKE ?{n}
              OR lower(t.filename) LIKE ?{n} OR lower(t.tags) LIKE ?{n})",
            n = index + 2
        ));
        values.push(format!("%{token}%"));
    }
    let sql = format!(
        "SELECT {HIT_COLUMNS}
           FROM catalogue_track t
          WHERE {clause}
            AND (SELECT COUNT(*) FROM catalogue_seeder s
                  WHERE s.file_id = t.file_id AND s.expires_at > ?1) > 0
          ORDER BY seeders DESC, t.last_seen DESC
          LIMIT ?{limit_index}",
        limit_index = tokens.len() + 2
    );
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| error.to_string())?;
    let mut bound: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    bound.push(Box::new(now));
    for value in &values {
        bound.push(Box::new(value.clone()));
    }
    bound.push(Box::new(limit));
    let references = bound
        .iter()
        .map(|value| value.as_ref() as &dyn rusqlite::ToSql)
        .collect::<Vec<_>>();
    let hits = statement
        .query_map(references.as_slice(), read_hit)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(hits)
}

/// What the network is offering, most-served first.
///
/// The empty-query case: no words to match, so the only ordering that means
/// anything is how many people are holding each file.
pub(crate) fn browse(connection: &Connection, limit: usize, now: i64) -> Result<Vec<Hit>, String> {
    let limit = limit.clamp(1, 50_000) as i64;
    let mut statement = connection
        .prepare(&format!(
            "SELECT {HIT_COLUMNS}
               FROM catalogue_track t
              WHERE seeders > 0
              ORDER BY seeders DESC, t.last_seen DESC
              LIMIT ?2"
        ))
        .map_err(|error| error.to_string())?;
    let hits = statement
        .query_map(params![now, limit], read_hit)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(hits)
}

/// How many live seeders hold one file, and who they are.
///
/// The shape a caller wants when it has a file id from somewhere else — a
/// playlist member, a row on a screen — and needs to know whether it is playable
/// and by whom.
pub(crate) fn seeders_of(
    connection: &Connection,
    file_id: &str,
    now: i64,
) -> Result<Vec<String>, String> {
    let Some(file_id) = id_bytes(file_id) else {
        return Ok(Vec::new());
    };
    let mut statement = connection
        .prepare(
            "SELECT hex(pubkey) FROM catalogue_seeder
              WHERE file_id=?1 AND expires_at > ?2
              ORDER BY seen_at DESC LIMIT ?3",
        )
        .map_err(|error| error.to_string())?;
    let seeders = statement
        .query_map(params![file_id, now, MAX_NAMED_SEEDERS as i64], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(seeders)
}

/// How many tracks the mirror is describing, and how many heartbeats are live.
pub(crate) fn counts(connection: &Connection, now: i64) -> Result<(usize, usize), String> {
    let tracks: i64 = connection
        .query_row("SELECT COUNT(*) FROM catalogue_track", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    let live: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM catalogue_seeder WHERE expires_at > ?1",
            params![now],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    Ok((tracks.max(0) as usize, live.max(0) as usize))
}

/// Forget a file entirely, because somebody asked not to see it again.
pub(crate) fn forget_file(connection: &Connection, file_id: &str) -> Result<(), String> {
    let Some(file_id) = id_bytes(file_id) else {
        return Ok(());
    };
    connection
        .execute("DELETE FROM catalogue_seeder WHERE file_id=?1", params![file_id])
        .map_err(|error| error.to_string())?;
    connection
        .execute("DELETE FROM catalogue_track WHERE file_id=?1", params![file_id])
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Forget that an author holds anything.
///
/// Only their heartbeats go. What a file *is* is the network's description rather
/// than theirs, and if they are the only one holding it then the file stops being
/// a result anyway — which is the honest reading of "do not show me this person".
pub(crate) fn forget_author(connection: &Connection, pubkey: &str) -> Result<(), String> {
    let Some(pubkey) = id_bytes(pubkey) else {
        return Ok(());
    };
    connection
        .execute("DELETE FROM catalogue_seeder WHERE pubkey=?1", params![pubkey])
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Drop what has gone: heartbeats that have expired, and tracks nobody has
/// announced in a month.
///
/// Returns how many of each went, because a person watching a mirror wants to
/// know it is being kept honest rather than only appended to.
pub(crate) fn prune(connection: &Connection, now: i64) -> Result<(usize, usize), String> {
    let seeders = connection
        .execute(
            "DELETE FROM catalogue_seeder WHERE expires_at <= ?1",
            params![now],
        )
        .map_err(|error| error.to_string())?;
    let tracks = connection
        .execute(
            "DELETE FROM catalogue_track WHERE last_seen <= ?1",
            params![now - TRACK_LIFETIME_SECONDS],
        )
        .map_err(|error| error.to_string())?;
    Ok((seeders, tracks))
}

/// Whether a set of file ids are worth asking the network about, given what the
/// mirror already knows.
///
/// This is what keeps a backfill polite: the ids somebody is holding are asked
/// about once, and an id already described (or described recently enough) is not
/// asked for again.
pub(crate) fn missing_tracks(
    connection: &Connection,
    file_ids: &[String],
    limit: usize,
) -> Result<Vec<String>, String> {
    let mut missing = Vec::new();
    let mut seen = std::collections::HashSet::new();
    {
        let mut statement = connection
            .prepare("SELECT 1 FROM catalogue_track WHERE file_id=?1")
            .map_err(|error| error.to_string())?;
        for file_id in file_ids {
            if missing.len() >= limit {
                break;
            }
            let Some(bytes) = id_bytes(file_id) else {
                continue;
            };
            if !seen.insert(file_id.clone()) {
                continue;
            }
            let known = statement
                .exists(params![bytes])
                .map_err(|error| error.to_string())?;
            if !known {
                missing.push(file_id.clone());
            }
        }
    }
    Ok(missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        crate::network::initialise_network_schema(&connection).unwrap();
        connection
    }

    fn hex_id(seed: u8) -> String {
        format!("{:02x}", seed).repeat(32)
    }

    fn track<'a>(file_id: &'a str, title: &'a str, artist: &'a str, album: &'a str) -> Track<'a> {
        Track {
            file_id,
            title,
            artist,
            album,
            filename: format!("{title}.flac").leak(),
            tags: "napstr rock",
            format: "FLAC",
            mime: "audio/flac",
            size: 42_000_000,
            license: "CC0-1.0",
            service: "audio",
        }
    }

    fn holding(connection: &Connection, file_id: &str, seeder: u8, now: i64) {
        remember_seeders(
            connection,
            &[file_id.to_string()],
            &hex_id(seeder),
            now + 600,
            now,
        )
        .unwrap();
    }

    #[test]
    fn a_track_is_found_by_its_words() {
        let connection = database();
        let now = 1_700_000_000;
        remember_track(&connection, &track(&hex_id(1), "Midnight City", "M83", "Hurry Up"), now)
            .unwrap();
        remember_track(&connection, &track(&hex_id(2), "Ghost Song", "Other", "Album"), now)
            .unwrap();
        holding(&connection, &hex_id(1), 9, now);
        holding(&connection, &hex_id(2), 9, now);

        let hits = search(&connection, "midnight", 50, now).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file_id, hex_id(1));
        assert_eq!(hits[0].title, "Midnight City");
        assert_eq!(hits[0].seeders, vec![hex_id(9)]);
    }

    #[test]
    fn a_file_nobody_is_holding_is_not_a_result() {
        // A result list is a promise that something can be played, so a track the
        // mirror remembers with no live seeder is not in it. This is also what
        // makes blocking work: dropping a blocked author's heartbeats takes their
        // files out of the results without touching what the files are.
        let connection = database();
        let now = 1_700_000_000;
        remember_track(&connection, &track(&hex_id(1), "Midnight City", "M83", "Hurry Up"), now)
            .unwrap();
        assert!(search(&connection, "midnight", 50, now).unwrap().is_empty());
        assert!(browse(&connection, 50, now).unwrap().is_empty());

        holding(&connection, &hex_id(1), 9, now);
        assert_eq!(search(&connection, "midnight", 50, now).unwrap().len(), 1);

        // The heartbeat expires and the file stops being available, without the
        // mirror forgetting what the track is.
        let later = now + 601;
        assert!(search(&connection, "midnight", 50, later).unwrap().is_empty());
        let (seeders, tracks) = prune(&connection, later).unwrap();
        assert_eq!((seeders, tracks), (1, 0));
    }

    #[test]
    fn what_more_people_hold_comes_first() {
        let connection = database();
        let now = 1_700_000_000;
        remember_track(&connection, &track(&hex_id(1), "Ghost", "Solo", "One"), now).unwrap();
        remember_track(&connection, &track(&hex_id(2), "Ghost", "Crowd", "Two"), now).unwrap();
        holding(&connection, &hex_id(1), 9, now);
        for seeder in 1..5u8 {
            holding(&connection, &hex_id(2), seeder, now);
        }
        let hits = search(&connection, "ghost", 50, now).unwrap();
        assert_eq!(
            hits.iter().map(|hit| hit.file_id.as_str()).collect::<Vec<_>>(),
            vec![hex_id(2), hex_id(1)],
            "four people can serve the second one"
        );
        assert_eq!(hits[0].seeders.len(), 4);
    }

    #[test]
    fn half_a_word_still_finds_it() {
        let connection = database();
        let now = 1_700_000_000;
        remember_track(&connection, &track(&hex_id(1), "Midnights", "Taylor Swift", "Midnights"), now)
            .unwrap();
        holding(&connection, &hex_id(1), 9, now);
        // A whole-word index cannot answer this; the substring pass can.
        let hits = search(&connection, "nights", 50, now).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Midnights");
        // And so can a prefix, which is what a search box needs while it is being
        // typed into.
        assert_eq!(search(&connection, "midn", 50, now).unwrap().len(), 1);
    }

    #[test]
    fn a_re_announced_track_replaces_what_the_index_knew() {
        let connection = database();
        let now = 1_700_000_000;
        remember_track(&connection, &track(&hex_id(1), "Wrong Title", "M83", "Hurry Up"), now)
            .unwrap();
        holding(&connection, &hex_id(1), 9, now);
        assert_eq!(search(&connection, "wrong", 50, now).unwrap().len(), 1);

        remember_track(&connection, &track(&hex_id(1), "Midnight City", "M83", "Hurry Up"), now + 10)
            .unwrap();
        assert!(
            search(&connection, "wrong", 50, now).unwrap().is_empty(),
            "the words of the row it replaced have to leave the index with it"
        );
        assert_eq!(search(&connection, "midnight", 50, now).unwrap().len(), 1);
    }

    #[test]
    fn a_track_nobody_says_anything_about_for_a_month_is_forgotten() {
        let connection = database();
        let now = 1_700_000_000;
        remember_track(&connection, &track(&hex_id(1), "Ghost", "Solo", "One"), now).unwrap();
        holding(&connection, &hex_id(1), 9, now);
        let soon = now + 60;
        assert_eq!(prune(&connection, soon).unwrap(), (0, 0));

        let late = now + TRACK_LIFETIME_SECONDS + 1;
        let (seeders, tracks) = prune(&connection, late).unwrap();
        assert_eq!(seeders, 1);
        assert_eq!(tracks, 1);
        assert!(search(&connection, "ghost", 50, late).unwrap().is_empty());
        assert_eq!(track_count(&connection).unwrap(), 0);
    }

    #[test]
    fn an_author_who_asked_not_to_be_shown_stops_holding_anything() {
        let connection = database();
        let now = 1_700_000_000;
        remember_track(&connection, &track(&hex_id(1), "Ghost", "Solo", "One"), now).unwrap();
        holding(&connection, &hex_id(1), 7, now);
        holding(&connection, &hex_id(1), 8, now);
        assert_eq!(search(&connection, "ghost", 50, now).unwrap()[0].seeders.len(), 2);

        forget_author(&connection, &hex_id(7)).unwrap();
        let hits = search(&connection, "ghost", 50, now).unwrap();
        assert_eq!(hits[0].seeders, vec![hex_id(8)], "the other seeder is untouched");

        forget_author(&connection, &hex_id(8)).unwrap();
        assert!(
            search(&connection, "ghost", 50, now).unwrap().is_empty(),
            "nobody is holding it now, so it is not a result"
        );
        assert_eq!(track_count(&connection).unwrap(), 1, "the track is still known");

        forget_file(&connection, &hex_id(1)).unwrap();
        assert_eq!(track_count(&connection).unwrap(), 0);
    }

    #[test]
    fn a_heartbeat_that_has_already_expired_is_not_kept() {
        let connection = database();
        let now = 1_700_000_000;
        remember_track(&connection, &track(&hex_id(1), "Ghost", "Solo", "One"), now).unwrap();
        let written = remember_seeders(
            &connection,
            &[hex_id(1)],
            &hex_id(9),
            now - 1,
            now,
        )
        .unwrap();
        assert_eq!(written, 0);
        assert!(search(&connection, "ghost", 50, now).unwrap().is_empty());
    }

    #[test]
    fn what_the_mirror_does_not_have_is_what_gets_asked_for() {
        let connection = database();
        let now = 1_700_000_000;
        remember_track(&connection, &track(&hex_id(1), "Ghost", "Solo", "One"), now).unwrap();
        let missing = missing_tracks(
            &connection,
            &[
                hex_id(1).to_string(),
                hex_id(2).to_string(),
                hex_id(2).to_string(),
                "not-a-hash".into(),
            ],
            10,
        )
        .unwrap();
        assert_eq!(missing, vec![hex_id(2)], "once, and only the one nobody described");
    }

    #[test]
    fn an_id_that_is_not_a_hash_is_refused_rather_than_stored() {
        let connection = database();
        let now = 1_700_000_000;
        assert!(remember_track(&connection, &track("nope", "Ghost", "Solo", "One"), now).is_err());
        assert!(remember_seeders(&connection, &[hex_id(1)], "nope", now + 600, now).is_err());
        assert_eq!(track_count(&connection).unwrap(), 0);
    }

    fn track_count(connection: &Connection) -> Result<usize, String> {
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM catalogue_track", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        Ok(count.max(0) as usize)
    }

    #[test]
    fn a_query_of_nothing_but_punctuation_matches_nothing_rather_than_everything() {
        assert_eq!(match_expression("!!!"), None);
        assert_eq!(match_expression("  "), None);
        // The last word is a prefix, so a search finds a word while it is still
        // being typed into the box.
        assert_eq!(match_expression("Ghost").as_deref(), Some("\"ghost\"*"));
        assert_eq!(
            match_expression("midnight city").as_deref(),
            Some("\"midnight\" AND \"city\"*")
        );
        // Words that are FTS syntax are words.
        assert_eq!(match_expression("AND").as_deref(), Some("\"and\"*"));
    }
}
