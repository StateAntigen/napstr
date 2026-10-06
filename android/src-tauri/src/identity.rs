//! The phone's own Nostr identity.
//!
//! A phone used to hold no key at all. Everything it wanted said in public was
//! said by the computer it is paired with, under that computer's identity, which
//! is why "may not act as you" was the whole of what a lent pairing could not
//! do. This is the other half of that bargain: a key of its own, made here,
//! kept here, and never handed to anybody. A comment, a report or a playlist
//! from this phone is authored by this phone.
//!
//! What that costs is honest and worth stating: comments and playlists signed
//! here are the *device's*, not the computer's. A playlist published from this
//! phone belongs to the phone's key and can only ever be edited by it - which is
//! why the key has to survive a reinstall, and why it can be exported.
//!
//! The secret is a file in this app's own storage rather than something held in
//! memory only. A key that cannot be backed up is a key that loses everything it
//! owns the day the app is reinstalled, so the file is written with owner-only
//! permissions wherever the platform has them, and it leaves this process only
//! when somebody deliberately exports it.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use nostr::{Keys, SecretKey, ToBech32};
use serde::Serialize;

/// Where the key is kept, beside the Iroh identity and the pairings.
const IDENTITY_FILE: &str = "nostr-identity";

/// What the app may say about the identity without showing anything secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NostrIdentity {
    /// Lowercase hex, which is what an event's `pubkey` field is.
    pub pubkey: String,
    /// The same key in the form people exchange: `npub1…`.
    pub npub: String,
}

#[derive(Debug)]
pub struct DeviceIdentity {
    /// Behind a lock rather than held outright: importing an exported key
    /// replaces it, and everything that signs holds the same identity.
    keys: RwLock<Keys>,
    path: PathBuf,
}

impl DeviceIdentity {
    /// The key this install already has, or a new one.
    ///
    /// A file that cannot be read is not a reason to start over with a new key:
    /// a new key silently orphaned is a playlist nobody can edit again. The
    /// caller is told instead, and the app can offer to restore a backup.
    pub fn load_or_create(app_data: &Path) -> Result<Self, String> {
        let path = app_data.join(IDENTITY_FILE);
        let keys = match std::fs::read(&path) {
            Ok(bytes) => keys_from_bytes(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let keys = Keys::generate();
                write_secret(&path, keys.secret_key())?;
                keys
            }
            Err(error) => {
                return Err(format!(
                    "The saved Nostr identity could not be read: {error}"
                ))
            }
        };
        Ok(Self {
            keys: RwLock::new(keys),
            path,
        })
    }

    pub fn describe(&self) -> Result<NostrIdentity, String> {
        describe_keys(&self.keys()?)
    }

    /// The one thing that must never be shown without being asked for: the
    /// secret itself, in the form people write down and restore from.
    pub fn export(&self) -> Result<String, String> {
        let keys = self.keys()?;
        keys.secret_key()
            .to_bech32()
            .map_err(|error| format!("This identity cannot be exported: {error}"))
    }

    /// Take an exported key as this phone's identity, in place.
    ///
    /// Everything this phone published belongs to the key that signed it, so
    /// adopting another key is not a preference: it leaves the old identity's
    /// playlists behind, editable only by a key this phone no longer holds. The
    /// caller warns about that first; this only refuses what cannot be a key at
    /// all, and answers with what was adopted.
    pub fn adopt(&self, secret: &str) -> Result<NostrIdentity, String> {
        let secret = secret.trim();
        if secret.is_empty() {
            return Err("Paste the key you exported (nsec1…) or its 64 hex characters".into());
        }
        let keys = Keys::parse(secret).map_err(|_| {
            "That is not a Nostr secret key: paste an nsec1… value or 64 hex characters".to_string()
        })?;
        write_secret(&self.path, keys.secret_key())?;
        let described = describe_keys(&keys)?;
        // Held for the rest of the app: anything that was about to sign with the
        // old key signs with this one instead.
        self.keys
            .write()
            .map(|mut held| *held = keys)
            .map_err(|_| "The phone's own identity is unavailable".to_string())?;
        Ok(described)
    }

    pub fn keys(&self) -> Result<Keys, String> {
        self.keys
            .read()
            .map(|keys| keys.clone())
            .map_err(|_| "The phone's own identity is unavailable".to_string())
    }

