//! Album art bytes, held by the computer that fetched them.
//!
//! The claims in `cover.rs` carry URLs; this holds the pictures those URLs point
//! at, so a paired phone can be handed pixels instead of an address. Before this
//! existed, the phone asked the host *which* art an album had and then fetched
//! the pixels itself, which told `is1-ssl.mzstatic.com` and
//! `coverartarchive.org` the phone's address and the album it was looking at.
//!
//! Two properties shape everything here:
//!
//! * **Content-addressed.** A file is named by the SHA-256 of its own bytes, so
//!   storing one cover twice costs one file, a cover that changes becomes a
//!   different file rather than a stale one, and the phone can verify what it was
//!   sent without being told what to expect. Two albums with identical art — a
//!   single and the album it came from — share one file.
//! * **Validated on the way in.** What a URL claims to be and what its bytes are
//!   need not agree, and these bytes are served to a webview from a local
//!   address. The magic bytes decide the mime, and anything that is not an image
//!   is refused rather than cached. A file the cache serves therefore can never
//!   be something other than what it says it is.
//!
//! Nothing here fetches. The pass that fills the cache lives with the cover
//! worker, which already knows which albums are worth asking about and how to be
//! polite to the services that answer.

use napstr_remote_protocol::ArtRendition;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The directory the bytes live in, under the app's cache directory.
pub const ART_DIRECTORY: &str = "art";

/// One rendition of one album's art, as it sits on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedArt {
    /// Lowercase SHA-256 of the bytes, which is also the file's name.
    pub hash: String,
    pub mime: String,
    pub bytes: u64,
    pub path: PathBuf,
}

/// What the cache is holding, for the Covers view.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtCacheStats {
    /// Rows, which is renditions of albums rather than files: two albums sharing
    /// a picture are two entries and one file.
    pub entries: usize,
    /// Files on disk.
    pub files: usize,
    pub bytes: u64,
}

/// The table that maps an album and a rendition to the bytes that answer it.
///
/// `used_at` is what eviction orders by, and `source` is kept so the Covers view
/// can say where a picture came from rather than only that it exists.
pub fn initialise_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS art_cache (
               cover_key TEXT NOT NULL,
               rendition TEXT NOT NULL,
               hash TEXT NOT NULL,
               mime TEXT NOT NULL,
               bytes INTEGER NOT NULL,
               source TEXT NOT NULL DEFAULT '',
               fetched_at TEXT NOT NULL,
               used_at TEXT NOT NULL,
               PRIMARY KEY(cover_key, rendition)
             );
             CREATE INDEX IF NOT EXISTS art_cache_used ON art_cache(used_at);
             CREATE INDEX IF NOT EXISTS art_cache_hash ON art_cache(hash);",
        )
        .map_err(|error| error.to_string())
}

/// The mime these bytes really are, from their own first bytes.
///
/// Only the formats a cover is published in: JPEG because the Cover Art Archive
/// and Apple both serve it, PNG because the archive holds some scans as PNG, and
/// WebP because it is what a smaller rendition is likely to be. `None` means
/// "not a picture", and the caller refuses it.
pub fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    // WebP is `RIFF` + four length bytes + `WEBP`, so the tag is not at the
    // start and the length has to be skipped over.
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// The file suffix a mime gets. Only ever chosen by [`image_mime`], so the
/// fallback is unreachable rather than a guess.
fn extension_for(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/webp" => "webp",
        _ => "jpg",
    }
}

/// The `rendition` column's value for a rendition.
pub fn rendition_name(rendition: ArtRendition) -> &'static str {
    match rendition {
        ArtRendition::Thumb => "thumb",
        ArtRendition::Full => "full",
    }
}

