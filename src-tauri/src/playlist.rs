//! Playlists: a named, ordered list of file ids that a person curates.
//!
//! A playlist owns no bytes and asserts nothing about who holds them. Its
//! members are ordinary kind `30421` catalogue entries, so a client that ignores
//! playlists entirely stays fully interoperable, and a member's availability
//! resolves from kind `30422` heartbeats from **any** author rather than from
//! the playlist. A member that cannot be resolved renders from the playlist's
//! own display hints instead of being dropped, because the order is part of what
//! the playlist means.
//!
//! The format of a public playlist is specified by `NIP-NAPSTR-PLAYLIST.md`
//! (kind `30425`). A private playlist has no wire format at all: it stays on the
//! devices that are its owner's, stored here and served to a paired companion
//! over the companion channel. Both are stored in one shape, because a
//! companion lists and plays them the same way, and `private` is the one thing
//! that has to travel with the row rather than being derived from where it was
//! found - it is also what forbids publishing it.
//!
//! Members are stored one row each rather than as one blob, so a page of them
//! can be read without loading the whole playlist, and so "which playlists name
//! this file" is an index lookup rather than a scan of every playlist body.

use napstr_remote_protocol::{
    RemotePlaylist, RemotePlaylistCoordinate, RemotePlaylistSummary, RemotePlaylistTrack,
};
use nostr_sdk::prelude::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const PLAYLIST_KIND: u16 = 30425;
/// A marker tag is mandatory in both directions. This kind is co-occupied by
/// other protocols, so a query without the marker answers about their events.
pub const PLAYLIST_MARKER: &str = "napstr-playlist";
/// `content` is JSON with camel-case keys, at most 128 KiB - the same budget as
/// a kind `30423` audiobook manifest, so one validator serves both.
pub const PLAYLIST_CONTENT_BYTE_LIMIT: usize = 128 * 1024;
/// Members one playlist may name, also matching the audiobook manifest.
pub const PLAYLIST_MEMBER_LIMIT: usize = 500;
/// Playlists one list answer carries. This computer's Playlists page reads one
/// page of them, and a companion pages through the same rows.
pub const PLAYLIST_LIST_LIMIT: usize = 200;
/// The `alt` string a public playlist carries, which is what a client that does
/// not implement this kind shows to a person.
const PLAYLIST_ALT: &str = "Napstr public playlist";

/// Untrusted display text - a title, or a member hint - is at most this long.
/// A title is normative, so one longer than this is refused; hints are advisory,
/// so they are cut to fit instead.
const PLAYLIST_TEXT_CHARACTER_LIMIT: usize = 256;
/// A playlist's own search words follow the catalogue's tag rules exactly, so a
/// word that found a track finds a playlist the same way: at most 12 words, no
/// longer than 32 characters each, and no control or bidirectional formatting
/// characters.
const PLAYLIST_TAG_LIMIT: usize = 12;
const PLAYLIST_TAG_CHARACTER_LIMIT: usize = 32;

/// The raw JSON body of a kind `30425` event. Unknown properties are ignored, as
/// the NIP requires.
///
/// `Serialize` is derived for the publishing half, so the body this host writes
/// is produced by the same declaration that reads it and the two cannot drift.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlaylistContent {
    #[serde(default)]
    protocol: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    playlist_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    title: String,
    /// Album artist, when the playlist describes one release group rather than a
    /// mix. Self-asserted and never a trust decision.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    artist: String,
    /// MusicBrainz release-group MBID, matching the meaning and spelling of
    /// `mbid` in kind `30427`. Self-asserted, and only ever used to reconcile a
    /// key that already matched.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    mbid: String,
    /// The playlist's own artwork, as the file id of a picture.
    ///
    /// A file id and never a URL: the picture travels as bytes over the same
    /// transfer path as everything else, so no reader has to tell a third party
    /// that it is looking at this playlist. A value that is not a file id is
    /// ignored on the way in and refused on the way out.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    image: String,
    /// The author's own search words, comma-separated, in the same shape as a
    /// catalogue entry's `tags`.
    ///
    /// Carried in the body as well as in the `t` tags so that the author's
    /// choice survives a round trip: the `t` tags also hold words a client
    /// suggested, and a client that read those back as the author's own would
    /// silently adopt its own suggestions as theirs on the next edit.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    tags: String,
    #[serde(default, skip_serializing_if = "is_false")]
    deleted: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tracks: Vec<PlaylistMember>,
}

/// One member. Only the file id and its position are normative; the rest are
/// display hints that a catalogue entry always overrides.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlaylistMember {
    position: u32,
    file_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    artist: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    album: String,
}

/// What one kind `30425` event says about a playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaylistEvent {
    Playlist(Box<RemotePlaylist>),
    /// The author withdrew the coordinate, so no older revision of theirs may be
    /// offered again.
    Withdrawn {
        playlist_id: String,
        author: String,
    },
}

pub fn initialise_schema(connection: &Connection) -> Result<(), String> {
    // A playlist is keyed by its coordinate: its author and its id together. The
    // id is chosen by the author, so two authors may choose the same one, and a
    // table keyed by the id alone would let the second of them quietly replace a
    // playlist the first had already stored here.
    if previous_shape_present(connection)? {
        connection
            .execute_batch(
                "DROP TABLE IF EXISTS playlist_tracks;
                 DROP TABLE IF EXISTS playlists;
                 DROP TABLE IF EXISTS playlist_withdrawals;",
            )
            .map_err(|error| error.to_string())?;
    }
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS playlists (
               playlist_id TEXT NOT NULL,
               author TEXT NOT NULL,
               title TEXT NOT NULL,
               display_name TEXT NOT NULL DEFAULT '',
               artist TEXT NOT NULL DEFAULT '',
               mbid TEXT NOT NULL DEFAULT '',
               image TEXT NOT NULL DEFAULT '',
               tags TEXT NOT NULL DEFAULT '',
               private INTEGER NOT NULL DEFAULT 0,
               published INTEGER NOT NULL DEFAULT 0,
               updated_at INTEGER NOT NULL DEFAULT 0,
               PRIMARY KEY (playlist_id, author)
             );
             CREATE TABLE IF NOT EXISTS playlist_tracks (
               playlist_id TEXT NOT NULL,
               author TEXT NOT NULL,
               position INTEGER NOT NULL,
               file_id TEXT NOT NULL,
               title TEXT NOT NULL DEFAULT '',
               artist TEXT NOT NULL DEFAULT '',
               album TEXT NOT NULL DEFAULT '',
               PRIMARY KEY (playlist_id, author, position)
             );
             CREATE INDEX IF NOT EXISTS idx_playlist_tracks_file
               ON playlist_tracks(file_id);
             -- The newest withdrawal an author published for one coordinate.
             --
             -- A revision is read from whichever relay still answers with it,
             -- and a relay that never received the withdrawal has no way to
             -- know it should stop serving the older revision. So the fact
             -- that a coordinate was withdrawn is kept here, and an older
             -- revision is refused for as long as it stands. It is not a
             -- stored playlist: it says one thing about a coordinate, which is
             -- that its author has taken it back.
             CREATE TABLE IF NOT EXISTS playlist_withdrawals (
               playlist_id TEXT NOT NULL,
               author TEXT NOT NULL,
               withdrawn_at INTEGER NOT NULL,
               PRIMARY KEY (playlist_id, author)
             );",
        )
        .map_err(|error| error.to_string())
}

/// Whether these tables were built in an earlier, narrower shape.
///
/// Checked against the stored declaration rather than a version number because
/// SQLite cannot alter a primary key, and the first shape fails silently: keyed
/// by the id alone, a second author's playlist would overwrite the first
/// author's. No released build ever wrote a row - the only writer is the
/// publishing path, which arrived with the coordinate - so there is nothing to
/// preserve.
fn previous_shape_present(connection: &Connection) -> Result<bool, String> {
    let schema = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='playlists'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    Ok(schema.is_some_and(|sql| {
        !sql.contains("PRIMARY KEY (playlist_id, author)")
            || !sql.contains("tags TEXT")
            || !sql.contains("published INTEGER")
            || !sql.contains("image TEXT")
    }))
}

