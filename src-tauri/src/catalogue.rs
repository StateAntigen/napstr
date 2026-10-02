//! The network's catalogue, indexed locally, and who is holding what.
//!
//! Relays are the source of truth, and asking them is slow for a plain reason: a
//! search is a fan-out over several relays with a timeout on each, so it is only
//! as fast as the slowest one that answers. That is the right shape for a question
//! nobody has asked before and the wrong one for a search box, where the same
//! words are typed over and over while the network's state barely moves.
//!
//! So this module owns two things around the record that already exists:
//!
//! * **A full-text index** over the words a search matches, so a query is an index
//!   lookup instead of a scan of the newest twenty-five thousand rows.
//! * **The heartbeats**, kept as rows carrying their own expiry rather than as a
//!   five-second snapshot in memory, so who can serve a file survives a restart and
//!   is as live as the network can make it.
//!
//! Why the index is over `remote_catalogue` rather than over a table of its own: a
//! result's *sources* are authors who announced the file **and** are holding it
//! right now. That pairing is not a detail — [`crate::network`]'s
//! `request_download` looks up `(file_id, source_pubkey)` in `remote_catalogue` to
//! find the size a transfer verifies the bytes against, and a report cites the
//! source's own event id. So the searchable unit has to be the announcement. A
//! normalised copy of the network would be about a third smaller and would have to
//! invent a per-announcement identity to keep both of those honest.
//!
//! One invariant everything else follows from: **a result is a file somebody is
//! holding right now.** A file the catalogue remembers with no live seeder is not a
//! result, whatever is known about it.

use std::collections::HashSet;

use rusqlite::{params, Connection};

/// The most seeders named for one result. A row shows who can serve a file;
/// naming hundreds of pubkeys for one file is bytes nobody reads.
const MAX_NAMED_SEEDERS: usize = 32;

/// The most words of a query that are matched on. Past a handful, extra words only
/// narrow a result set that is already narrow, which is what the relay-side search
/// does with them too.
const MAX_QUERY_TERMS: usize = 8;

/// What this module remembers about its own index, so the one-off walk below
/// happens once per database instead of on every start.
///
/// `-1` is "the announcements that were already stored have not been indexed yet",
/// which is also what an upgrade reads as: a database created before any of this
/// existed has the row inserted with the default. `0` is a legitimate answer — a
/// fresh installation whose catalogue was empty when it was marked — so the two
/// cannot share a value.
const CATALOGUE_STATE_TABLE: &str = "\
  CREATE TABLE IF NOT EXISTS catalogue_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    announcements_indexed INTEGER NOT NULL DEFAULT -1
  );
  INSERT OR IGNORE INTO catalogue_state(id, announcements_indexed) VALUES (1, -1);";

/// Every table, index and trigger this module needs. Idempotent, like the rest of
/// the schema in this app.
pub(crate) fn initialise_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(CATALOGUE_STATE_TABLE)
        .map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS catalogue_seeder (
               file_id TEXT NOT NULL,
               pubkey TEXT NOT NULL,
               expires_at INTEGER NOT NULL,
               seen_at INTEGER NOT NULL,
               PRIMARY KEY(file_id, pubkey)
             );
             CREATE INDEX IF NOT EXISTS catalogue_seeder_pubkey ON catalogue_seeder(pubkey);
             CREATE INDEX IF NOT EXISTS catalogue_seeder_expiry ON catalogue_seeder(expires_at);
             -- What the backfill has asked the network about and heard nothing for,
             -- so a question that keeps being answered with silence cannot hold the
             -- queue still on the first page of files.
             CREATE TABLE IF NOT EXISTS catalogue_asked (
               file_id TEXT PRIMARY KEY,
               asked_at INTEGER NOT NULL,
               next_ask INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS catalogue_asked_next ON catalogue_asked(next_ask);
             -- External content: the index holds the words, `remote_catalogue`
             -- holds the rows, and the triggers below keep the two in step.
             CREATE VIRTUAL TABLE IF NOT EXISTS remote_catalogue_fts USING fts5(
               filename, title, artist, album, tags,
               content='remote_catalogue', content_rowid='rowid',
               tokenize = 'unicode61 remove_diacritics 2'
             );
             CREATE TRIGGER IF NOT EXISTS remote_catalogue_indexed
             AFTER INSERT ON remote_catalogue BEGIN
               INSERT INTO remote_catalogue_fts(rowid,filename,title,artist,album,tags)
               VALUES (new.rowid,new.filename,new.title,new.artist,new.album,new.tags);
             END;
             -- Reindexed only when a word that is searched actually changed.
             CREATE TRIGGER IF NOT EXISTS remote_catalogue_reindexed
             AFTER UPDATE ON remote_catalogue
             WHEN old.filename IS NOT new.filename OR old.title IS NOT new.title
               OR old.artist IS NOT new.artist OR old.album IS NOT new.album
               OR old.tags IS NOT new.tags BEGIN
               INSERT INTO remote_catalogue_fts(remote_catalogue_fts,rowid,filename,title,artist,album,tags)
               VALUES ('delete',old.rowid,old.filename,old.title,old.artist,old.album,old.tags);
               INSERT INTO remote_catalogue_fts(rowid,filename,title,artist,album,tags)
               VALUES (new.rowid,new.filename,new.title,new.artist,new.album,new.tags);
             END;
             CREATE TRIGGER IF NOT EXISTS remote_catalogue_unindexed
             AFTER DELETE ON remote_catalogue BEGIN
               INSERT INTO remote_catalogue_fts(remote_catalogue_fts,rowid,filename,title,artist,album,tags)
               VALUES ('delete',old.rowid,old.filename,old.title,old.artist,old.album,old.tags);
             END;",
        )
        .map_err(|error| error.to_string())?;
    if let Err(error) = index_stored_announcements(connection) {
        // A catalogue whose words could not all be indexed is slower to search,
        // not broken: a search the index cannot answer falls through to the
        // substring pass and then to the network, and the next start tries again
        // because the marker is only written when the count came out right.
        eprintln!("Could not index the stored catalogue: {error}");
    }
    Ok(())
}

