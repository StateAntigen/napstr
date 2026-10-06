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
//! * **Remembered when it fails.** A picture that did not arrive is written down
//!   with the address that failed and a wait before it is tried again. Without
//!   that, "bounded" and "once" only hold for work in flight: a caller asking in
//!   a loop — the fill asks every couple of seconds — turns one dead address into
//!   a request every couple of seconds, for as long as the app is open. An
//!   address the host says is *gone* is offered to the archive to be corrected
//!   first, because the Cover Art Archive re-keys an image when a cover is
//!   replaced and the picture is still there under another name.
//!
//! Nothing here reports what it achieved. A download that lands is announced by
//! the cover revision moving, which is the signal every other change to this
//! computer's art already uses, and a download that fails is written into the
//! lookup log where the Covers tab can show it.

use crate::art_cache;
use crate::cover;
use crate::cover_publish::{ArchiveAnswer, FAILED_LOOKUP_RETRY_SECONDS, USER_AGENT};
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
/// How long an address the host says is *gone* is left alone.
///
/// A 404 from the Cover Art Archive is an answer rather than a failure: the
/// archive re-keys an image when a cover is replaced, so a claim can name a
/// picture that is still there under another name. Asking again in ten minutes
/// cannot change that, and the repair that can is tried before this wait is
/// written.
const GONE_PARK: Duration = Duration::from_secs(3 * 24 * 60 * 60);
/// The most a repeated failure grows to. Long enough that an address nothing can
/// fix is close to free, and short enough that a host which fixes itself is found
/// again without anybody having to intervene.
const MAX_PARK: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// How long a failed download waits before its address is tried again.
///
/// The first failure waits the least and every failure after it doubles, so an
/// address that is broken for a month costs a handful of requests rather than one
/// a second. A *gone* address starts at days, because the host has already said
/// the file is not there.
fn failure_delay(gone: bool, attempts: i64) -> Duration {
    let base = if gone {
        GONE_PARK
    } else {
        Duration::from_secs(FAILED_LOOKUP_RETRY_SECONDS as u64)
    };
    let doublings = attempts.saturating_sub(1).clamp(0, 4) as u32;
    base.saturating_mul(1 << doublings).min(MAX_PARK)
}

/// Whether an HTTP status means the address itself is no longer there.
///
/// `410 Gone` is the same news as `404` stated more firmly. A `403`, a `429` or a
/// `5xx` is not: those are answers about the request or the moment, and the same
/// address may well work later.
fn status_says_gone(status: u16) -> bool {
    matches!(status, 404 | 410)
}

/// Why a download failed, and what that says about the address.
#[derive(Debug)]
struct FetchFailure {
    /// What went wrong, as a person reads it.
    message: String,
    /// The status the host answered with, when the host answered at all.
    status: String,
    /// The host said there is no such file, which is an answer about the address
    /// rather than a bad moment for the host.
    gone: bool,
}

impl FetchFailure {
    /// The host answered, and the answer was not a picture.
    fn from_status(status: reqwest::StatusCode) -> Self {
        Self {
            gone: status_says_gone(status.as_u16()),
            status: status.as_u16().to_string(),
            message: format!("the artwork host answered {status}"),
        }
    }

    /// Anything else: a slow host, a refused connection, bytes that are not a
    /// picture, an address the list does not cover.
    fn busy(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status: String::new(),
            gone: false,
        }
    }
}