/// Write a playlist revision, members and all.
///
/// The members are replaced wholesale: a revision is the whole list, so an edit
/// that drops a track must not leave the dropped one behind.
pub fn save(connection: &Connection, playlist: &RemotePlaylist) -> Result<(), String> {
    let tags = validate_stored(playlist)?;
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO playlists(playlist_id,author,title,display_name,artist,mbid,image,tags,private,published,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
             ON CONFLICT(playlist_id,author) DO UPDATE SET
               title=excluded.title,
               display_name=excluded.display_name,
               artist=excluded.artist,
               mbid=excluded.mbid,
               image=excluded.image,
               tags=excluded.tags,
               private=excluded.private,
               published=excluded.published,
               updated_at=excluded.updated_at",
            params![
                playlist.playlist_id,
                playlist.author,
                playlist.title,
                playlist.display_name,
                playlist.artist,
                playlist.mbid,
                playlist.image,
                tags,
                i64::from(playlist.private),
                i64::from(playlist.published),
                playlist.updated_at,
            ],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "DELETE FROM playlist_tracks WHERE playlist_id=?1 AND author=?2",
            params![playlist.playlist_id, playlist.author],
        )
        .map_err(|error| error.to_string())?;
    for member in &playlist.tracks {
        transaction
            .execute(
                "INSERT INTO playlist_tracks(playlist_id,author,position,file_id,title,artist,album)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    playlist.playlist_id,
                    playlist.author,
                    i64::from(member.position),
                    member.file_id,
                    member.title,
                    member.artist,
                    member.album,
                ],
            )
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())
}

/// Forget a playlist entirely. A withdrawal is not stored as a tombstone here
/// because the store holds only the newest state of each coordinate, so
/// forgetting one *is* the withdrawal.
pub fn remove(connection: &Connection, author: &str, playlist_id: &str) -> Result<(), String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "DELETE FROM playlist_tracks WHERE playlist_id=?1 AND author=?2",
            params![playlist_id, author],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "DELETE FROM playlists WHERE playlist_id=?1 AND author=?2",
            params![playlist_id, author],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

/// Write a revision down on behalf of whoever is asking, as their own.
///
/// The author and the edit time are stamped here rather than taken from the
/// caller: the coordinate has to be the one a later publication will use, or
/// saving a draft and publishing it would leave two rows behind under one id,
/// only one of which is the author's.
///
/// A revision handed in under somebody else's coordinate is a **copy**, not a
/// revision of theirs, and gets an id of its own. An id is chosen by the author
/// and travels as `d` for as long as the playlist exists, so a second author
/// reusing one is legal - two playlists may share an `d` under different
/// authors - but it is never what somebody meant: editing a playlist you do not
/// own means "keep one of these for myself", and the copy has to be findable
/// under a name of its own rather than colliding with the revision it came
/// from. Nothing is ever signed for a coordinate this identity does not own.
pub fn file_revision(
    connection: &Connection,
    mut playlist: RemotePlaylist,
    author: &str,
    updated_at: i64,
) -> Result<RemotePlaylist, String> {
    if !playlist.author.is_empty() && playlist.author != author {
        playlist.playlist_id = new_playlist_id();
        // A copy has never been signed by this identity, whatever the original
        // coordinate's history was.
        playlist.published = false;
    }
    playlist.author = author.to_string();
    playlist.updated_at = updated_at;
    save(connection, &playlist)?;
    Ok(playlist)
}

/// The revision this computer holds of one coordinate, or `None`.
fn stored_updated_at(
    connection: &Connection,
    author: &str,
    playlist_id: &str,
) -> Result<Option<i64>, String> {
    connection
        .query_row(
            "SELECT updated_at FROM playlists WHERE playlist_id=?1 AND author=?2",
            params![playlist_id, author],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| error.to_string())
}

/// The newest withdrawal an author published for one coordinate, or `None` if
/// this computer has never seen one.
///
/// A withdrawal is the author's last word about a coordinate, so every older
/// revision of it is finished. It is stored as a time rather than a naked flag
/// because an author may publish the playlist again afterwards: a revision
/// newer than the withdrawal is a live playlist, and anything at or before it
/// is not.
pub fn withdrawn_at(
    connection: &Connection,
    author: &str,
    playlist_id: &str,
) -> Result<Option<i64>, String> {
    connection
        .query_row(
            "SELECT withdrawn_at FROM playlist_withdrawals WHERE playlist_id=?1 AND author=?2",
            params![playlist_id, author],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| error.to_string())
}

