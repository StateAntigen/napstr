//! Finding album art for what this computer holds, and publishing it as a kind
//! `30427` claim.
//!
//! Why the desktop and not the phone: the cover NIP ranks a claim from an active
//! seeder of the album above anybody else's, and this computer is the seeder. It
//! also owns the relay pool and the signing key, and "no Nostr keys leave your
//! computer" is a property worth keeping.
//!
//! Two rules shape everything here:
//!
//! * MusicBrainz asks for about one request a second and a user agent that says
//!   who is calling, so the scan is a throttled queue that identifies itself.
//! * A wrong cover is worse than no cover, so a release group is only used when
//!   its title really matches the album, and a claim is only published when no
//!   other author already has a winning one.
//!
//! Nothing runs unless the user asks for it: [`CoverPublisher::set_enabled`] is
//! the opt-in, and `scan` refuses without it.

use crate::cover::{self, CoverClaimFields};
use crate::network::NetworkService;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::{AppHandle, Emitter};

/// MusicBrainz requires a descriptive user agent and roughly one request a
/// second. Napstr says what it is rather than pretending to be a browser.
const USER_AGENT: &str = concat!(
    "Napstr/",
    env!("CARGO_PKG_VERSION"),
    " ( https://github.com/lnbits/napstr )"
);
const REQUEST_INTERVAL: Duration = Duration::from_millis(1100);
const HTTP_TIMEOUT: Duration = Duration::from_secs(12);
const DEFAULT_SCAN_LIMIT: usize = 40;
const MAX_SCAN_LIMIT: usize = 200;
/// Reported to the window on every step, so a scan of a large library is not a
/// frozen button.
pub const COVER_SCAN_EVENT: &str = "napstr-cover-scan";

/// One album this computer holds that has no cover yet.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverCandidate {
    pub key: String,
    pub artist: String,
    pub album: String,
    pub track_count: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverScanStatus {
    pub running: bool,
    /// False until the user switches cover publishing on.
    pub enabled: bool,
    pub published: usize,
    pub skipped: usize,
    pub failed: usize,
    pub remaining: usize,
    /// The album being looked up right now.
    pub current: String,
    pub message: String,
}

pub struct CoverPublisher {
    db_path: PathBuf,
    network: Arc<NetworkService>,
    app: AppHandle,
    cancel: Arc<AtomicBool>,
    status: Mutex<CoverScanStatus>,
}

impl CoverPublisher {
    pub fn new(
        db_path: PathBuf,
        network: Arc<NetworkService>,
        app: AppHandle,
    ) -> Arc<Self> {
        Arc::new(Self {
            db_path,
            network,
            app,
            cancel: Arc::new(AtomicBool::new(false)),
            status: Mutex::new(CoverScanStatus::default()),
        })
    }

    pub fn status(&self) -> CoverScanStatus {
        self.status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default()
    }

