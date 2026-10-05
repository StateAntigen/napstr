//! What a computer keeps for a device, and what proves the device is who it says.
//!
//! Two things a phone owns that are nobody else's business: the file ids it
//! liked, and the playlists it has not published. Neither is signed and neither
//! goes on a relay, so neither belongs to this computer's identity either - they
//! belong to the phone, and a phone is its key. The table here is therefore keyed
//! by public key rather than by the pairing: pair again, reinstall, restore the
//! key on another phone, and the list is still yours, because the key is.
//!
//! Which leaves the question this module exists to answer: a key is not a
//! password, so what stops one paired device from naming another's key and
//! reading their list? The answer is that a key has to be *proved* before it can
//! be used: NIP-42 exists for exactly that, and a challenge signed by the device
//! is a proof that costs one round trip. Until a device has proved a key, this
//! computer keeps nothing under it and hands nothing back from it.
//!
//! The column on the pairings table is what remembers a proof between launches.
//! A phone that has proved its key once is not asked again on every request; it
//! is asked again when it restores a different key, which is the only moment the
//! answer could have changed.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use napstr_remote_protocol::{SignedEvent, AUTHENTICATION_KIND, MAX_DISLIKES, MAX_LIKES};
use nostr_sdk::nostr::{Event, JsonUtil};

use crate::open_connection;

/// How long a challenge is worth signing.
///
/// The same five minutes the pairing codes get, and for the same reason: the
/// question and its answer are one exchange.
pub const CHALLENGE_TTL: Duration = Duration::from_secs(300);

/// How far from now a signed authentication may be, either way.
///
/// NIP-42 leaves this to the relay, and a signed event is not much of a proof if
/// anybody who saw it can present it tomorrow.
const AUTHENTICATION_WINDOW: i64 = 300;

/// The nonce each device was given last, and when.
///
/// In memory only: a challenge that outlived the process that made it is not a
/// challenge, and nothing here is worth keeping across a restart.
#[derive(Default)]
pub struct Challenges {
    issued: Mutex<HashMap<String, (String, Instant)>>,
}

impl Challenges {
    /// A fresh nonce for one device, replacing whatever it was given before.
    pub fn issue(&self, endpoint_id: &str) -> String {
        let nonce = hex::encode(rand::random::<[u8; 32]>());
        if let Ok(mut issued) = self.issued.lock() {
            issued.retain(|_, (_, at)| at.elapsed() < CHALLENGE_TTL);
            issued.insert(endpoint_id.to_string(), (nonce.clone(), Instant::now()));
        }
        nonce
    }

    /// The nonce this device was given, if it is still good.
    ///
    /// Not taken away: a signature that fails for a reason the device can fix -
    /// a clock a minute out - should not cost it a new round trip and a new
    /// challenge, and the nonce is useless to anybody who cannot sign with the
    /// key anyway.
    pub fn current(&self, endpoint_id: &str) -> Option<String> {
        let issued = self.issued.lock().ok()?;
        issued
            .get(endpoint_id)
            .filter(|(_, at)| at.elapsed() < CHALLENGE_TTL)
            .map(|(nonce, _)| nonce.clone())
    }
}

/// Check that an authentication is what it claims: this kind, this nonce, this
/// key, and a signature over all of it.
///
/// Answers with the public key, which is the only thing the caller wanted. Every
/// check here is one the computer must make for itself - the id is recomputed
/// from the six other fields rather than read, because an id that a device wrote
/// down is a device's word for what it signed.
pub fn verify_authentication(
    signed: &SignedEvent,
    challenge: &str,
    now: i64,
) -> Result<String, String> {
    if signed.kind != AUTHENTICATION_KIND {
        return Err("That is not the shape of a device proving its key".into());
    }
    let signed_at = signed.created_at as i64;
    if (signed_at - now).abs() > AUTHENTICATION_WINDOW {
        return Err("That proof was signed too long ago to be one. Check this phone's clock, then try again.".into());
    }
    // NIP-42 carries the nonce in a tag rather than in the content, which is
    // empty, so this is also the check that the device answered *this* question.
    let answered = signed
        .tags
        .iter()
        .find(|tag| tag.first().is_some_and(|name| name == "challenge"))
        .and_then(|tag| tag.get(1))
        .map(String::as_str)
        .unwrap_or_default();
    if answered != challenge {
        return Err("That proof is not an answer to the question this computer asked".into());
    }
    let event = to_event(signed)?;
    event
        .verify()
        .map_err(|_| "That proof is not signed by the key it names".to_string())?;
    Ok(event.pubkey.to_hex())
}

