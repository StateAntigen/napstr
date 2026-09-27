//! Going and getting the pictures a paired phone will be shown.
//!
//! `art_cache` holds art and refuses anything that is not an image; this is the
//! part that fetches it. It exists because of what the phone used to do: ask
//! this computer which picture an album had, then fetch that picture itself,
//! which told a publisher the phone's address and the album it was looking at.
//! Handing the bytes over instead means the fetching happens here, once, for
//! every paired phone, on a connection that already exists.
//!
//! Three rules shape it:
//!
//! * **Bounded.** Downloads share a couple of permits, so a phone opening a
//!   screen of albums cannot become a burst at a picture host, and the art a
//!   person is actually waiting for is not queued behind forty other pictures.
//! * **Once.** A rendition already being fetched is not fetched again, and one
//!   already held is not fetched at all. A phone asks about the same screen
//!   repeatedly, and every one of those must cost nothing.
//! * **Allowed, twice.** The same domain list that governs what this computer
//!   will display governs what it will download, checked before the request and
//!   again on the address it finally lands on, because a redirect is a second
//!   host asking to be trusted on the first one's word.
//!
//! Nothing here reports what it achieved. A download that lands is announced by
//! the cover revision moving, which is the signal every other change to this
//! computer's art already uses, and a download that fails is written into the
//! lookup log where the Covers tab can show it.

use crate::art_cache;
use crate::cover;
use crate::cover_publish::USER_AGENT;
use futures_util::StreamExt;
use napstr_remote_protocol::ArtRendition;
use rusqlite::Connection;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

/// Where a redirect chain may end. The Cover Art Archive answers an image path
/// with one redirect into archive.org, so refusing redirects outright would
/// refuse art this computer is allowed to hold; an unlimited chain is a way of
/// leaving an allow-list behind one hop at a time.
const ART_REDIRECTS: usize = 3;
/// How long a picture may take. Generous beside a lookup, because a scan can be
/// a megabyte over a slow link and nothing is waiting on it but a phone's
/// placeholder.
const ART_TIMEOUT: Duration = Duration::from_secs(20);
/// Downloads in flight at once. Deliberately small: a phone opening a screen of
/// albums asks about up to forty of them in one breath, and forty simultaneous
/// requests to one host is the shape that gets an address refused.
const ART_PERMITS: usize = 2;
/// The largest picture worth holding. Anything larger is a scan of a sleeve
/// rather than a sleeve, and it would be shipped to a phone over a link the
/// phone's owner pays for.
const MAX_ART_BYTES: u64 = 8 * 1024 * 1024;

/// One album's pictures, as the claim states them.
pub struct ArtWant<'a> {
    /// The cover key, which is the identity of the album on both ends.
    pub key: &'a str,
    /// The full-size picture, or empty when the claim names none.
    pub art: &'a str,
    /// The smaller rendition, or empty. Apple publishes one; the archive does
    /// not, and a cover with no thumb is served to the phone as the full one.
    pub thumb: &'a str,
    /// Where the claim says the art came from, kept so the Covers view can say
    /// it rather than only that a picture exists.
    pub source: &'a str,
}

/// The one place art is downloaded.
pub struct ArtFetcher {
    db_path: PathBuf,
    root: PathBuf,
    client: reqwest::Client,
    /// `key|rendition` of the downloads running now, so asking twice about one
    /// album does not download it twice.
    in_flight: Mutex<HashSet<String>>,
    permits: Arc<Semaphore>,
}