    pub fn set_enabled(&self, enabled: bool) -> CoverScanStatus {
        if let Ok(mut status) = self.status.lock() {
            status.enabled = enabled;
            if !enabled {
                status.message = "Cover publishing is off".into();
            }
        }
        self.report()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Albums this computer holds that no author has covered yet.
    ///
    /// Only the local library is offered: it is the part this computer can be
    /// trusted to know something about, which is exactly what the NIP's ranking
    /// is about.
    pub fn candidates(&self, limit: usize) -> Result<Vec<CoverCandidate>, String> {
        let limit = limit.clamp(1, MAX_SCAN_LIMIT);
        let connection = crate::open_connection(&self.db_path)?;
        let covered = stored_cover_keys(&connection)?;
        let mut statement = connection
            .prepare(
                "SELECT artist, album, COUNT(*) FROM files
                 WHERE format IN ('MP3','FLAC','WAV','OGG','OPUS')
                   AND TRIM(artist) <> '' AND TRIM(album) <> ''
                   AND NOT EXISTS(SELECT 1 FROM blocked_files WHERE blocked_files.file_id=files.file_id)
                 GROUP BY artist, album
                 ORDER BY artist, album",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|error| error.to_string())?;
        let mut candidates = Vec::new();
        for row in rows {
            let (artist, album, track_count) = row.map_err(|error| error.to_string())?;
            let Some(key) = cover::cover_key(&artist, &album) else {
                continue;
            };
            // A claim filed under another selector of the same album counts.
            if cover::cover_lookup_keys(&key)
                .iter()
                .any(|selector| covered.contains(selector))
            {
                continue;
            }
            candidates.push(CoverCandidate {
                key,
                artist,
                album,
                track_count: track_count.max(0) as usize,
            });
            if candidates.len() >= limit {
                break;
            }
        }
        Ok(candidates)
    }

    /// Resolve and publish covers for a batch of albums.
    pub async fn scan(&self, limit: usize) -> Result<CoverScanStatus, String> {
        {
            let mut status = self
                .status
                .lock()
                .map_err(|_| "cover scan state is unavailable")?;
            if !status.enabled {
                return Err("Cover publishing is switched off for this Napstr".into());
            }
            if status.running {
                return Err("A cover scan is already running".into());
            }
            status.running = true;
            status.published = 0;
            status.skipped = 0;
            status.failed = 0;
            status.current.clear();
            status.message = "Looking for albums without covers".into();
        }
        let candidates = self.candidates(limit)?;
        self.cancel.store(false, Ordering::SeqCst);
        if let Ok(mut status) = self.status.lock() {
            status.remaining = candidates.len();
            status.message = format!("Looking up {} albums", candidates.len());
        }
        self.report();

        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|error| format!("could not prepare the cover lookup client: {error}"))?;

        for candidate in &candidates {
            if self.cancel.load(Ordering::SeqCst) {
                break;
            }
            if let Ok(mut status) = self.status.lock() {
                status.current = format!("{} — {}", candidate.artist, candidate.album);
                status.remaining = status.remaining.saturating_sub(1);
            }
            self.report();
            // MusicBrainz's rate limit is the reason this loop is slow.
            tokio::time::sleep(REQUEST_INTERVAL).await;
            match self.consider(&client, candidate).await {
                Ok(Some(_)) => {
                    if let Ok(mut status) = self.status.lock() {
                        status.published += 1;
                    }
                }
                Ok(None) => {
                    if let Ok(mut status) = self.status.lock() {
                        status.skipped += 1;
                    }
                }
                Err(error) => {
                    if let Ok(mut status) = self.status.lock() {
                        status.failed += 1;
                        status.message = error;
                    }
                }
            }
            self.report();
        }

        if let Ok(mut status) = self.status.lock() {
            status.running = false;
            status.current.clear();
            if status.failed == 0 {
                status.message = format!(
                    "Published {} covers, skipped {}",
                    status.published, status.skipped
                );
            }
        }
        Ok(self.status())
    }

    /// One album: skip when it has been covered meanwhile, else resolve and
    /// publish. `Ok(None)` means "nothing worth claiming", not a failure.
    async fn consider(
        &self,
        client: &reqwest::Client,
        candidate: &CoverCandidate,
    ) -> Result<Option<String>, String> {
        // A relay may have answered this album while the scan was running, and
        // somebody else's claim is not this host's to overwrite.
        if !self
            .network
            .album_covers(vec![candidate.key.clone()])
            .await?
            .is_empty()
        {
            return Ok(None);
        }
        let Some(art) = resolve(client, candidate).await? else {
            return Ok(None);
        };
        let fields = CoverClaimFields {
            key: candidate.key.clone(),
            art: art.art,
            thumb: art.thumb,
            mbid: art.mbid,
            year: art.year,
            genre: String::new(),
            collection: art.collection,
            source: "musicbrainz".into(),
        };
        self.network.publish_cover(fields).await.map(Some)
    }

    fn report(&self) -> CoverScanStatus {
        let status = self.status();
        let _ = self.app.emit(COVER_SCAN_EVENT, status.clone());
        status
    }
}

/// What MusicBrainz and the Cover Art Archive agreed on for one album.
struct ResolvedArt {
    art: String,
    thumb: String,
    mbid: String,
    year: String,
    collection: String,
}

#[derive(Deserialize)]
struct MusicBrainzSearch {
    #[serde(default, rename = "release-groups")]
    release_groups: Vec<MusicBrainzGroup>,
}

#[derive(Deserialize)]
struct MusicBrainzGroup {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default, rename = "first-release-date")]
    first_release_date: String,
}