/// Keep these bytes for one album's rendition, and say where they are.
///
/// Idempotent: the file is named by the hash of its own bytes, so storing the
/// same picture again writes nothing and leaves the row where it was. If the
/// album's art has changed, the row moves to the new hash and the old file is
/// removed when nothing else points at it.
pub fn store(
    connection: &Connection,
    root: &Path,
    key: &str,
    rendition: ArtRendition,
    bytes: &[u8],
    source: &str,
) -> Result<CachedArt, String> {
    let mime = image_mime(bytes)
        .ok_or_else(|| "those bytes are not an image this cache will hold".to_string())?;
    let hash = hex::encode(Sha256::digest(bytes));
    let path = root.join(format!("{hash}.{}", extension_for(mime)));
    let previous: Option<String> = connection
        .query_row(
            "SELECT hash FROM art_cache WHERE cover_key=?1 AND rendition=?2",
            params![key, rendition_name(rendition)],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;

    if !path.exists() {
        std::fs::create_dir_all(root).map_err(|error| error.to_string())?;
        // Written under a temporary name and moved into place, so a file that
        // exists is always a whole picture: an interrupted write cannot leave a
        // half-sized file that the cache then serves.
        let staging = root.join(format!("{hash}.part"));
        std::fs::write(&staging, bytes).map_err(|error| error.to_string())?;
        std::fs::rename(&staging, &path).map_err(|error| error.to_string())?;
    }

    let now = chrono::Utc::now().to_rfc3339();
    connection
        .execute(
            "INSERT INTO art_cache(cover_key,rendition,hash,mime,bytes,source,fetched_at,used_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?7)
             ON CONFLICT(cover_key,rendition) DO UPDATE SET
               hash=excluded.hash, mime=excluded.mime, bytes=excluded.bytes,
               source=excluded.source, fetched_at=excluded.fetched_at, used_at=excluded.used_at",
            params![
                key,
                rendition_name(rendition),
                hash,
                mime,
                bytes.len() as i64,
                source,
                now
            ],
        )
        .map_err(|error| error.to_string())?;

    if let Some(previous) = previous.filter(|previous| *previous != hash) {
        drop_unreferenced(connection, root, &previous)?;
    }
    Ok(CachedArt {
        hash,
        mime: mime.to_string(),
        bytes: bytes.len() as u64,
        path,
    })
}

/// The bytes held for one album's rendition, if they are still there.
///
/// A row whose file has been deleted behind the cache's back — someone emptied
/// the directory, or a disk was cleaned by hand — is removed as it is found and
/// answered as absent. Clearing the cache is a supported action, so finding it
/// already clear must not be an error a caller has to handle.
pub fn lookup(
    connection: &Connection,
    root: &Path,
    key: &str,
    rendition: ArtRendition,
) -> Result<Option<CachedArt>, String> {
    let row: Option<(String, String, i64)> = connection
        .query_row(
            "SELECT hash,mime,bytes FROM art_cache WHERE cover_key=?1 AND rendition=?2",
            params![key, rendition_name(rendition)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((hash, mime, bytes)) = row else {
        return Ok(None);
    };
    let path = root.join(format!("{hash}.{}", extension_for(&mime)));
    if !path.exists() {
        connection
            .execute(
                "DELETE FROM art_cache WHERE cover_key=?1 AND rendition=?2",
                params![key, rendition_name(rendition)],
            )
            .map_err(|error| error.to_string())?;
        return Ok(None);
    }
    Ok(Some(CachedArt {
        hash,
        mime,
        bytes: bytes.max(0) as u64,
        path,
    }))
}

/// Note that these bytes were just used, which is what eviction orders by.
pub fn touch(
    connection: &Connection,
    key: &str,
    rendition: ArtRendition,
) -> Result<(), String> {
    connection
        .execute(
            "UPDATE art_cache SET used_at=?3 WHERE cover_key=?1 AND rendition=?2",
            params![
                key,
                rendition_name(rendition),
                chrono::Utc::now().to_rfc3339()
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Forget every rendition of one album. Returns how many entries went.
pub fn forget_album(connection: &Connection, root: &Path, key: &str) -> Result<usize, String> {
    let hashes: Vec<String> = connection
        .prepare("SELECT hash FROM art_cache WHERE cover_key=?1")
        .map_err(|error| error.to_string())?
        .query_map(params![key], |row| row.get(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let removed = connection
        .execute("DELETE FROM art_cache WHERE cover_key=?1", params![key])
        .map_err(|error| error.to_string())?;
    for hash in hashes {
        drop_unreferenced(connection, root, &hash)?;
    }
    Ok(removed)
}

/// What the cache is holding.
///
/// The file count is read from the directory rather than from the table,
/// because the difference between the two is exactly what a person clearing the
/// cache by hand would cause, and the Covers view should not lie about it.
pub fn stats(connection: &Connection, root: &Path) -> Result<ArtCacheStats, String> {
    let (entries, bytes): (i64, i64) = connection
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(bytes),0) FROM art_cache",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| error.to_string())?;
    let files = std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().is_file())
                .count()
        })
        .unwrap_or(0);
    Ok(ArtCacheStats {
        entries: entries.max(0) as usize,
        files,
        bytes: bytes.max(0) as u64,
    })
}

/// Remove a file once no entry points at it any more.
///
/// Two albums can share one picture, so the file may only go when the last row
/// naming it has. Called after a row moves or is deleted, never on its own.
fn drop_unreferenced(connection: &Connection, root: &Path, hash: &str) -> Result<(), String> {
    let referenced: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM art_cache WHERE hash=?1)",
            params![hash],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if referenced {
        return Ok(());
    }
    // The extension came from the mime at write time, so the file is found by
    // its stem rather than by guessing what it was called.
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stem == hash)
            {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of its own per test, so nothing depends on the order tests
    /// run in. Removed by the test that made it.
    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("napstr-art-cache-{name}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        initialise_schema(&connection).unwrap();
        connection
    }

    /// The smallest thing that passes for a JPEG: the three bytes that identify
    /// one, then whatever the test needs to make it unique.
    fn jpeg(tail: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8, 0xFF];
        bytes.extend_from_slice(tail);
        bytes
    }

    #[test]
    fn the_mime_comes_from_the_bytes_rather_than_from_the_url() {
        assert_eq!(image_mime(&jpeg(b"body")), Some("image/jpeg"));
        assert_eq!(
            image_mime(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00]),
            Some("image/png")
        );
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBP");
        assert_eq!(image_mime(&webp), Some("image/webp"));
        // Everything else is refused, including things that are pictures to a
        // human reader but not to this: the point is a mime this cache chose.
        assert_eq!(image_mime(b"<html><body>not a picture"), None);
        assert_eq!(image_mime(b""), None);
        // A `RIFF` header that is not WebP is a wave file, not a picture.
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&[0, 0, 0, 0]);
        wav.extend_from_slice(b"WAVE");
        assert_eq!(image_mime(&wav), None);
    }

    #[test]
    fn a_stored_image_is_found_by_its_own_hash() {
        let root = scratch("stored");
        let connection = database();
        let bytes = jpeg(b"a cover");
        let stored = store(
            &connection,
            &root,
            "kream|annihilation",
            ArtRendition::Thumb,
            &bytes,
            "musicbrainz",
        )
        .unwrap();
        assert_eq!(stored.hash, hex::encode(Sha256::digest(&bytes)));
        assert_eq!(stored.hash.len(), 64);
        assert_eq!(stored.mime, "image/jpeg");
        assert!(stored.path.ends_with(format!("{}.jpg", stored.hash)));
        assert!(stored.path.exists());

        let found = lookup(&connection, &root, "kream|annihilation", ArtRendition::Thumb)
            .unwrap()
            .expect("the bytes were just stored");
        assert_eq!(found, stored);
        // The other rendition of the same album is a different question, and the
        // answer to it is "not yet".
        assert!(lookup(&connection, &root, "kream|annihilation", ArtRendition::Full)
            .unwrap()
            .is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn bytes_that_are_not_an_image_are_refused_rather_than_cached() {
        let root = scratch("refused");
        let connection = database();
        let refused = store(
            &connection,
            &root,
            "kream|annihilation",
            ArtRendition::Thumb,
            b"<html>this is what a 404 page looks like</html>",
            "musicbrainz",
        );
        assert!(refused.is_err(), "a body that is not a picture must not land");
        assert!(
            lookup(&connection, &root, "kream|annihilation", ArtRendition::Thumb)
                .unwrap()
                .is_none()
        );
        assert_eq!(stats(&connection, &root).unwrap().entries, 0);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn two_albums_with_one_picture_share_one_file() {
        let root = scratch("shared");
        let connection = database();
        // The same sleeves published for a single and for the album it came off
        // is an ordinary case, and it should cost one file.
        let bytes = jpeg(b"one sleeve, two albums");
        let first = store(&connection, &root, "a|single", ArtRendition::Full, &bytes, "").unwrap();
        let second = store(&connection, &root, "a|album", ArtRendition::Full, &bytes, "").unwrap();
        assert_eq!(first.hash, second.hash);
        assert_eq!(stats(&connection, &root).unwrap().files, 1);
        assert_eq!(stats(&connection, &root).unwrap().entries, 2);

        // Forgetting one album must leave the other's picture alone.
        assert_eq!(forget_album(&connection, &root, "a|single").unwrap(), 1);
        assert!(first.path.exists(), "the other album still points at it");
        assert!(lookup(&connection, &root, "a|album", ArtRendition::Full)
            .unwrap()
            .is_some());
        // And forgetting the last one takes the file with it.
        assert_eq!(forget_album(&connection, &root, "a|album").unwrap(), 1);
        assert!(!first.path.exists(), "nothing points at it any more");
        assert_eq!(stats(&connection, &root).unwrap().files, 0);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn art_that_changes_replaces_the_row_and_takes_the_old_file() {
        let root = scratch("replaced");
        let connection = database();
        let older = jpeg(b"the first scan");
        let newer = jpeg(b"a better scan");
        let first = store(&connection, &root, "a|album", ArtRendition::Full, &older, "").unwrap();
        let second = store(&connection, &root, "a|album", ArtRendition::Full, &newer, "itunes").unwrap();
        assert_ne!(first.hash, second.hash);
        let found = lookup(&connection, &root, "a|album", ArtRendition::Full)
            .unwrap()
            .unwrap();
        assert_eq!(found.hash, second.hash, "the row moved to the new bytes");
        assert!(!first.path.exists(), "the abandoned file went with it");
        assert_eq!(first.hash, hex::encode(Sha256::digest(&older)));
        assert_eq!(stats(&connection, &root).unwrap().files, 1);
        // Storing the same bytes again is not a change and does not rewrite it.
        let again = store(&connection, &root, "a|album", ArtRendition::Full, &newer, "itunes").unwrap();
        assert_eq!(again.hash, second.hash);
        assert_eq!(stats(&connection, &root).unwrap().files, 1);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_file_deleted_behind_the_cache_reports_itself_missing() {
        let root = scratch("cleared");
        let connection = database();
        let stored = store(
            &connection,
            &root,
            "a|album",
            ArtRendition::Thumb,
            &jpeg(b"a cover"),
            "",
        )
        .unwrap();
        // Clearing the cache is a supported action, and a person may simply
        // empty the directory. That has to read as "no art", not as a path to a
        // file that is not there.
        std::fs::remove_file(&stored.path).unwrap();
        assert!(lookup(&connection, &root, "a|album", ArtRendition::Thumb)
            .unwrap()
            .is_none());
        assert_eq!(
            stats(&connection, &root).unwrap().entries,
            0,
            "the stale row was tidied as it was found"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