    /// Where the secret lives, for a message that wants to say so.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// The raw 32 bytes of a secret key, which is the shape this file is kept in.
fn keys_from_bytes(bytes: &[u8]) -> Result<Keys, String> {
    let secret = SecretKey::from_slice(bytes)
        .map_err(|error| format!("The saved Nostr identity is not a valid key: {error}"))?;
    Ok(Keys::new(secret))
}

/// What may be said about a key without showing it.
fn describe_keys(keys: &Keys) -> Result<NostrIdentity, String> {
    let public = keys.public_key();
    Ok(NostrIdentity {
        pubkey: public.to_hex(),
        npub: public
            .to_bech32()
            .map_err(|error| format!("This identity cannot be written down: {error}"))?,
    })
}

/// Written with owner-only permissions where the platform has them, and written
/// aside first so a failure partway cannot leave a truncated key behind.
fn write_secret(path: &Path, secret: &SecretKey) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = path.with_extension("writing");
    // `write_private_key` refuses to overwrite, which is the right guard for a
    // key file and the wrong one for a scratch file: clear the scratch name
    // first, and the create-then-rename below is the whole write.
    let _ = std::fs::remove_file(&temporary);
    crate::write_private_key(&temporary, secret.as_secret_bytes())?;
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        format!("The identity could not be saved: {error}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("napstrfy-identity-{name}"));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    /// The key is made once and read back, which is the whole of "it survives".
    #[test]
    fn an_identity_is_made_once_and_kept() {
        let directory = scratch("kept");
        let first = DeviceIdentity::load_or_create(&directory).unwrap();
        let described = first.describe().unwrap();
        assert_eq!(described.pubkey.len(), 64);
        assert!(described.npub.starts_with("npub1"));
        assert_eq!(
            first.path().file_name().unwrap().to_str().unwrap(),
            IDENTITY_FILE
        );
        assert_eq!(
            std::fs::read(first.path()).unwrap().len(),
            32,
            "the file is the secret itself, not a wrapper around it"
        );

        let second = DeviceIdentity::load_or_create(&directory).unwrap();
        assert_eq!(second.describe().unwrap(), described);
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// Two installs do not share a key, and a key moves between them by export.
    #[test]
    fn an_identity_can_be_exported_and_adopted() {
        let original = scratch("exported");
        let source = DeviceIdentity::load_or_create(&original).unwrap();
        let exported = source.export().unwrap();
        assert!(exported.starts_with("nsec1"));
        let described = source.describe().unwrap();

        // The same key restored into a fresh install, which is what reinstalling
        // and pasting the backup is.
        let fresh = scratch("restored");
        let reinstalled = DeviceIdentity::load_or_create(&fresh).unwrap();
        assert_ne!(reinstalled.describe().unwrap(), described);
        assert_eq!(reinstalled.adopt(&exported).unwrap(), described);
        // The identity in memory is the restored one, not only the file.
        assert_eq!(reinstalled.describe().unwrap(), described);
        // And the next launch finds it without being asked again.
        assert_eq!(
            DeviceIdentity::load_or_create(&fresh)
                .unwrap()
                .describe()
                .unwrap(),
            described
        );

        // Hex is the other way people have a key written down. It must be the
        // same key, not a second one.
        let hex = source.keys().unwrap().secret_key().to_secret_hex();
        let from_hex = DeviceIdentity::load_or_create(&scratch("hex")).unwrap();
        assert_eq!(from_hex.adopt(&hex).unwrap(), described);

        let _ = std::fs::remove_dir_all(&original);
        let _ = std::fs::remove_dir_all(&fresh);
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("napstrfy-identity-hex"));
    }

    /// Anything that is not a key is refused with something a person can act on,
    /// rather than replacing a working identity with rubbish.
    #[test]
    fn what_is_not_a_key_is_refused() {
        let directory = scratch("refused");
        let existing = DeviceIdentity::load_or_create(&directory).unwrap();
        let described = existing.describe().unwrap();

        for rubbish in ["", "   ", "not-a-key", "npub1qqqqq", &"z".repeat(64)] {
            let error = existing.adopt(rubbish).unwrap_err();
            assert!(!error.is_empty());
        }
        // The identity that was already there is untouched by any of that.
        assert_eq!(existing.describe().unwrap(), described);
        assert_eq!(
            DeviceIdentity::load_or_create(&directory)
                .unwrap()
                .describe()
                .unwrap(),
            described
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// A file that is not a key is an error rather than a new identity: a key
    /// replaced in silence takes a playlist's editability with it.
    #[test]
    fn a_damaged_file_is_reported_rather_than_replaced() {
        let directory = scratch("damaged");
        std::fs::write(directory.join(IDENTITY_FILE), b"half a key").unwrap();
        let error = DeviceIdentity::load_or_create(&directory).unwrap_err();
        assert!(error.contains("not a valid key"), "said: {error}");
        assert_eq!(
            std::fs::read(directory.join(IDENTITY_FILE)).unwrap(),
            b"half a key"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
}