#[derive(Deserialize)]
struct CoverArtArchive {
    #[serde(default)]
    images: Vec<CoverArtImage>,
}

#[derive(Deserialize, Default)]
struct CoverArtImage {
    #[serde(default)]
    image: String,
    #[serde(default)]
    front: bool,
    #[serde(default)]
    thumbnails: CoverArtThumbnails,
}

#[derive(Deserialize, Default)]
struct CoverArtThumbnails {
    #[serde(default)]
    small: String,
}

/// Ask MusicBrainz for the release group, then the Cover Art Archive for its
/// front image. `Ok(None)` is a considered answer: this album has no art there.
async fn resolve(
    client: &reqwest::Client,
    candidate: &CoverCandidate,
) -> Result<Option<ResolvedArt>, String> {
    let query = format!(
        "release:\"{}\" AND artist:\"{}\"",
        escape_query(&candidate.album),
        escape_query(&candidate.artist)
    );
    let search = client
        .get("https://musicbrainz.org/ws/2/release-group/")
        .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "3")])
        .send()
        .await
        .map_err(|error| format!("MusicBrainz lookup failed: {error}"))?;
    if !search.status().is_success() {
        return Err(format!("MusicBrainz answered {}", search.status()));
    }
    let found: MusicBrainzSearch = search
        .json()
        .await
        .map_err(|error| format!("MusicBrainz sent something unreadable: {error}"))?;
    // Only a release group whose title really is this album is worth publishing:
    // a cover on the wrong record is worse than a blank square.
    let Some(group) = found
        .release_groups
        .into_iter()
        .find(|group| !group.id.is_empty() && alike(&group.title, &candidate.album))
    else {
        return Ok(None);
    };

    let archive = client
        .get(format!(
            "https://coverartarchive.org/release-group/{}",
            group.id
        ))
        .send()
        .await
        .map_err(|error| format!("Cover Art Archive lookup failed: {error}"))?;
    // A 404 here is the ordinary "nobody has scanned this record" answer.
    if archive.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !archive.status().is_success() {
        return Err(format!("Cover Art Archive answered {}", archive.status()));
    }
    let archive: CoverArtArchive = archive
        .json()
        .await
        .map_err(|error| format!("Cover Art Archive sent something unreadable: {error}"))?;
    let Some(image) = archive
        .images
        .iter()
        .find(|image| image.front && !image.image.is_empty())
        .or_else(|| archive.images.iter().find(|image| !image.image.is_empty()))
    else {
        return Ok(None);
    };
    Ok(Some(ResolvedArt {
        art: image.image.clone(),
        thumb: image.thumbnails.small.clone(),
        mbid: group.id,
        year: group.first_release_date.chars().take(4).collect(),
        collection: group.title,
    }))
}

/// MusicBrainz's Lucene syntax treats these as operators.
fn escape_query(value: &str) -> String {
    value
        .chars()
        .filter(|character| !"\"\\()[]{}^~*?:".contains(*character))
        .collect::<String>()
        .trim()
        .to_string()
}

/// Compare album names the way a person would: case, punctuation and spacing do
/// not distinguish one record from another.
fn alike(left: &str, right: &str) -> bool {
    fn fold(value: &str) -> String {
        value
            .to_lowercase()
            .chars()
            .map(|character| {
                if character.is_alphanumeric() {
                    character
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
    let left = fold(left);
    let right = fold(right);
    !left.is_empty() && !right.is_empty() && (left == right || left.contains(&right) || right.contains(&left))
}

/// Cover keys that already have a live claim from somebody.
fn stored_cover_keys(
    connection: &rusqlite::Connection,
) -> Result<HashSet<String>, String> {
    let mut statement = connection
        .prepare("SELECT DISTINCT cover_key FROM album_covers WHERE deleted=0")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<HashSet<_>, _>>()
        .map_err(|error| error.to_string())
}

/// The scan limit a caller may ask for, clamped to something interactive.
pub fn clamp_scan_limit(limit: usize) -> usize {
    if limit == 0 {
        DEFAULT_SCAN_LIMIT
    } else {
        limit.clamp(1, MAX_SCAN_LIMIT)
    }
}