/// Remember that an author took one coordinate back.
///
/// Deliberately says nothing about what this computer holds: a withdrawal read
/// from a relay is a fact about the relays, and forgetting a playlist because a
/// *different* installation of the same identity withdrew it is the hazard the
/// NIP names. Whether the local revision goes is the caller's decision, taken
/// where the identity is known.
pub fn mark_withdrawn(
    connection: &Connection,
    author: &str,
    playlist_id: &str,
    withdrawn_at: i64,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO playlist_withdrawals(playlist_id,author,withdrawn_at) VALUES(?1,?2,?3)
             ON CONFLICT(playlist_id,author) DO UPDATE SET
               withdrawn_at=MAX(withdrawn_at,excluded.withdrawn_at)",
            params![playlist_id, author, withdrawn_at],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Store a revision read from a relay, unless it has been overtaken.
///
/// Answers whether it was stored, because "the store already had a newer one"
/// and "the author has withdrawn this" are ordinary answers rather than
/// failures. Two things can overtake a revision off a relay, and both are facts
/// this computer knows and the relay may not: the coordinate was withdrawn
/// after it, or this computer already holds a newer revision of it. A relay
/// that never received the newest revision must not be able to talk this
/// computer back into an older one - the same reason `list` orders by
/// `updated_at` at all.
pub fn store_from_relay(
    connection: &Connection,
    playlist: &RemotePlaylist,
) -> Result<bool, String> {
    if let Some(withdrawn_at) = withdrawn_at(connection, &playlist.author, &playlist.playlist_id)? {
        if withdrawn_at >= playlist.updated_at {
            return Ok(false);
        }
    }
    if let Some(stored) = stored_updated_at(connection, &playlist.author, &playlist.playlist_id)? {
        if stored > playlist.updated_at {
            return Ok(false);
        }
    }
    save(connection, playlist)?;
    Ok(true)
}

/// Playlists by name, newest first, without their members.
///
/// A companion lists these and asks for the members of the one it is about to
/// play, so browsing never carries the members of playlists nobody opened.
pub fn list(
    connection: &Connection,
    offset: usize,
    limit: usize,
) -> Result<(Vec<RemotePlaylistSummary>, usize), String> {
    let total = connection
        .query_row("SELECT COUNT(*) FROM playlists", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| error.to_string())?
        .max(0) as usize;
    let mut statement = connection
        .prepare(
            "SELECT p.playlist_id, p.title, p.author, p.display_name, p.private, p.published,
                    p.image, p.updated_at,
                    (SELECT COUNT(*) FROM playlist_tracks t
                      WHERE t.playlist_id=p.playlist_id AND t.author=p.author),
                    (SELECT t.file_id FROM playlist_tracks t
                      WHERE t.playlist_id=p.playlist_id AND t.author=p.author
                      ORDER BY t.position LIMIT 1)
               FROM playlists p
              ORDER BY p.updated_at DESC, p.playlist_id, p.author
              LIMIT ?1 OFFSET ?2",
        )
        .map_err(|error| error.to_string())?;
    let playlists = statement
        .query_map(params![limit as i64, offset as i64], |row| {
            Ok(RemotePlaylistSummary {
                playlist_id: row.get(0)?,
                title: row.get(1)?,
                author: row.get(2)?,
                display_name: row.get(3)?,
                private: row.get::<_, i64>(4)? != 0,
                published: row.get::<_, i64>(5)? != 0,
                image: row.get(6)?,
                updated_at: row.get(7)?,
                track_count: row.get::<_, i64>(8)?.max(0) as usize,
                // A list row draws the album the playlist opens with, so the
                // file it starts with travels with the name. `get` answers
                // `None` for a playlist that names nothing yet, which is an
                // empty string rather than an error.
                first_file_id: row.get::<_, Option<String>>(9)?.unwrap_or_default(),
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok((playlists, total))
}

/// One page of a playlist's members, in `position` order, or `None` when this
/// computer has no such playlist.
///
/// The author is half the identity, so it is half the lookup: a caller that
/// names only an id is asking for "the playlist you hold under this id", which
/// is answered only while there is exactly one. Two authors may share an id, and
/// guessing between them would be guessing whose playlist to play.
pub fn page(
    connection: &Connection,
    author: &str,
    playlist_id: &str,
    offset: usize,
    limit: usize,
) -> Result<Option<RemotePlaylist>, String> {
    let author = if author.is_empty() {
        sole_author(connection, playlist_id)?
    } else {
        author.to_string()
    };
    let head = connection
        .query_row(
            "SELECT title, display_name, artist, mbid, image, tags, private, published, updated_at,
                    (SELECT COUNT(*) FROM playlist_tracks t
                      WHERE t.playlist_id=p.playlist_id AND t.author=p.author)
               FROM playlists p WHERE playlist_id=?1 AND author=?2",
            params![playlist_id, author],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)? != 0,
                    row.get::<_, i64>(7)? != 0,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?.max(0) as usize,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((title, display_name, artist, mbid, image, tags, private, published, updated_at, total)) = head
    else {
        return Ok(None);
    };
    let mut statement = connection
        .prepare(
            "SELECT position, file_id, title, artist, album
               FROM playlist_tracks WHERE playlist_id=?1 AND author=?2
              ORDER BY position LIMIT ?3 OFFSET ?4",
        )
        .map_err(|error| error.to_string())?;
    let tracks = statement
        .query_map(
            params![playlist_id, author, limit as i64, offset as i64],
            |row| {
                Ok(RemotePlaylistTrack {
                    position: row.get::<_, i64>(0)?.max(0) as u32,
                    file_id: row.get(1)?,
                    title: row.get(2)?,
                    artist: row.get(3)?,
                    album: row.get(4)?,
                })
            },
        )
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(Some(RemotePlaylist {
        playlist_id: playlist_id.to_string(),
        title,
        author,
        display_name,
        artist,
        mbid,
        image,
        tags,
        private,
        published,
        updated_at,
        tracks,
        total,
    }))
}

/// Which playlists name a file, as coordinates.
///
/// A track's menu draws a tick beside every playlist that already holds it, and
/// this answers all of them at once: members are one row each, so the question is
/// an index lookup on `file_id` rather than a page of members per playlist.
///
/// Coordinates rather than ids, because the id is chosen by the author and two
/// authors may choose the same one. A playlist the caller may not edit is still
/// an honest answer here; deciding what may be done about it is the caller's
/// business.
pub fn containing(
    connection: &Connection,
    file_id: &str,
) -> Result<Vec<RemotePlaylistCoordinate>, String> {
    let mut statement = connection
        .prepare(
            "SELECT DISTINCT author, playlist_id FROM playlist_tracks
              WHERE file_id=?1 ORDER BY playlist_id, author",
        )
        .map_err(|error| error.to_string())?;
    let playlists = statement
        .query_map([file_id], |row| {
            Ok(RemotePlaylistCoordinate {
                author: row.get(0)?,
                playlist_id: row.get(1)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(playlists)
}

/// The author of the only playlist this computer holds under an id.
///
/// Empty when it holds none, which leaves the page lookup to return nothing.
fn sole_author(connection: &Connection, playlist_id: &str) -> Result<String, String> {
    let mut statement = connection
        .prepare("SELECT author FROM playlists WHERE playlist_id=?1 LIMIT 2")
        .map_err(|error| error.to_string())?;
    let authors = statement
        .query_map([playlist_id], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    match authors.len() {
        0 => Ok(String::new()),
        1 => Ok(authors[0].clone()),
        _ => Err("Two playlists here share that id, so which author's is meant has to be said".into()),
    }
}

/// Validate a kind `30425` event and turn it into a playlist revision.
///
/// `None` means the event is not a valid playlist of ours, which is the normal
/// answer for the other protocols that share this kind. A withdrawal is a valid
/// event that carries no playlist, so it is told apart rather than dropped.
pub fn playlist_event(event: &Event) -> Option<PlaylistEvent> {
    if event.kind != Kind::from(PLAYLIST_KIND)
        || event.verify().is_err()
        || event.content.len() > PLAYLIST_CONTENT_BYTE_LIMIT
    {
        return None;
    }
    let content = serde_json::from_str::<PlaylistContent>(&event.content).ok()?;
    if content.protocol != "napstr/1" || !event.tags.hashtags().any(|tag| tag == PLAYLIST_MARKER) {
        return None;
    }
    let author = event.pubkey.to_hex();
    // The coordinate is the `d` tag, which is the playlist both bodies are
    // about. A withdrawal is the one body that does not repeat it, because it
    // describes no playlist - only the end of one.
    let playlist_id = event.tags.identifier()?;
    if !is_canonical_uuid(playlist_id) {
        return None;
    }
    if content.deleted {
        // A withdrawal that does repeat the id has to repeat the right one.
        if !content.playlist_id.is_empty() && content.playlist_id != playlist_id {
            return None;
        }
        return Some(PlaylistEvent::Withdrawn {
            playlist_id: playlist_id.to_string(),
            author,
        });
    }
    // A revision must name the coordinate it is filed under, so a body can never
    // be stored against an id its author did not sign for.
    if content.playlist_id != playlist_id {
        return None;
    }
    let title = playlist_text(&content.title);
    if title.is_empty()
        || content.title.chars().count() > PLAYLIST_TEXT_CHARACTER_LIMIT
        || tag_text(event, "title").as_deref() != Some(content.title.trim())
    {
        return None;
    }
    let tracks = content
        .tracks
        .into_iter()
        .map(|member| RemotePlaylistTrack {
            position: member.position,
            file_id: member.file_id,
            title: playlist_text(&member.title),
            artist: playlist_text(&member.artist),
            album: playlist_text(&member.album),
        })
        .collect::<Vec<_>>();
    if !valid_members(&tracks) {
        return None;
    }
    Some(PlaylistEvent::Playlist(Box::new(RemotePlaylist {
        playlist_id: content.playlist_id,
        title,
        author,
        // A profile lookup fills this in; the event itself carries no name.
        display_name: String::new(),
        artist: playlist_text(&content.artist),
        // An MBID that is not a canonical UUID is ignored rather than fatal: it
        // can only ever enrich a key match, never cause one.
        mbid: if is_canonical_uuid(&content.mbid) {
            content.mbid
        } else {
            String::new()
        },
        // Likewise for artwork: an `https://` value is not a file id, and a
        // reader that fetched it would hand a stranger the fact that this
        // listener is looking at this playlist. Ignored, not fetched.
        image: if is_file_id(&content.image) {
            content.image
        } else {
            String::new()
        },
        // The author's words, not the `t` tags: those also carry whatever a
        // client suggested, and a suggestion is not something they chose.
        tags: playlist_tags(&content.tags),
        private: false,
        // A playlist that arrived as an event has a revision on the relays, which
        // is what this records. Nothing this host reads is a draft.
        published: true,
        updated_at: event.created_at.as_secs() as i64,
        tracks,
        total: 0,
    })))
}

/// Sign a kind `30425` event for a public playlist.
///
/// This is the publishing half of the contract [`playlist_event`] reads, so the
/// two sit side by side: a playlist this host would refuse to parse is a
/// playlist no other client should have to read either.
///
/// `suggest_tags` is the author's answer to "suggest words from the title?", and
/// it only applies while they have written no tags of their own. Their words are
/// the whole answer whenever there are any: a client MUST NOT add its own on top
/// of a choice, and an author who does not want suggested words is entitled to
/// exactly that.
///
/// There are three answers, not two. Tags of their own win and are the whole list;
/// `suggest_tags: false` with no tags of their own publishes the marker and no
/// words at all, which is the author's "find this by nothing but its name and its
/// members"; and `suggest_tags: true` with no tags of their own takes words from
/// the title.
/// The event cannot carry the difference between the last two, so whoever calls
/// this must remember the answer per playlist rather than re-ask: an author who
/// said no must not be given our words back on their next revision.
pub fn playlist_event_builder(
    playlist: &RemotePlaylist,
    suggest_tags: bool,
    keys: &Keys,
) -> Result<Event, String> {
    if !is_canonical_uuid(&playlist.playlist_id) {
        return Err("a playlist id is a canonical lowercase UUID".into());
    }
    if playlist.private {
        return Err(
            "a private playlist is never published: it stays on this computer and the phones \
             paired with it"
                .into(),
        );
    }
    let title = playlist_text(&playlist.title);
    if title.is_empty()
        || playlist.title.chars().count() > PLAYLIST_TEXT_CHARACTER_LIMIT
        || title != playlist.title.trim()
    {
        return Err("a playlist needs a title of at most 256 characters".into());
    }
    if !valid_members(&playlist.tracks) {
        return Err("a playlist needs 1 to 500 unique members in position order".into());
    }
    // Artwork is a file id or nothing. A URL is refused rather than ignored,
    // because a publisher that meant to attach a picture should hear about it
    // rather than ship a playlist that silently has none.
    if !playlist.image.is_empty() && !is_file_id(&playlist.image) {
        return Err("a playlist's artwork is the file id of a picture, never a URL".into());
    }
    let content = PlaylistContent {
        protocol: "napstr/1".into(),
        playlist_id: playlist.playlist_id.clone(),
        title: title.clone(),
        artist: playlist_text(&playlist.artist),
        mbid: if is_canonical_uuid(&playlist.mbid) {
            playlist.mbid.clone()
        } else {
            String::new()
        },
        image: playlist.image.clone(),
        tags: crate::normalise_tags(&playlist.tags)
            .map_err(|error| format!("this playlist's tags cannot be published: {error}"))?,
        deleted: false,
        tracks: playlist
            .tracks
            .iter()
            .map(|member| PlaylistMember {
                position: member.position,
                file_id: member.file_id.clone(),
                title: playlist_text(&member.title),
                artist: playlist_text(&member.artist),
                album: playlist_text(&member.album),
            })
            .collect(),
    };
    let encoded = serde_json::to_string(&content).map_err(|error| error.to_string())?;
    if encoded.len() > PLAYLIST_CONTENT_BYTE_LIMIT {
        return Err("this playlist is too large to publish".into());
    }
    let mut tags = vec![
        Tag::parse(["d", playlist.playlist_id.as_str()]).map_err(|error| error.to_string())?,
        Tag::parse(["t", PLAYLIST_MARKER]).map_err(|error| error.to_string())?,
        Tag::parse(["title", title.as_str()]).map_err(|error| error.to_string())?,
        Tag::parse(["alt", PLAYLIST_ALT]).map_err(|error| error.to_string())?,
        Tag::parse(["client", "Napstr"]).map_err(|error| error.to_string())?,
    ];
    // One `x` tag per member, in order, so a relay can answer "which playlists
    // include this file" without fetching and parsing every playlist body.
    for member in &playlist.tracks {
        tags.push(Tag::parse(["x", member.file_id.as_str()]).map_err(|error| error.to_string())?);
    }
    // The words this playlist can be found by. The author's own tags come first
    // and, when there are any, are the only words: a suggestion never outranks a
    // choice, and an author who has made one does not need ours.
    let words = if content.tags.is_empty() {
        if suggest_tags {
            crate::network::catalogue_search_tokens(&[title.as_str()])
        } else {
            Vec::new()
        }
    } else {
        crate::network::catalogue_search_tokens(&content.tags.split(',').collect::<Vec<_>>())
    };
    for word in words {
        tags.push(Tag::hashtag(word));
    }
    EventBuilder::new(Kind::from(PLAYLIST_KIND), encoded)
        .tags(tags)
        .sign_with_keys(keys)
        .map_err(|error| error.to_string())
}

/// Sign the body that withdraws this author's revision of a playlist.
pub fn playlist_withdrawal_builder(
    playlist_id: &str,
    keys: &Keys,
) -> Result<Event, String> {
    if !is_canonical_uuid(playlist_id) {
        return Err("a playlist id is a canonical lowercase UUID".into());
    }
    let content = PlaylistContent {
        protocol: "napstr/1".into(),
        deleted: true,
        ..PlaylistContent::default()
    };
    let encoded = serde_json::to_string(&content).map_err(|error| error.to_string())?;
    EventBuilder::new(Kind::from(PLAYLIST_KIND), encoded)
        .tags(vec![
            Tag::parse(["d", playlist_id]).map_err(|error| error.to_string())?,
            Tag::parse(["t", PLAYLIST_MARKER]).map_err(|error| error.to_string())?,
        ])
        .sign_with_keys(keys)
        .map_err(|error| error.to_string())
}

/// A playlist that is about to be stored, so a broken one never becomes a row.
///
/// Answers with the author's tags in the shape the catalogue would publish them,
/// so a row and the event it came from are the same words in the same order.
fn validate_stored(playlist: &RemotePlaylist) -> Result<String, String> {
    if !is_canonical_uuid(&playlist.playlist_id) {
        return Err("a playlist id is a canonical lowercase UUID".into());
    }
    if playlist.title.trim().is_empty() {
        return Err("a playlist needs a title".into());
    }
    // A playlist on this computer may be empty. An empty one is a draft nobody
    // has published, and "1 to 500 members" is a rule an event is held to, which
    // is where it stays: `playlist_event_builder` refuses to sign an empty
    // playlist, so nothing that reaches a relay can be one.
    if !playlist.tracks.is_empty() && !valid_members(&playlist.tracks) {
        return Err("a playlist needs up to 500 unique members in position order".into());
    }
    if !playlist.image.is_empty() && !is_file_id(&playlist.image) {
        return Err("a playlist's artwork is the file id of a picture, never a URL".into());
    }
    // The same rules a catalogue entry's tags are held to, so a word that can be
    // stored here is one that could be published.
    crate::normalise_tags(&playlist.tags)
        .map_err(|error| format!("a playlist's tags are invalid: {error}"))
}

/// 1 to 500 members, positions contiguous from 1, file ids unique and lowercase.
///
/// The position check is what makes `tracks` order meaningful on its own: the
/// members of a playlist arrive in that order, and a playlist that skipped a
/// position would play in an order nobody chose. The lower bound is the event's:
/// the store also holds empty drafts, so a caller holding one checks for it
/// first rather than being told its draft is malformed.
fn valid_members(members: &[RemotePlaylistTrack]) -> bool {
    if members.is_empty() || members.len() > PLAYLIST_MEMBER_LIMIT {
        return false;
    }
    let mut seen = HashSet::new();
    members.iter().enumerate().all(|(index, member)| {
        member.position as usize == index + 1
            && is_file_id(&member.file_id)
            && seen.insert(member.file_id.as_str())
    })
}

/// A file id is the lowercase hex SHA-256 the catalogue uses everywhere.
fn is_file_id(value: &str) -> bool {
    value.len() == 64
        && value
            .chars()
            .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character))
}

/// A canonical lowercase UUID in `8-4-4-4-12` form. Any version is accepted, as
/// the NIP requires, because v4 and v7 are both in use.
fn is_canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value.chars().enumerate().all(|(index, character)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                character == '-'
            } else {
                character.is_ascii_digit() || ('a'..='f').contains(&character)
            }
        })
}

/// A playlist id that has not been published yet.
pub fn new_playlist_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Keep `false` out of a published body, so the JSON this host writes looks like
/// the example in the NIP rather than carrying a field that says nothing.
fn is_false(value: &bool) -> bool {
    !*value
}

/// The author's own words, as an untrusted event carries them.
///
/// Sanitised and bounded rather than rejected: these words only decide whether a
/// playlist can be found, and a stray control character in one is not a reason
/// to refuse a playlist a person may be playing. What survives here is what the
/// catalogue could itself have carried, so a reader never stores tags it could
/// not have written.
fn playlist_tags(value: &str) -> String {
    let mut seen = HashSet::new();
    let mut words = Vec::new();
    for word in value.split(',').map(str::trim).filter(|word| !word.is_empty()) {
        let word = crate::audio::sanitise_public_text(word);
        let word = word.trim();
        if word.is_empty() || word.chars().count() > PLAYLIST_TAG_CHARACTER_LIMIT {
            continue;
        }
        if seen.insert(word.to_lowercase()) {
            words.push(word.to_string());
        }
        if words.len() == PLAYLIST_TAG_LIMIT {
            break;
        }
    }
    words.join(", ")
}

/// Untrusted display text: stripped of control and bidirectional formatting
/// characters, bounded, and trimmed. Never rejected, because it is informational.
fn playlist_text(value: &str) -> String {
    crate::audio::sanitise_public_text(value)
        .chars()
        .take(PLAYLIST_TEXT_CHARACTER_LIMIT)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Read the first value of a tag by its raw name, so optional tags such as
/// `title` need no dedicated `TagKind` constructor.
fn tag_text(event: &Event, name: &str) -> Option<String> {
    event
        .tags
        .iter()
        .find(|tag| tag.kind() == TagKind::from(name))
        .and_then(|tag| tag.content())
        .map(|value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAYLIST_ID: &str = "77abf082-7075-4d36-afe2-e9710ac6b33c";
    const ENTER_SANDMAN: &str =
        "48e5979efa6a56dc3cab293b954ae84e36f464b936bd8c534838189abcc93c68";
    const ROOSTER: &str = "cdf1741591bf1e580b1e7a2712ce781ef7ade8b12ebf309731d264498accf5ce";

    fn playlist() -> RemotePlaylist {
        RemotePlaylist {
            playlist_id: PLAYLIST_ID.into(),
            title: "rock".into(),
            author: "a".repeat(64),
            display_name: "Sean Parker".into(),
            artist: String::new(),
            mbid: String::new(),
            image: String::new(),
            tags: String::new(),
            private: false,
            published: false,
            updated_at: 1_787_680_200,
            tracks: vec![
                RemotePlaylistTrack {
                    position: 1,
                    file_id: ENTER_SANDMAN.into(),
                    title: "Enter Sandman".into(),
                    artist: "Metallica".into(),
                    album: "Metallica".into(),
                },
                RemotePlaylistTrack {
                    position: 2,
                    file_id: ROOSTER.into(),
                    title: "Rooster".into(),
                    artist: "Alice In Chains".into(),
                    album: "Dirt".into(),
                },
            ],
            total: 2,
        }
    }

    /// The example in `NIP-NAPSTR-PLAYLIST.md`, verbatim, so the spec and the
    /// code are checked against the same bytes.
    fn spec_example_event() -> Event {
        let tags = vec![
            Tag::parse(["d", PLAYLIST_ID]).unwrap(),
            Tag::parse(["t", "napstr-playlist"]).unwrap(),
            Tag::parse(["title", "rock"]).unwrap(),
            Tag::parse(["alt", "Napstr public playlist"]).unwrap(),
            Tag::parse(["client", "Napstr"]).unwrap(),
            Tag::parse(["x", ENTER_SANDMAN]).unwrap(),
            Tag::parse(["x", ROOSTER]).unwrap(),
            Tag::parse(["t", "rock"]).unwrap(),
            Tag::parse(["t", "metallica"]).unwrap(),
        ];
        let content = format!(
            r#"{{"protocol":"napstr/1","playlistId":"{PLAYLIST_ID}","title":"rock","tracks":[{{"position":1,"fileId":"{ENTER_SANDMAN}","title":"Enter Sandman","artist":"Metallica","album":"Metallica"}},{{"position":2,"fileId":"{ROOSTER}","title":"Rooster","artist":"Alice In Chains"}}]}}"#
        );
        EventBuilder::new(Kind::from(PLAYLIST_KIND), content)
            .tags(tags)
            .sign_with_keys(&Keys::generate())
            .unwrap()
    }

    #[test]
    fn the_specs_example_playlist_validates() {
        let PlaylistEvent::Playlist(read) = playlist_event(&spec_example_event()).unwrap() else {
            panic!("a published playlist is not a withdrawal")
        };
        assert_eq!(read.playlist_id, PLAYLIST_ID);
        assert_eq!(read.title, "rock");
        assert_eq!(read.tracks.len(), 2);
        // Position order is what the playlist means, and the hints come with it.
        assert_eq!(read.tracks[0].file_id, ENTER_SANDMAN);
        assert_eq!(read.tracks[1].file_id, ROOSTER);
        assert_eq!(read.tracks[1].artist, "Alice In Chains");
        // An empty hint stays empty rather than being invented.
        assert_eq!(read.tracks[1].album, "");
        assert!(!read.private);
    }

    #[test]
    fn a_playlist_survives_the_publish_and_read_round_trip() {
        let keys = Keys::generate();
        let mut published = playlist();
        published.artist = "Metallica".into();
        published.mbid = "60691bed-fdd7-32f9-92dc-b151aac9e271".into();
        published.image = "9".repeat(64);
        let event = playlist_event_builder(&published, true, &keys).unwrap();
        let PlaylistEvent::Playlist(read) = playlist_event(&event).unwrap() else {
            panic!("a published playlist is not a withdrawal")
        };
        assert_eq!(read.title, published.title);
        assert_eq!(read.artist, published.artist);
        assert_eq!(read.mbid, published.mbid);
        assert_eq!(read.image, published.image, "the picture survives as a file id");
        assert_eq!(read.author, keys.public_key().to_hex());
        assert_eq!(read.updated_at, event.created_at.as_secs() as i64);
        assert_eq!(
            read.tracks,
            published.tracks,
            "the members and their order must come back unchanged"
        );
        // The picture is not a member: it gets no `x` tag, so a relay answering
        // "which playlists name this file" is not told about artwork.
        assert!(
            !event
                .tags
                .iter()
                .filter_map(|tag| tag.content())
                .any(|value| value == published.image),
            "artwork must not travel as a member"
        );
        // Every member gets its own `x` tag, in order, for relay-side lookups.
        let members = event
            .tags
            .iter()
            .filter(|tag| tag.kind() == TagKind::from("x"))
            .filter_map(|tag| tag.content())
            .collect::<Vec<_>>();
        assert_eq!(members, vec![ENTER_SANDMAN, ROOSTER]);
        // The marker, plus the words the title searches by - which is how a
        // client that does not know the playlist's id finds it at all.
        let hashtags = event.tags.hashtags().collect::<Vec<_>>();
        assert_eq!(hashtags[0], PLAYLIST_MARKER);
        assert!(hashtags.contains(&"rock"), "{hashtags:?}");
        // A `t` tag holding `napstr` would file this playlist under the
        // catalogue marker, so the tokeniser drops it.
        assert!(!hashtags.contains(&"napstr"), "{hashtags:?}");
        // A playlist with no album of its own says so by leaving the fields out,
        // so that what this host publishes is the example in the NIP.
        let bare = playlist_event_builder(&playlist(), true, &keys).unwrap();
        let shape: serde_json::Value = serde_json::from_str(&bare.content).unwrap();
        assert!(shape.get("mbid").is_none());
        assert!(shape.get("artist").is_none());
        assert!(shape.get("deleted").is_none());
        assert_eq!(shape["protocol"], "napstr/1");
        assert!(playlist_event(&bare).is_some());
    }

    #[test]
    fn a_withdrawal_is_told_apart_from_a_playlist() {
        let keys = Keys::generate();
        let event = playlist_withdrawal_builder(PLAYLIST_ID, &keys).unwrap();
        assert_eq!(
            playlist_event(&event),
            Some(PlaylistEvent::Withdrawn {
                playlist_id: PLAYLIST_ID.into(),
                author: keys.public_key().to_hex(),
            })
        );
        // A withdrawal is not a playlist, so nothing may store it as one.
        assert!(!matches!(
            playlist_event(&event),
            Some(PlaylistEvent::Playlist(_))
        ));
    }

    /// The author's own words are what a playlist is found by, and a suggestion
    /// never outranks a choice - including the choice of having none.
    #[test]
    fn the_authors_own_tags_are_the_search_words() {
        let keys = Keys::generate();
        let words = |event: &Event| event.tags.hashtags().map(str::to_string).collect::<Vec<_>>();

        // Their words, and nothing of ours on top of them.
        let mut mine = playlist();
        mine.tags = "driving, late night".into();
        let with_tags = playlist_event_builder(&mine, true, &keys).unwrap();
        assert_eq!(
            words(&with_tags),
            vec![
                PLAYLIST_MARKER.to_string(),
                "driving".into(),
                "late".into(),
                "night".into(),
            ],
            "the title must not be mined for words once the author has chosen some"
        );
        // They survive the round trip as their own, not as the `t` tags, so an
        // edit does not quietly adopt our suggestion as theirs.
        let Some(PlaylistEvent::Playlist(read)) = playlist_event(&with_tags) else {
            panic!("a published playlist is not a withdrawal")
        };
        assert_eq!(read.tags, "driving, late night");

        // No words of their own and no suggestions asked for: the marker alone,
        // which is all the format requires. A playlist nobody can search for by
        // name is a choice an author is allowed to make.
        let quiet = playlist_event_builder(&playlist(), false, &keys).unwrap();
        assert_eq!(words(&quiet), vec![PLAYLIST_MARKER.to_string()]);
        let Some(PlaylistEvent::Playlist(read)) = playlist_event(&quiet) else {
            panic!("a published playlist is not a withdrawal")
        };
        assert_eq!(read.tags, "", "silence must not read back as our words");

        // No words of their own, suggestions asked for: the title's words, so
        // that a playlist is still reachable by the name it shows.
        let helped = playlist_event_builder(&playlist(), true, &keys).unwrap();
        assert_eq!(
            words(&helped),
            vec![PLAYLIST_MARKER.to_string(), "rock".to_string()]
        );
        // And a suggestion is never stored as the author's own words.
        let Some(PlaylistEvent::Playlist(read)) = playlist_event(&helped) else {
            panic!("a published playlist is not a withdrawal")
        };
        assert_eq!(read.tags, "");
    }

    /// Tags are the catalogue's rules exactly, and a broken list is refused
    /// rather than published, because a word no client can carry is not a word.
    #[test]
    fn a_playlist_tag_list_is_held_to_the_catalogue_rules() {
        let keys = Keys::generate();
        for broken in [
            "a,b,c,d,e,f,g,h,i,j,k,l,m".to_string(),
            "x".repeat(33),
            "ok,\u{202e}reversed".to_string(),
        ] {
            let mut mine = playlist();
            mine.tags = broken.clone();
            assert!(
                playlist_event_builder(&mine, true, &keys).is_err(),
                "{broken} must not be publishable"
            );
        }
        // What the reader takes from an untrusted event is trimmed to the same
        // rules instead of refusing the playlist: these words only decide whether
        // it can be found, and somebody may be playing it right now.
        assert_eq!(playlist_tags("ok, Ok,   , "), "ok", "blanks and repeats go");
        assert_eq!(playlist_tags(&"x".repeat(40)), "", "an over-long word goes");
        assert_eq!(playlist_tags("a\u{202e}b"), "a b", "formatting characters go");
        let many = (1..=13)
            .map(|index| format!("w{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        assert_eq!(playlist_tags(&many).split(", ").count(), PLAYLIST_TAG_LIMIT);
    }

    #[test]
    fn a_private_playlist_is_never_published() {
        let keys = Keys::generate();
        let mut private = playlist();
        private.private = true;
        // A private playlist has no wire format at all. Publishing one would put
        // a coordinate, a size and an edit time on a relay - the existence and
        // the scale of what it exists to conceal - and a companion loses nothing
        // by it, because its own computer is the source.
        let refused = playlist_event_builder(&private, true, &keys).unwrap_err();
        assert!(refused.contains("never published"), "{refused}");
        assert!(refused.contains("stays on this computer"), "{refused}");
        // And the store keeps the flag, which is the one thing that travels with
        // a playlist rather than being derived from where it was found.
        let directory =
            std::env::temp_dir().join(format!("napstr-playlist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let db_path = directory.join("napstr.sqlite3");
        crate::initialise_database(&db_path, &directory).unwrap();
        let connection = crate::open_connection(&db_path).unwrap();
        save(&connection, &private).unwrap();
        let (listed, total) = list(&connection, 0, 10).unwrap();
        assert_eq!(total, 1);
        assert!(listed[0].private);
        assert_eq!(listed[0].track_count, 2);
        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// Every requirement the NIP puts on a consumer, one broken playlist each.
    #[test]
    fn a_broken_playlist_is_refused() {
        let keys = Keys::generate();
        let rebuild = |content: String, tags: Vec<Tag>| {
            EventBuilder::new(Kind::from(PLAYLIST_KIND), content)
                .tags(tags)
                .sign_with_keys(&keys)
                .unwrap()
        };
        let body = |playlist_id: &str| {
            format!(
                r#"{{"protocol":"napstr/1","playlistId":"{playlist_id}","title":"rock","tracks":[{{"position":1,"fileId":"{ENTER_SANDMAN}"}}]}}"#
            )
        };
        let id_tags = |playlist_id: &str| {
            vec![
                Tag::parse(["d", playlist_id]).unwrap(),
                Tag::parse(["t", PLAYLIST_MARKER]).unwrap(),
                Tag::parse(["title", "rock"]).unwrap(),
            ]
        };
        // The harness itself, so that a refusal below means something.
        assert!(playlist_event(&rebuild(body(PLAYLIST_ID), id_tags(PLAYLIST_ID))).is_some());
        // The marker is the only supported discovery path, so an event without
        // it is not ours - which is what protects a kind other protocols share.
        assert!(playlist_event(&rebuild(
            body(PLAYLIST_ID),
            vec![
                Tag::parse(["d", PLAYLIST_ID]).unwrap(),
                Tag::parse(["title", "rock"]).unwrap(),
            ],
        ))
        .is_none());
        // `x` tags are for relay-side lookups and are never authoritative, so a
        // stray one is ignored rather than added or refused.
        let stray = {
            let mut tags = id_tags(PLAYLIST_ID);
            tags.push(Tag::parse(["x", ROOSTER]).unwrap());
            rebuild(body(PLAYLIST_ID), tags)
        };
        let Some(PlaylistEvent::Playlist(parsed)) = playlist_event(&stray) else {
            panic!("a stray x tag does not make a playlist unreadable")
        };
        assert_eq!(parsed.tracks.len(), 1);
        // A title tag that disagrees with the content is not the same playlist.
        let mut mismatched = id_tags(PLAYLIST_ID);
        mismatched[2] = Tag::parse(["title", "jazz"]).unwrap();
        assert!(playlist_event(&rebuild(body(PLAYLIST_ID), mismatched)).is_none());
        // Neither is an empty one.
        let untitled = body(PLAYLIST_ID).replace(r#""title":"rock""#, r#""title":"""#);
        assert!(playlist_event(&rebuild(untitled, id_tags(PLAYLIST_ID))).is_none());
        // The `d` tag cannot address a coordinate the author did not sign for.
        assert!(playlist_event(&rebuild(
            body(PLAYLIST_ID),
            id_tags("0f9de6a1-5c2e-47a8-9a3c-2f6f8f1b7d40")
        ))
        .is_none());
        // Nor can an id be anything but a canonical lowercase UUID. The tag and
        // the body agree in each of these, so it is the id itself being refused.
        for bad in [
            String::new(),
            "77abf0827075".to_string(),
            "77ABF082-7075-4D36-AFE2-E9710AC6B33C".to_string(),
            "x".repeat(36),
        ] {
            assert!(
                playlist_event(&rebuild(body(&bad), id_tags(&bad))).is_none(),
                "{bad} must not be a playlist id"
            );
        }
        // Members: none, a duplicate, positions that skip one, positions that
        // run backwards, an uppercase id, a short id, and one too many.
        let one = |position: u32, file_id: &str| {
            format!(r#"{{"position":{position},"fileId":"{file_id}"}}"#)
        };
        let tracks_body = |tracks: &str| {
            format!(
                r#"{{"protocol":"napstr/1","playlistId":"{PLAYLIST_ID}","title":"rock","tracks":[{tracks}]}}"#
            )
        };
        let broken = [
            String::new(),
            format!("{},{}", one(1, ENTER_SANDMAN), one(2, ENTER_SANDMAN)),
            format!("{},{}", one(2, ENTER_SANDMAN), one(3, ROOSTER)),
            format!("{},{}", one(2, ENTER_SANDMAN), one(1, ROOSTER)),
            one(1, &"A".repeat(64)),
            one(1, &"a".repeat(63)),
            (1..=PLAYLIST_MEMBER_LIMIT + 1)
                .map(|index| one(index as u32, &format!("{index:064x}")))
                .collect::<Vec<_>>()
                .join(","),
        ];
        for (index, tracks) in broken.iter().enumerate() {
            assert!(
                playlist_event(&rebuild(tracks_body(tracks), id_tags(PLAYLIST_ID))).is_none(),
                "broken member list {index} was accepted: {}",
                &tracks[..tracks.len().min(60)]
            );
        }
        // The protocol literal, the byte budget, and the kind itself.
        let wrong_protocol = body(PLAYLIST_ID).replace("napstr/1", "napstr/2");
        assert!(playlist_event(&rebuild(wrong_protocol, id_tags(PLAYLIST_ID))).is_none());
        let padded = tracks_body(&format!(
            "{},{{\"position\":2,\"fileId\":\"{ROOSTER}\",\"title\":\"{}\"}}",
            one(1, ENTER_SANDMAN),
            "t".repeat(PLAYLIST_CONTENT_BYTE_LIMIT)
        ));
        assert!(playlist_event(&rebuild(padded, id_tags(PLAYLIST_ID))).is_none());
        // Another protocol's event that happens to use this kind, which is the
        // case the marker exists for.
        let foreign = rebuild(body(PLAYLIST_ID), vec![Tag::parse(["d", PLAYLIST_ID]).unwrap()]);
        assert!(playlist_event(&foreign).is_none());
        // And our own body under a kind that is not ours: kind `30423` carries
        // audiobook manifests, which share this envelope's shape.
        let wrong_kind = EventBuilder::new(Kind::from(30423), body(PLAYLIST_ID))
            .tags(id_tags(PLAYLIST_ID))
            .sign_with_keys(&keys)
            .unwrap();
        assert!(playlist_event(&wrong_kind).is_none());
    }

    #[test]
    fn a_playlist_stores_its_members_in_position_order_and_replaces_them_wholesale() {
        let directory =
            std::env::temp_dir().join(format!("napstr-playlist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let db_path = directory.join("napstr.sqlite3");
        crate::initialise_database(&db_path, &directory).unwrap();
        let connection = crate::open_connection(&db_path).unwrap();

        let mut stored = playlist();
        stored.display_name = "Sean Parker".into();
        save(&connection, &stored).unwrap();
        // The same coordinate saved again is a revision, not a second playlist.
        let mut revised = stored.clone();
        revised.title = "rock and roll".into();
        revised.tracks.remove(1);
        revised.total = 1;
        save(&connection, &revised).unwrap();

        let (listed, total) = list(&connection, 0, 10).unwrap();
        assert_eq!(total, 1);
        assert_eq!(listed[0].title, "rock and roll");
        assert_eq!(listed[0].track_count, 1);
        assert_eq!(listed[0].display_name, "Sean Parker");
        // A list row draws the album the playlist opens with, and it is told
        // which file that is without being handed the members.
        assert_eq!(listed[0].first_file_id, ENTER_SANDMAN);
        // The summary carries the author, because it is half the coordinate a
        // companion has to send back to ask for this playlist again.
        assert_eq!(listed[0].author, stored.author);
        let stored_page = page(&connection, &stored.author, PLAYLIST_ID, 0, 10)
            .unwrap()
            .unwrap();
        assert_eq!(stored_page.total, 1, "a dropped member is gone, not left behind");
        assert_eq!(stored_page.tracks.len(), 1);
        assert_eq!(stored_page.tracks[0].file_id, ENTER_SANDMAN);
        // A caller that names only an id is answered while it is unambiguous.
        assert!(page(&connection, "", PLAYLIST_ID, 0, 10).unwrap().is_some());
        // Paging is by position, so a page boundary never reorders anyone.
        let mut long = playlist();
        long.playlist_id = "0f9de6a1-5c2e-47a8-9a3c-2f6f8f1b7d40".into();
        long.tracks = (1..=5)
            .map(|index| RemotePlaylistTrack {
                position: index,
                file_id: format!("{index:064x}"),
                title: String::new(),
                artist: String::new(),
                album: String::new(),
            })
            .collect();
        long.total = 5;
        save(&connection, &long).unwrap();
        let second = page(&connection, &long.author, &long.playlist_id, 1, 2)
            .unwrap()
            .unwrap();
        assert_eq!(
            second.tracks.iter().map(|track| track.position).collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(second.total, 5);
        // A list is newest first, and paging it cannot lose or repeat a row.
        let (first_page, total) = list(&connection, 0, 1).unwrap();
        assert_eq!(total, 2);
        let (second_page, _) = list(&connection, 1, 1).unwrap();
        assert_eq!(first_page.len(), 1);
        assert_eq!(second_page.len(), 1);
        assert_ne!(first_page[0].playlist_id, second_page[0].playlist_id);

        remove(&connection, &stored.author, PLAYLIST_ID).unwrap();
        assert!(page(&connection, &stored.author, PLAYLIST_ID, 0, 10)
            .unwrap()
            .is_none());
        assert_eq!(list(&connection, 0, 10).unwrap().1, 1);
        // Nothing about a playlist may be stored half-way.
        let mut broken = playlist();
        broken.tracks[1].position = 3;
        assert!(save(&connection, &broken).is_err());
        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// A track's "add to playlist" picker needs to know which playlists already
    /// hold it, and it has to be able to say "none of them" as readily as it says
    /// "these three".
    #[test]
    fn which_playlists_hold_a_file_is_answered_by_coordinate() {
        let directory =
            std::env::temp_dir().join(format!("napstr-playlist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let db_path = directory.join("napstr.sqlite3");
        crate::initialise_database(&db_path, &directory).unwrap();
        let connection = crate::open_connection(&db_path).unwrap();

        let mine = playlist();
        save(&connection, &mine).unwrap();
        // A second author under the same id: whoever asks has to be told which
        // coordinate holds the file, not just that some playlist with that id
        // does.
        let mut theirs = playlist();
        theirs.author = "b".repeat(64);
        let mut other = playlist();
        other.playlist_id = "0f9de6a1-5c2e-47a8-9a3c-2f6f8f1b7d40".into();
        other.tracks = vec![RemotePlaylistTrack {
            position: 1,
            file_id: ENTER_SANDMAN.into(),
            ..Default::default()
        }];
        other.total = 1;
        save(&connection, &other).unwrap();
        save(&connection, &theirs).unwrap();

        let holding = containing(&connection, ENTER_SANDMAN).unwrap();
        assert_eq!(
            holding,
            vec![
                RemotePlaylistCoordinate {
                    author: "a".repeat(64),
                    playlist_id: other.playlist_id.clone(),
                },
                RemotePlaylistCoordinate {
                    author: "a".repeat(64),
                    playlist_id: PLAYLIST_ID.into(),
                },
                RemotePlaylistCoordinate {
                    author: "b".repeat(64),
                    playlist_id: PLAYLIST_ID.into(),
                },
            ],
            "one coordinate per playlist that names the file, whichever author holds it"
        );

        // A file nothing names is an empty answer rather than an error: that is
        // the state every track starts in.
        assert!(containing(&connection, &"e".repeat(64)).unwrap().is_empty());

        // Dropping the member is what takes the playlist back out of the answer,
        // which is what makes the picker's toggle tell the truth on the next ask.
        // Positions are the order on screen, so a removal renumbers the rest -
        // the store refuses a gap rather than quietly keeping one.
        let mut emptied = mine.clone();
        emptied.tracks = emptied
            .tracks
            .into_iter()
            .skip(1)
            .enumerate()
            .map(|(index, member)| RemotePlaylistTrack {
                position: index as u32 + 1,
                ..member
            })
            .collect();
        emptied.total = emptied.tracks.len();
        save(&connection, &emptied).unwrap();
        assert_eq!(
            containing(&connection, ENTER_SANDMAN).unwrap().len(),
            2,
            "the member that was dropped is not still counted"
        );

        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// A playlist this computer is still building has no members yet, and that is
    /// a draft rather than a broken playlist: the "1 to 500 members" rule belongs
    /// to the published event, so the store keeps the draft and the publisher is
    /// the thing that refuses it.
    #[test]
    fn a_playlist_may_be_an_empty_draft_but_never_an_empty_event() {
        let directory =
            std::env::temp_dir().join(format!("napstr-playlist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let db_path = directory.join("napstr.sqlite3");
        crate::initialise_database(&db_path, &directory).unwrap();
        let connection = crate::open_connection(&db_path).unwrap();

        let mut draft = playlist();
        draft.tracks.clear();
        draft.total = 0;
        save(&connection, &draft).unwrap();
        let read_back = page(&connection, &draft.author, PLAYLIST_ID, 0, 10)
            .unwrap()
            .unwrap();
        assert!(read_back.tracks.is_empty());
        assert_eq!(read_back.total, 0);
        assert!(
            !read_back.published,
            "nothing has been signed, so nothing of it is out there"
        );
        assert_eq!(list(&connection, 0, 10).unwrap().0[0].track_count, 0);
        assert!(playlist_event_builder(&draft, false, &Keys::generate()).is_err());

        // `published` is a stored fact about the coordinate, not about this
        // revision: editing a playlist that is already on the relays leaves it
        // true, or the next save would have the author believe their published
        // playlist had never been sent anywhere.
        draft.published = true;
        save(&connection, &draft).unwrap();
        let stored = page(&connection, &draft.author, PLAYLIST_ID, 0, 10)
            .unwrap()
            .unwrap();
        assert!(stored.published);
        assert!(list(&connection, 0, 10).unwrap().0[0].published);

        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// Artwork is the file id of a picture, and a URL is not a file id.
    ///
    /// A reader ignores a URL rather than fetching it, because fetching it would
    /// tell a stranger that this listener, on this machine, is looking at this
    /// playlist. A publisher is refused instead of ignored, so that a client that
    /// meant to attach a picture hears about it rather than shipping a playlist
    /// that silently has none.
    #[test]
    fn artwork_is_the_file_id_of_a_picture_and_never_a_url() {
        let directory =
            std::env::temp_dir().join(format!("napstr-playlist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let db_path = directory.join("napstr.sqlite3");
        crate::initialise_database(&db_path, &directory).unwrap();
        let connection = crate::open_connection(&db_path).unwrap();

        let picture = "9".repeat(64);
        let mut stored = playlist();
        stored.image = picture.clone();
        save(&connection, &stored).unwrap();
        let read_back = page(&connection, &stored.author, PLAYLIST_ID, 0, 10)
            .unwrap()
            .unwrap();
        assert_eq!(read_back.image, picture);
        assert_eq!(list(&connection, 0, 10).unwrap().0[0].image, picture);

        let mut linked = playlist();
        linked.image = "https://example.com/art.png".into();
        assert!(save(&connection, &linked).is_err());
        assert!(playlist_event_builder(&linked, false, &Keys::generate()).is_err());

        // The reader's side of the same rule: an event carrying a link still
        // parses as a playlist, and the field is simply not there.
        let tags = vec![
            Tag::parse(["d", PLAYLIST_ID]).unwrap(),
            Tag::parse(["t", PLAYLIST_MARKER]).unwrap(),
            Tag::parse(["title", "rock"]).unwrap(),
        ];
        let content = format!(
            r#"{{"protocol":"napstr/1","playlistId":"{PLAYLIST_ID}","title":"rock","image":"https://example.com/art.png","tracks":[{{"position":1,"fileId":"{ENTER_SANDMAN}"}}]}}"#
        );
        let event = EventBuilder::new(Kind::from(PLAYLIST_KIND), content)
            .tags(tags)
            .sign_with_keys(&Keys::generate())
            .unwrap();
        let Some(PlaylistEvent::Playlist(read)) = playlist_event(&event) else {
            panic!("a playlist that names a link is still a playlist")
        };
        assert_eq!(read.image, "");
        assert_eq!(read.tracks.len(), 1);

        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// A withdrawal is the author's last word about a coordinate.
    ///
    /// A relay that never received it keeps serving the older revision, so the
    /// fact that the coordinate was withdrawn is what stops this computer from
    /// storing it again - and an author who republishes afterwards is a live
    /// playlist rather than a withdrawal that outlives its author's mind.
    #[test]
    fn a_withdrawn_coordinate_is_never_offered_again() {
        let directory =
            std::env::temp_dir().join(format!("napstr-playlist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let db_path = directory.join("napstr.sqlite3");
        crate::initialise_database(&db_path, &directory).unwrap();
        let connection = crate::open_connection(&db_path).unwrap();

        let theirs = |updated_at: i64| RemotePlaylist {
            updated_at,
            ..playlist()
        };
        assert!(store_from_relay(&connection, &theirs(1_000)).unwrap());
        assert_eq!(list(&connection, 0, 10).unwrap().1, 1);

        // The withdrawal is what the relays answer with from now on, and the
        // revision it replaced is gone with it.
        mark_withdrawn(&connection, &theirs(0).author, PLAYLIST_ID, 1_100).unwrap();
        remove(&connection, &theirs(0).author, PLAYLIST_ID).unwrap();
        assert_eq!(withdrawn_at(&connection, &theirs(0).author, PLAYLIST_ID).unwrap(), Some(1_100));
        // A relay that still answers with the older revision cannot put it back.
        assert!(!store_from_relay(&connection, &theirs(1_000)).unwrap());
        assert_eq!(list(&connection, 0, 10).unwrap().1, 0);
        assert!(page(&connection, &theirs(0).author, PLAYLIST_ID, 0, 10)
            .unwrap()
            .is_none());
        // An author who publishes the coordinate again is newer than the
        // withdrawal, which is the one thing a withdrawal does not forbid.
        assert!(store_from_relay(&connection, &theirs(1_200)).unwrap());
        assert_eq!(list(&connection, 0, 10).unwrap().1, 1);

        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// An older revision from a relay must not undo a newer one already here.
    #[test]
    fn a_relay_cannot_talk_this_computer_back_to_an_older_revision() {
        let directory =
            std::env::temp_dir().join(format!("napstr-playlist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let db_path = directory.join("napstr.sqlite3");
        crate::initialise_database(&db_path, &directory).unwrap();
        let connection = crate::open_connection(&db_path).unwrap();

        let mut newer = playlist();
        newer.author = "b".repeat(64);
        newer.updated_at = 2_000;
        newer.title = "the newest one".into();
        assert!(store_from_relay(&connection, &newer).unwrap());
        let mut older = newer.clone();
        older.updated_at = 1_500;
        older.title = "what a slow relay still has".into();
        assert!(!store_from_relay(&connection, &older).unwrap());
        let (listed, _) = list(&connection, 0, 10).unwrap();
        assert_eq!(listed[0].title, "the newest one");
        // The same revision again is not older than itself, so re-reading the
        // relays is idempotent rather than a fight over which copy wins.
        assert!(store_from_relay(&connection, &newer).unwrap());

        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// Editing a playlist somebody else wrote makes a copy of its own.
    ///
    /// An id is what its author chose and what a reader sends back, so a
    /// revision of somebody else's coordinate is never signed for and never
    /// filed under it: it becomes this identity's playlist under an id of its
    /// own, which is what stops two authors' rows from being confused for one.
    #[test]
    fn a_revision_of_another_authors_playlist_becomes_a_copy() {
        let directory =
            std::env::temp_dir().join(format!("napstr-playlist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let db_path = directory.join("napstr.sqlite3");
        crate::initialise_database(&db_path, &directory).unwrap();
        let connection = crate::open_connection(&db_path).unwrap();

        let mine = "c".repeat(64);
        let mut theirs = playlist();
        theirs.author = "b".repeat(64);
        theirs.published = true;
        save(&connection, &theirs).unwrap();

        let copy = file_revision(&connection, theirs.clone(), &mine, 99).unwrap();
        assert_ne!(copy.playlist_id, theirs.playlist_id, "a copy needs its own id");
        assert!(is_canonical_uuid(&copy.playlist_id));
        assert_eq!(copy.author, mine);
        assert_eq!(copy.updated_at, 99);
        assert!(
            !copy.published,
            "nothing of the copy has been signed by this identity"
        );
        assert_eq!(copy.title, theirs.title);
        assert_eq!(copy.tracks, theirs.tracks);
        assert!(page(&connection, &mine, &copy.playlist_id, 0, 10)
            .unwrap()
            .is_some());
        // Their own revision is untouched by the copy.
        assert!(page(&connection, &theirs.author, &theirs.playlist_id, 0, 10)
            .unwrap()
            .is_some());

        // A revision of this identity's own playlist keeps its coordinate, or
        // an edit would leave a second row behind under a new id.
        let revision = file_revision(&connection, copy.clone(), &mine, 120).unwrap();
        assert_eq!(revision.playlist_id, copy.playlist_id);
        assert_eq!(list(&connection, 0, 10).unwrap().1, 2);

        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// The id is chosen by the author, so it is not the identity on its own.
    #[test]
    fn two_authors_may_use_one_playlist_id_without_replacing_each_other() {
        let directory =
            std::env::temp_dir().join(format!("napstr-playlist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let db_path = directory.join("napstr.sqlite3");
        crate::initialise_database(&db_path, &directory).unwrap();
        let connection = crate::open_connection(&db_path).unwrap();

        let mine = playlist();
        let mut theirs = playlist();
        theirs.author = "b".repeat(64);
        theirs.title = "not rock".into();
        theirs.tracks = vec![RemotePlaylistTrack {
            position: 1,
            file_id: ROOSTER.into(),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
        }];
        theirs.total = 1;
        save(&connection, &mine).unwrap();
        save(&connection, &theirs).unwrap();

        assert_eq!(list(&connection, 0, 10).unwrap().1, 2, "one coordinate each");
        let mine_page = page(&connection, &mine.author, PLAYLIST_ID, 0, 10)
            .unwrap()
            .unwrap();
        assert_eq!(mine_page.title, "rock");
        assert_eq!(
            mine_page.tracks.len(),
            2,
            "another author's revision must not shorten this one"
        );
        let theirs_page = page(&connection, &theirs.author, PLAYLIST_ID, 0, 10)
            .unwrap()
            .unwrap();
        assert_eq!(theirs_page.title, "not rock");
        // An id alone is refused while two authors share it, rather than
        // answering with whichever row the store happened to find first.
        assert!(page(&connection, "", PLAYLIST_ID, 0, 10).is_err());
        // Withdrawing one leaves the other alone.
        remove(&connection, &theirs.author, PLAYLIST_ID).unwrap();
        assert!(page(&connection, &theirs.author, PLAYLIST_ID, 0, 10)
            .unwrap()
            .is_none());
        assert!(page(&connection, &mine.author, PLAYLIST_ID, 0, 10)
            .unwrap()
            .is_some());
        // And an id alone is answered again once it is unambiguous.
        assert_eq!(
            page(&connection, "", PLAYLIST_ID, 0, 10)
                .unwrap()
                .unwrap()
                .author,
            mine.author
        );
        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_new_playlist_id_is_a_canonical_uuid() {
        let id = new_playlist_id();
        assert!(is_canonical_uuid(&id), "{id} is not a canonical UUID");
        assert_ne!(id, new_playlist_id());
    }
}