impl ArtFetcher {
    pub fn new(db_path: PathBuf, root: PathBuf) -> Result<Arc<Self>, String> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(ART_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(ART_REDIRECTS))
            .build()
            .map_err(|error| format!("Could not prepare the artwork client: {error}"))?;
        Ok(Arc::new(Self {
            db_path,
            root,
            client,
            in_flight: Mutex::new(HashSet::new()),
            permits: Arc::new(Semaphore::new(ART_PERMITS)),
        }))
    }

    /// Take on the pictures a batch of albums needs, and return at once.
    ///
    /// There is no answer here about whether the bytes arrived: a phone that
    /// asked a moment too early is told to ask again by the cover revision
    /// moving when they land, which is the same signal any other change to this
    /// computer's art uses. The batch is asked about together because that is
    /// how a phone asks — one call for a screen of albums — and one question of
    /// the cache per screen is the point of it.
    pub fn ensure_all(self: &Arc<Self>, wants: &[ArtWant<'_>]) {
        if wants.is_empty() {
            return;
        }
        let connection = match crate::open_connection(&self.db_path) {
            Ok(connection) => connection,
            Err(error) => {
                self.log("", "failed", "", &error);
                return;
            }
        };
        for want in wants {
            let pending = match pending_renditions(&connection, &self.root, want) {
                Ok(pending) => pending,
                Err(error) => {
                    self.log(want.key, "failed", want.source, &error);
                    continue;
                }
            };
            for (rendition, url) in pending {
                let token = format!("{}|{}", want.key, art_cache::rendition_name(rendition));
                if !self.claim(&token) {
                    continue;
                }
                let fetcher = self.clone();
                let key = want.key.to_string();
                let source = want.source.to_string();
                tauri::async_runtime::spawn(async move {
                    let result = fetcher
                        .fetch_rendition(&key, &url, rendition, &source)
                        .await;
                    fetcher.release(&token);
                    if let Err(error) = result {
                        fetcher.log(&key, "failed", &source, &error);
                    }
                });
            }
        }
    }

    /// Whether this download may start, claiming it if so.
    fn claim(&self, token: &str) -> bool {
        match self.in_flight.lock() {
            Ok(mut in_flight) => in_flight.insert(token.to_string()),
            // A poisoned lock is a thread that panicked while holding it. The
            // set is only a way of not doing the same work twice, so the safe
            // answer is to do the work.
            Err(_) => true,
        }
    }

    fn release(&self, token: &str) {
        if let Ok(mut in_flight) = self.in_flight.lock() {
            in_flight.remove(token);
        }
    }

    /// Download one rendition and keep it. Returns what went wrong, for the log.
    async fn fetch_rendition(
        &self,
        key: &str,
        url: &str,
        rendition: ArtRendition,
        source: &str,
    ) -> Result<(), String> {
        let address = fetchable_address(url)?;
        // Read once and check the same list before the request and after it.
        let allowed = {
            let connection = crate::open_connection(&self.db_path)?;
            cover::allowed_art_hosts(&connection)
        };
        if !cover::art_host_allowed(&allowed, url) {
            return Err(format!(
                "art from {} is not on the list this computer accepts",
                cover::art_url_host(url).unwrap_or_else(|| url.to_string())
            ));
        }
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| "the artwork queue was closed".to_string())?;
        let response = self
            .client
            .get(address)
            .send()
            .await
            .map_err(|error| format!("could not fetch the artwork: {error}"))?;
        if !response.status().is_success() {
            return Err(format!("the artwork host answered {}", response.status()));
        }
        // The address the bytes actually came from, which a redirect may have
        // changed to a host the list does not cover.
        let landed = response.url().clone();
        if landed.scheme() != "https" {
            return Err("art may only travel over HTTPS".into());
        }
        if !cover::art_host_allowed(&allowed, landed.as_str()) {
            return Err(format!(
                "the artwork redirected to {}, which is not on the list this computer accepts",
                landed.host_str().unwrap_or("an unnamed host")
            ));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_ART_BYTES)
        {
            return Err("that artwork is too large to hold".into());
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| format!("the artwork download failed: {error}"))?;
            if bytes.len().saturating_add(chunk.len()) as u64 > MAX_ART_BYTES {
                return Err("that artwork is too large to hold".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let connection = crate::open_connection(&self.db_path)?;
        art_cache::store(&connection, &self.root, key, rendition, &bytes, source)?;
        Ok(())
    }

    /// Write one attempt into the lookup log.
    ///
    /// Logging is diagnostics: a database that cannot be written must not stop
    /// a picture being fetched, so the result is dropped on purpose.
    fn log(&self, key: &str, outcome: &str, source: &str, message: &str) {
        let (artist, album) = key_halves(key);
        if let Ok(connection) = crate::open_connection(&self.db_path) {
            let _ = cover::record_lookup_log(
                &connection,
                cover::LookupLogEntry {
                    key,
                    artist: &artist,
                    album: &album,
                    outcome,
                    source,
                    message,
                },
            );
        }
    }
}

/// Which renditions of an album this computer still has to fetch.
///
/// Separated from the fetching so the rule can be read and tested on its own:
/// only renditions the cache does not already hold are wanted, and a claim that
/// names no address for one of them is not asking for anything.
fn pending_renditions(
    connection: &Connection,
    root: &Path,
    want: &ArtWant<'_>,
) -> Result<Vec<(ArtRendition, String)>, String> {
    let mut pending = Vec::new();
    for (rendition, url) in [
        (ArtRendition::Full, want.art),
        (ArtRendition::Thumb, want.thumb),
    ] {
        if url.trim().is_empty() {
            continue;
        }
        if art_cache::lookup(connection, root, want.key, rendition)?.is_some() {
            continue;
        }
        pending.push((rendition, url.trim().to_string()));
    }
    Ok(pending)
}

/// The address to fetch, or why it may not be fetched at all.
///
/// Plain HTTP is refused rather than upgraded: a picture fetched over it could
/// be replaced on the way, and this computer is the one handing it on to a
/// phone that will trust it. A claim that names no absolute HTTPS address is
/// refused for the same reason a pasted one is.
fn fetchable_address(url: &str) -> Result<reqwest::Url, String> {
    let parsed = reqwest::Url::parse(url.trim())
        .map_err(|_| format!("{url} is not an address art can be fetched from"))?;
    if parsed.scheme() != "https" {
        return Err(format!("{url} is not an HTTPS address"));
    }
    if parsed.host_str().is_none() {
        return Err(format!("{url} names no host"));
    }
    Ok(parsed)
}

/// The halves of a cover key, for the log's own columns.
///
/// A key is `artist|album`, lowercased and trimmed. A caller here knows the key
/// and not the tags, and the key is what the log is read back by, so the halves
/// are better than an empty artist and album.
fn key_halves(key: &str) -> (String, String) {
    match key.split_once('|') {
        Some((artist, album)) => (artist.to_string(), album.to_string()),
        None => (key.to_string(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        art_cache::initialise_schema(&connection).unwrap();
        connection
    }

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("napstr-art-fetch-{name}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn jpeg() -> Vec<u8> {
        vec![0xFF, 0xD8, 0xFF, b'a', b'c', b'o', b'v', b'e', b'r']
    }

    #[test]
    fn art_may_not_be_fetched_over_plain_http() {
        // Upgrading is not the answer either: a picture fetched over HTTP can be
        // replaced in flight, and this computer hands it on to a phone that
        // trusts it.
        assert!(fetchable_address("http://archive.org/a.jpg").is_err());
        assert!(fetchable_address("https://archive.org/a.jpg").is_ok());
        // A claim that names no absolute HTTPS address is refused, exactly as a
        // pasted one would be.
        assert!(fetchable_address("coverartarchive.org/a.jpg").is_err());
        assert!(fetchable_address("data:image/png;base64,AAAA").is_err());
        assert!(fetchable_address("").is_err());
        // Ports are kept: they are part of where the bytes come from.
        assert_eq!(
            fetchable_address("https://example.com:8443/a.jpg")
                .unwrap()
                .port(),
            Some(8443)
        );
    }

    #[test]
    fn what_is_already_held_is_not_asked_for_again() {
        let root = scratch("pending");
        let connection = database();
        let want = ArtWant {
            key: "kream|annihilation",
            art: "https://archive.org/full.jpg",
            thumb: "https://is1-ssl.mzstatic.com/thumb.jpg",
            source: "itunes",
        };
        // Nothing held: both renditions are wanted, full first.
        let pending = pending_renditions(&connection, &root, &want).unwrap();
        assert_eq!(
            pending
                .iter()
                .map(|(rendition, _)| *rendition)
                .collect::<Vec<_>>(),
            vec![ArtRendition::Full, ArtRendition::Thumb]
        );

        art_cache::store(
            &connection,
            &root,
            want.key,
            ArtRendition::Thumb,
            &jpeg(),
            "itunes",
        )
        .unwrap();
        // Holding the thumb leaves only the full one wanted, which is what a
        // phone asking about the same screen twice costs.
        let pending = pending_renditions(&connection, &root, &want).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, ArtRendition::Full);

        art_cache::store(
            &connection,
            &root,
            want.key,
            ArtRendition::Full,
            &jpeg(),
            "archive",
        )
        .unwrap();
        assert!(pending_renditions(&connection, &root, &want)
            .unwrap()
            .is_empty());

        // A claim that names no address for a rendition is not asking for it.
        let art_only = ArtWant {
            thumb: "",
            ..want
        };
        assert!(pending_renditions(&connection, &root, &art_only)
            .unwrap()
            .is_empty());
        // Whitespace is not an address either.
        let blank = ArtWant {
            key: "a|b",
            art: "   ",
            thumb: "",
            source: "",
        };
        assert!(pending_renditions(&connection, &root, &blank)
            .unwrap()
            .is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn one_album_is_queued_once_however_often_it_is_asked_about() {
        // A lock rather than a fetch: what is being checked is that the second
        // ask is refused the claim, because a screen of albums is asked about
        // again every time it redraws.
        // The client cannot exist without a TLS provider, exactly as in the app,
        // which installs one at startup. A second install is a no-op, so this is
        // safe wherever the tests run in any order.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let fetcher = ArtFetcher::new(PathBuf::from("unused.sqlite3"), PathBuf::from("unused"))
            .expect("the artwork client is buildable");
        assert!(fetcher.claim("a|b|full"));
        assert!(!fetcher.claim("a|b|full"), "the first claim still stands");
        assert!(fetcher.claim("a|b|thumb"), "a rendition is its own download");
        fetcher.release("a|b|full");
        assert!(fetcher.claim("a|b|full"), "released work can be asked for again");
    }

    #[test]
    fn a_key_is_read_back_as_the_artist_and_the_album() {
        assert_eq!(
            key_halves("kream / korolova|annihilation"),
            (
                "kream / korolova".to_string(),
                "annihilation".to_string()
            )
        );
        // A key that is not a pair is still worth logging as itself.
        assert_eq!(key_halves("not-a-pair"), ("not-a-pair".to_string(), String::new()));
    }

    #[test]
    fn the_user_agent_says_what_is_asking() {
        assert!(USER_AGENT.starts_with("Napstr/"));
    }
}