/// The wire event as the event type that knows how to check itself.
fn to_event(signed: &SignedEvent) -> Result<Event, String> {
    let json = serde_json::to_string(signed).map_err(|error| error.to_string())?;
    Event::from_json(&json).map_err(|error| format!("That is not a signed event: {error}"))
}

/// The key this device has proved, if it has proved one.
pub fn proved_key(db_path: &Path, endpoint_id: &str) -> Result<Option<String>, String> {
    let key = open_connection(db_path)?
        .query_row(
            "SELECT pubkey FROM mobile_devices WHERE endpoint_id=?1",
            [endpoint_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
        .filter(|key| !key.is_empty());
    Ok(key)
}

/// Remember a proof, which is what makes the key usable on the next request.
pub fn remember_key(db_path: &Path, endpoint_id: &str, pubkey: &str) -> Result<(), String> {
    let changed = open_connection(db_path)?
        .execute(
            "UPDATE mobile_devices SET pubkey=?1 WHERE endpoint_id=?2",
            rusqlite::params![pubkey, endpoint_id],
        )
        .map_err(|error| error.to_string())?;
    if changed == 0 {
        return Err("This phone is not paired with Napstr".into());
    }
    Ok(())
}

/// The file ids this key liked, oldest first.
pub fn likes(db_path: &Path, pubkey: &str) -> Result<Vec<String>, String> {
    stored_list(db_path, LIKES_TABLE, pubkey)
}

/// Replace this key's likes with a list, and answer with what was stored.
///
/// The whole list rather than one at a time: a like is a state and not an event,
/// and two devices holding the same key would otherwise interleave into a list
/// neither of them chose.
pub fn set_likes(db_path: &Path, pubkey: &str, file_ids: &[String]) -> Result<Vec<String>, String> {
    replace_list(db_path, LIKES_TABLE, MAX_LIKES, "likes", pubkey, file_ids)
}

/// The file ids this key never wants played again, oldest first.
pub fn dislikes(db_path: &Path, pubkey: &str) -> Result<Vec<String>, String> {
    stored_list(db_path, DISLIKES_TABLE, pubkey)
}

/// Replace this key's dislikes with a list, and answer with what was stored.
///
/// A list of its own rather than a sign on the likes list: a device may hold a
/// file in neither, in one, or - after a change of mind - in the other, and
/// keeping them apart is what makes "undo" mean the earlier state rather than
/// the absence of a mark.
pub fn set_dislikes(
    db_path: &Path,
    pubkey: &str,
    file_ids: &[String],
) -> Result<Vec<String>, String> {
    replace_list(
        db_path,
        DISLIKES_TABLE,
        MAX_DISLIKES,
        "dislikes",
        pubkey,
        file_ids,
    )
}

/// The table one device's likes are kept in.
const LIKES_TABLE: &str = "device_likes";
/// And its dislikes. Two tables rather than one with a column, because a list of
/// ids is the whole of what either is and a second column would only ever hold
/// one of two values.
const DISLIKES_TABLE: &str = "device_dislikes";

/// One device's list, out of one of the two tables above.
///
/// The table name is interpolated into the SQL rather than bound, which is only
/// safe because both callers pass a constant from this file - never anything a
/// device said. Naming that here because a bound parameter is impossible for a
/// table name and the next reader deserves to know why this looks careless.
fn stored_list(db_path: &Path, table: &str, pubkey: &str) -> Result<Vec<String>, String> {
    let connection = open_connection(db_path)?;
    let mut statement = connection
        .prepare(&format!(
            "SELECT file_id FROM {table} WHERE pubkey=?1 ORDER BY added_at, file_id"
        ))
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([pubkey], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

/// Replace one device's list in one of those tables, and answer with what stuck.
///
/// A file id that is not a file id is dropped rather than refused, because the
/// rest of the list is still the person's answer. A list too long to fit a
/// control frame is refused rather than trimmed: the cap is not a policy about
/// how much somebody may like, it is the size of the wire, and quietly dropping
/// the end of a list would delete marks the phone still believes it saved.
fn replace_list(
    db_path: &Path,
    table: &str,
    limit: usize,
    what: &str,
    pubkey: &str,
    file_ids: &[String],
) -> Result<Vec<String>, String> {
    if file_ids.len() > limit {
        return Err(format!(
            "That is {} {what}, and this computer keeps at most {limit}",
            file_ids.len()
        ));
    }
    let mut wanted = Vec::new();
    for file_id in file_ids {
        let file_id = file_id.trim().to_ascii_lowercase();
        let is_a_file_id =
            file_id.len() == 64 && file_id.bytes().all(|byte| byte.is_ascii_hexdigit());
        if is_a_file_id && !wanted.contains(&file_id) {
            wanted.push(file_id);
        }
    }
    let mut connection = open_connection(db_path)?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(&format!("DELETE FROM {table} WHERE pubkey=?1"), [pubkey])
        .map_err(|error| error.to_string())?;
    {
        let mut insert = transaction
            .prepare(&format!(
                "INSERT INTO {table}(pubkey,file_id,added_at) VALUES(?1,?2,?3)"
            ))
            .map_err(|error| error.to_string())?;
        let now = chrono::Utc::now().timestamp();
        for (index, file_id) in wanted.iter().enumerate() {
            insert
                .execute(rusqlite::params![pubkey, file_id, now + index as i64])
                .map_err(|error| error.to_string())?;
        }
    }
    transaction.commit().map_err(|error| error.to_string())?;
    stored_list(db_path, table, pubkey)
}

/// The tables and the column this module reads, added to a database that
/// predates them.
pub fn initialise_schema(db_path: &Path) -> Result<(), String> {
    let connection = open_connection(db_path)?;
    for table in [LIKES_TABLE, DISLIKES_TABLE] {
        connection
            .execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS {table} (
                   pubkey TEXT NOT NULL,
                   file_id TEXT NOT NULL,
                   added_at INTEGER NOT NULL,
                   PRIMARY KEY (pubkey, file_id)
                 );"
            ))
            .map_err(|error| error.to_string())?;
    }
    let has_pubkey: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('mobile_devices') WHERE name='pubkey')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !has_pubkey {
        // Empty rather than a value: a device that has not proved a key has no
        // key, which is different from a device whose key is the empty string.
        connection
            .execute_batch("ALTER TABLE mobile_devices ADD COLUMN pubkey TEXT NOT NULL DEFAULT '';")
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};
    use std::path::PathBuf;

    /// A database with the pairings table in it, which is what a proved key is
    /// remembered on.
    fn device_database(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("napstr-device-{name}"));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let db = directory.join("napstr.sqlite3");
        rusqlite::Connection::open(&db)
            .unwrap()
            .execute_batch(
                "CREATE TABLE mobile_devices (
                   endpoint_id TEXT PRIMARY KEY,
                   name TEXT NOT NULL,
                   paired_at TEXT NOT NULL,
                   last_seen TEXT NOT NULL
                 );
                 INSERT INTO mobile_devices(endpoint_id,name,paired_at,last_seen)
                 VALUES('endpoint-one','A phone','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z');",
            )
            .unwrap();
        initialise_schema(&db).unwrap();
        db
    }

    fn challenge_for(keys: &Keys, nonce: &str, kind: u16, created_at: i64) -> SignedEvent {
        let event = EventBuilder::new(Kind::from(kind), "")
            .tags([
                Tag::parse(["relay", "napstr"]).unwrap(),
                Tag::parse(["challenge", nonce]).unwrap(),
            ])
            .custom_created_at(Timestamp::from(created_at as u64))
            .sign_with_keys(keys)
            .unwrap();
        serde_json::from_str(&event.as_json()).unwrap()
    }

    /// The proof works, and it is the key that is signed with rather than the one
    /// that is named.
    #[test]
    fn a_signed_challenge_proves_the_key_that_signed_it() {
        let keys = Keys::generate();
        let nonce = "a".repeat(64);
        let signed = challenge_for(&keys, &nonce, AUTHENTICATION_KIND, 1_800_000_000);
        assert_eq!(
            verify_authentication(&signed, &nonce, 1_800_000_000).unwrap(),
            keys.public_key().to_hex()
        );
    }

    /// Everything that is not an answer to this question is refused, and each
    /// refusal names what was wrong rather than saying "no".
    #[test]
    fn a_proof_that_is_not_one_is_refused() {
        let keys = Keys::generate();
        let nonce = "a".repeat(64);
        let now = 1_800_000_000;

        // Another device's nonce, which is the case that matters: a proof is
        // worthless if it can be replayed against a different question.
        let signed = challenge_for(&keys, &"b".repeat(64), AUTHENTICATION_KIND, now);
        assert!(verify_authentication(&signed, &nonce, now).is_err());

        // The wrong kind of event.
        let signed = challenge_for(&keys, &nonce, 1, now);
        let error = verify_authentication(&signed, &nonce, now).unwrap_err();
        assert!(error.contains("not the shape"), "said: {error}");

        // Signed long ago, or in a future that has not happened.
        let old = challenge_for(&keys, &nonce, AUTHENTICATION_KIND, now - 3_600);
        assert!(verify_authentication(&old, &nonce, now).is_err());
        let ahead = challenge_for(&keys, &nonce, AUTHENTICATION_KIND, now + 3_600);
        assert!(verify_authentication(&ahead, &nonce, now).is_err());

        // A signature that does not match the key it names: the same event, with
        // somebody else's public key on it. This is the one check a computer must
        // never skip, because everything else here is the device's own words.
        let mut forged = challenge_for(&keys, &nonce, AUTHENTICATION_KIND, now);
        forged.pubkey = Keys::generate().public_key().to_hex();
        assert!(verify_authentication(&forged, &nonce, now).is_err());

        // And a tampered payload: the content changed after signing.
        let mut edited = challenge_for(&keys, &nonce, AUTHENTICATION_KIND, now);
        edited.content = "something else".into();
        assert!(verify_authentication(&edited, &nonce, now).is_err());
    }

    /// A challenge is issued once, good for five minutes, and replaced by the
    /// next ask.
    #[test]
    fn a_challenge_is_fresh_and_tied_to_one_device() {
        let challenges = Challenges::default();
        let first = challenges.issue("phone-a");
        assert_eq!(
            challenges.current("phone-a").as_deref(),
            Some(first.as_str())
        );
        assert_eq!(challenges.current("phone-b"), None);
        let second = challenges.issue("phone-a");
        assert_ne!(first, second);
        assert_eq!(
            challenges.current("phone-a").as_deref(),
            Some(second.as_str())
        );
    }

    /// The key a device proved is the key its likes are kept under, and a second
    /// device with its own key sees its own list.
    #[test]
    fn likes_belong_to_the_key_rather_than_to_the_pairing() {
        let db = device_database("likes");
        let phone = "b".repeat(64);
        let other = "c".repeat(64);
        let file_a = "1".repeat(64);
        let file_b = "2".repeat(64);

        assert!(likes(&db, &phone).unwrap().is_empty());
        let stored = set_likes(&db, &phone, &[file_a.clone(), file_b.clone()]).unwrap();
        assert_eq!(stored, vec![file_a.clone(), file_b.clone()]);
        // The order a person made is the order they get back.
        assert_eq!(
            likes(&db, &phone).unwrap(),
            vec![file_a.clone(), file_b.clone()]
        );

        // Replacing is replacing, not adding.
        assert_eq!(
            set_likes(&db, &phone, &[file_b.clone()]).unwrap(),
            vec![file_b.clone()]
        );
        assert_eq!(likes(&db, &phone).unwrap(), vec![file_b.clone()]);

        // Another key is another list, which is the whole reason the table is
        // keyed by the key and not by the pairing.
        assert!(likes(&db, &other).unwrap().is_empty());

        let _ = std::fs::remove_dir_all(db.parent().unwrap());
    }
    /// Rubbish is dropped rather than refused, and a repeat is one like.
    #[test]
    fn an_unkempt_likes_list_is_cleaned_rather_than_refused() {
        let db = device_database("unkempt");
        let phone = "b".repeat(64);
        let file = "1".repeat(64);
        let stored = set_likes(
            &db,
            &phone,
            &[
                file.clone(),
                file.to_ascii_uppercase(),
                "not-a-file-id".into(),
                String::new(),
                "2".repeat(63),
            ],
        )
        .unwrap();
        assert_eq!(stored, vec![file]);
        let _ = std::fs::remove_dir_all(db.parent().unwrap());
    }

    /// A list longer than the wire holds is refused rather than trimmed, and
    /// nothing of it is stored.
    #[test]
    fn a_likes_list_longer_than_a_frame_is_refused() {
        let db = device_database("overflow");
        let phone = "b".repeat(64);
        let file = "1".repeat(64);
        set_likes(&db, &phone, &[file.clone()]).unwrap();

        let too_many: Vec<String> = (0..=MAX_LIKES)
            .map(|index| format!("{index:064x}"))
            .collect();
        let error = set_likes(&db, &phone, &too_many).unwrap_err();
        assert!(error.contains(&MAX_LIKES.to_string()), "said: {error}");
        // The refusal leaves the list it had, rather than half of the new one.
        assert_eq!(likes(&db, &phone).unwrap(), vec![file]);

        let _ = std::fs::remove_dir_all(db.parent().unwrap());
    }

    /// The two lists are two lists, not one list with a sign on it.
    ///
    /// This is what makes undo mean the state before rather than the absence of
    /// a mark: a file can be liked, then disliked while still liked elsewhere, and
    /// taking the dislike back has to leave the like where it was.
    #[test]
    fn dislikes_are_a_list_of_their_own_rather_than_the_likes_upside_down() {
        let db = device_database("dislikes");
        let phone = "b".repeat(64);
        let other = "c".repeat(64);
        let liked = "1".repeat(64);
        let disliked = "2".repeat(64);

        set_likes(&db, &phone, &[liked.clone()]).unwrap();
        assert_eq!(
            set_dislikes(&db, &phone, &[disliked.clone()]).unwrap(),
            vec![disliked.clone()]
        );
        assert_eq!(dislikes(&db, &phone).unwrap(), vec![disliked.clone()]);
        assert_eq!(
            likes(&db, &phone).unwrap(),
            vec![liked.clone()],
            "the like is still a like"
        );
        // Another key is another list here too.
        assert!(dislikes(&db, &other).unwrap().is_empty());
        // And taking the dislike back leaves the like standing.
        set_dislikes(&db, &phone, &[]).unwrap();
        assert!(dislikes(&db, &phone).unwrap().is_empty());
        assert_eq!(likes(&db, &phone).unwrap(), vec![liked]);

        let _ = std::fs::remove_dir_all(db.parent().unwrap());
    }

    /// A dislikes list is cleaned and capped by the same rules as a likes list.
    #[test]
    fn a_dislikes_list_is_cleaned_and_capped_the_same_way() {
        let db = device_database("dislikes-clean");
        let phone = "b".repeat(64);
        let file = "1".repeat(64);
        assert_eq!(
            set_dislikes(&db, &phone, &[file.clone(), "nonsense".into()]).unwrap(),
            vec![file.clone()]
        );

        let too_many: Vec<String> = (0..=MAX_DISLIKES)
            .map(|index| format!("{index:064x}"))
            .collect();
        let error = set_dislikes(&db, &phone, &too_many).unwrap_err();
        assert!(error.contains(&MAX_DISLIKES.to_string()), "said: {error}");
        assert_eq!(dislikes(&db, &phone).unwrap(), vec![file]);

        let _ = std::fs::remove_dir_all(db.parent().unwrap());
    }

    /// The proof is remembered against the pairing, and a device that never
    /// proved one has none.
    #[test]
    fn a_proved_key_is_remembered_against_the_pairing() {
        let db = device_database("proved");
        let pairing = "endpoint-one";
        assert_eq!(proved_key(&db, pairing).unwrap(), None);

        let keys = Keys::generate();
        remember_key(&db, pairing, &keys.public_key().to_hex()).unwrap();
        assert_eq!(
            proved_key(&db, pairing).unwrap(),
            Some(keys.public_key().to_hex())
        );
        // A device this computer never paired with cannot be given a key.
        assert!(remember_key(&db, "never-paired", &keys.public_key().to_hex()).is_err());

        let _ = std::fs::remove_dir_all(db.parent().unwrap());
    }
}
