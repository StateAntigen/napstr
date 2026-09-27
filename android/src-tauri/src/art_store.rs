//! Album art this phone holds itself.
//!
//! The pictures are fetched from the paired computer over Iroh and kept here,
//! for two reasons: the phone is often away from the computer that has them, and
//! a picture that had to come down the wire every time a list redrew would make
//! scrolling cost bandwidth.
//!
//! Two properties are inherited from the host's own cache, because they are what
//! makes this safe to serve from a local address:
//!
//! * **Content-addressed.** A file is named by the SHA-256 of its own bytes, so
//!   two albums with one sleeve cost one file, a picture that changes becomes a
//!   different file rather than a stale one, and the address a webview caches is
//!   an address whose content can never change.
//! * **Verified on the way in.** The hash is checked against the bytes rather
//!   than trusted, and the magic bytes decide the type. A host cannot hand this
//!   phone something that is not the picture it named, and nothing that is not an
//!   image is written at all.
//!
//! Names come from the network, so every one is validated before it is used as a
//! path: a hash is 64 lowercase hexadecimal characters and nothing else. Anything
//! else is refused rather than joined onto a directory.
//!
//! Nothing here fetches, and nothing here decides what to keep. `artwork.ts`
//! fetches what a screen needs, and the policy for what may be evicted lives with
//! the pass that fills this directory.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The directory the pictures live in, under the app's data directory.
///
/// Deliberately not the cache directory: this is what lets the phone draw art
/// while the computer that owns it is unreachable, so it is not worth letting
/// the system trim it. What goes is our decision, made in one place.
pub const ART_DIRECTORY: &str = "art";

/// The largest picture worth holding, matching the host's own limit.
pub const MAX_ART_BYTES: u64 = 8 * 1024 * 1024;

/// Whether a string is a name this store will use as a path.
///
/// The strict answer is the point: a hash arrives from the network or from
/// storage, and anything that is not exactly a SHA-256 could be a way out of the
/// directory or a name two spellings of.
pub fn is_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The type these bytes really are, from their own first bytes.
///
/// The same rule the host applies before it stores a picture, kept here as well
/// because this end must not depend on the other end having been honest. `None`
/// means "not an image", and the caller refuses it.
fn image_mime(bytes: &[u8]) -> Option<&'static str> {
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

/// Keep these bytes under the name their own hash gives them.
///
/// Refuses anything that is not the picture it claims to be, either because the
/// bytes do not hash to `hash` or because they are not an image at all. Writing
/// is done under a temporary name and moved into place, so a file that exists is
/// always a whole picture.
pub fn store(root: &Path, hash: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    if !is_hash(hash) {
        return Err("the host named artwork by something that is not a hash".into());
    }
    if bytes.len() as u64 > MAX_ART_BYTES {
        return Err("that artwork is too large to hold".into());
    }
    if image_mime(bytes).is_none() {
        return Err("those bytes are not an image".into());
    }
    let actual = hex::encode(Sha256::digest(bytes));
    if actual != hash {
        return Err("the artwork that arrived is not the artwork that was promised".into());
    }
    let path = root.join(hash);
    if path.is_file() {
        // Already held, which is the ordinary case for a screen that is drawn
        // twice. Nothing to write, and the address stays the same.
        return Ok(path);
    }
    std::fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let staging = root.join(format!(".{hash}.part"));
    std::fs::write(&staging, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&staging, &path).map_err(|error| error.to_string())?;
    Ok(path)
}

/// Where a hash's picture is, if this phone is holding it.
pub fn path(root: &Path, hash: &str) -> Option<PathBuf> {
    if !is_hash(hash) {
        return None;
    }
    let path = root.join(hash);
    path.is_file().then_some(path)
}

/// The type of a held picture, from the bytes on disk.
///
/// Read rather than remembered: the file is the authority on what it is, so a
/// directory somebody edited by hand still cannot serve something as an image
/// that is not one.
pub fn mime_of(path: &Path) -> Option<&'static str> {
    let head = std::fs::read(path).ok()?;
    image_mime(&head)
}

/// How many pictures are held, and how much they take.
pub fn stats(root: &Path) -> (usize, u64) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return (0, 0);
    };
    entries
        .flatten()
        .filter(|entry| entry.path().is_file())
        .fold((0, 0), |(files, bytes), entry| {
            let size = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
            (files + 1, bytes + size)
        })
}