/// What asking the archive about an address that is gone produced.
enum Repaired {
    /// An address for this rendition, to fetch instead.
    Address(String),
    /// The archive answered, and it holds no front picture for that release. A
    /// considered "nobody has art for this record" rather than a failure.
    NoArt,
    /// This rendition has no address any more, which the corrected resolution now
    /// says. There is nothing to fetch and nothing to wait for.
    NothingToFetch,
    /// Nothing was learned: the address is not one the archive owns, the album's
    /// own resolution does not name it, or the ask did not answer.
    Unknown,
}

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

    /// The directory the pictures are kept in, for a caller that has to serve
    /// them or report on them from a different part of the app.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// How many downloads are queued or running right now.
    ///
    /// The fill asks before adding more, so a library's worth of albums is not
    /// queued in one go: what limits this is the work the fetcher can actually
    /// do, not the size of the list behind it.
    pub fn in_flight(&self) -> usize {
        self.in_flight
            .lock()
            .map(|in_flight| in_flight.len())
            .unwrap_or(0)
    }

    /// Take on the pictures a batch of albums needs, and return at once.
    ///
    /// There is no answer here about whether the bytes arrived: a phone that
    /// asked a moment too early is told to ask again by the cover revision
    /// moving when they land, which is the same signal any other change to this
    /// computer's art uses. The batch is asked about together because that is
    /// how a phone asks — one call for a screen of albums — and one question of
    /// the cache per screen is the point of it.
    ///
    /// Answers how many downloads it took on. The fill reads that as "is there
    /// work", so an album left waiting for a reason rather than for a turn lets
    /// the fill rest instead of asking after it every couple of seconds for ever.
    pub fn ensure_all(self: &Arc<Self>, wants: &[ArtWant<'_>]) -> usize {
        if wants.is_empty() {
            return 0;
        }
        let mut claimed = 0;
        let connection = match crate::open_connection(&self.db_path) {
            Ok(connection) => connection,
            Err(error) => {
                self.log("", "failed", "", &error);
                return 0;
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
                claimed += 1;
                let fetcher = self.clone();
                let key = want.key.to_string();
                let source = want.source.to_string();
                tauri::async_runtime::spawn(async move {
                    let result = fetcher
                        .fetch_rendition(&key, &url, rendition, &source)
                        .await;
                    fetcher.release(&token);
                    match result {
                        // The picture is held, so whatever this address did before
                        // is no longer true of it.
                        Ok(()) => fetcher.forget_failure(&key, rendition),
                        Err(failure) => {
                            fetcher.remember_failure(&key, rendition, &url, &source, failure)
                        }
                    }
                });
            }
        }
        claimed
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
    ///
    /// An address the host says is *gone* is not written off: the Cover Art
    /// Archive re-keys an image when a cover is replaced, so the archive is asked
    /// where that release's picture is now and, when it names one, that is
    /// fetched instead. One request against the alternative, which is a dead
    /// address retried until somebody notices.
    async fn fetch_rendition(
        &self,
        key: &str,
        url: &str,
        rendition: ArtRendition,
        source: &str,
    ) -> Result<(), FetchFailure> {
        match self.download(key, url, rendition, source).await {
            Ok(()) => Ok(()),
            Err(failure) if failure.gone => {
                match self.repair(key, rendition, url).await {
                    Repaired::Address(found) => {
                        match self.download(key, &found, rendition, source).await {
                            Ok(()) => {
                                // Said out loud, because a claim whose address changed
                                // on its own is a mystery to whoever reads it next.
                                self.log(
                                key,
                                "found",
                                source,
                                &format!("the address on record was gone; the picture came from {found}"),
                            );
                                Ok(())
                            }
                            Err(next) => Err(next),
                        }
                    }
                    // The archive answered, and it holds no picture for that release
                    // any more. A considered answer rather than a failure, so the
                    // album leaves the queue instead of coming back in three days.
                    Repaired::NoArt => {
                        self.record_no_art(key, source);
                        Ok(())
                    }
                    // This rendition has no address any more, which the corrected
                    // resolution now says: nothing to fetch and nothing to wait for.
                    Repaired::NothingToFetch => Ok(()),
                    // The address is not one the archive owns, or it is not this
                    // computer's to correct. It waits its turn like any other failure.
                    Repaired::Unknown => Err(failure),
                }
            }
            Err(failure) => Err(failure),
        }
    }

    /// Fetch one address and keep the bytes.
    async fn download(
        &self,
        key: &str,
        url: &str,
        rendition: ArtRendition,
        source: &str,
    ) -> Result<(), FetchFailure> {
        let address = fetchable_address(url).map_err(|error| FetchFailure::busy(error))?;
        // Read once and check the same list before the request and after it.
        let allowed = {
            let connection =
                crate::open_connection(&self.db_path).map_err(|error| FetchFailure::busy(error))?;
            cover::allowed_art_hosts(&connection)
        };
        if !cover::art_host_allowed(&allowed, url) {
            return Err(FetchFailure::busy(format!(
                "art from {} is not on the list this computer accepts",
                cover::art_url_host(url).unwrap_or_else(|| url.to_string())
            )));
        }
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| FetchFailure::busy("the artwork queue was closed"))?;
        let response =
            self.client.get(address).send().await.map_err(|error| {
                FetchFailure::busy(format!("could not fetch the artwork: {error}"))
            })?;
        if !response.status().is_success() {
            return Err(FetchFailure::from_status(response.status()));
        }
        // The address the bytes actually came from, which a redirect may have
        // changed to a host the list does not cover.
        let landed = response.url().clone();
        if landed.scheme() != "https" {
            return Err(FetchFailure::busy("art may only travel over HTTPS"));
        }
        if !cover::art_host_allowed(&allowed, landed.as_str()) {
            return Err(FetchFailure::busy(format!(
                "the artwork redirected to {}, which is not on the list this computer accepts",
                landed.host_str().unwrap_or("an unnamed host")
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_ART_BYTES)
        {
            return Err(FetchFailure::busy("that artwork is too large to hold"));
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                FetchFailure::busy(format!("the artwork download failed: {error}"))
            })?;
            if bytes.len().saturating_add(chunk.len()) as u64 > MAX_ART_BYTES {
                return Err(FetchFailure::busy("that artwork is too large to hold"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let connection =
            crate::open_connection(&self.db_path).map_err(|error| FetchFailure::busy(error))?;
        art_cache::store(&connection, &self.root, key, rendition, &bytes, source)
            .map_err(|error| FetchFailure::busy(error))?;
        Ok(())
    }

    /// This computer's own answer for an album, when it has one.
    fn stored_resolution(&self, key: &str) -> Option<cover::ArtLookup> {
        let connection = crate::open_connection(&self.db_path).ok()?;
        cover::stored_art(&connection, key).ok().flatten()
    }

    /// Whether this computer's own answer for an album still names `url`.
    ///
    /// A resolution that names something else has been corrected, and a failure
    /// against the address it used to name is not this album's news.
    fn resolution_names(&self, key: &str, rendition: ArtRendition, url: &str) -> bool {
        match self.stored_resolution(key) {
            Some(named) => answer_names(&named, rendition, url),
            // No answer of this computer's own - another author's claim, say - so
            // nothing has corrected anything and this is the only address there is.
            None => true,
        }
    }

    /// Ask the archive where a picture went, when the address on record is gone.
    ///
    /// Only this computer's own resolution is corrected. An address that came
    /// from another author's claim is theirs to fix, and rewriting this
    /// computer's answer because somebody else's link is stale would replace a
    /// good address with a worse one.
    async fn repair(&self, key: &str, rendition: ArtRendition, url: &str) -> Repaired {
        let Some(release) = crate::cover_publish::archive_release_id(url) else {
            return Repaired::Unknown;
        };
        let Some(named) = self.stored_resolution(key) else {
            return Repaired::Unknown;
        };
        if !answer_names(&named, rendition, url) {
            return Repaired::Unknown;
        }
        let Ok(connection) = crate::open_connection(&self.db_path) else {
            return Repaired::Unknown;
        };
        let (art, thumb) = match crate::cover_publish::archive_repaired_front(&release, url).await {
            ArchiveAnswer::Front { art, thumb } => (art, thumb),
            ArchiveAnswer::None => return Repaired::NoArt,
            ArchiveAnswer::Unknown => return Repaired::Unknown,
        };
        if art.is_empty() {
            return Repaired::Unknown;
        }
        // Written down whether or not the new address fetches: what this computer
        // publishes should name the address the archive holds, and a claim that
        // names a snapshot of last year's index is what started this.
        let corrected = cover::ArtLookup {
            art: art.clone(),
            thumb: thumb.clone(),
            ..named
        };
        let _ =
            cover::record_art_lookup(&connection, key, cover::ArtLookupOutcome::Found(&corrected));
        match rendition {
            ArtRendition::Full => Repaired::Address(art),
            // The archive publishes no thumbnail of its own for some records, and
            // a claim that names none is an ordinary claim: a phone draws the full
            // picture for that row instead.
            ArtRendition::Thumb if thumb.is_empty() => Repaired::NothingToFetch,
            ArtRendition::Thumb => Repaired::Address(thumb),
        }
    }

    /// Write down that a download failed, and when that address may be tried again.
    fn remember_failure(
        &self,
        key: &str,
        rendition: ArtRendition,
        url: &str,
        source: &str,
        failure: FetchFailure,
    ) {
        // The log is about this computer's album, so an address it no longer
        // believes in stays quiet: it was corrected while this download was in
        // flight, and the fetch that corrected it is the one worth reading about.
        // The wait below is written either way - a caller holding the old address
        // - a phone reading somebody else's claim, say - will offer it again.
        if self.resolution_names(key, rendition, url) {
            self.log(key, "failed", source, &failure.message);
        }
        let Ok(connection) = crate::open_connection(&self.db_path) else {
            return;
        };
        // A failure that follows the same address escalates. One that follows a
        // *different* address starts again, because that address has just been
        // corrected and has earned the first wait rather than the last.
        let attempts = art_cache::failures(&connection, key)
            .ok()
            .and_then(|failures| failures.get(art_cache::rendition_name(rendition)).cloned())
            .filter(|previous| previous.url == url)
            .map(|previous| previous.attempts.saturating_add(1))
            .unwrap_or(1);
        let wait = failure_delay(failure.gone, attempts);
        let next_at = (chrono::Utc::now()
            + chrono::Duration::seconds(wait.as_secs().min(i64::MAX as u64) as i64))
        .to_rfc3339();
        let _ = art_cache::record_failure(
            &connection,
            key,
            rendition,
            url,
            &failure.status,
            attempts,
            &next_at,
        );
    }

    /// Forget that a rendition failed, because the picture arrived.
    fn forget_failure(&self, key: &str, rendition: ArtRendition) {
        if let Ok(connection) = crate::open_connection(&self.db_path) {
            let _ = art_cache::clear_failure(&connection, key, rendition);
        }
    }

    /// Write an album off as "nobody has art for this record".
    ///
    /// That is a considered answer rather than a failure — the archive was asked
    /// and it holds nothing — so it is recorded as one: the album leaves the queue
    /// and is asked again in a fortnight rather than in three days.
    fn record_no_art(&self, key: &str, source: &str) {
        if let Ok(connection) = crate::open_connection(&self.db_path) {
            let _ = cover::record_art_lookup(&connection, key, cover::ArtLookupOutcome::NoArt);
        }
        self.log(
            key,
            "none",
            source,
            "the archive holds no picture for this release any more",
        );
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

/// Whether an answer names this address for this rendition.
fn answer_names(answer: &cover::ArtLookup, rendition: ArtRendition, url: &str) -> bool {
    match rendition {
        ArtRendition::Full => answer.art == url,
        ArtRendition::Thumb => answer.thumb == url,
    }
}

/// Which renditions of an album this computer still has to fetch.
///
/// Separated from the fetching so the rule can be read and tested on its own:
/// only renditions the cache does not already hold are wanted, a claim that names
/// no address for one of them is not asking for anything, and an address that has
/// just failed is left alone until its wait is over.
///
/// The wait is keyed on the address that failed. A claim that names a different
/// address for the same rendition has been corrected, and a correction is asked
/// for at once — that is the whole point of correcting one.
fn pending_renditions(
    connection: &Connection,
    root: &Path,
    want: &ArtWant<'_>,
) -> Result<Vec<(ArtRendition, String)>, String> {
    let failures = art_cache::failures(connection, want.key)?;
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
        let waiting = failures
            .get(art_cache::rendition_name(rendition))
            .is_some_and(|failure| failure.url == url.trim() && failure.is_waiting());
        if waiting {
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

    /// Leave one address waiting, as a failed download does.
    fn park(connection: &Connection, key: &str, rendition: ArtRendition, url: &str) {
        let next_at = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        art_cache::record_failure(connection, key, rendition, url, "404", 1, &next_at).unwrap();
    }

    /// The renditions a want would actually fetch, in the order it asks.
    fn wanted(connection: &Connection, root: &Path, want: &ArtWant<'_>) -> Vec<ArtRendition> {
        pending_renditions(connection, root, want)
            .unwrap()
            .into_iter()
            .map(|(rendition, _)| rendition)
            .collect()
    }

    fn annihilated() -> ArtWant<'static> {
        ArtWant {
            key: "kream|annihilation",
            art: "https://coverartarchive.org/release/7aa940e5-6128-4ed1-9d89-86458a1b5ec6/8008267577.jpg",
            thumb: "https://coverartarchive.org/release/7aa940e5-6128-4ed1-9d89-86458a1b5ec6/8008267577-250.jpg",
            source: "musicbrainz",
        }
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
        let art_only = ArtWant { thumb: "", ..want };
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
        assert!(
            fetcher.claim("a|b|thumb"),
            "a rendition is its own download"
        );
        fetcher.release("a|b|full");
        assert!(
            fetcher.claim("a|b|full"),
            "released work can be asked for again"
        );
    }

    #[test]
    fn a_key_is_read_back_as_the_artist_and_the_album() {
        assert_eq!(
            key_halves("kream / korolova|annihilation"),
            ("kream / korolova".to_string(), "annihilation".to_string())
        );
        // A key that is not a pair is still worth logging as itself.
        assert_eq!(
            key_halves("not-a-pair"),
            ("not-a-pair".to_string(), String::new())
        );
    }

    #[test]
    fn the_user_agent_says_what_is_asking() {
        assert!(USER_AGENT.starts_with("Napstr/"));
    }

    #[test]
    fn an_address_that_failed_is_left_alone_but_a_new_one_is_not() {
        // This is the whole of the fix for the album that hammered the archive:
        // four hundred and seventy lookups, one dead address, and a fill loop that
        // asked twice every two seconds because nothing remembered the failure.
        let root = scratch("waiting");
        let connection = database();
        let want = annihilated();
        assert_eq!(
            wanted(&connection, &root, &want),
            vec![ArtRendition::Full, ArtRendition::Thumb],
            "with nothing remembered, both renditions are wanted"
        );

        park(&connection, &want.key, ArtRendition::Full, want.art);
        assert_eq!(
            wanted(&connection, &root, &want),
            vec![ArtRendition::Thumb],
            "the address that failed waits, and the other rendition does not"
        );

        // A claim that names a *different* address for that rendition has been
        // corrected, and a correction is fetched at once - waiting on it would
        // make the repair pointless for as long as the wait lasted.
        let corrected = ArtWant {
            art: "https://coverartarchive.org/release/7aa940e5-6128-4ed1-9d89-86458a1b5ec6/46231447287-1200.jpg",
            ..want
        };
        assert_eq!(
            wanted(&connection, &root, &corrected),
            vec![ArtRendition::Full, ArtRendition::Thumb]
        );
        art_cache::clear_failure(&connection, &want.key, ArtRendition::Full).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_wait_that_has_run_out_is_asked_again() {
        let root = scratch("expired");
        let connection = database();
        let want = annihilated();
        let past = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        art_cache::record_failure(
            &connection,
            want.key,
            ArtRendition::Full,
            want.art,
            "404",
            3,
            &past,
        )
        .unwrap();
        assert_eq!(
            wanted(&connection, &root, &want),
            vec![ArtRendition::Full, ArtRendition::Thumb],
            "a wait that is over is not a wait"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_picture_that_arrived_forgets_the_failure() {
        let root = scratch("forgotten");
        let connection = database();
        let want = annihilated();
        park(&connection, &want.key, ArtRendition::Thumb, want.thumb);
        assert!(art_cache::failures(&connection, want.key)
            .unwrap()
            .contains_key("thumb"));
        art_cache::clear_failure(&connection, &want.key, ArtRendition::Thumb).unwrap();
        assert!(
            art_cache::failures(&connection, want.key)
                .unwrap()
                .is_empty(),
            "art that is held cannot still be waiting to be fetched"
        );

        // Clearing the artwork is a person asking this computer to try again, so
        // it cannot leave a month-long wait behind. It also removes the directory,
        // which is why this does not clean up after itself.
        park(&connection, &want.key, ArtRendition::Thumb, want.thumb);
        art_cache::clear(&connection, &root).unwrap();
        assert!(art_cache::failures(&connection, want.key)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_gone_address_waits_days_and_a_busy_one_waits_minutes() {
        // The host saying "no such file" is an answer about the address. A host
        // that refused, throttled or stumbled is an answer about the moment, and
        // the same address may work in a minute.
        assert!(status_says_gone(404));
        assert!(status_says_gone(410));
        assert!(!status_says_gone(403));
        assert!(!status_says_gone(429));
        assert!(!status_says_gone(500));

        assert_eq!(failure_delay(true, 1), GONE_PARK);
        assert_eq!(failure_delay(true, 2), GONE_PARK * 2);
        assert_eq!(
            failure_delay(true, 99),
            MAX_PARK,
            "waiting longer than a month is the same as never asking again"
        );
        let busy = Duration::from_secs(FAILED_LOOKUP_RETRY_SECONDS as u64);
        assert_eq!(failure_delay(false, 1), busy);
        assert_eq!(
            failure_delay(false, 5),
            failure_delay(false, 99),
            "the wait stops growing instead of growing without a bound"
        );
        assert!(failure_delay(true, 1) > failure_delay(false, 1));
    }

    #[test]
    fn a_batch_whose_addresses_are_all_waiting_takes_on_nothing() {
        // What the fill reads as "is there work". Answering with the number of
        // albums instead is what kept the loop at two seconds for ever: it handed
        // the same dead address over, took nothing on, and asked again.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let root = scratch("nothing-to-take-on");
        let db_path = root.join("art.sqlite3");
        let want = annihilated();
        {
            let connection = Connection::open(&db_path).unwrap();
            art_cache::initialise_schema(&connection).unwrap();
            park(&connection, want.key, ArtRendition::Full, want.art);
            park(&connection, want.key, ArtRendition::Thumb, want.thumb);
        }
        let fetcher = ArtFetcher::new(db_path, root.clone()).expect("a client");
        assert_eq!(fetcher.ensure_all(&[want]), 0);
        assert_eq!(fetcher.in_flight(), 0);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The whole path, against the live archive: an address that has been re-keyed
    /// under a claim is corrected, and the picture arrives from where it lives now
    /// instead of the album waiting out a month of threes-and-days.
    ///
    /// Ignored because it needs the network:
    /// `cargo test --ignored live_gone_address -- --nocapture`
    #[tokio::test]
    #[ignore = "requires the Cover Art Archive"]
    async fn the_live_gone_address_is_corrected_and_the_picture_arrives() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let root = scratch("live-gone-address");
        let db_path = root.join("art.sqlite3");
        // The album this was found on: measured in a real library, its address had
        // been re-keyed and the app asked for it twice every two seconds for as
        // long as it was open.
        let key = "sugar ray|lemonade and brownies";
        let gone = "https://coverartarchive.org/release/c8cb6f90-d97f-4bbb-b79a-35861be2e98e/14894787254.jpg";
        let old_thumb = "https://coverartarchive.org/release/c8cb6f90-d97f-4bbb-b79a-35861be2e98e/14894787254-250.jpg";
        {
            let connection = Connection::open(&db_path).unwrap();
            // The app's own schema, because a bare one leaves the triggers that
            // keep the catalogue in step pointing at tables nobody created.
            crate::network::initialise_network_schema(&connection).unwrap();
            cover::record_art_lookup(
                &connection,
                key,
                cover::ArtLookupOutcome::Found(&cover::ArtLookup {
                    key: key.to_string(),
                    art: gone.to_string(),
                    thumb: old_thumb.to_string(),
                    source: "musicbrainz".to_string(),
                    ..Default::default()
                }),
            )
            .unwrap();
        }

        let fetcher = ArtFetcher::new(db_path.clone(), root.clone()).expect("a client");
        fetcher
            .fetch_rendition(key, gone, ArtRendition::Full, "musicbrainz")
            .await
            .expect("the picture is fetchable once the address is corrected");

        let connection = Connection::open(&db_path).unwrap();
        let held = art_cache::lookup(&connection, &root, key, ArtRendition::Full)
            .unwrap()
            .expect("the picture has to be in the cache");
        assert!(
            held.bytes > 0,
            "and it has to be a picture, not an error page"
        );
        let corrected = cover::stored_art(&connection, key)
            .unwrap()
            .expect("the resolution survives the repair");
        assert_ne!(
            corrected.art, gone,
            "the resolution has to name the live address"
        );
        assert_ne!(corrected.thumb, old_thumb, "and so does the thumbnail");
        assert!(
            art_cache::failures(&connection, key).unwrap().is_empty(),
            "an album that was repaired is not an album that failed"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