/// Index what is already stored, once.
///
/// An installation that had a catalogue before this index existed answers a search
/// for anything in it from the substring fallback: slower, and blind past the page
/// of rows that pass reads. This is what walks that catalogue into the index.
///
/// **A bare count over an external-content index is not a count of the index.** An
/// external-content table answers a plain `SELECT` with the content table's rows,
/// so `SELECT COUNT(*) FROM remote_catalogue_fts` returns the announcement count
/// whatever the index holds. Measured on a real installation it said 34,792 while
/// the index held 2,399 documents — which is how this function's predecessor
/// concluded there was nothing to do, and left a day of searches to the fallback
/// without saying so. The documents are counted instead, one per indexed
/// announcement, and the pass is only marked done when the two agree, so an
/// attempt that did not take is made again on the next start.
fn index_stored_announcements(connection: &Connection) -> Result<i64, String> {
    let already = connection
        .query_row(
            "SELECT announcements_indexed FROM catalogue_state WHERE id=1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0);
    if already >= 0 {
        return Ok(0);
    }
    let stored = count(connection, "SELECT COUNT(*) FROM remote_catalogue")?;
    if stored > 0 {
        rebuild_index(connection)?;
    }
    let documents = count(connection, "SELECT COUNT(*) FROM remote_catalogue_fts_docsize")?;
    if documents < stored {
        return Err(format!(
            "the word index holds {documents} of {stored} announcements"
        ));
    }
    connection
        .execute(
            "UPDATE catalogue_state SET announcements_indexed=?1 WHERE id=1",
            params![stored],
        )
        .map_err(|error| error.to_string())?;
    Ok(stored)
}

/// How many rows a count answers, for the two counts this module compares.
fn count(connection: &Connection, sql: &str) -> Result<i64, String> {
    connection
        .query_row(sql, [], |row| row.get::<_, i64>(0))
        .map_err(|error| error.to_string())
}

/// Build the word index again from the announcements it describes.
///
/// The index is derived data: everything in it came from `remote_catalogue`, so
/// there is nothing to lose by throwing it away and reading the table again. That
/// is what makes a damaged index a repair rather than a failure, and it is why
/// callers are willing to reach for this on an error they cannot explain.
///
/// Not free on a large catalogue — it reads the whole table — so it belongs after
/// something has already failed, never on a timer or on the way to a result.
pub(crate) fn rebuild_index(connection: &Connection) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO remote_catalogue_fts(remote_catalogue_fts) VALUES('rebuild')",
            [],
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Whether the word index passes its own integrity check.
///
/// Better to ask than to infer. A damaged index reports itself in the words of
/// whatever statement happened to reach it — measured against a deliberately
/// damaged one, a `MATCH` says the database is malformed while an insert into the
/// index can say a constraint failed — so the decision to rebuild is made by
/// asking the index, and only after something has already gone wrong, which is why
/// this is cheap enough to call on a failure.
///
/// A *stale* index — rows whose announcements were deleted without the triggers —
/// is not damage and this does not report it: it quietly answers with more rows
/// than the table holds, and the join to `remote_catalogue` drops them.
pub(crate) fn index_is_sound(connection: &Connection) -> bool {
    connection
        .execute(
            "INSERT INTO remote_catalogue_fts(remote_catalogue_fts) VALUES('integrity-check')",
            [],
        )
        .is_ok()
}

/// One search result: an announcement, and who is holding it right now.
pub(crate) struct Hit {
    pub file_id: String,
    pub filename: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub format: String,
    pub mime: String,
    pub size: u64,
    pub license: String,
    pub tags: String,
    /// One per live seeder: the author, and the id of *their* announcement.
    ///
    /// Together rather than separately because they have to agree — a report
    /// names both, and a download is verified against the announcement that
    /// author published.
    pub sources: Vec<(String, String)>,
}

/// Whether a string is a file id or a public key: 32 bytes, as hex.
fn is_hash(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.len() == 64 && trimmed.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Write a heartbeat: this author is holding these files until `expires_at`.
///
/// A heartbeat names file ids and says nothing about what they are, which is why
/// this is separate from the catalogue writes — neither implies the other. Rows
/// carry the heartbeat's own expiry, so a seeder is a fact with a lifetime rather
/// than a flag that is set once and never cleared.
///
/// Runs inside whatever transaction the caller has open. Called outside one, each
/// row is its own commit — which for a heartbeat naming a few hundred files is a
/// few hundred disk flushes, so a caller storing many of them should open one.
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
    let seeder = seeder.trim().to_lowercase();
    if !is_hash(&seeder) {
        return Err("that is not a seeder's public key".into());
    }
    // A heartbeat that has already expired says nothing worth keeping.
    if expires_at <= now {
        return Ok(0);
    }
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
        let mut seen = HashSet::new();
        for file_id in file_ids {
            let file_id = file_id.trim().to_lowercase();
            if !is_hash(&file_id) || !seen.insert(file_id.clone()) {
                continue;
            }
            statement
                .execute(params![file_id, seeder, expires_at, now])
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

/// What every result row needs, in one place so the three queries cannot drift.
///
/// The join is the whole of the availability rule: a seeder counts when its
/// heartbeat is live **and** it is the author of the announcement being shown,
/// which is the same pairing the relay path produces and the one a download
/// depends on. Grouping by file folds a file announced by several people into one
/// row, exactly as the merge on the live path does, and the bare columns come from
/// the newest announcement that matched.
///
/// There is no relevance column, and that is a limitation rather than a choice:
/// SQLite refuses `bm25()` from any query that does not read the index one row at
/// a time, and the folding above is a grouped query. Finding the words is what the
/// index is for; the order is *how many people can serve it, then how recently
/// they said so*, which is what `browse` and the substring pass use too.
const HIT_COLUMNS: &str = "c.file_id,
       c.filename, c.title, c.artist, c.album, c.format, c.mime, c.size, c.license, c.tags,
       MAX(c.seen_at) AS newest,
       COUNT(s.pubkey) AS seeders,
       GROUP_CONCAT(s.pubkey || '|' || c.event_id) AS sources";

/// The join every query makes against the index, and the availability rule in one
/// place: a seeder counts when its heartbeat is live and it is the author of the
/// announcement being shown.
const LIVE_SEEDERS: &str = "JOIN catalogue_seeder s
             ON s.file_id = c.file_id AND s.pubkey = c.source_pubkey AND s.expires_at > ?1";

fn read_hit(row: &rusqlite::Row<'_>) -> rusqlite::Result<Hit> {
    let sources: Option<String> = row.get(12)?;
    Ok(Hit {
        file_id: row.get(0)?,
        filename: row.get(1)?,
        title: row.get(2)?,
        artist: row.get(3)?,
        album: row.get(4)?,
        format: row.get(5)?,
        mime: row.get(6)?,
        size: row.get::<_, i64>(7)?.max(0) as u64,
        license: row.get(8)?,
        tags: row.get(9)?,
        sources: sources
            .unwrap_or_default()
            .split(',')
            .filter(|pair| !pair.is_empty())
            .take(MAX_NAMED_SEEDERS)
            .filter_map(|pair| pair.split_once('|'))
            .map(|(pubkey, event_id)| (pubkey.to_string(), event_id.to_string()))
            .collect(),
    })
}

/// Search the index, best first.
///
/// Ranked by how many people can serve it and then by how recently they said so,
/// which is the order that matters: the point of a result is that somebody can
/// play it, and the freshest announcement of a file is the likeliest to still be
/// true.
///
/// Whole words first, with a prefix on the last one so a search box answers while
/// a word is still being typed; and only if that finds nothing, a substring pass —
/// because a person searching for `nights` means `Midnights`, and a word index
/// does not know that.
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
                   FROM remote_catalogue_fts
                   JOIN remote_catalogue c ON c.rowid = remote_catalogue_fts.rowid
                   {LIVE_SEEDERS}
                  WHERE remote_catalogue_fts MATCH ?2
                  GROUP BY c.file_id
                  ORDER BY seeders DESC, newest DESC
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

/// Every word of the query has to appear in the row, as a substring.
///
/// The second chance for a query the word index cannot answer: half a word, the
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
        // `?1` is the moment the join compares against, so the words start at `?2`.
        clause.push_str(&format!(
            "(lower(c.filename) LIKE ?{n} OR lower(c.title) LIKE ?{n} OR lower(c.artist) LIKE ?{n}
              OR lower(c.album) LIKE ?{n} OR lower(c.tags) LIKE ?{n})",
            n = index + 2
        ));
        values.push(format!("%{token}%"));
    }
    let sql = format!(
        "SELECT {HIT_COLUMNS}
           FROM remote_catalogue c
           {LIVE_SEEDERS}
          WHERE {clause}
          GROUP BY c.file_id
          ORDER BY seeders DESC, newest DESC
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

/// The live files this catalogue cannot describe yet, so the network is worth
/// asking about.
///
/// This is what keeps a backfill polite: a file somebody is holding is asked about
/// once, a file already described is not asked about again, and a file the network
/// has already said nothing about is left alone for a while. Asked as one query
/// rather than as a lookup per live file, because the live set is the size of the
/// network and what is wanted from it is small.
///
/// Two of those exclusions are load-bearing rather than tidy:
///
/// * **Silence is remembered.** Without it the backfill asked the same fifteen
///   hundred files every minute and never reached the rest: nothing about a
///   question that is answered with silence ever changes, and the live set is
///   bigger than one page. Measured on a real installation: 4,885 files were
///   never described, four fifths of them by holders that had never announced
///   them at all, and the count did not move all day.
/// * **A file this computer holds without having published is never asked about.**
///   The same rule `network::unpublished_holds` applies to a batch a phone named:
///   the question names the bytes, and asking would say this computer has them,
///   which is what not publishing them kept quiet. Reads the main schema's `files`
///   and `published_catalogue`, which is where that state lives.
pub(crate) fn undescribed_live_files(
    connection: &Connection,
    now: i64,
    limit: usize,
) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(
            "SELECT DISTINCT s.file_id FROM catalogue_seeder s
              WHERE s.expires_at > ?1
                AND NOT EXISTS (SELECT 1 FROM remote_catalogue c WHERE c.file_id = s.file_id)
                AND NOT EXISTS (SELECT 1 FROM catalogue_asked a
                                 WHERE a.file_id = s.file_id AND a.next_ask > ?1)
                AND NOT EXISTS (SELECT 1 FROM files h
                                 WHERE h.file_id = s.file_id
                                   AND NOT EXISTS (SELECT 1 FROM published_catalogue p
                                                    WHERE p.file_id = h.file_id))
              LIMIT ?2",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![now, limit.clamp(1, 10_000) as i64], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

/// Which of these files the catalogue still cannot describe.
///
/// Asked of a backfill once it has written what it fetched: what is left is what
/// the network answered with silence, which is what the cool-off is recorded
/// against. A file whose announcement came back but was refused counts as silent
/// too, and rightly so — nothing usable was learned, and asking again in six hours
/// is a reasonable thing to do about that.
pub(crate) fn still_undescribed(
    connection: &Connection,
    file_ids: &[String],
) -> Result<Vec<String>, String> {
    let mut missing = Vec::new();
    let mut seen = HashSet::new();
    {
        let mut statement = connection
            .prepare("SELECT 1 FROM remote_catalogue WHERE file_id=?1 LIMIT 1")
            .map_err(|error| error.to_string())?;
        for file_id in file_ids {
            let file_id = file_id.trim().to_lowercase();
            if !is_hash(&file_id) || !seen.insert(file_id.clone()) {
                continue;
            }
            let described = statement
                .exists(params![file_id])
                .map_err(|error| error.to_string())?;
            if !described {
                missing.push(file_id);
            }
        }
    }
    Ok(missing)
}

/// How long a file the network said nothing about is left alone.
///
/// Silence is not a verdict the way "no art for this record" is: a seeder can
/// start announcing a file it was already holding, and a relay that was down can
/// come back. So the file is asked about again eventually — just not every minute,
/// which is what made the queue stand still.
const ASK_AGAIN_AFTER_SECONDS: i64 = 6 * 60 * 60;

/// How long to leave a page alone after the fetch itself failed.
///
/// A relay that will not answer is not a reason to ask it again about the same
/// page on the next pass, which is a minute later. Ten minutes is the lifetime of
/// a heartbeat, so a blip costs about one heartbeat's worth of delay; a relay that
/// is unwell gets one question per heartbeat instead of sixty.
pub(crate) const ASK_AGAIN_AFTER_FAILURE_SECONDS: i64 = 10 * 60;

/// Remember that these files were asked about and nothing usable came back.
///
/// Runs inside whatever transaction the caller has open, like the seeder writes.
/// Returns how many were recorded.
pub(crate) fn remember_asked(
    connection: &Connection,
    file_ids: &[String],
    now: i64,
) -> Result<usize, String> {
    remember_asked_for(connection, file_ids, now, ASK_AGAIN_AFTER_SECONDS)
}

/// The same, with the wait stated: `cool_off_seconds` until it is asked about again.
pub(crate) fn remember_asked_for(
    connection: &Connection,
    file_ids: &[String],
    now: i64,
    cool_off_seconds: i64,
) -> Result<usize, String> {
    let mut written = 0;
    let mut seen = HashSet::new();
    {
        let mut statement = connection
            .prepare(
                "INSERT INTO catalogue_asked(file_id,asked_at,next_ask) VALUES (?1,?2,?3)
                 ON CONFLICT(file_id) DO UPDATE SET
                   asked_at=excluded.asked_at, next_ask=excluded.next_ask",
            )
            .map_err(|error| error.to_string())?;
        for file_id in file_ids {
            let file_id = file_id.trim().to_lowercase();
            if !is_hash(&file_id) || !seen.insert(file_id.clone()) {
                continue;
            }
            statement
                .execute(params![file_id, now, now + cool_off_seconds])
                .map_err(|error| error.to_string())?;
            written += 1;
        }
    }
    Ok(written)
}

/// Forget what was asked about files nobody is holding any more.
///
/// The table would otherwise keep an entry for every file the network ever
/// mentioned, which is a table that only grows to remember something that can no
/// longer be asked about. A file still being held keeps its place until the
/// cool-off has passed, so a seeder that was quiet for a while is asked again.
///
/// The liveness test is against `now` rather than "does a heartbeat row exist",
/// so this is right wherever it is called: the worker prunes expired heartbeats in
/// the same transaction, but a rule that is only true because of the order it runs
/// in is a rule waiting to be broken.
pub(crate) fn forget_asked_that_are_gone(
    connection: &Connection,
    now: i64,
) -> Result<usize, String> {
    connection
        .execute(
            "DELETE FROM catalogue_asked
              WHERE next_ask <= ?1
                AND NOT EXISTS (SELECT 1 FROM catalogue_seeder s
                                 WHERE s.file_id = catalogue_asked.file_id
                                   AND s.expires_at > ?1)",
            params![now],
        )
        .map_err(|error| error.to_string())
}

/// Forget a file entirely, because somebody asked not to see it again.
pub(crate) fn forget_file(connection: &Connection, file_id: &str) -> Result<(), String> {
    if !is_hash(file_id) {
        return Ok(());
    }
    let file_id = file_id.trim().to_lowercase();
    connection
        .execute(
            "DELETE FROM catalogue_seeder WHERE file_id=?1",
            params![file_id],
        )
        .map_err(|error| error.to_string())?;
    // The question it was asked about goes too: a file somebody blocked is not one
    // to ask after again, whatever the cool-off says.
    connection
        .execute(
            "DELETE FROM catalogue_asked WHERE file_id=?1",
            params![file_id],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Forget that an author holds anything.
///
/// Only their heartbeats go. What a file *is* was written down from an
/// announcement, and those rows are the caller's to delete — if they were the only
/// one holding it, the file stops being a result anyway, which is the honest
/// reading of "do not show me this person".
pub(crate) fn forget_author(connection: &Connection, pubkey: &str) -> Result<(), String> {
    if !is_hash(pubkey) {
        return Ok(());
    }
    connection
        .execute(
            "DELETE FROM catalogue_seeder WHERE pubkey=?1",
            params![pubkey.trim().to_lowercase()],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Drop heartbeats that have expired. Returns how many went.
///
/// A seeder that has stopped saying it is there is not a seeder, and this table is
/// the only place that is recorded — so it stays the size of the network's live
/// availability rather than of its history.
pub(crate) fn prune(connection: &Connection, now: i64) -> Result<usize, String> {
    let seeders = connection
        .execute(
            "DELETE FROM catalogue_seeder WHERE expires_at <= ?1",
            params![now],
        )
        .map_err(|error| error.to_string())?;
    Ok(seeders)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalogue's own tables, plus the two the backfill's question reads:
    /// `files` and `published_catalogue` belong to the main schema, which is what
    /// decides whether a file this computer holds is one it has published.
    fn database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE files (file_id TEXT PRIMARY KEY);",
            )
            .unwrap();
        crate::network::initialise_network_schema(&connection).unwrap();
        connection
    }

    fn hex_id(seed: u8) -> String {
        format!("{:02x}", seed).repeat(32)
    }

    /// One announcement, written the way the search path writes them.
    fn announce(connection: &Connection, file_id: &str, author: u8, title: &str, album: &str) {
        connection
            .execute(
                "INSERT INTO remote_catalogue
                   (file_id,source_pubkey,filename,title,artist,album,format,mime,size,license,
                    event_id,seen_at,description,tags,cover_key,canonical_cover_key)
                 VALUES (?1,?2,?3,?4,?5,?6,'FLAC','audio/flac',42000000,'CC0-1.0',?7,?8,'','napstr rock','','')
                 ON CONFLICT(file_id,source_pubkey) DO UPDATE SET
                   filename=excluded.filename, title=excluded.title, artist=excluded.artist,
                   album=excluded.album, tags=excluded.tags, event_id=excluded.event_id,
                   seen_at=excluded.seen_at",
                params![
                    file_id,
                    hex_id(author),
                    format!("{title}.flac"),
                    title,
                    "M83",
                    album,
                    hex_id(author.wrapping_add(100)),
                    format!("2026-09-30T00:00:{:02}+00:00", author),
                ],
            )
            .unwrap();
    }

    fn holding(connection: &Connection, file_id: &str, seeder: u8, now: i64) {
        remember_seeders(connection, &[file_id.to_string()], &hex_id(seeder), now + 600, now)
            .unwrap();
    }

    #[test]
    fn a_track_is_found_by_its_words() {
        let connection = database();
        let now = 1_700_000_000;
        announce(&connection, &hex_id(1), 9, "Midnight City", "Hurry Up");
        announce(&connection, &hex_id(2), 9, "Ghost Song", "Album");
        holding(&connection, &hex_id(1), 9, now);
        holding(&connection, &hex_id(2), 9, now);

        let hits = search(&connection, "midnight", 50, now).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file_id, hex_id(1));
        assert_eq!(hits[0].title, "Midnight City");
        // Each source is named with its own announcement, which is what a report
        // cites and what a download is verified against.
        assert_eq!(hits[0].sources, vec![(hex_id(9), hex_id(109))]);
    }

    #[test]
    fn a_file_nobody_is_holding_is_not_a_result() {
        // A result list is a promise that something can be played, so a file with
        // no live seeder is not in it. This is also what makes blocking work:
        // dropping a blocked author's heartbeats takes their files out of the
        // results without pretending the network forgot what the files are.
        let connection = database();
        let now = 1_700_000_000;
        announce(&connection, &hex_id(1), 9, "Midnight City", "Hurry Up");
        assert!(search(&connection, "midnight", 50, now).unwrap().is_empty());

        holding(&connection, &hex_id(1), 9, now);
        assert_eq!(search(&connection, "midnight", 50, now).unwrap().len(), 1);

        // The heartbeat expires and the file stops being available, while the
        // catalogue keeps the announcement.
        let later = now + 601;
        assert!(search(&connection, "midnight", 50, later).unwrap().is_empty());
        assert_eq!(prune(&connection, later).unwrap(), 1);
        assert_eq!(rows(&connection, "SELECT COUNT(*) FROM catalogue_seeder"), 0);
        assert_eq!(rows(&connection, "SELECT COUNT(*) FROM remote_catalogue"), 1);
    }

    #[test]
    fn a_seeder_must_have_announced_it_too() {
        // The pairing a download depends on: the source named in a result is an
        // author whose announcement this computer holds, because that row is where
        // the size a transfer verifies against comes from.
        let connection = database();
        let now = 1_700_000_000;
        announce(&connection, &hex_id(1), 9, "Ghost", "One");
        // Somebody else says they are holding it, but nothing of theirs describes
        // the file.
        holding(&connection, &hex_id(1), 8, now);
        assert!(search(&connection, "ghost", 50, now).unwrap().is_empty());

        holding(&connection, &hex_id(1), 9, now);
        let hits = search(&connection, "ghost", 50, now).unwrap();
        assert_eq!(hits[0].sources.len(), 1);
        assert_eq!(hits[0].sources[0].0, hex_id(9));
    }

    #[test]
    fn what_more_people_hold_comes_first() {
        let connection = database();
        let now = 1_700_000_000;
        announce(&connection, &hex_id(1), 9, "Ghost", "One");
        for seeder in 1..5u8 {
            announce(&connection, &hex_id(2), seeder, "Ghost", "Two");
            holding(&connection, &hex_id(2), seeder, now);
        }
        holding(&connection, &hex_id(1), 9, now);
        let hits = search(&connection, "ghost", 50, now).unwrap();
        assert_eq!(
            hits.iter().map(|hit| hit.file_id.as_str()).collect::<Vec<_>>(),
            vec![hex_id(2), hex_id(1)],
            "four people can serve the second one"
        );
        assert_eq!(hits[0].sources.len(), 4);
    }

    #[test]
    fn one_file_announced_twice_is_one_result_with_two_sources() {
        let connection = database();
        let now = 1_700_000_000;
        announce(&connection, &hex_id(1), 1, "Ghost", "One");
        announce(&connection, &hex_id(1), 2, "Ghost", "One");
        holding(&connection, &hex_id(1), 1, now);
        holding(&connection, &hex_id(1), 2, now);
        let hits = search(&connection, "ghost", 50, now).unwrap();
        assert_eq!(hits.len(), 1, "one file, however many people announced it");
        assert_eq!(hits[0].sources.len(), 2);
        let mut event_ids = hits[0]
            .sources
            .iter()
            .map(|(_, event_id)| event_id.clone())
            .collect::<Vec<_>>();
        event_ids.sort();
        assert_eq!(event_ids, vec![hex_id(101), hex_id(102)]);
    }

    #[test]
    fn half_a_word_still_finds_it() {
        let connection = database();
        let now = 1_700_000_000;
        announce(&connection, &hex_id(1), 9, "Midnights", "Midnights");
        holding(&connection, &hex_id(1), 9, now);
        // A word index cannot answer this; the substring pass can.
        let hits = search(&connection, "nights", 50, now).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Midnights");
        // And a prefix is what a search box needs while a word is being typed.
        assert_eq!(search(&connection, "midn", 50, now).unwrap().len(), 1);
    }

    #[test]
    fn a_re_announced_track_replaces_what_the_index_knew() {
        let connection = database();
        let now = 1_700_000_000;
        announce(&connection, &hex_id(1), 9, "Wrong Title", "Hurry Up");
        holding(&connection, &hex_id(1), 9, now);
        assert_eq!(search(&connection, "wrong", 50, now).unwrap().len(), 1);

        announce(&connection, &hex_id(1), 9, "Midnight City", "Hurry Up");
        assert!(
            search(&connection, "wrong", 50, now).unwrap().is_empty(),
            "the words of the row it replaced have to leave the index with it"
        );
        assert_eq!(search(&connection, "midnight", 50, now).unwrap().len(), 1);
    }

    #[test]
    fn deleting_an_announcement_takes_its_words_out_of_the_index() {
        // What `block_file` and `block_user` do to the catalogue: the words have to
        // go with the rows.
        let connection = database();
        let now = 1_700_000_000;
        announce(&connection, &hex_id(1), 9, "Ghost", "One");
        holding(&connection, &hex_id(1), 9, now);
        assert_eq!(search(&connection, "ghost", 50, now).unwrap().len(), 1);
        connection
            .execute("DELETE FROM remote_catalogue", [])
            .unwrap();
        assert!(search(&connection, "ghost", 50, now).unwrap().is_empty());
    }

    #[test]
    fn an_author_who_asked_not_to_be_shown_stops_holding_anything() {
        let connection = database();
        let now = 1_700_000_000;
        announce(&connection, &hex_id(1), 7, "Ghost", "One");
        announce(&connection, &hex_id(1), 8, "Ghost", "One");
        holding(&connection, &hex_id(1), 7, now);
        holding(&connection, &hex_id(1), 8, now);
        assert_eq!(search(&connection, "ghost", 50, now).unwrap()[0].sources.len(), 2);

        forget_author(&connection, &hex_id(7)).unwrap();
        let hits = search(&connection, "ghost", 50, now).unwrap();
        assert_eq!(hits[0].sources.len(), 1, "the other seeder is untouched");
        assert_eq!(hits[0].sources[0].0, hex_id(8));

        forget_author(&connection, &hex_id(8)).unwrap();
        assert!(
            search(&connection, "ghost", 50, now).unwrap().is_empty(),
            "nobody is holding it now, so it is not a result"
        );
        assert_eq!(rows(&connection, "SELECT COUNT(*) FROM remote_catalogue"), 2);
        forget_file(&connection, &hex_id(1)).unwrap();
        assert_eq!(rows(&connection, "SELECT COUNT(*) FROM catalogue_seeder"), 0);
        assert_eq!(rows(&connection, "SELECT COUNT(*) FROM remote_catalogue"), 2);
    }

    #[test]
    fn a_heartbeat_that_has_already_expired_is_not_kept() {
        let connection = database();
        let now = 1_700_000_000;
        announce(&connection, &hex_id(1), 9, "Ghost", "One");
        assert_eq!(
            remember_seeders(&connection, &[hex_id(1)], &hex_id(9), now - 1, now).unwrap(),
            0
        );
        assert!(search(&connection, "ghost", 50, now).unwrap().is_empty());
    }

    #[test]
    fn what_nobody_has_described_is_what_gets_asked_for() {
        let connection = database();
        let now = 1_700_000_000;
        announce(&connection, &hex_id(1), 9, "Ghost", "One");
        holding(&connection, &hex_id(1), 9, now);
        holding(&connection, &hex_id(2), 9, now);
        assert_eq!(
            undescribed_live_files(&connection, now, 10).unwrap(),
            vec![hex_id(2)],
            "a file somebody has described is not asked about again"
        );
        // A heartbeat that has run out is not worth asking about either.
        assert!(undescribed_live_files(&connection, now + 601, 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn an_id_that_is_not_a_hash_is_refused_rather_than_stored() {
        let connection = database();
        let now = 1_700_000_000;
        assert!(remember_seeders(&connection, &[hex_id(1)], "nope", now + 600, now).is_err());
        assert_eq!(rows(&connection, "SELECT COUNT(*) FROM catalogue_seeder"), 0);
        assert_eq!(rows(&connection, "SELECT COUNT(*) FROM remote_catalogue"), 0);
    }

    /// How many rows a query finds, for the tests that care that something was
    /// removed rather than that it stopped being returned.
    fn rows(connection: &Connection, sql: &str) -> i64 {
        connection
            .query_row(sql, [], |row| row.get(0))
            .expect("the count query should run")
    }

    /// A damaged word index is a repair, not a failure.
    ///
    /// The damage is real, not simulated: overwriting the index's own b-tree is
    /// how the failure was reproduced outside the app, and it is what the app
    /// reported as "database disk image is malformed" while the table beside it
    /// was perfectly readable. Everything in the index came from that table, so
    /// the repair is to build it again.
    #[test]
    fn a_damaged_index_is_repaired_rather_than_reported() {
        let connection = database();
        let now = 1_700_000_000;
        announce(&connection, &hex_id(1), 9, "Midnight City", "Hurry Up");
        holding(&connection, &hex_id(1), 9, now);
        assert_eq!(search(&connection, "midnight", 10, now).unwrap().len(), 1);

        connection
            .execute("UPDATE remote_catalogue_fts_data SET block = X'DEADBEEF'", [])
            .unwrap();
        assert!(
            !index_is_sound(&connection),
            "a damaged index has to be able to say so, because that is the \
             difference between repairing it and reporting it"
        );
        // The table itself is untouched, which is what makes the repair safe: a
        // search can still fall back to it, and nothing has been lost.
        assert_eq!(rows(&connection, "SELECT COUNT(*) FROM remote_catalogue"), 1);

        rebuild_index(&connection).unwrap();
        assert!(index_is_sound(&connection));
        assert_eq!(
            search(&connection, "midnight", 10, now).unwrap().len(),
            1,
            "the rebuilt index answers again"
        );
    }

    #[test]
    fn a_file_the_network_said_nothing_about_is_left_alone_for_a_while() {
        // The starvation this exists to stop: a question that keeps being answered
        // with silence used to ask itself again every minute, so the backfill never
        // reached anything past the first page of files.
        let connection = database();
        let now = 1_700_000_000;
        announce(&connection, &hex_id(1), 9, "Ghost", "One");
        holding(&connection, &hex_id(1), 9, now);
        holding(&connection, &hex_id(2), 9, now);

        let asked = undescribed_live_files(&connection, now, 10).unwrap();
        assert_eq!(asked, vec![hex_id(2)], "the one nobody has described");
        // Nothing came back for it, so silence is recorded and the next question
        // does not include it.
        assert_eq!(remember_asked(&connection, &asked, now).unwrap(), 1);
        assert!(undescribed_live_files(&connection, now, 10).unwrap().is_empty());
        assert_eq!(
            still_undescribed(&connection, &asked).unwrap(),
            asked,
            "and it is what the silence is recorded against"
        );

        // Long enough later it is worth asking again: a seeder can start announcing
        // a file it was already holding, and a relay that was down can come back.
        // The heartbeat has to be live for that to be a question at all, so it is
        // refreshed the way a seeder that is still there would refresh it.
        let later = now + ASK_AGAIN_AFTER_SECONDS + 1;
        holding(&connection, &hex_id(2), 9, later);
        assert_eq!(
            undescribed_live_files(&connection, later, 10).unwrap(),
            vec![hex_id(2)]
        );
    }

    #[test]
    fn a_fetch_that_failed_is_tried_again_within_the_hour_rather_than_every_minute() {
        // Silence from a relay that answered is worth six hours of not asking; a
        // fetch that failed outright is worth one heartbeat's worth of waiting, so
        // an outage recovers without the pass retrying the same page every minute.
        let connection = database();
        let now = 1_700_000_000;
        holding(&connection, &hex_id(2), 9, now);
        assert_eq!(
            remember_asked_for(
                &connection,
                &[hex_id(2)],
                now,
                ASK_AGAIN_AFTER_FAILURE_SECONDS
            )
            .unwrap(),
            1
        );
        // The seeder is still there, refreshing its heartbeat as it does.
        holding(&connection, &hex_id(2), 9, now + 60);
        assert!(undescribed_live_files(&connection, now + 60, 10)
            .unwrap()
            .is_empty());
        assert_eq!(
            undescribed_live_files(&connection, now + ASK_AGAIN_AFTER_FAILURE_SECONDS + 1, 10)
                .unwrap(),
            vec![hex_id(2)]
        );
    }

    #[test]
    fn what_was_asked_about_is_forgotten_once_nobody_holds_it() {
        let connection = database();
        let now = 1_700_000_000;
        holding(&connection, &hex_id(2), 9, now);
        assert_eq!(remember_asked(&connection, &[hex_id(2)], now).unwrap(), 1);

        // A heartbeat lasts ten minutes and the cool-off lasts six hours, so the
        // two are only ever both true of the same file if the seeder is still
        // there — which is what refreshing the beat says.
        let later = now + ASK_AGAIN_AFTER_SECONDS + 1;
        holding(&connection, &hex_id(2), 9, later);
        assert_eq!(
            undescribed_live_files(&connection, later, 10).unwrap(),
            vec![hex_id(2)],
            "still being held, so the cool-off is the only thing that held it back"
        );
        assert_eq!(
            forget_asked_that_are_gone(&connection, later).unwrap(),
            0,
            "and the question is kept while it is still worth asking"
        );
        assert_eq!(rows(&connection, "SELECT COUNT(*) FROM catalogue_asked"), 1);

        // Once the heartbeat has run out nothing can be asked about it at all, so
        // keeping the question would only be a table that grows.
        let gone = later + 601;
        assert_eq!(forget_asked_that_are_gone(&connection, gone).unwrap(), 1);
        assert_eq!(rows(&connection, "SELECT COUNT(*) FROM catalogue_asked"), 0);
    }

    #[test]
    fn a_file_this_computer_holds_without_publishing_is_never_asked_about() {
        // The rule `network::unpublished_holds` applies to a batch a phone named,
        // here so the backfill cannot ask a relay about a file this computer has
        // chosen not to publish: the question names the bytes.
        let connection = database();
        let now = 1_700_000_000;
        holding(&connection, &hex_id(3), 9, now);
        connection
            .execute("INSERT INTO files(file_id) VALUES(?1)", params![hex_id(3)])
            .unwrap();
        assert!(
            undescribed_live_files(&connection, now, 10).unwrap().is_empty(),
            "a file held here and not published is not a fair question"
        );

        connection
            .execute(
                "INSERT INTO published_catalogue(file_id,published_at) VALUES(?1,'now')",
                params![hex_id(3)],
            )
            .unwrap();
        assert_eq!(
            undescribed_live_files(&connection, now, 10).unwrap(),
            vec![hex_id(3)],
            "once it is published, asking about it says nothing new"
        );
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

    #[test]
    fn a_catalogue_that_predates_the_index_is_indexed_once() {
        // An installation whose catalogue was filled before this index existed must
        // not answer every search from the slow path until something is
        // re-announced.
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE remote_catalogue (
                   file_id TEXT NOT NULL, source_pubkey TEXT NOT NULL, filename TEXT NOT NULL,
                   title TEXT NOT NULL, artist TEXT NOT NULL, album TEXT NOT NULL, format TEXT NOT NULL,
                   mime TEXT NOT NULL, size INTEGER NOT NULL, license TEXT NOT NULL, event_id TEXT NOT NULL,
                   seen_at TEXT NOT NULL, description TEXT NOT NULL DEFAULT '', tags TEXT NOT NULL DEFAULT '',
                   cover_key TEXT NOT NULL DEFAULT '', canonical_cover_key TEXT NOT NULL DEFAULT '',
                   PRIMARY KEY(file_id, source_pubkey));",
            )
            .unwrap();
        announce(&connection, &hex_id(1), 9, "Midnight City", "Hurry Up");
        announce(&connection, &hex_id(2), 9, "Ghost Song", "One");
        let now = 1_700_000_000;
        // The seeder table does not exist until the schema runs, so nothing can
        // have been indexed before it either.
        initialise_schema(&connection).unwrap();
        holding(&connection, &hex_id(1), 9, now);
        holding(&connection, &hex_id(2), 9, now);

        // The index itself, not a result the substring fallback could have
        // produced: this asserted a hit once before, and the fallback answered it
        // while the index held nothing at all.
        assert_eq!(
            rows(&connection, "SELECT COUNT(*) FROM remote_catalogue_fts_docsize"),
            rows(&connection, "SELECT COUNT(*) FROM remote_catalogue"),
            "every stored announcement has to have a document of its own"
        );
        assert!(found_by_the_index(&connection, "midnight", &hex_id(1)));
        assert!(found_by_the_index(&connection, "ghost", &hex_id(2)));
        assert_eq!(search(&connection, "midnight", 50, now).unwrap().len(), 1);

        // And the walk is remembered, so it is not repeated on every start.
        assert_eq!(index_stored_announcements(&connection).unwrap(), 0);
    }

    /// The trap that hid the fault above for a day.
    ///
    /// An external-content index answers a plain `SELECT` with the content
    /// table's rows, so counting the index counts the announcement table and says
    /// everything is in step whether it is or not. Measured on a real
    /// installation: 34,792 against 2,399 documents.
    #[test]
    fn a_bare_count_over_the_index_is_not_a_count_of_the_index() {
        // The state that installation was in: every announcement stored, and not
        // one of them in the index. Reached here by emptying the index afterwards,
        // which is what it was — a table created after the catalogue was filled.
        let connection = database();
        announce(&connection, &hex_id(1), 9, "Midnight City", "Hurry Up");
        connection
            .execute("DELETE FROM remote_catalogue_fts", [])
            .unwrap();
        assert_eq!(rows(&connection, "SELECT COUNT(*) FROM remote_catalogue"), 1);
        assert_eq!(
            rows(&connection, "SELECT COUNT(*) FROM remote_catalogue_fts_docsize"),
            0
        );
        assert_eq!(
            count(&connection, "SELECT COUNT(*) FROM remote_catalogue_fts").unwrap(),
            1,
            "a bare count reports the announcement, whatever the index holds"
        );
        assert!(!found_by_the_index(&connection, "midnight", &hex_id(1)));
    }

    /// Whether the *index* answers for a word, with no fallback behind it.
    fn found_by_the_index(connection: &Connection, word: &str, file_id: &str) -> bool {
        connection
            .query_row(
                "SELECT 1 FROM remote_catalogue_fts
                   JOIN remote_catalogue c ON c.rowid = remote_catalogue_fts.rowid
                  WHERE remote_catalogue_fts MATCH ?1 AND c.file_id = ?2 LIMIT 1",
                params![format!("\"{word}\""), file_id],
                |row| row.get::<_, i64>(0),
            )
            .is_ok()
    }
}