/// Forget one picture. Returns whether there was one to forget.
pub fn forget(root: &Path, hash: &str) -> bool {
    let Some(path) = path(root, hash) else {
        return false;
    };
    std::fs::remove_file(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("napstrfy-art-{name}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn jpeg(tail: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8, 0xFF];
        bytes.extend_from_slice(tail);
        bytes
    }

    fn hash_of(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    #[test]
    fn a_name_from_the_network_is_only_ever_a_hash() {
        // The value decides a path, so anything that is not exactly a SHA-256
        // cannot be allowed to decide one.
        assert!(is_hash(&hash_of(b"anything")));
        assert!(!is_hash(""));
        assert!(!is_hash("../../../etc/passwd"));
        assert!(!is_hash(".."));
        assert!(!is_hash(&"A".repeat(64)), "uppercase is a different name");
        assert!(!is_hash(&"a".repeat(63)));
        assert!(!is_hash(&"a".repeat(65)));
        assert!(!is_hash(&format!("{}g", "a".repeat(63))));
        // Which means a traversal attempt cannot even be looked up.
        assert_eq!(path(Path::new("/tmp"), "../../../etc/passwd"), None);
    }

    #[test]
    fn a_picture_is_stored_under_the_hash_of_its_own_bytes() {
        let root = scratch("store");
        let bytes = jpeg(b"a sleeve");
        let hash = hash_of(&bytes);
        let held = store(&root, &hash, &bytes).unwrap();
        assert_eq!(held, root.join(&hash));
        assert!(held.is_file());
        assert_eq!(path(&root, &hash), Some(root.join(&hash)));
        assert_eq!(mime_of(&held), Some("image/jpeg"));

        // Storing it again is not writing it again, and the address is the same.
        assert_eq!(store(&root, &hash, &bytes).unwrap(), held);

        let (files, bytes_held) = stats(&root);
        assert_eq!(files, 1);
        assert_eq!(bytes_held, bytes.len() as u64);

        assert!(forget(&root, &hash));
        assert!(!forget(&root, &hash));
        assert_eq!(stats(&root), (0, 0));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn bytes_that_are_not_the_picture_that_was_promised_are_refused() {
        let root = scratch("refused");
        let bytes = jpeg(b"a sleeve");
        let hash = hash_of(&bytes);

        // A host that names the wrong picture does not get to have it stored:
        // the name is what a webview caches and what every screen will ask for.
        let other = jpeg(b"another sleeve");
        assert!(store(&root, &hash, &other).is_err());
        assert_eq!(stats(&root), (0, 0));

        // Neither does one that sends something that is not a picture at all.
        assert!(store(&root, &hash_of(b"<html>"), b"<html>").is_err());
        assert!(store(&root, &"a".repeat(64), b"not a picture").is_err());
        assert_eq!(stats(&root), (0, 0));

        // And a name that is not a hash is refused before anything is written.
        assert!(store(&root, "../escape", &bytes).is_err());
        assert_eq!(stats(&root), (0, 0));

        // A picture larger than a sleeve is refused rather than held.
        let mut huge = jpeg(b"");
        huge.resize(MAX_ART_BYTES as usize + 1, 0);
        assert!(store(&root, &hash_of(&huge), &huge).is_err());
        assert_eq!(stats(&root), (0, 0));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_type_comes_from_the_bytes_on_disk() {
        let root = scratch("mime");
        // A PNG named as a JPEG is served as a PNG: the file is the authority,
        // which is also what makes a hand-edited directory harmless.
        let png = [
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x00,
        ];
        let hash = hash_of(&png);
        let held = store(&root, &hash, &png).unwrap();
        assert_eq!(mime_of(&held), Some("image/png"));

        // Something that is not an image has no type, so it is never served.
        std::fs::write(root.join("b".repeat(64)), b"<html>").unwrap();
        assert_eq!(mime_of(&root.join("b".repeat(64))), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_picture_that_is_not_held_simply_is_not_there() {
        let root = scratch("absent");
        assert_eq!(path(&root, &"a".repeat(64)), None);
        let (files, bytes) = stats(&root);
        assert_eq!((files, bytes), (0, 0));
        // A directory that does not exist yet is empty rather than an error:
        // art is asked for before anything has been fetched.
        let missing = root.join("not-created");
        assert_eq!(stats(&missing), (0, 0));
        assert_eq!(path(&missing, &"a".repeat(64)), None);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
