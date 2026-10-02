mod art_store;
mod public_http;
use public_http::{podcast_http_client, safe_public_https_url};

use futures_util::StreamExt;
use iroh::{endpoint::presets, Endpoint, EndpointAddr, EndpointId, SecretKey};
use napstr_remote_protocol::{
    ArtRendition, ClientRequest, DeviceRights, DiscoverMode, PairingTicket, PlaybackCommand, RemoteAlbumCover,
    RemoteAudiobook, RemoteAudiobookSummary, RemoteDiscussionActivity, RemoteDiscussionMessage,
    RemotePlaybackState, RemotePlaylist, RemotePlaylistCoordinate, RemotePlaylistSummary,
    RemoteTrack, RemoteTransfer, ServerResponse, ALPN, MAX_ART_KEY_CHARS, MAX_CONTROL_FRAME_BYTES,
    MAX_COVER_KEYS, MAX_PAGE_SIZE, MAX_PLAYLIST_MEMBERS, MAX_PLAYLIST_PAGE, MAX_PLAY_QUEUE,
    MAX_QR_SVG_BYTES, MAX_REPORT_NOTE_CHARS, MAX_TRACKS_BY_ID, REPORT_REASONS, shuffle_key,
};
use quick_xml::{events::Event, Reader};
use qrcode::{render::svg, QrCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::Duration,
};
use tauri::{Manager, State};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, Notify, RwLock},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedHost {
    endpoint_id: String,
    endpoint_addr: String,
    desktop_name: String,
    #[serde(default)]
    stream_only: bool,
    /// What this computer allows, as it last said.
    ///
    /// A host that has never said - an older one, or a file written before
    /// grants existed - leaves this empty, and then the boolean above is taken
    /// at its word, which is what it has always meant.
    #[serde(default)]
    rights: Option<DeviceRights>,
    /// Whether this phone reads from this computer.
    ///
    /// A computer left out keeps everything about it - the pairing, its key, the
    /// right to act through it - and is only left out of what the phone offers:
    /// its library stops appearing among the others, its rows stop being
    /// searched, and its files stop being asked for. A computer that has never
    /// said is included, which is what every computer has always been.
    #[serde(default = "included_by_default")]
    included: bool,
    /// This computer's own public key, as it last reported it.
    ///
    /// Kept here rather than only in the answer that carried it: it is what
    /// tells this phone which of its playlists are its computer's own, and being
    /// out of reach is exactly when a playlist has to be drawn without asking
    /// anybody. An older host - or one that has not started its own network yet -
    /// reports nothing, which leaves whatever was learned before standing.
    #[serde(default)]
    pubkey: String,
}

impl SavedHost {
    /// What this computer allows.
    fn grant(&self) -> DeviceRights {
        self.rights.unwrap_or_else(|| legacy_grant(self.stream_only))
    }

    /// What this phone can say about the computer without hearing from it.
    ///
    /// `connected` is whether the last exchange succeeded; everything else is
    /// what was saved, so an outage changes the flags and nothing about who the
    /// computer is or which playlists are its own.
    fn status(&self, connected: bool, connecting: bool, error: String) -> CompanionStatus {
        CompanionStatus {
            stream_only: self.grant().is_read_only(),
            paired: true,
            connected,
            connecting,
            desktop_name: self.desktop_name.clone(),
            endpoint_id: self.endpoint_id.clone(),
            library_revision: 0,
            cover_revision: 0,
            pubkey: self.pubkey.clone(),
            error,
        }
    }
}

/// Where the computers this phone may talk to are kept.
///
/// A new file rather than the old one's name in a new shape: the old file is
/// still read once below, and leaving it alone is what lets a phone that has not
/// paired since keep its pairing.
const PAIRED_HOSTS_FILE: &str = "paired-hosts.json";
const LEGACY_PAIRED_FILE: &str = "paired-desktop.json";

/// The computers this phone may talk to.
///
/// A bare list rather than an object wrapping one, because serde ignores fields
/// it does not know: the old file - one computer, written flat - would parse as
/// a wrapper with an empty list, and the pairing would vanish quietly.
fn load_hosts(path: &Path, legacy: &Path) -> Vec<SavedHost> {
    if let Ok(bytes) = fs::read(path) {
        if let Ok(hosts) = serde_json::from_slice::<Vec<SavedHost>>(&bytes) {
            return hosts;
        }
    }
    let Ok(bytes) = fs::read(legacy) else {
        return Vec::new();
    };
    serde_json::from_slice::<SavedHost>(&bytes)
        .map(|host| vec![host])
        .unwrap_or_default()
}

fn save_hosts(path: &Path, hosts: &[SavedHost]) -> Result<(), String> {
    save_json(path, &hosts)
}

/// Add a computer, or replace what is known about one that is already here.
///
/// Pairing a computer this phone knows is how its grant is changed by scanning a
/// code, so it must not leave a second copy of the same machine behind.
fn upsert_host(hosts: &mut Vec<SavedHost>, host: SavedHost) {
    match hosts
        .iter_mut()
        .find(|saved| saved.endpoint_id == host.endpoint_id)
    {
        Some(saved) => *saved = host,
        None => hosts.push(host),
    }
}

/// The computer this phone acts through.
///
/// The one that lets it act as its owner, when any of them do: that pairing is
/// what this phone signs with, and it is the computer whose library and status
/// the rest of the app is drawn from. With none of them privileged - a phone
/// lent browse-and-play, or one paired only with a friend - it is the first it
/// was paired with, which is the single computer it has ever had.
fn primary_host(hosts: &[SavedHost]) -> Option<SavedHost> {
    hosts
        .iter()
        .find(|host| host.grant().privileged)
        .or_else(|| hosts.first())
        .cloned()
}

/// What the old boolean meant, for a host that only speaks it.
fn legacy_grant(stream_only: bool) -> DeviceRights {
    if stream_only {
        DeviceRights::read_only()
    } else {
        DeviceRights::full()
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CompanionStatus {
    stream_only: bool,
    paired: bool,
    connected: bool,
    /// A tunnel to the computer is being opened right now.
    ///
    /// Its own state rather than a flavour of `connected`, because the two say
    /// different things to a person: "connecting" is the app doing something and
    /// asking to be waited for, and "offline" is nothing happening at all. A cold
    /// start spends its first seconds in the first, and used to be shown as the
    /// second.
    connecting: bool,
    desktop_name: String,
    endpoint_id: String,
    library_revision: u64,
    /// Moves when the host's art changes, so cached covers - including "the host
    /// has none" - are asked about again instead of being trusted forever.
    cover_revision: u64,
    /// The computer's own public key, or empty while it has not said.
    ///
    /// A phone holds no key of its own, so this is the only thing that can tell
    /// a playlist this computer wrote down from a public one somebody else
    /// published - and therefore the only thing that decides what may be edited
    /// here and what may only be read and copied.
    pubkey: String,
    error: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryPage {
    tracks: Vec<RemoteTrack>,
    total: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlaylistPage {
    playlists: Vec<RemotePlaylistSummary>,
    total: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OfflineLibrary {
    stream_only: bool,
    tracks: Vec<RemoteTrack>,
    total: usize,
    paired: bool,
    desktop_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedRemoteAudio {
    track: RemoteTrack,
    #[serde(default = "default_library_visible")]
    library_visible: bool,
}

fn default_library_visible() -> bool {
    true
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AudiobookLibraryPage {
    audiobooks: Vec<RemoteAudiobookSummary>,
    total: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CachedAudio {
    url: String,
    track: RemoteTrack,
}

/// A read-only pairing code, minted by the host, for this phone to show.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadOnlyTicketOffer {
    uri: String,
    /// Empty when the host drew no QR, or drew something this app is not willing
    /// to insert into its own page.
    qr_svg: String,
    expires_at: i64,
    desktop_name: String,
}

/// What the host signed and published on this phone's behalf.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CoverReport {
    report_id: String,
    queued: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PodcastFeed {
    id: u64,
    title: String,
    author: String,
    description: String,
    feed_url: String,
    image: String,
    language: String,
    episode_count: u64,
    #[serde(default)]
    genres: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PodcastEpisode {
    id: u64,
    feed_id: u64,
    feed_title: String,
    title: String,
    description: String,
    enclosure_url: String,
    enclosure_type: String,
    enclosure_length: u64,
    date_published: i64,
    duration: u64,
    image: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CachedPodcastAudio {
    url: String,
    downloaded: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PodcastDownload {
    episode: PodcastEpisode,
    progress: f64,
    status: String,
    ready: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredPodcast {
    episode: PodcastEpisode,
    received: u64,
    total: u64,
    status: String,
    ready: bool,
}

#[derive(Deserialize)]
struct PublicPodcastSearch {
    results: Vec<PublicPodcastFeed>,
}

#[derive(Deserialize)]
struct PublicPodcastEpisodeLookup {
    results: Vec<PublicPodcastEpisode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublicPodcastFeed {
    collection_id: Option<u64>,
    track_id: Option<u64>,
    collection_name: Option<String>,
    track_name: Option<String>,
    artist_name: Option<String>,
    feed_url: Option<String>,
    artwork_url600: Option<String>,
    artwork_url100: Option<String>,
    country: Option<String>,
    track_count: Option<u64>,
    genres: Option<Vec<String>>,
    primary_genre_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublicPodcastEpisode {
    track_id: Option<u64>,
    collection_id: Option<u64>,
    collection_name: Option<String>,
    track_name: Option<String>,
    description: Option<String>,
    short_description: Option<String>,
    episode_url: Option<String>,
    episode_content_type: Option<String>,
    episode_file_extension: Option<String>,
    track_time_millis: Option<u64>,
    release_date: Option<String>,
    artwork_url600: Option<String>,
    artwork_url160: Option<String>,
}

#[derive(Default)]
struct PodcastEpisodeBuilder {
    guid: String,
    title: String,
    description: String,
    enclosure_url: String,
    enclosure_type: String,
    enclosure_length: u64,
    date_published: i64,
    duration: u64,
    image: String,
}

struct MediaEntry {
    track: RemoteTrack,
    final_path: PathBuf,
    temporary_path: PathBuf,
    received: AtomicU64,
    complete: AtomicBool,
    error: StdMutex<Option<String>>,
    changed: Notify,
}

impl MediaEntry {
    fn completed(track: RemoteTrack, final_path: PathBuf, temporary_path: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            received: AtomicU64::new(track.size),
            complete: AtomicBool::new(true),
            track,
            final_path,
            temporary_path,
            error: StdMutex::new(None),
            changed: Notify::new(),
        })
    }

    fn downloading(track: RemoteTrack, final_path: PathBuf, temporary_path: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            track,
            final_path,
            temporary_path,
            received: AtomicU64::new(0),
            complete: AtomicBool::new(false),
            error: StdMutex::new(None),
            changed: Notify::new(),
        })
    }

    fn failure(&self) -> Option<String> {
        self.error.lock().ok().and_then(|error| error.clone())
    }

    fn fail(&self, message: String) {
        if let Ok(mut error) = self.error.lock() {
            *error = Some(message);
        }
        self.changed.notify_waiters();
    }
}

struct MediaServer {
    port: u16,
    token: String,
    /// Where the artwork this phone holds is served from. The same server as
    /// audio because it is the same origin and the same token: one local address
    /// for the window's policy to allow, and one place a picture can come from.
    art_root: PathBuf,
    entries: RwLock<HashMap<String, Arc<MediaEntry>>>,
    prepare_lock: Mutex<()>,
    scheduled_prefetches: Mutex<HashSet<String>>,
}

impl MediaServer {
    fn start(art_root: PathBuf) -> Result<Arc<Self>, String> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
            .map_err(|error| format!("could not start the private audio player: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("could not configure the private audio player: {error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        let token = hex::encode(SecretKey::generate().to_bytes());
        let server = Arc::new(Self {
            port,
            token,
            art_root,
            entries: RwLock::new(HashMap::new()),
            prepare_lock: Mutex::new(()),
            scheduled_prefetches: Mutex::new(HashSet::new()),
        });
        let service = server.clone();
        tauri::async_runtime::spawn(async move {
            let Ok(listener) = TcpListener::from_std(listener) else {
                return;
            };
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let service = service.clone();
                tokio::spawn(async move {
                    let _ = service.serve(socket).await;
                });
            }
        });
        Ok(server)
    }

    fn url(&self, track: &RemoteTrack) -> Result<String, String> {
        let extension = safe_extension(&track.format)?;
        Ok(format!(
            "http://127.0.0.1:{}/{}/{}.{}",
            self.port, self.token, track.file_id, extension
        ))
    }

    /// Where one picture may be drawn from.
    ///
    /// The hash is in the address rather than a name the server looks up, so a
    /// webview caching this address caches something that can never change: a
    /// different picture is a different hash and therefore a different address.
    /// The token is in it too, which is why this is minted fresh rather than
    /// stored anywhere: a new run is a new token.
    fn art_url(&self, hash: &str) -> String {
        format!(
            "http://127.0.0.1:{}/{}/art/{}",
            self.port, self.token, hash
        )
    }

    /// Serve one held picture.
    async fn serve_art(
        self: &Arc<Self>,
        socket: &mut TcpStream,
        method: &str,
        hash: &str,
    ) -> Result<(), String> {
        // Not held, or not a name this store uses: the same answer either way,
        // because a caller cannot tell the difference and neither can an
        // attacker who guessed.
        let Some(path) = art_store::path(&self.art_root, hash) else {
            return write_http_error(socket, 404, "Not Found").await;
        };
        let Some(mime) = art_store::mime_of(&path) else {
            return write_http_error(socket, 404, "Not Found").await;
        };
        let size = std::fs::metadata(&path)
            .map_err(|error| error.to_string())?
            .len();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {size}\r\nCache-Control: private, max-age=31536000, immutable\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n"
        );
        socket
            .write_all(headers.as_bytes())
            .await
            .map_err(|error| error.to_string())?;
        if method == "HEAD" {
            return Ok(());
        }
        let mut file = tokio::fs::File::open(&path)
            .await
            .map_err(|error| error.to_string())?;
        tokio::io::copy(&mut file, socket)
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    async fn entry(&self, file_id: &str) -> Option<Arc<MediaEntry>> {
        self.entries.read().await.get(file_id).cloned()
    }

    async fn insert(&self, entry: Arc<MediaEntry>) {
        self.entries
            .write()
            .await
            .insert(entry.track.file_id.clone(), entry);
    }

    async fn wait_until_complete(&self, file_id: &str) -> Result<(), String> {
        let entry = self
            .entry(file_id)
            .await
            .ok_or("the current track is not being cached")?;
        loop {
            let changed = entry.changed.notified();
            if let Some(error) = entry.failure() {
                return Err(error);
            }
            if entry.complete.load(Ordering::Acquire) {
                return Ok(());
            }
            changed.await;
        }
    }

    async fn serve(self: Arc<Self>, mut socket: TcpStream) -> Result<(), String> {
        let mut request = Vec::with_capacity(2048);
        let mut buffer = [0u8; 2048];
        loop {
            let count = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buffer))
                .await
                .map_err(|_| "local audio request timed out".to_string())?
                .map_err(|error| error.to_string())?;
            if count == 0 {
                return Ok(());
            }
            request.extend_from_slice(&buffer[..count]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
            if request.len() > 16 * 1024 {
                return write_http_error(&mut socket, 431, "Request Header Fields Too Large").await;
            }
        }
        let request = std::str::from_utf8(&request).map_err(|_| "invalid HTTP request")?;
        let mut lines = request.split("\r\n");
        let first = lines.next().ok_or("empty HTTP request")?;
        let mut first = first.split_whitespace();
        let method = first.next().ok_or("missing HTTP method")?;
        let path = first.next().ok_or("missing HTTP path")?;
        if method != "GET" && method != "HEAD" {
            return write_http_error(&mut socket, 405, "Method Not Allowed").await;
        }
        let mut segments = path.trim_start_matches('/').split('/');
        if segments.next() != Some(self.token.as_str()) {
            return write_http_error(&mut socket, 404, "Not Found").await;
        }
        let requested = segments.next().ok_or("missing audio ID")?;
        if requested == "art" {
            let hash = segments.next().ok_or("missing art hash")?;
            if segments.next().is_some() {
                return write_http_error(&mut socket, 404, "Not Found").await;
            }
            return self.serve_art(&mut socket, method, hash).await;
        }
        if segments.next().is_some() {
            return write_http_error(&mut socket, 404, "Not Found").await;
        }
        let (file_id, extension) = requested.rsplit_once('.').ok_or("invalid audio ID")?;
        validate_file_id(file_id)?;
        let entry = self.entry(file_id).await.ok_or("audio is not prepared")?;
        let track = &entry.track;
        if safe_extension(&track.format)? != extension {
            return write_http_error(&mut socket, 404, "Not Found").await;
        }
        let range_header = lines.find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("range")
                .then(|| value.trim().to_string())
        });
        let range = match requested_audio_range(range_header.as_deref(), track.size) {
            Ok(range) => range,
            Err(()) => {
                let response = format!(
                    "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{}\r\nConnection: close\r\n\r\n",
                    track.size
                );
                socket
                    .write_all(response.as_bytes())
                    .await
                    .map_err(|error| error.to_string())?;
                return Ok(());
            }
        };
        let (start, end, status) = match range {
            Some((start, end)) => (start, end, "206 Partial Content"),
            None => (0, track.size - 1, "200 OK"),
        };
        let mut headers = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {}\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: private, no-store\r\nConnection: close\r\n",
            audio_mime(extension).ok_or("unsupported audio format")?,
            end - start + 1
        );
        if range.is_some() {
            headers.push_str(&format!(
                "Content-Range: bytes {start}-{end}/{}\r\n",
                track.size
            ));
        }
        headers.push_str("\r\n");
        socket
            .write_all(headers.as_bytes())
            .await
            .map_err(|error| error.to_string())?;
        if method == "HEAD" {
            return Ok(());
        }
        stream_cached_audio(&mut socket, entry, start, end).await
    }
}

async fn write_http_error(socket: &mut TcpStream, code: u16, reason: &str) -> Result<(), String> {
    let response =
        format!("HTTP/1.1 {code} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    socket
        .write_all(response.as_bytes())
        .await
        .map_err(|error| error.to_string())
}

async fn stream_cached_audio(
    socket: &mut TcpStream,
    entry: Arc<MediaEntry>,
    start: u64,
    end: u64,
) -> Result<(), String> {
    let mut offset = start;
    let mut file = loop {
        let changed = entry.changed.notified();
        let path = if entry.complete.load(Ordering::Acquire) {
            &entry.final_path
        } else {
            &entry.temporary_path
        };
        match tokio::fs::File::open(path).await {
            Ok(file) => break file,
            Err(_) => {
                if let Some(error) = entry.failure() {
                    return Err(error);
                }
                changed.await;
            }
        }
    };
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|error| error.to_string())?;
    let mut buffer = vec![0u8; 128 * 1024];
    while offset <= end {
        let changed = entry.changed.notified();
        if let Some(error) = entry.failure() {
            return Err(error);
        }
        let complete = entry.complete.load(Ordering::Acquire);
        let available = entry.received.load(Ordering::Acquire);
        if available <= offset {
            if complete {
                return Err("verified audio cache ended unexpectedly".into());
            }
            changed.await;
            continue;
        }
        let count = (available - offset)
            .min(end - offset + 1)
            .min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..count])
            .await
            .map_err(|error| format!("could not read the audio cache: {error}"))?;
        socket
            .write_all(&buffer[..count])
            .await
            .map_err(|error| error.to_string())?;
        offset += count as u64;
    }
    Ok(())
}

struct PodcastStore {
    root: PathBuf,
    metadata_client: reqwest::Client,
    download_client: reqwest::Client,
    downloads: RwLock<HashMap<u64, StoredPodcast>>,
}

impl PodcastStore {
    fn new(app_data: &Path) -> Result<Arc<Self>, String> {
        let root = app_data.join("podcasts");
        fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        let mut downloads = HashMap::new();
        if let Ok(entries) = fs::read_dir(&root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                let Some(stored) = fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<StoredPodcast>(&bytes).ok())
                else {
                    continue;
                };
                if stored.ready
                    && validate_podcast_episode(&stored.episode).is_ok()
                    && podcast_audio_path(&root, &stored.episode).is_ok_and(|audio| audio.is_file())
                {
                    downloads.insert(stored.episode.id, stored);
                }
            }
        }
        let metadata_client =
            podcast_http_client(Duration::from_secs(20), Duration::from_secs(10))?;
        let download_client =
            podcast_http_client(Duration::from_secs(60 * 60), Duration::from_secs(45))?;
        Ok(Arc::new(Self {
            root,
            metadata_client,
            download_client,
            downloads: RwLock::new(downloads),
        }))
    }

    async fn feed_episodes(
        &self,
        feed: &PodcastFeed,
        limit: usize,
    ) -> Result<Vec<PodcastEpisode>, String> {
        // Older directory entries may not expose episodes through lookup. RSS is
        // retained as a native compatibility fallback. Apple directory requests
        // use Android WebView networking and are parsed by a separate command.
        let url = reqwest::Url::parse(&feed.feed_url)
            .map_err(|_| "Podcast directory returned an invalid feed URL")?;
        if !safe_public_https_url(&url) {
            return Err("Only public HTTPS podcast feeds are supported".into());
        }
        let bytes = self
            .get_bounded(url, 8 * 1024 * 1024, "podcast publisher")
            .await?;
        parse_podcast_feed(feed, &bytes, limit.clamp(1, 50))
    }

    async fn get_bounded(
        &self,
        url: reqwest::Url,
        maximum: u64,
        source: &str,
    ) -> Result<Vec<u8>, String> {
        let request = async {
            let response = self
                .metadata_client
                .get(url)
                .send()
                .await
                .map_err(|error| format!("Could not reach {source}: {error}"))?;
            if !response.status().is_success() || !safe_public_https_url(response.url()) {
                return Err(format!("{source} returned {}", response.status()));
            }
            if response
                .content_length()
                .is_some_and(|length| length > maximum)
            {
                return Err(format!("{source} response is too large"));
            }
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|error| format!("Could not read {source}: {error}"))?;
                if bytes.len().saturating_add(chunk.len()) > maximum as usize {
                    return Err(format!("{source} response is too large"));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        };
        tokio::time::timeout(Duration::from_secs(15), request)
            .await
            .map_err(|_| {
                format!(
                    "{source} did not respond within 15 seconds. Check your connection and retry."
                )
            })?
    }

    async fn list(&self) -> Vec<PodcastDownload> {
        let mut downloads = self
            .downloads
            .read()
            .await
            .values()
            .map(|stored| PodcastDownload {
                episode: stored.episode.clone(),
                progress: if stored.total == 0 {
                    0.0
                } else {
                    (stored.received as f64 / stored.total as f64 * 100.0).clamp(0.0, 100.0)
                },
                status: stored.status.clone(),
                ready: stored.ready,
            })
            .collect::<Vec<_>>();
        downloads.sort_by(|left, right| {
            right
                .episode
                .date_published
                .cmp(&left.episode.date_published)
        });
        downloads
    }

    async fn start(self: &Arc<Self>, episode: PodcastEpisode) -> Result<(), String> {
        validate_podcast_episode(&episode)?;
        {
            let mut downloads = self.downloads.write().await;
            if downloads
                .get(&episode.id)
                .is_some_and(|download| download.ready || download.status == "Downloading")
            {
                return Ok(());
            }
            downloads.insert(
                episode.id,
                StoredPodcast {
                    total: episode.enclosure_length,
                    episode: episode.clone(),
                    received: 0,
                    status: "Downloading".into(),
                    ready: false,
                },
            );
        }
        let store = self.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = store.download(episode.clone()).await {
                let _ = tokio::fs::remove_file(store.temporary_path(episode.id)).await;
                if let Some(download) = store.downloads.write().await.get_mut(&episode.id) {
                    download.status = format!("Failed: {error}");
                    download.ready = false;
                }
            }
        });
        Ok(())
    }

    async fn download(&self, episode: PodcastEpisode) -> Result<(), String> {
        let response = self
            .download_client
            .get(&episode.enclosure_url)
            .send()
            .await
            .map_err(|error| format!("Podcast download failed: {error}"))?;
        if !response.status().is_success() || !safe_public_https_url(response.url()) {
            return Err(format!("publisher returned {}", response.status()));
        }
        let advertised = episode.enclosure_length;
        let response_length = response.content_length().unwrap_or_default();
        let total = response_length.max(advertised);
        if total > 2 * 1024 * 1024 * 1024 {
            return Err("podcast episode is larger than 2 GB".into());
        }
        if let Some(download) = self.downloads.write().await.get_mut(&episode.id) {
            download.total = total;
        }
        let temporary = self.temporary_path(episode.id);
        let final_path = podcast_audio_path(&self.root, &episode)?;
        let _ = tokio::fs::remove_file(&temporary).await;
        let mut output = tokio::fs::File::create(&temporary)
            .await
            .map_err(|error| error.to_string())?;
        let mut stream = response.bytes_stream();
        let mut received = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| format!("publisher stream failed: {error}"))?;
            received = received.saturating_add(chunk.len() as u64);
            if received > 2 * 1024 * 1024 * 1024
                || (response_length > 0 && received > response_length)
            {
                return Err("publisher sent more audio than advertised".into());
            }
            output
                .write_all(&chunk)
                .await
                .map_err(|error| error.to_string())?;
            if let Some(download) = self.downloads.write().await.get_mut(&episode.id) {
                download.received = received;
            }
        }
        output.flush().await.map_err(|error| error.to_string())?;
        drop(output);
        if received == 0 || (response_length > 0 && received != response_length) {
            return Err("podcast download ended before the advertised size".into());
        }
        tokio::fs::rename(&temporary, &final_path)
            .await
            .map_err(|error| error.to_string())?;
        let stored = {
            let mut downloads = self.downloads.write().await;
            let stored = downloads
                .get_mut(&episode.id)
                .ok_or("podcast download disappeared")?;
            stored.received = received;
            stored.total = received;
            stored.status = "Downloaded".into();
            stored.ready = true;
            stored.clone()
        };
        tokio::fs::write(
            self.metadata_path(episode.id),
            serde_json::to_vec(&stored).map_err(|error| error.to_string())?,
        )
        .await
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    async fn playback_url(
        &self,
        episode: PodcastEpisode,
        media: Arc<MediaServer>,
    ) -> Result<CachedPodcastAudio, String> {
        validate_podcast_episode(&episode)?;
        let stored = self.downloads.read().await.get(&episode.id).cloned();
        let Some(stored) = stored.filter(|stored| stored.ready) else {
            return Ok(CachedPodcastAudio {
                url: episode.enclosure_url,
                downloaded: false,
            });
        };
        let path = podcast_audio_path(&self.root, &stored.episode)?;
        let size = fs::metadata(&path)
            .map_err(|error| error.to_string())?
            .len();
        if size == 0 || size != stored.received {
            return Err("the offline podcast copy is incomplete".into());
        }
        let track = podcast_media_track(&stored.episode, size)?;
        media
            .insert(MediaEntry::completed(
                track.clone(),
                path,
                self.temporary_path(stored.episode.id),
            ))
            .await;
        Ok(CachedPodcastAudio {
            url: media.url(&track)?,
            downloaded: true,
        })
    }

    fn metadata_path(&self, episode_id: u64) -> PathBuf {
        self.root.join(format!("{episode_id}.json"))
    }

    fn temporary_path(&self, episode_id: u64) -> PathBuf {
        self.root.join(format!(".{episode_id}.part"))
    }
}

fn public_podcast_feed(feed: PublicPodcastFeed) -> Option<PodcastFeed> {
    let id = feed.collection_id.or(feed.track_id)?;
    let feed_url = normalized_public_https_url(feed.feed_url.as_deref()?, None)?;
    if id == 0 {
        return None;
    }
    let image = feed
        .artwork_url600
        .or(feed.artwork_url100)
        .and_then(|value| normalized_public_https_url(&value, None))
        .map(|url| url.to_string())
        .unwrap_or_default();
    let mut genres = feed
        .genres
        .unwrap_or_default()
        .into_iter()
        .map(|genre| clean_podcast_text(&genre, 64))
        .filter(|genre| !genre.is_empty())
        .collect::<Vec<_>>();
    if let Some(primary) = feed.primary_genre_name {
        let primary = clean_podcast_text(&primary, 64);
        if !primary.is_empty()
            && !genres
                .iter()
                .any(|genre| genre.eq_ignore_ascii_case(&primary))
        {
            genres.insert(0, primary);
        }
    }
    let mut unique_genres = Vec::new();
    for genre in genres {
        if !unique_genres
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&genre))
        {
            unique_genres.push(genre);
        }
        if unique_genres.len() == 12 {
            break;
        }
    }
    Some(PodcastFeed {
        id,
        title: clean_podcast_text(
            feed.collection_name
                .or(feed.track_name)
                .as_deref()
                .unwrap_or("Untitled podcast"),
            300,
        ),
        author: clean_podcast_text(feed.artist_name.as_deref().unwrap_or_default(), 200),
        description: String::new(),
        feed_url: feed_url.to_string(),
        image,
        language: clean_podcast_text(feed.country.as_deref().unwrap_or_default(), 32),
        episode_count: feed.track_count.unwrap_or_default(),
        genres: unique_genres,
    })
}

fn parse_public_podcast_search(payload: &str, limit: usize) -> Result<Vec<PodcastFeed>, String> {
    if payload.len() > 4 * 1024 * 1024 {
        return Err("Podcast directory response is too large".into());
    }
    let response: PublicPodcastSearch = serde_json::from_str(payload)
        .map_err(|_| "Podcast directory returned invalid search results".to_string())?;
    Ok(response
        .results
        .into_iter()
        .take(limit.clamp(1, 50))
        .filter_map(public_podcast_feed)
        .collect())
}

fn parse_public_podcast_episodes(
    feed: &PodcastFeed,
    payload: &str,
    limit: usize,
) -> Result<Vec<PodcastEpisode>, String> {
    if payload.len() > 8 * 1024 * 1024 {
        return Err("Podcast directory response is too large".into());
    }
    let response: PublicPodcastEpisodeLookup = serde_json::from_str(payload)
        .map_err(|_| "Podcast directory returned invalid episode results".to_string())?;
    Ok(response
        .results
        .into_iter()
        .filter_map(|episode| public_podcast_episode(feed, episode))
        .take(limit.clamp(1, 50))
        .collect())
}

fn public_podcast_episode(
    feed: &PodcastFeed,
    episode: PublicPodcastEpisode,
) -> Option<PodcastEpisode> {
    let enclosure = normalized_public_https_url(episode.episode_url.as_deref()?, None)?;
    let extension = episode
        .episode_file_extension
        .as_deref()
        .unwrap_or_default()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    let enclosure_type = match extension.as_str() {
        "mp3" => "audio/mpeg".to_string(),
        "m4a" | "mp4" | "aac" => "audio/mp4".to_string(),
        "ogg" | "oga" => "audio/ogg".to_string(),
        "opus" => "audio/opus".to_string(),
        "wav" => "audio/wav".to_string(),
        _ if episode
            .episode_content_type
            .as_deref()
            .is_some_and(|kind| kind.eq_ignore_ascii_case("audio")) =>
        {
            podcast_mime_from_url(&enclosure)?
        }
        _ => podcast_mime_from_url(&enclosure)?,
    };
    let id = episode.track_id.unwrap_or_else(|| {
        let digest = Sha256::digest(enclosure.as_str().as_bytes());
        u64::from_be_bytes(digest[..8].try_into().unwrap_or_default()).max(1)
    });
    let image = episode
        .artwork_url600
        .or(episode.artwork_url160)
        .and_then(|value| normalized_public_https_url(&value, None))
        .map(|url| url.to_string())
        .unwrap_or_else(|| feed.image.clone());
    let description = episode.description.or(episode.short_description);
    Some(PodcastEpisode {
        id,
        feed_id: episode.collection_id.unwrap_or(feed.id),
        feed_title: clean_podcast_text(
            episode.collection_name.as_deref().unwrap_or(&feed.title),
            300,
        ),
        title: clean_podcast_text(
            episode.track_name.as_deref().unwrap_or("Untitled episode"),
            500,
        ),
        description: clean_podcast_text(description.as_deref().unwrap_or_default(), 4_000),
        enclosure_url: enclosure.to_string(),
        enclosure_type,
        enclosure_length: 0,
        date_published: episode
            .release_date
            .as_deref()
            .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
            .map(|date| date.timestamp())
            .unwrap_or_default(),
        duration: episode.track_time_millis.unwrap_or_default() / 1_000,
        image,
    })
}

fn parse_podcast_feed(
    feed: &PodcastFeed,
    bytes: &[u8],
    limit: usize,
) -> Result<Vec<PodcastEpisode>, String> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut current_tag = String::new();
    let mut current: Option<PodcastEpisodeBuilder> = None;
    let mut episodes = Vec::new();

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) => {
                let tag =
                    String::from_utf8_lossy(element.local_name().as_ref()).to_ascii_lowercase();
                if tag == "item" || tag == "entry" {
                    current = Some(PodcastEpisodeBuilder::default());
                } else if let Some(episode) = current.as_mut() {
                    apply_podcast_element_attributes(&reader, &element, &tag, episode);
                }
                current_tag = tag;
            }
            Ok(Event::Empty(element)) => {
                if let Some(episode) = current.as_mut() {
                    let tag =
                        String::from_utf8_lossy(element.local_name().as_ref()).to_ascii_lowercase();
                    apply_podcast_element_attributes(&reader, &element, &tag, episode);
                }
            }
            Ok(Event::Text(text)) => {
                if let (Some(episode), Ok(value)) = (current.as_mut(), text.decode()) {
                    apply_podcast_text(episode, &current_tag, &value);
                }
            }
            Ok(Event::CData(text)) => {
                if let (Some(episode), Ok(value)) = (current.as_mut(), text.decode()) {
                    apply_podcast_text(episode, &current_tag, &value);
                }
            }
            Ok(Event::End(element)) => {
                let tag =
                    String::from_utf8_lossy(element.local_name().as_ref()).to_ascii_lowercase();
                if tag == "item" || tag == "entry" {
                    if let Some(episode) = current
                        .take()
                        .and_then(|item| finish_podcast_episode(feed, item))
                    {
                        episodes.push(episode);
                        if episodes.len() >= limit {
                            break;
                        }
                    }
                }
                current_tag.clear();
            }
            Ok(Event::Eof) => break,
            Err(_) => return Err("The publisher returned an invalid podcast feed".into()),
            _ => {}
        }
        buffer.clear();
    }
    Ok(episodes)
}

fn apply_podcast_element_attributes(
    reader: &Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
    tag: &str,
    episode: &mut PodcastEpisodeBuilder,
) {
    let mut url = None;
    let mut mime = None;
    let mut length = None;
    let mut relationship = None;
    for attribute in element.attributes().with_checks(false).flatten() {
        let key = String::from_utf8_lossy(attribute.key.local_name().as_ref()).to_ascii_lowercase();
        let Ok(value) = attribute
            .decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, reader.decoder())
        else {
            continue;
        };
        match key.as_str() {
            "url" | "href" => url = Some(value.into_owned()),
            "type" => mime = Some(value.into_owned()),
            "length" => length = value.parse().ok(),
            "rel" => relationship = Some(value.into_owned()),
            _ => {}
        }
    }
    if tag == "image" {
        if let Some(value) = url {
            episode.image = value;
        }
        return;
    }
    let relationship = relationship.as_deref().unwrap_or_default();
    let media_content = tag == "content" || tag == "source";
    if tag == "enclosure"
        || (tag == "link" && relationship.eq_ignore_ascii_case("enclosure"))
        || media_content
    {
        if let Some(value) = url {
            episode.enclosure_url = value;
        }
        if let Some(value) = mime {
            episode.enclosure_type = value;
        }
        if let Some(value) = length {
            episode.enclosure_length = value;
        }
    }
}

fn apply_podcast_text(episode: &mut PodcastEpisodeBuilder, tag: &str, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    match tag {
        "guid" | "id" => episode.guid.push_str(value),
        "title" => episode.title.push_str(value),
        "description" | "summary" | "content" => episode.description.push_str(value),
        "pubdate" | "published" | "updated" => {
            episode.date_published = chrono::DateTime::parse_from_rfc2822(value)
                .or_else(|_| chrono::DateTime::parse_from_rfc3339(value))
                .map(|date| date.timestamp())
                .unwrap_or_default();
        }
        "duration" => episode.duration = parse_podcast_duration(value),
        _ => {}
    }
}

fn finish_podcast_episode(
    feed: &PodcastFeed,
    item: PodcastEpisodeBuilder,
) -> Option<PodcastEpisode> {
    let feed_url = normalized_public_https_url(&feed.feed_url, None)?;
    let enclosure = normalized_public_https_url(&item.enclosure_url, Some(&feed_url))?;
    let enclosure_type = if item
        .enclosure_type
        .to_ascii_lowercase()
        .starts_with("audio/")
    {
        clean_podcast_text(&item.enclosure_type, 100)
    } else {
        podcast_mime_from_url(&enclosure)?
    };
    let identity = if item.guid.is_empty() {
        enclosure.as_str()
    } else {
        &item.guid
    };
    let digest = Sha256::digest(identity.as_bytes());
    let id = u64::from_be_bytes(digest[..8].try_into().ok()?).max(1);
    let image = normalized_public_https_url(&item.image, Some(&feed_url))
        .map(|url| url.to_string())
        .unwrap_or_else(|| feed.image.clone());
    Some(PodcastEpisode {
        id,
        feed_id: feed.id,
        feed_title: feed.title.clone(),
        title: clean_podcast_text(
            if item.title.is_empty() {
                "Untitled episode"
            } else {
                &item.title
            },
            500,
        ),
        description: clean_podcast_text(&item.description, 4_000),
        enclosure_url: enclosure.to_string(),
        enclosure_type,
        enclosure_length: item.enclosure_length,
        date_published: item.date_published,
        duration: item.duration,
        image,
    })
}

fn podcast_mime_from_url(url: &reqwest::Url) -> Option<String> {
    let path = url.path().to_ascii_lowercase();
    if path.ends_with(".mp3") {
        Some("audio/mpeg".into())
    } else if path.ends_with(".m4a") || path.ends_with(".mp4") {
        Some("audio/mp4".into())
    } else if path.ends_with(".ogg") || path.ends_with(".oga") {
        Some("audio/ogg".into())
    } else if path.ends_with(".opus") {
        Some("audio/opus".into())
    } else if path.ends_with(".wav") {
        Some("audio/wav".into())
    } else {
        None
    }
}

fn parse_podcast_duration(value: &str) -> u64 {
    value
        .split(':')
        .filter_map(|part| part.trim().parse::<u64>().ok())
        .fold(0u64, |total, part| {
            total.saturating_mul(60).saturating_add(part)
        })
}

fn clean_podcast_text(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(
                    character,
                    '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
                )
        })
        .take(limit)
        .collect::<String>()
        .trim()
        .to_string()
}

fn validate_podcast_episode(episode: &PodcastEpisode) -> Result<(), String> {
    if episode.id == 0
        || episode.feed_id == 0
        || episode.enclosure_length > 2 * 1024 * 1024 * 1024
        || !episode
            .enclosure_type
            .to_ascii_lowercase()
            .starts_with("audio/")
    {
        return Err("Podcast directory returned an invalid audio episode".into());
    }
    let url = reqwest::Url::parse(&episode.enclosure_url)
        .map_err(|_| "Podcast directory returned an invalid enclosure URL")?;
    if !safe_public_https_url(&url) {
        return Err("Only public HTTPS podcast audio is supported".into());
    }
    podcast_extension(episode)?;
    Ok(())
}

fn normalized_public_https_url(value: &str, base: Option<&reqwest::Url>) -> Option<reqwest::Url> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let mut url = reqwest::Url::parse(value)
        .ok()
        .or_else(|| base.and_then(|base| base.join(value).ok()))?;
    if url.scheme() == "http" {
        url.set_scheme("https").ok()?;
    }
    safe_public_https_url(&url).then_some(url)
}

fn podcast_extension(episode: &PodcastEpisode) -> Result<&'static str, String> {
    let mime = episode.enclosure_type.to_ascii_lowercase();
    if mime.contains("mpeg") || mime.contains("mp3") {
        return Ok("mp3");
    }
    if mime.contains("mp4") || mime.contains("m4a") || mime.contains("aac") {
        return Ok("m4a");
    }
    if mime.contains("ogg") {
        return Ok("ogg");
    }
    if mime.contains("opus") {
        return Ok("opus");
    }
    if mime.contains("wav") {
        return Ok("wav");
    }
    Err("This podcast audio format is not supported".into())
}

fn podcast_audio_path(root: &Path, episode: &PodcastEpisode) -> Result<PathBuf, String> {
    Ok(root.join(format!("{}.{}", episode.id, podcast_extension(episode)?)))
}

fn podcast_media_track(episode: &PodcastEpisode, size: u64) -> Result<RemoteTrack, String> {
    let extension = podcast_extension(episode)?;
    let file_id = hex::encode(Sha256::digest(episode.enclosure_url.as_bytes()));
    Ok(RemoteTrack {
        file_id,
        filename: format!("{}.{}", episode.id, extension),
        title: episode.title.clone(),
        artist: episode.feed_title.clone(),
        album: episode.feed_title.clone(),
        format: extension.to_ascii_uppercase(),
        mime: episode.enclosure_type.clone(),
        size,
        tags: "podcast".into(),
        local: true,
        sources: Vec::new(),
        // A podcast episode is not in the music library and a feed's duration
        // field is not reliable enough to be worth filtering on, so a podcast
        // track reports nothing about its audio.
        bitrate_kbps: 0,
        sample_rate_hz: 0,
        channels: 0,
        lossless: false,
        duration_ms: 0,
    })
}

struct RemoteClient {
    app_data: PathBuf,
    endpoint: tokio::sync::RwLock<Option<Endpoint>>,
    /// One connection per computer.
    ///
    /// A map rather than a slot, because these fail one at a time: a friend's
    /// computer being asleep must not cost this phone the tunnel it acts
    /// through, and a track that came from one computer has to stay playable
    /// while another is out of reach.
    connections:
        tokio::sync::RwLock<std::collections::HashMap<String, iroh::endpoint::Connection>>,
    /// Every computer this phone may talk to, in the order they were paired.
    hosts: tokio::sync::RwLock<Vec<SavedHost>>,
    /// The rows of the shuffled mix collected so far, for the seed they belong
    /// to. Empty until a shuffle is asked for.
    mixed: tokio::sync::RwLock<Option<MixedLibrary>>,
    /// Which computer answered with which file.
    ///
    /// A queue is a list of file ids, and a file id is a hash of the file's own
    /// bytes, so any computer holding it can serve it. What this remembers is the
    /// one that answered with the row, so that playing a friend's track asks the
    /// friend rather than asking the phone's own computer and waiting for it to
    /// say it has no such file.
    origins: tokio::sync::RwLock<HashMap<String, String>>,
    /// The computers a tunnel is being opened to right now.
    ///
    /// A status question that finds no tunnel answers "connecting" and opens one
    /// in the background rather than waiting for it: the wait is up to
    /// twenty-five seconds, and a status line should not hold a screen for that.
    /// Kept here so two questions in the same second do not open two tunnels to
    /// the same computer.
    connecting: tokio::sync::RwLock<std::collections::HashSet<String>>,
    start_lock: tokio::sync::Mutex<()>,
}

/// Why one attempt at one request failed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AttemptFailure {
    /// The stream or the connection broke, which is the one failure that says
    /// the tunnel itself is gone.
    Transport,
    /// The computer did not answer in time. Slow is not the same as gone: a
    /// computer busy with a large library page is still a computer.
    Timeout,
}

/// Whether a failed attempt should drop the tunnel.
///
/// Two reasons to keep it, and both of them were bugs. A probe never drops it: a
/// question is not evidence about the connection it was asked over, and this one
/// is asked on every connect and then every thirty seconds, so dropping it took
/// down every other request using the same tunnel - which is how a phone that was
/// playing music was reported as offline. A timed-out request does not drop it
/// either, so that the retry has somewhere to go rather than paying for a fresh
/// connect.
fn attempt_drops_tunnel(may_close: bool, failure: AttemptFailure) -> bool {
    may_close && failure == AttemptFailure::Transport
}

/// What one computer has given of the mix so far.
struct MixedHost {
    endpoint_id: String,
    /// The rows it has sent, in the order its own seed produced them.
    rows: Vec<RemoteTrack>,
    /// How many rows it says it holds, once it has said. A computer that has not
    /// answered has not said it holds nothing.
    total: Option<usize>,
    /// It could not be reached, so this mix has what it gave and no more.
    failed: bool,
}

/// The mix of everything this phone may read, collected a page at a time.
///
/// Nothing here decides an order: a shuffle is a property of the seed, and the
/// order is the shared key over every row that arrives. What is kept is rows
/// already fetched, because a page deep into a mix needs every computer's rows
/// down to that depth, and collecting them again on each step of a scroll would
/// transfer the same rows over and over.
///
/// It stands for as long as the seed does. A phone mints a seed for a shuffle and
/// drops it when the person turns the shuffle off or asks for another, so a mix
/// kept here is stale exactly when the seed is replaced.
struct MixedLibrary {
    /// The seed its rows were collected for. `None` is the plain list, which is
    /// every computer's library one after another rather than a mix.
    seed: Option<u64>,
    /// The computers it was collected from, so pairing or forgetting one starts
    /// the mix again rather than mixing in rows from a computer that has gone.
    endpoints: Vec<String>,
    hosts: Vec<MixedHost>,
}

/// Whether a computer this phone has never been told about is read from.
///
/// Yes: a file written before a computer could be left out holds no such answer,
/// and every computer was included before one could be left out.
fn included_by_default() -> bool {
    true
}

/// The computers this phone may read music from, in the order it asks them: the
/// one it acts through first, then the others as it holds them - or why none can
/// be asked.
///
/// That order is the whole of the weighting. A file two computers both hold is
/// one file, and taking it from the first of them keeps the phone's own copy -
/// the row that knows a bitrate and where the file is - rather than a friend's.
///
/// A computer left out in Settings is not asked at all, which is the whole of
/// what leaving one out means: it is still paired, and this phone still acts
/// through it if it is the one that may.
fn readable_hosts(hosts: &[SavedHost]) -> Result<Vec<SavedHost>, String> {
    if hosts.is_empty() {
        return Err("Pair Napstrfy with Napstr first".into());
    }
    if !hosts.iter().any(|host| host.included) {
        return Err("Every computer is left out in Settings".into());
    }
    let primary = primary_host(hosts).map(|host| host.endpoint_id);
    let mut readable = hosts
        .iter()
        .filter(|host| host.included && host.grant().browse)
        .cloned()
        .collect::<Vec<_>>();
    if readable.is_empty() {
        return Err("This phone may not read these computers' libraries".into());
    }
    // Stable, so the computers after the first keep the order they were paired in.
    readable.sort_by_key(|host| primary.as_deref() != Some(host.endpoint_id.as_str()));
    Ok(readable)
}

/// What several computers answered to one search, one row per file.
///
/// A file id is a hash of the file's own bytes, so the same recording found on
/// two computers is one row: the first answer wins, and because the phone's own
/// computer is asked first, that is its row.
fn merge_search_results(answers: Vec<Vec<RemoteTrack>>) -> Vec<RemoteTrack> {
    let mut merged: Vec<RemoteTrack> = Vec::new();
    let mut seen = HashSet::new();
    for tracks in answers {
        for track in tracks {
            if seen.insert(track.file_id.clone()) {
                merged.push(track);
            }
        }
    }
    merged
}

/// One page of a list, out of the rows collected so far.
///
/// Every computer's rows are tried for a place, each file once. A seed puts the
/// rows in the order it gives - the same key the computers sorted their own
/// libraries by - so a friend's music falls among the phone's own; without one,
/// the computers keep their own order, one after another.
fn mixed_page(
    hosts: &[MixedHost],
    seed: Option<u64>,
    offset: usize,
    limit: usize,
) -> (Vec<RemoteTrack>, usize) {
    let mut seen = HashSet::new();
    let mut order = Vec::new();
    for host in hosts {
        for track in &host.rows {
            if seen.insert(track.file_id.as_str()) {
                order.push(track);
            }
        }
    }
    if let Some(seed) = seed {
        order.sort_by_cached_key(|track| shuffle_key(seed, &track.file_id));
    }
    let page = order
        .into_iter()
        .skip(offset)
        .take(limit)
        .cloned()
        .collect::<Vec<_>>();
    // What the computers together say they hold. A file they both hold is
    // counted twice here, so this is an upper bound - and an exact number when
    // there is one computer, which is every phone that has paired with one.
    let total = hosts
        .iter()
        .map(|host| host.total.unwrap_or(host.rows.len()))
        .sum();
    (page, total)
}

/// What to ask one computer for next, or nothing when it has given all it has.
///
/// `held` is how many rows of its mix are here, `need` how deep the page being
/// built goes. Answers are capped by the protocol, so a page deeper than that cap
/// arrives as several requests rather than one larger one.
fn next_mix_request(held: usize, total: Option<usize>, need: usize) -> Option<(usize, usize)> {
    if held >= need {
        return None;
    }
    let remaining = match total {
        Some(total) if held >= total => return None,
        Some(total) => total - held,
        None => need - held,
    };
    Some((held, (need - held).min(MAX_PAGE_SIZE).min(remaining)))
}

/// The computers to ask for a file, in the order to ask them, or why none of them
/// may be asked.
///
/// The computer that answered with the file is asked first, because it is the one
/// known to hold it, then the computer this phone acts through, then the others as
/// it holds them. Every one of them is a copy of the same file: a file id is a
/// hash of the file's own bytes, so a copy that differs is not this file and is
/// refused when its bytes are checked. That is what makes a fallback safe rather
/// than a guess - and what makes a queue built from several computers play on.
///
/// Only computers granted the fetching right are asked. One that may be read but
/// not taken audio from is not asked for a file at all, which is the same refusal
/// the computer itself would give, made here where it can name the problem.
fn fetch_order(origin: Option<&str>, hosts: &[SavedHost]) -> Result<Vec<SavedHost>, String> {
    if hosts.is_empty() {
        return Err("Pair Napstrfy with Napstr first".into());
    }
    let primary = primary_host(hosts).map(|host| host.endpoint_id);
    let mut may_fetch = hosts
        .iter()
        .filter(|host| host.included && host.grant().fetch)
        .cloned()
        .collect::<Vec<_>>();
    // Stable, so the computers after the first keep the order they were paired in.
    may_fetch.sort_by_key(|host| primary.as_deref() != Some(host.endpoint_id.as_str()));
    let mut order = Vec::new();
    if let Some(origin) = origin {
        if let Some(host) = may_fetch.iter().find(|host| host.endpoint_id == origin) {
            order.push(host.clone());
        }
    }
    for host in may_fetch {
        if !order.iter().any(|asked| asked.endpoint_id == host.endpoint_id) {
            order.push(host);
        }
    }
    if order.is_empty() {
        return Err("This phone may not take audio from these computers".into());
    }
    Ok(order)
}

/// The computers this phone holds, minus the one named, or why it holds no such
/// computer.
///
/// Forgetting one computer is not forgetting the others, which is the whole
/// reason this is separate from `forget`: a friend who is dropped must not take
/// the phone's own computer with them. The last computer can be forgotten too -
/// that is exactly what a phone holding nothing is - and naming one this phone
/// never had is refused rather than quietly leaving it a host short.
fn without_host(hosts: Vec<SavedHost>, endpoint_id: &str) -> Result<Vec<SavedHost>, String> {
    if !hosts.iter().any(|host| host.endpoint_id == endpoint_id) {
        return Err("That computer is not paired with this phone".into());
    }
    Ok(hosts
        .into_iter()
        .filter(|host| host.endpoint_id != endpoint_id)
        .collect())
}

impl RemoteClient {
    async fn close_connection(&self, endpoint_id: &str) {
        if let Some(connection) = self.connections.write().await.remove(endpoint_id) {
            connection.close(0u32.into(), b"pairing changed");
        }
    }

    async fn disconnect(&self) {
        for (_, connection) in self.connections.write().await.drain() {
            connection.close(0u32.into(), b"pairing changed");
        }
    }

    fn new(app_data: PathBuf) -> Arc<Self> {
        let hosts = load_hosts(
            &app_data.join(PAIRED_HOSTS_FILE),
            &app_data.join(LEGACY_PAIRED_FILE),
        );
        Arc::new(Self {
            app_data,
            endpoint: tokio::sync::RwLock::new(None),
            connections: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            hosts: tokio::sync::RwLock::new(hosts),
            mixed: tokio::sync::RwLock::new(None),
            origins: tokio::sync::RwLock::new(HashMap::new()),
            connecting: tokio::sync::RwLock::new(std::collections::HashSet::new()),
            start_lock: tokio::sync::Mutex::new(()),
        })
    }

    async fn endpoint(&self) -> Result<Endpoint, String> {
        if let Some(endpoint) = self.endpoint.read().await.clone() {
            return Ok(endpoint);
        }
        let _guard = self.start_lock.lock().await;
        if let Some(endpoint) = self.endpoint.read().await.clone() {
            return Ok(endpoint);
        }
        let key = load_or_create_key(&self.app_data.join("iroh-identity"))?;
        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(key)
            .bind()
            .await
            .map_err(|error| format!("Iroh failed to start: {error}"))?;
        *self.endpoint.write().await = Some(endpoint.clone());
        Ok(endpoint)
    }

    /// The computer this phone acts through.
    async fn primary(&self) -> Result<SavedHost, String> {
        let hosts = self.hosts.read().await;
        primary_host(&hosts).ok_or_else(|| "Pair Napstrfy with Napstr first".into())
    }

    /// Every computer this phone may talk to, for a screen that offers more than
    /// one.
    async fn hosts(&self) -> Vec<SavedHost> {
        self.hosts.read().await.clone()
    }

    /// A tunnel to one computer, opened if it is not already.
    async fn connection(&self, host: &SavedHost) -> Result<iroh::endpoint::Connection, String> {
        if let Some(connection) = self.connections.read().await.get(&host.endpoint_id).cloned() {
            return Ok(connection);
        }
        let address = decode_endpoint_addr(host)?;
        let connection = tokio::time::timeout(
            Duration::from_secs(25),
            self.endpoint().await?.connect(address, ALPN),
        )
        .await
        .map_err(|_| "Napstr did not answer over Iroh")?
        .map_err(|error| format!("Could not reach Napstr: {error}"))?;
        self.connections
            .write()
            .await
            .insert(host.endpoint_id.clone(), connection.clone());
        Ok(connection)
    }

    async fn pair(&self, code: &str, device_name: &str) -> Result<String, String> {
        let ticket = PairingTicket::from_uri(code)?;
        if ticket.expires_at < chrono_timestamp() {
            return Err("This pairing code has expired. Create another in Napstr.".into());
        }
        let host = SavedHost {
            endpoint_id: ticket.endpoint_id.clone(),
            endpoint_addr: ticket.endpoint_addr.clone(),
            desktop_name: ticket.desktop_name.clone(),
            stream_only: false,
            // What this computer allows arrives in the answer to the code that
            // was scanned, so nothing is assumed before it does.
            rights: None,
            // Read from until someone says otherwise, which is what a pairing
            // has always meant.
            included: true,
            // The computer's own key is learned from its first status answer,
            // which is where "is this playlist mine?" is answered from.
            pubkey: String::new(),
        };
        let endpoint = self.endpoint().await?;
        let address = decode_endpoint_addr(&host)?;
        let connection =
            tokio::time::timeout(Duration::from_secs(25), endpoint.connect(address, ALPN))
                .await
                .map_err(|_| "Napstr did not answer. Keep its Mobile page open and try again.")?
                .map_err(|error| format!("Could not pair over Iroh: {error}"))?;
        let response = tokio::time::timeout(
            Duration::from_secs(15),
            exchange_on(
                &connection,
                ClientRequest::Pair {
                    token: ticket.token,
                    device_name: clean_device_name(device_name),
                },
            ),
        )
        .await
        .map_err(|_| "Napstr did not complete pairing in time")??
        .0;
        let (desktop_name, grant) = match response {
            ServerResponse::Paired {
                desktop_name,
                stream_only,
                rights,
            } => (
                desktop_name,
                rights.unwrap_or_else(|| legacy_grant(stream_only)),
            ),
            other => return Err(unexpected_response(&other)),
        };
        let mut saved = host;
        saved.desktop_name = desktop_name.clone();
        saved.stream_only = grant.is_read_only();
        saved.rights = Some(grant);
        // The connection this pairing was made on is the one to keep for this
        // computer, and every other computer's tunnel is left as it was: a
        // second code for a friend is not a reason to drop the first.
        {
            let mut hosts = self.hosts.write().await;
            upsert_host(&mut hosts, saved.clone());
            save_hosts(&self.app_data.join(PAIRED_HOSTS_FILE), &hosts)?;
        }
        self.close_connection(&saved.endpoint_id).await;
        self.connections
            .write()
            .await
            .insert(saved.endpoint_id.clone(), connection);
        Ok(desktop_name)
    }

    async fn forget(&self) -> Result<(), String> {
        self.disconnect().await;
        self.hosts.write().await.clear();
        self.origins.write().await.clear();
        // Both files. Forgetting and then reading the file the older build wrote
        // is how a pairing comes back from the dead.
        for name in [PAIRED_HOSTS_FILE, LEGACY_PAIRED_FILE] {
            match fs::remove_file(self.app_data.join(name)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(())
    }

    async fn request(&self, request: ClientRequest) -> Result<ServerResponse, String> {
        let (response, _) = self.exchange(request).await?;
        match response {
            ServerResponse::Error { message } => Err(message),
            response => Ok(response),
        }
    }

    async fn exchange(
        &self,
        request: ClientRequest,
    ) -> Result<(ServerResponse, iroh::endpoint::RecvStream), String> {
        let host = self.primary().await?;
        self.exchange_with(&host, request).await
    }

    /// Ask one computer, named by the endpoint id a screen chose.
    ///
    /// `None` is the computer this phone acts through, which is what every
    /// caller meant before there could be more than one - so a screen that has
    /// not learned about sources keeps asking exactly whom it asked before.
    ///
    /// A question worth asking every computer - a search, a shuffled page - fans
    /// out before it reaches here, so what arrives here is always one computer.
    async fn request_from(
        &self,
        source: Option<&str>,
        request: ClientRequest,
    ) -> Result<ServerResponse, String> {
        let (response, _) = self.exchange_from(source, request).await?;
        match response {
            ServerResponse::Error { message } => Err(message),
            response => Ok(response),
        }
    }

    async fn exchange_from(
        &self,
        source: Option<&str>,
        request: ClientRequest,
    ) -> Result<(ServerResponse, iroh::endpoint::RecvStream), String> {
        let host = self.host_named(source).await?;
        self.exchange_with(&host, request).await
    }

    /// One computer out of the ones this phone holds.
    async fn host_named(&self, source: Option<&str>) -> Result<SavedHost, String> {
        let Some(endpoint_id) = source else {
            return self.primary().await;
        };
        self.hosts
            .read()
            .await
            .iter()
            .find(|host| host.endpoint_id == endpoint_id)
            .cloned()
            .ok_or_else(|| "That computer is not paired with this phone".into())
    }

    /// What one computer answers, or why it could not.
    ///
    /// A refusal arrives as a message rather than as a transport failure, which
    /// is how `request_from` reports one too: both are things a person may be
    /// told about a particular computer.
    async fn try_request(
        &self,
        host: &SavedHost,
        request: ClientRequest,
    ) -> Result<ServerResponse, String> {
        match self.exchange_with(host, request).await {
            Ok((ServerResponse::Error { message }, _)) => Err(message),
            Ok((response, _)) => Ok(response),
            Err(error) => Err(error),
        }
    }

    /// A search answered by every computer this phone may read from.
    ///
    /// They are asked at once. A computer that has gone is only known to have
    /// gone by waiting for it, and asking them one after another would put every
    /// vanished friend's wait in front of the answer.
    ///
    /// The computer this phone acts through is the exception to tolerating a
    /// failure: it is the one that speaks for the phone, so a person whose own
    /// computer cannot be reached is told that, rather than being shown a
    /// friend's matches as though all were well. A friend who cannot be reached
    /// contributes nothing and is not an error - a library that is not there is a
    /// library with nothing in it, and the rest of the answer is still worth
    /// having.
    async fn search_everywhere(&self, query: &str) -> Result<Vec<RemoteTrack>, String> {
        let hosts = readable_hosts(&self.hosts().await)?;
        let primary = primary_host(&hosts).map(|host| host.endpoint_id);
        let answers = futures_util::future::join_all(hosts.iter().map(|host| {
            let request = ClientRequest::Search {
                query: query.to_string(),
            };
            async move {
                (
                    host.endpoint_id.clone(),
                    self.try_request(host, request).await,
                )
            }
        }))
        .await;
        let asks_the_primary = |endpoint_id: &str| primary.as_deref() == Some(endpoint_id);
        let mut merged = Vec::new();
        for (endpoint_id, answer) in answers {
            match answer {
                Ok(ServerResponse::Search { tracks }) => {
                    // Remembered before the rows are merged: a result from a
                    // friend is playable because the friend is on record as the
                    // one holding it.
                    self.remember_origins(&endpoint_id, &tracks).await;
                    merged.push(tracks);
                }
                Ok(response) => {
                    if asks_the_primary(&endpoint_id) {
                        return Err(unexpected_response(&response));
                    }
                }
                Err(error) => {
                    if asks_the_primary(&endpoint_id) {
                        return Err(error);
                    }
                }
            }
        }
        Ok(merge_search_results(merged))
    }

    /// The rows for particular file ids, from every computer this phone may read.
    ///
    /// A playlist, or a queue handed over, is a list of file ids - and the ids are
    /// hashes, so a member may be held by any of the computers this phone may read
    /// while none of them knows about the others. So they are all asked, and the
    /// answers are put back into the order the ids were given in, which is the
    /// order the playlist has. A member two computers hold comes from the phone's
    /// own computer, because that is the one asked first.
    async fn library_by_ids_everywhere(
        &self,
        file_ids: &[String],
    ) -> Result<Vec<RemoteTrack>, String> {
        let hosts = readable_hosts(&self.hosts().await)?;
        let primary = primary_host(&hosts).map(|host| host.endpoint_id);
        let request = ClientRequest::LibraryByIds {
            file_ids: file_ids.to_vec(),
        };
        let answers = futures_util::future::join_all(hosts.iter().map(|host| {
            let request = request.clone();
            async move {
                (
                    host.endpoint_id.clone(),
                    self.try_request(host, request).await,
                )
            }
        }))
        .await;
        let asks_the_primary = |endpoint_id: &str| primary.as_deref() == Some(endpoint_id);
        let mut held: HashMap<String, RemoteTrack> = HashMap::new();
        for (endpoint_id, answer) in answers {
            match answer {
                Ok(ServerResponse::LibraryByIds { tracks }) => {
                    // On record as this computer's, so that playing a member asks
                    // the computer that has it rather than the phone's own.
                    self.remember_origins(&endpoint_id, &tracks).await;
                    for track in tracks {
                        held.entry(track.file_id.clone()).or_insert(track);
                    }
                }
                Ok(response) => {
                    if asks_the_primary(&endpoint_id) {
                        return Err(unexpected_response(&response));
                    }
                }
                Err(error) => {
                    if asks_the_primary(&endpoint_id) {
                        return Err(error);
                    }
                }
            }
        }
        Ok(file_ids
            .iter()
            .filter_map(|file_id| held.remove(file_id))
            .collect())
    }

    /// One page of every library this phone may read, as one list.
    ///
    /// No computer is named, because none of them is the whole library any more.
    /// The phone's own music sits among what its friends hold, each file once - a
    /// file id is a hash of the file's own bytes, so two computers holding the
    /// same recording hold one file - and the computer this phone acts through
    /// answers for a file more than one of them has.
    ///
    /// A seed orders the page; without one, each computer's own order stands, one
    /// computer after another. A filtered page is asked afresh every time, while
    /// a whole library is collected and kept, because a scroll asks for the same
    /// rows over and over.
    async fn mixed_library(
        &self,
        query: &str,
        seed: Option<u64>,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<RemoteTrack>, usize), String> {
        let hosts = readable_hosts(&self.hosts().await)?;
        let endpoints = hosts
            .iter()
            .map(|host| host.endpoint_id.clone())
            .collect::<Vec<_>>();
        let need = offset + limit;
        let primary = primary_host(&hosts).map(|host| host.endpoint_id);
        let asks_the_primary = |endpoint_id: &str| primary.as_deref() == Some(endpoint_id);

        if !query.is_empty() {
            // A filtered page is a question rather than a list: it is asked of
            // every computer at once and merged here, and not kept, because the
            // next question is a different one.
            let mut collected = endpoints
                .iter()
                .map(|endpoint_id| MixedHost {
                    endpoint_id: endpoint_id.clone(),
                    rows: Vec::new(),
                    total: None,
                    failed: false,
                })
                .collect::<Vec<_>>();
            let answers = futures_util::future::join_all(
                hosts
                    .iter()
                    .zip(collected.iter_mut())
                    .map(|(host, rows)| {
                        let is_primary = asks_the_primary(&host.endpoint_id);
                        async move {
                            self.fill_from(host, is_primary, query, None, need, rows)
                                .await
                        }
                    }),
            )
            .await;
            for answer in answers {
                answer?;
            }
            return Ok(mixed_page(&collected, None, offset, limit));
        }

        // Held while the rows below are collected: the rows kept here are the
        // answer to a question about an ordering, so two pages asked for at once
        // would each collect the same rows into it. Nothing else takes this lock,
        // so a page being built never holds up the host list or a connection.
        let mut mixed = self.mixed.write().await;
        let stale = match mixed.as_ref() {
            Some(held) => held.seed != seed || held.endpoints != endpoints,
            None => true,
        };
        if stale {
            *mixed = Some(MixedLibrary {
                seed,
                endpoints: endpoints.clone(),
                hosts: endpoints
                    .iter()
                    .map(|endpoint_id| MixedHost {
                        endpoint_id: endpoint_id.clone(),
                        rows: Vec::new(),
                        total: None,
                        failed: false,
                    })
                    .collect(),
            });
        }
        let mix = mixed.as_mut().expect("a mix was just built");
        for index in 0..mix.hosts.len() {
            let is_primary = asks_the_primary(&mix.hosts[index].endpoint_id);
            self.fill_from(&hosts[index], is_primary, query, seed, need, &mut mix.hosts[index])
                .await?;
        }

        Ok(mixed_page(&mix.hosts, seed, offset, limit))
    }

    /// What one computer has of one page, asked for in as many parts as the
    /// protocol needs.
    ///
    /// Answers are capped, so a page deeper than the cap arrives as several
    /// requests rather than one larger one. A friend who cannot be reached is
    /// left out of the page and the rest of it still stands; the computer this
    /// phone acts through is the one whose failure is the person's business.
    async fn fill_from(
        &self,
        host: &SavedHost,
        is_primary: bool,
        query: &str,
        seed: Option<u64>,
        need: usize,
        collected: &mut MixedHost,
    ) -> Result<(), String> {
        loop {
            if collected.failed {
                return Ok(());
            }
            let Some((at, take)) = next_mix_request(collected.rows.len(), collected.total, need)
            else {
                return Ok(());
            };
            let request = ClientRequest::Library {
                query: query.to_string(),
                offset: at,
                limit: take,
                shuffle_seed: seed,
            };
            match self.try_request(host, request).await {
                Ok(ServerResponse::Library { tracks, total }) => {
                    collected.total = Some(total);
                    // On record as this computer's, so that playing one of these
                    // rows asks the computer that answered with it.
                    self.remember_origins(&collected.endpoint_id, &tracks).await;
                    if tracks.is_empty() {
                        return Ok(());
                    }
                    collected.rows.extend(tracks);
                }
                Ok(response) => {
                    if is_primary {
                        return Err(unexpected_response(&response));
                    }
                    collected.failed = true;
                    return Ok(());
                }
                Err(error) => {
                    if is_primary {
                        return Err(error);
                    }
                    // Left out rather than waited for on every step of a scroll:
                    // the page is as complete as it could be when it was built,
                    // and asking again - a new shuffle, or a filter - tries once
                    // more.
                    collected.failed = true;
                    return Ok(());
                }
            }
        }
    }

    /// Remember which computer answered with which files.
    ///
    /// Only rows from another computer are worth keeping: the one this phone acts
    /// through is asked for a file anyway, and a map of every file it holds would
    /// be a copy of its library on a phone that has no room for one.
    async fn remember_origins(&self, endpoint_id: &str, rows: &[RemoteTrack]) {
        if self
            .primary()
            .await
            .map(|host| host.endpoint_id)
            .is_ok_and(|primary| primary == endpoint_id)
        {
            return;
        }
        let mut origins = self.origins.write().await;
        for row in rows {
            origins.insert(row.file_id.clone(), endpoint_id.to_string());
        }
    }

    /// Forget one computer, keeping every other one this phone holds.
    ///
    /// What goes is the pairing, its tunnel, and anything remembered as having
    /// come from it: a file remembered as a friend's must not be fetched from a
    /// computer this phone no longer speaks to. The mix of libraries goes too,
    /// because it was collected from a set of computers that has just changed -
    /// the next page of a shuffle asks again from what is left.
    async fn forget_host(&self, endpoint_id: &str) -> Result<(), String> {
        {
            let mut hosts = self.hosts.write().await;
            let remaining = without_host(hosts.clone(), endpoint_id)?;
            *hosts = remaining.clone();
            save_hosts(&self.app_data.join(PAIRED_HOSTS_FILE), &remaining)?;
        }
        self.close_connection(endpoint_id).await;
        self.origins
            .write()
            .await
            .retain(|_, origin| origin != endpoint_id);
        *self.mixed.write().await = None;
        Ok(())
    }

    /// Which computer answered with which file, for a row that wants to say so.
    ///
    /// Only files that came from a computer other than the phone's own are here:
    /// a row with no entry is one the phone's own computer answered with, which
    /// is what most of them are.
    async fn file_hosts(&self) -> HashMap<String, String> {
        self.origins.read().await.clone()
    }

    /// Which of these computers answer, and which do not.
    ///
    /// Asked all at once and cut off after a few seconds: this draws a status
    /// line, and a computer that is asleep must not hold up the ones that are
    /// awake. Anything at all back is an answer - a refusal is as good as a
    /// greeting, because both mean the computer is there.
    ///
    /// Two things it does not do, and both were bugs. It never drops a tunnel,
    /// because a question about a computer is not a verdict on a connection. And
    /// it does not ask the computer this phone acts through while a tunnel to it
    /// is already open: the status question that opened it has just answered
    /// this, so asking again spends a round trip per status line learning what is
    /// already known.
    async fn reachable(&self, hosts: &[SavedHost]) -> HashMap<String, bool> {
        let primary = self.primary().await.ok().map(|host| host.endpoint_id);
        let primary = primary.as_ref();
        let held = self.connections.read().await.clone();
        let held = &held;
        let answers = futures_util::future::join_all(hosts.iter().map(|host| async move {
            if primary.is_some_and(|id| id == &host.endpoint_id) && held.contains_key(&host.endpoint_id) {
                return (host.endpoint_id.clone(), true);
            }
            let answered = matches!(
                tokio::time::timeout(
                    Duration::from_secs(6),
                    self.exchange_probe(host, ClientRequest::Ping),
                )
                .await,
                Ok(Ok(_))
            );
            (host.endpoint_id.clone(), answered)
        }))
        .await;
        answers.into_iter().collect()
    }

    /// Whether this phone reads from one of the computers it holds.
    ///
    /// Leaving one out keeps everything about it: the pairing, its key, and the
    /// right to act through it if it is the one that may. What changes is only
    /// what the phone offers - its library among the others, its rows in a
    /// search, its files when a track is played - so a friend who is being
    /// borrowed from, or a connection being paid for, can be left out without
    /// pairing again.
    async fn set_host_included(&self, endpoint_id: &str, included: bool) -> Result<(), String> {
        {
            let mut hosts = self.hosts.write().await;
            let Some(host) = hosts
                .iter_mut()
                .find(|host| host.endpoint_id == endpoint_id)
            else {
                return Err("That computer is not paired with this phone".into());
            };
            host.included = included;
            save_hosts(&self.app_data.join(PAIRED_HOSTS_FILE), &hosts)?;
        }
        // What has been collected so far came from a set of computers that has
        // just changed.
        *self.mixed.write().await = None;
        Ok(())
    }

    /// One file's audio, from the first computer that offers it.
    ///
    /// A queue entry does not have to know which computer it came from: what came
    /// from a friend was remembered when the row arrived, so a friend's track is
    /// asked of the friend. If the friend is out of reach, the phone's own
    /// computer is asked next, and then any other that may be read - a file id is
    /// a hash, so whoever serves it is serving the same file.
    ///
    /// Each computer is tried once, except the last, which is tried twice because
    /// after it there is nowhere else to go. A second attempt at a computer that
    /// has already not answered puts its wait in front of the fallback, and the
    /// fallback is the whole point of the others.
    async fn fetch_audio_from_somewhere(
        &self,
        file_id: &str,
    ) -> Result<(RemoteTrack, iroh::endpoint::RecvStream), String> {
        let origin = self.origins.read().await.get(file_id).cloned();
        let hosts = fetch_order(origin.as_deref(), &self.hosts().await)?;
        let last = hosts.len() - 1;
        let mut last_error = None;
        for (index, host) in hosts.into_iter().enumerate() {
            let request = ClientRequest::FetchAudio {
                file_id: file_id.to_string(),
            };
            let answer = if index == last {
                self.exchange_with(&host, request).await
            } else {
                self.exchange_attempt(&host, request).await
            };
            match answer {
                Ok((ServerResponse::AudioReady { track }, receive)) => return Ok((track, receive)),
                Ok((ServerResponse::Error { message }, _)) => last_error = Some(message),
                Ok((other, _)) => last_error = Some(unexpected_response(&other)),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| "Napstr is unavailable".into()))
    }

    /// One request to one computer, retried once.
    async fn exchange_with(
        &self,
        host: &SavedHost,
        request: ClientRequest,
    ) -> Result<(ServerResponse, iroh::endpoint::RecvStream), String> {
        let mut last_error = "Napstr is unavailable".to_string();
        for _ in 0..2 {
            match self.exchange_attempt(host, request.clone()).await {
                Ok(response) => return Ok(response),
                Err(error) => last_error = error,
            }
        }
        Err(last_error)
    }

    /// One request to one computer, tried once.
    async fn exchange_attempt(
        &self,
        host: &SavedHost,
        request: ClientRequest,
    ) -> Result<(ServerResponse, iroh::endpoint::RecvStream), String> {
        self.exchange_once(host, request, true).await
    }

    /// One request to one computer for the sake of the answer alone, tried once.
    ///
    /// This is what asks whether a computer is there, so it never drops the
    /// tunnel: the asker wants one question answered, and the answer is used to
    /// draw a status line, not to judge the connection.
    async fn exchange_probe(
        &self,
        host: &SavedHost,
        request: ClientRequest,
    ) -> Result<(ServerResponse, iroh::endpoint::RecvStream), String> {
        self.exchange_once(host, request, false).await
    }

    /// One request to one computer, tried once.
    async fn exchange_once(
        &self,
        host: &SavedHost,
        request: ClientRequest,
        may_close: bool,
    ) -> Result<(ServerResponse, iroh::endpoint::RecvStream), String> {
        let (result, failure) = match self.connection(host).await {
            Ok(connection) => match tokio::time::timeout(
                Duration::from_secs(30),
                exchange_on(&connection, request),
            )
            .await
            {
                Ok(Ok(answered)) => (Ok(answered), None),
                Ok(Err(error)) => (Err(error), Some(AttemptFailure::Transport)),
                Err(_) => (
                    Err("Napstr did not answer the request in time".into()),
                    Some(AttemptFailure::Timeout),
                ),
            },
            // Nothing was opened, so there is nothing here to drop: the next
            // attempt is what opens one.
            Err(error) => (Err(error), None),
        };
        match result {
            Ok(response) => Ok(response),
            Err(error) => {
                match failure {
                    Some(failure) if attempt_drops_tunnel(may_close, failure) => {
                        self.close_connection(&host.endpoint_id).await;
                    }
                    _ => {}
                }
                Err(error)
            }
        }
    }

    /// Fetch one rendition of one album's art from the paired computer.
    ///
    /// `None` means the host holds no picture for that album, which is a
    /// considered answer rather than a failure: art it has not fetched yet is an
    /// ordinary state, and this phone should keep whatever it already holds until
    /// the host's cover revision says there is something new.
    ///
    /// Read whole rather than streamed to disk, because a picture is small and
    /// its name is only known once all of it is here: a name this phone cannot
    /// verify is a name it must not write anything under.
    async fn fetch_art(
        &self,
        key: &str,
        rendition: ArtRendition,
    ) -> Result<Option<FetchedArt>, String> {
        let (response, mut receive) = self
            .exchange(ClientRequest::FetchArt {
                key: key.to_string(),
                rendition,
            })
            .await?;
        let (hash, length) = match response {
            ServerResponse::ArtReady { hash, length, .. } => (hash, length),
            ServerResponse::ArtMissing { .. } => return Ok(None),
            ServerResponse::Error { message } => return Err(message),
            other => return Err(unexpected_response(&other)),
        };
        // The size is the host's word, so it is bounded before anything is
        // allocated on its account.
        if length == 0 || length > art_store::MAX_ART_BYTES {
            return Err("Napstr offered artwork of an impossible size".into());
        }
        let mut bytes = Vec::with_capacity(length as usize);
        while (bytes.len() as u64) < length {
            let Some(chunk) = receive
                .read_chunk(256 * 1024)
                .await
                .map_err(|error| format!("Iroh artwork stream failed: {error}"))?
            else {
                return Err("Napstr stopped sending the artwork".into());
            };
            bytes.extend_from_slice(&chunk);
            if bytes.len() as u64 > length {
                return Err("Napstr sent more artwork than it announced".into());
            }
        }
        Ok(Some(FetchedArt { hash, bytes }))
    }

    /// True when the computer this phone acts through may only browse and play,
    /// which is what keeps the phone from offering controls it would refuse.
    async fn stream_only(&self) -> bool {
        self.primary()
            .await
            .map(|host| host.grant().is_read_only())
            .unwrap_or(false)
    }

    async fn status(self: &Arc<Self>) -> CompanionStatus {
        let Ok(host) = self.primary().await else {
            return CompanionStatus {
                stream_only: false,
                paired: false,
                connected: false,
                connecting: false,
                desktop_name: String::new(),
                endpoint_id: String::new(),
                library_revision: 0,
                cover_revision: 0,
                pubkey: String::new(),
                error: String::new(),
            };
        };
        // No tunnel to it yet, which is where every cold start begins. Opening one
        // takes up to twenty-five seconds, and a question that waited that long
        // would report a failure that only meant "not yet" - so it is opened in the
        // background and this answer says what is true meanwhile. The next question
        // is the one that finds the tunnel.
        if self.connections.read().await.get(&host.endpoint_id).is_none() {
            self.open_tunnel_in_background(&host).await;
            // "Connecting" only while something is actually being tried. A computer
            // that is asleep fails its attempt, and after that "offline" is the
            // honest word rather than an app that claims to be busy for ever.
            let trying = self.connecting.read().await.contains(&host.endpoint_id);
            return host.status(false, trying, String::new());
        }
        match tokio::time::timeout(Duration::from_secs(8), self.request(ClientRequest::Status))
            .await
        {
            Err(_) => host.status(false, false, "Napstr did not answer yet".into()),
            Ok(Ok(ServerResponse::Status {
                library_revision,
                cover_revision,
                stream_only,
                rights,
                pubkey,
            })) => {
                let grant = rights.unwrap_or_else(|| legacy_grant(stream_only));
                // An empty key is "this computer has not said", which leaves the
                // one learned earlier standing: a host that is not on the
                // network yet has no keys loaded, and that must not turn this
                // computer's own playlists into somebody else's.
                let pubkey = if pubkey.is_empty() {
                    host.pubkey.clone()
                } else {
                    pubkey
                };
                if grant != host.grant() || (!pubkey.is_empty() && pubkey != host.pubkey) {
                    let mut hosts = self.hosts.write().await;
                    if let Some(saved) = hosts
                        .iter_mut()
                        .find(|saved| saved.endpoint_id == host.endpoint_id)
                    {
                        saved.stream_only = grant.is_read_only();
                        saved.rights = Some(grant);
                        saved.pubkey = pubkey.clone();
                        let _ = save_hosts(&self.app_data.join(PAIRED_HOSTS_FILE), &hosts);
                    }
                }
                CompanionStatus {
                    stream_only: grant.is_read_only(),
                    paired: true,
                    connected: true,
                    connecting: false,
                    desktop_name: host.desktop_name,
                    endpoint_id: host.endpoint_id,
                    library_revision,
                    cover_revision,
                    pubkey,
                    error: String::new(),
                }
            }
            Ok(Err(error)) if error == "invalid Napstrfy request" => self.legacy_status(host).await,
            Ok(Ok(other)) => host.status(false, false, unexpected_response(&other)),
            Ok(Err(error)) => host.status(false, false, error),
        }
    }

    /// Opens a tunnel without waiting for it.
    ///
    /// Called when a status question finds none. Only one may be in flight to one
    /// computer, so a second question in the same second does not open a second
    /// tunnel; the mark is cleared when the attempt ends, however it ends.
    async fn open_tunnel_in_background(self: &Arc<Self>, host: &SavedHost) {
        {
            let mut connecting = self.connecting.write().await;
            if !connecting.insert(host.endpoint_id.clone()) {
                return;
            }
        }
        let client = Arc::clone(self);
        let target = host.clone();
        tokio::spawn(async move {
            // The failure is deliberately not remembered: a computer asleep now may
            // be awake in five seconds, and the status timer is what tries again.
            // Keeping the error would let one old failure outlive the condition that
            // caused it, which is the shape of the bug this is fixing.
            let _ = client.connection(&target).await;
            client.connecting.write().await.remove(&target.endpoint_id);
        });
    }

    async fn legacy_status(&self, host: SavedHost) -> CompanionStatus {
        match tokio::time::timeout(Duration::from_secs(8), self.request(ClientRequest::Ping)).await
        {
            Ok(Ok(ServerResponse::Pong)) => CompanionStatus {
                stream_only: host.grant().is_read_only(),
                paired: true,
                connected: true,
                connecting: false,
                desktop_name: host.desktop_name.clone(),
                endpoint_id: host.endpoint_id.clone(),
                library_revision: 0,
                cover_revision: 0,
                pubkey: host.pubkey.clone(),
                error: String::new(),
            },
            Ok(Ok(other)) => host.status(false, false, unexpected_response(&other)),
            Ok(Err(error)) => host.status(false, false, error),
            Err(_) => host.status(false, false, "Napstr did not answer yet".into()),
        }
    }

    async fn cache_audio(
        self: &Arc<Self>,
        requested_track: RemoteTrack,
        media: Arc<MediaServer>,
        library_visible: bool,
    ) -> Result<CachedAudio, String> {
        validate_cache_track(&requested_track)?;
        let _guard = media.prepare_lock.lock().await;
        if let Some(entry) = media.entry(&requested_track.file_id).await {
            if entry.failure().is_none() {
                validate_matching_track(&requested_track, &entry.track)?;
                return Ok(CachedAudio {
                    url: media.url(&entry.track)?,
                    track: entry.track.clone(),
                });
            }
            media.entries.write().await.remove(&requested_track.file_id);
        }

        let directory = self.app_data.join("audio");
        fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let extension = safe_extension(&requested_track.format)?;
        let path = directory.join(format!("{}.{}", requested_track.file_id, extension));
        let temporary = directory.join(format!(".{}.part", requested_track.file_id));
        let metadata_path = directory.join(format!("{}.json", requested_track.file_id));
        if fs::metadata(&path)
            .map(|metadata| metadata.len() == requested_track.size)
            .unwrap_or(false)
        {
            save_json(
                &metadata_path,
                &CachedRemoteAudio {
                    track: requested_track.clone(),
                    library_visible,
                },
            )?;
            let entry = MediaEntry::completed(requested_track.clone(), path, temporary);
            media.insert(entry).await;
            return Ok(CachedAudio {
                url: media.url(&requested_track)?,
                track: requested_track,
            });
        }

        let (track, mut receive) = self
            .fetch_audio_from_somewhere(&requested_track.file_id)
            .await?;
        validate_cache_track(&track)?;
        validate_matching_track(&requested_track, &track)?;
        let _ = tokio::fs::remove_file(&path).await;
        let _ = tokio::fs::remove_file(&temporary).await;
        let mut output = tokio::fs::File::create(&temporary)
            .await
            .map_err(|error| error.to_string())?;
        let entry = MediaEntry::downloading(track.clone(), path, temporary);
        media.insert(entry.clone()).await;
        tokio::spawn(async move {
            let result = async {
                let mut hasher = Sha256::new();
                let mut received = 0u64;
                while let Some(bytes) = receive
                    .read_chunk(256 * 1024)
                    .await
                    .map_err(|error| format!("Iroh audio stream failed: {error}"))?
                {
                    received = received.saturating_add(bytes.len() as u64);
                    if received > entry.track.size || received > 2 * 1024 * 1024 * 1024 {
                        return Err("Napstr sent more audio data than advertised".to_string());
                    }
                    hasher.update(&bytes);
                    output
                        .write_all(&bytes)
                        .await
                        .map_err(|error| error.to_string())?;
                    // Tokio file writes can still be buffered when write_all returns.
                    // Publish availability only once another file handle can read it.
                    output.flush().await.map_err(|error| error.to_string())?;
                    entry.received.store(received, Ordering::Release);
                    entry.changed.notify_waiters();
                }
                output.flush().await.map_err(|error| error.to_string())?;
                drop(output);
                if received != entry.track.size
                    || hex::encode(hasher.finalize()) != entry.track.file_id
                {
                    return Err(
                        "Audio verification failed; the cached copy was discarded".to_string()
                    );
                }
                tokio::fs::rename(&entry.temporary_path, &entry.final_path)
                    .await
                    .map_err(|error| error.to_string())?;
                save_json(
                    &metadata_path,
                    &CachedRemoteAudio {
                        track: entry.track.clone(),
                        library_visible,
                    },
                )?;
                entry.complete.store(true, Ordering::Release);
                entry.changed.notify_waiters();
                Ok::<(), String>(())
            }
            .await;
            if let Err(error) = result {
                let _ = tokio::fs::remove_file(&entry.temporary_path).await;
                entry.fail(error);
            }
        });
        Ok(CachedAudio {
            url: media.url(&track)?,
            track,
        })
    }

    async fn cached_entries(&self) -> Result<Vec<CachedRemoteAudio>, String> {
        let app_data = self.app_data.clone();
        tokio::task::spawn_blocking(move || cached_entries_in(&app_data))
            .await
            .map_err(|error| format!("could not read the offline audio cache: {error}"))?
    }

    async fn offline_library(&self) -> Result<OfflineLibrary, String> {
        let cached = self.cached_entries().await?;
        let host = self.primary().await.ok();
        let tracks = cached
            .into_iter()
            .filter(|item| item.library_visible)
            .map(|item| item.track)
            .collect::<Vec<_>>();
        Ok(OfflineLibrary {
            stream_only: host.as_ref().is_some_and(|host| host.grant().is_read_only()),
            total: tracks.len(),
            tracks,
            paired: host.is_some(),
            desktop_name: host.map(|item| item.desktop_name).unwrap_or_default(),
        })
    }

    async fn reconcile_cache(
        &self,
        media: Arc<MediaServer>,
        protected_file_ids: HashSet<String>,
    ) -> Result<bool, String> {
        let cached = self.cached_entries().await?;
        if cached.is_empty() {
            return Ok(true);
        }
        let mut available = HashSet::new();
        let file_ids = cached
            .iter()
            .map(|item| item.track.file_id.clone())
            .collect::<Vec<_>>();
        for batch in file_ids.chunks(200) {
            match self
                .request(ClientRequest::Available {
                    file_ids: batch.to_vec(),
                })
                .await?
            {
                ServerResponse::Available { file_ids } => available.extend(file_ids),
                response => return Err(unexpected_response(&response)),
            }
        }

        let directory = self.app_data.join("audio");
        let mut deferred = false;
        for item in cached.iter() {
            let file_id = &item.track.file_id;
            if available.contains(file_id) {
                continue;
            }
            if protected_file_ids.contains(file_id) {
                deferred = true;
                continue;
            }
            media.entries.write().await.remove(file_id);
            let extension = safe_extension(&item.track.format)?;
            remove_if_present(&directory.join(format!("{file_id}.{extension}")))?;
            remove_if_present(&directory.join(format!(".{file_id}.part")))?;
            remove_if_present(&directory.join(format!("{file_id}.json")))?;
        }
        // And the store has a bound. Without one, what a phone keeps is not "what
        // it has played" but "what has never been removed": the only eviction was
        // this loop, which drops files the computer no longer holds. Three tracks
        // pre-loaded deep on a two-hundred track queue grows this directory for as
        // long as the queue runs.
        evict_beyond_budget(&directory, &cached, &protected_file_ids, AUDIO_CACHE_BUDGET_BYTES)?;
        Ok(!deferred)
    }
}

/// How much audio this phone keeps beyond what it is playing and about to play.
///
/// Two gigabytes is roughly a long album's worth of lossless or several hundred
/// tracks of lossy audio: enough that the cap is never what a person runs into in
/// a session, and small enough to matter on a phone that is also holding the
/// system's own storage.
const AUDIO_CACHE_BUDGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Drop the least recently used audio until the store is inside its budget.
///
/// Least recently used is the sidecar's own timestamp, because `cache_audio`
/// rewrites it every time a track is played or found already here — so what goes is
/// what has not been listened to for longest, rather than what arrived first.
/// Anything the phone names as protected is never a candidate whatever it weighs:
/// that is what it is playing and what it has queued behind it.
fn evict_beyond_budget(
    directory: &Path,
    cached: &[CachedRemoteAudio],
    protected_file_ids: &HashSet<String>,
    budget: u64,
) -> Result<usize, String> {
    let mut held = Vec::new();
    let mut total = 0u64;
    for item in cached {
        let file_id = item.track.file_id.clone();
        let extension = safe_extension(&item.track.format)?;
        let audio = directory.join(format!("{file_id}.{extension}"));
        let size = fs::metadata(&audio).map(|data| data.len()).unwrap_or(0);
        total = total.saturating_add(size);
        if protected_file_ids.contains(&file_id) {
            continue;
        }
        // An entry with no usable timestamp sorts first, so something this cannot
        // order is what goes rather than something it keeps for ever.
        let used = fs::metadata(directory.join(format!("{file_id}.json")))
            .and_then(|data| data.modified())
            .ok();
        held.push((used, file_id, extension, size));
    }
    if total <= budget {
        return Ok(0);
    }
    held.sort_by_key(|(used, ..)| *used);
    let mut evicted = 0;
    for (_, file_id, extension, size) in held {
        if total <= budget {
            break;
        }
        remove_if_present(&directory.join(format!("{file_id}.{extension}")))?;
        remove_if_present(&directory.join(format!(".{file_id}.part")))?;
        remove_if_present(&directory.join(format!("{file_id}.json")))?;
        total = total.saturating_sub(size);
        evicted += 1;
    }
    Ok(evicted)
}

fn cached_entries_in(app_data: &Path) -> Result<Vec<CachedRemoteAudio>, String> {
    let directory = app_data.join("audio");
    let Ok(entries) = fs::read_dir(&directory) else {
        return Ok(Vec::new());
    };
    let mut cached = Vec::new();
    for entry in entries
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("json"))
        .take(10_000)
    {
        let path = entry.path();
        if entry
            .metadata()
            .map(|metadata| metadata.len() > 64 * 1024)
            .unwrap_or(true)
        {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(item) = serde_json::from_slice::<CachedRemoteAudio>(&bytes) else {
            continue;
        };
        if stem != item.track.file_id || validate_cache_track(&item.track).is_err() {
            continue;
        }
        let Ok(extension) = safe_extension(&item.track.format) else {
            continue;
        };
        let audio_path = directory.join(format!("{}.{}", item.track.file_id, extension));
        if fs::metadata(audio_path)
            .map(|metadata| metadata.is_file() && metadata.len() == item.track.size)
            .unwrap_or(false)
        {
            cached.push(item);
        }
    }
    cached.sort_by(|left, right| {
        left.track
            .title
            .to_ascii_lowercase()
            .cmp(&right.track.title.to_ascii_lowercase())
            .then_with(|| left.track.filename.cmp(&right.track.filename))
    });
    Ok(cached)
}

struct AppState {
    remote: Arc<RemoteClient>,
    media: Arc<MediaServer>,
    podcasts: Arc<PodcastStore>,
    /// Where the artwork this phone holds is kept. Shared with the server, which
    /// is the only thing allowed to hand it out.
    art_root: PathBuf,
}

#[tauri::command]
async fn companion_status(state: State<'_, AppState>) -> Result<CompanionStatus, String> {
    Ok(state.remote.status().await)
}

#[tauri::command]
async fn pair_desktop(
    code: String,
    device_name: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    state.remote.pair(&code, &device_name).await
}

#[tauri::command]
async fn forget_desktop(state: State<'_, AppState>) -> Result<(), String> {
    state.remote.forget().await
}

/// Forget one computer, leaving the others this phone may read.
///
/// A phone acts through exactly one computer, but it may read several, so
/// dropping one - a friend's, usually - has to be possible without unpairing
/// the one it acts through.
#[tauri::command]
async fn forget_mobile_host(endpoint_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.remote.forget_host(&endpoint_id).await
}

/// Read from one computer, or leave it out, without pairing again.
///
/// This is the same plumbing a computer is named with elsewhere: a computer left
/// out keeps its pairing and everything it may do, and is only left out of what
/// the phone offers.
#[tauri::command]
async fn set_mobile_host_included(
    endpoint_id: String,
    included: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.remote.set_host_included(&endpoint_id, included).await
}

/// Which computer answered with which file, for the marks a row draws.
#[tauri::command]
async fn remote_file_hosts(
    state: State<'_, AppState>,
) -> Result<HashMap<String, String>, String> {
    Ok(state.remote.file_hosts().await)
}

/// One computer this phone may talk to, for the settings sheet.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteHost {
    endpoint_id: String,
    desktop_name: String,
    rights: DeviceRights,
    /// The computer this phone acts through: the one whose library and status
    /// the rest of the app is drawn from.
    primary: bool,
    /// Whether this phone reads from it. A computer left out keeps its pairing.
    included: bool,
    /// Whether it answered just now. A computer out of reach is still one this
    /// phone holds, and this is what its row says.
    online: bool,
}

#[tauri::command]
async fn remote_hosts(state: State<'_, AppState>) -> Result<Vec<RemoteHost>, String> {
    let hosts = state.remote.hosts().await;
    let primary = primary_host(&hosts).map(|host| host.endpoint_id);
    // Asked all at once and cut off quickly: this draws a status line, and a
    // computer that is asleep must not hold up the ones that are awake.
    let online = state.remote.reachable(&hosts).await;
    Ok(hosts
        .into_iter()
        .map(|host| {
            // Read before the fields are moved out: `grant` borrows the whole
            // host, which is not available once one of its fields has gone.
            let grant = host.grant();
            let is_primary = primary.as_deref() == Some(host.endpoint_id.as_str());
            let is_online = online.get(&host.endpoint_id).copied().unwrap_or(false);
            RemoteHost {
                primary: is_primary,
                included: host.included,
                online: is_online,
                endpoint_id: host.endpoint_id,
                desktop_name: host.desktop_name,
                rights: grant,
            }
        })
        .collect())
}

#[tauri::command]
async fn remote_library(
    query: String,
    offset: usize,
    limit: usize,
    shuffle_seed: Option<u64>,
    source: Option<String>,
    state: State<'_, AppState>,
) -> Result<LibraryPage, String> {
    // No computer is named, because none of them is the whole library any more:
    // everything this phone may read is offered as one list, and a shuffled one
    // is a mix of all of it. Naming a computer asks that one alone, which is what
    // a track's own computer is named for.
    if source.is_none() {
        let (tracks, total) = state
            .remote
            .mixed_library(&query, shuffle_seed, offset, limit)
            .await?;
        return Ok(LibraryPage { tracks, total });
    }
    match state
        .remote
        .request_from(
            source.as_deref(),
            ClientRequest::Library {
                query,
                offset,
                limit,
                shuffle_seed,
            },
        )
        .await?
    {
        ServerResponse::Library { tracks, total } => {
            // Rows from a friend are on record as theirs, so playing one asks the
            // friend rather than the phone's own computer.
            if let Some(source) = source.as_deref() {
                state.remote.remember_origins(source, &tracks).await;
            }
            Ok(LibraryPage { tracks, total })
        }
        response => Err(unexpected_response(&response)),
    }
}

/// The catalogue records for particular file ids, in the order asked for.
///
/// A queue handed over from the computer, and a playlist, both arrive as file
/// ids: this is how the phone turns them into tracks it can show and play, and
/// how it learns that a member is one the computer no longer holds. Requests are
/// chunked so each one, and each answer, fits inside a single control frame.
#[tauri::command]
async fn remote_library_by_ids(
    file_ids: Vec<String>,
    source: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<RemoteTrack>, String> {
    if file_ids.len() > MAX_PLAY_QUEUE {
        return Err("That is more tracks than one queue can hold".into());
    }
    let mut tracks = Vec::with_capacity(file_ids.len());
    for batch in file_ids.chunks(MAX_TRACKS_BY_ID) {
        // A member the phone's own computer does not hold is not a member nobody
        // holds: it may be one a friend's computer holds, so a batch with no
        // computer named is asked of every computer this phone may read.
        match source.as_deref() {
            Some(source) => {
                let response = state
                    .remote
                    .request_from(
                        Some(source),
                        ClientRequest::LibraryByIds {
                            file_ids: batch.to_vec(),
                        },
                    )
                    .await?;
                match response {
                    ServerResponse::LibraryByIds { tracks: held } => {
                        // A queue handed over by a friend's computer, or a
                        // playlist read from one, is playable because those rows
                        // are on record as theirs.
                        state.remote.remember_origins(source, &held).await;
                        tracks.extend(held);
                    }
                    response => return Err(unexpected_response(&response)),
                }
            }
            None => tracks.extend(state.remote.library_by_ids_everywhere(batch).await?),
        }
    }
    Ok(tracks)
}

/// Playlists this computer can see, newest first and without their members.
///
/// `own_only` asks for this computer's own playlists alone, which is what the
/// picker beside a track wants: a public playlist somebody else published is one
/// to play, not a list to add a track to.
#[tauri::command]
async fn remote_playlists(
    offset: usize,
    limit: usize,
    own_only: Option<bool>,
    state: State<'_, AppState>,
) -> Result<PlaylistPage, String> {
    match state
        .remote
        .request(ClientRequest::Playlists {
            offset,
            limit,
            own_only: own_only.unwrap_or(false),
        })
        .await?
    {
        ServerResponse::Playlists { playlists, total } => Ok(PlaylistPage { playlists, total }),
        response => Err(unexpected_response(&response)),
    }
}

/// One playlist, a page of its members at a time in the order it puts them in.
///
/// Named by its coordinate - its author and its id together - because the id is
/// chosen by the author and two authors may choose the same one. Both come from
/// the summary this phone was shown.
#[tauri::command]
async fn remote_playlist(
    author: String,
    playlist_id: String,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<RemotePlaylist, String> {
    let limit = limit.clamp(1, MAX_PLAYLIST_PAGE);
    match state
        .remote
        .request(ClientRequest::Playlist {
            author,
            playlist_id,
            offset,
            limit,
        })
        .await?
    {
        ServerResponse::Playlist { playlist } => Ok(playlist),
        response => Err(unexpected_response(&response)),
    }
}

/// The playlists that already name a file, by coordinate.
///
/// The track menu's "add to playlist" picker draws a tick beside every playlist
/// that holds the track, and this answers all of them in one request. It is a
/// read, so a phone with read-only access can ask it and simply find nothing it
/// is allowed to change.
#[tauri::command]
async fn remote_playlists_containing(
    file_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<RemotePlaylistCoordinate>, String> {
    match state
        .remote
        .request(ClientRequest::PlaylistsContaining { file_id })
        .await?
    {
        ServerResponse::PlaylistsContaining { playlists } => Ok(playlists),
        response => Err(unexpected_response(&response)),
    }
}

/// A playlist request, refused here rather than at the computer for the two
/// things a phone can get wrong that would otherwise be answered from far away
/// in words that do not explain themselves: a member count over the spec's
/// limit, and an edit too large to travel in one control frame.
///
/// Everything else about a playlist is the computer's to judge, because it owns
/// the store and the keys - including who the author is, which the phone never
/// gets to say.
fn bounded_playlist_request(request: ClientRequest) -> Result<ClientRequest, String> {
    let members = match &request {
        ClientRequest::SavePlaylist { playlist }
        | ClientRequest::PublishPlaylist { playlist, .. } => playlist.tracks.len(),
        _ => 0,
    };
    if members > MAX_PLAYLIST_MEMBERS {
        return Err(format!(
            "A playlist may name at most {MAX_PLAYLIST_MEMBERS} tracks, and this one names {members}."
        ));
    }
    let payload = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
    if payload.len() > MAX_CONTROL_FRAME_BYTES {
        return Err("This playlist is too large to edit from a phone.".into());
    }
    Ok(request)
}

/// An id for a playlist that does not exist yet.
///
/// The computer mints it: a playlist's identity is its name and role for its
/// author rather than its contents, so there is nothing on this side to derive
/// one from, and the computer is the side that files the playlist.
#[tauri::command]
async fn remote_new_playlist_id(state: State<'_, AppState>) -> Result<String, String> {
    match state.remote.request(ClientRequest::NewPlaylistId).await? {
        ServerResponse::PlaylistId { playlist_id } => Ok(playlist_id),
        response => Err(unexpected_response(&response)),
    }
}

/// Write a playlist down on the computer without publishing it.
///
/// The whole playlist travels, because a revision is the whole list. What comes
/// back is what the computer stored - author and edit time stamped there - so
/// this phone keeps the same copy the computer will answer with later.
#[tauri::command]
async fn remote_save_playlist(
    playlist: RemotePlaylist,
    state: State<'_, AppState>,
) -> Result<RemotePlaylist, String> {
    match state
        .remote
        .request(bounded_playlist_request(ClientRequest::SavePlaylist {
            playlist,
        })?)
        .await?
    {
        ServerResponse::Playlist { playlist } => Ok(playlist),
        response => Err(unexpected_response(&response)),
    }
}

/// Sign a playlist and send it to the computer's relays.
///
/// `suggest_tags` is the author's answer to "suggest words from the title?".
/// It is `false` when the phone does not ask, which is the safe direction: an
/// event cannot carry the difference between "no" and "not asked", so the
/// answer is remembered on this side rather than guessed at there.
#[tauri::command]
async fn remote_publish_playlist(
    playlist: RemotePlaylist,
    suggest_tags: Option<bool>,
    state: State<'_, AppState>,
) -> Result<RemotePlaylist, String> {
    match state
        .remote
        .request(bounded_playlist_request(ClientRequest::PublishPlaylist {
            playlist,
            suggest_tags: suggest_tags.unwrap_or(false),
        })?)
        .await?
    {
        ServerResponse::Playlist { playlist } => Ok(playlist),
        response => Err(unexpected_response(&response)),
    }
}

/// Forget a playlist the computer holds and has never published.
#[tauri::command]
async fn remote_delete_playlist(
    author: String,
    playlist_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    match state
        .remote
        .request(ClientRequest::DeletePlaylist {
            author,
            playlist_id,
        })
        .await?
    {
        ServerResponse::PlaylistRemoved => Ok(()),
        response => Err(unexpected_response(&response)),
    }
}

/// Take a published playlist back off the computer's relays.
#[tauri::command]
async fn remote_withdraw_playlist(
    playlist_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    match state
        .remote
        .request(ClientRequest::WithdrawPlaylist { playlist_id })
        .await?
    {
        ServerResponse::PlaylistRemoved => Ok(()),
        response => Err(unexpected_response(&response)),
    }
}

#[tauri::command]
async fn cached_library(state: State<'_, AppState>) -> Result<OfflineLibrary, String> {
    state.remote.offline_library().await
}

/// Album artwork for the given `artist|album` keys, resolved by the paired
/// Napstr host from kind `30427` cover events.
///
/// The phone never talks to relays itself: the host owns the relay pool, the
/// catalogue, and the availability heartbeats that decide which claim wins.
/// Requests are chunked so each one, and each answer, fits inside a single
/// control frame.
async fn companion_covers(
    remote: &RemoteClient,
    keys: Vec<String>,
) -> Result<Vec<RemoteAlbumCover>, String> {
    let keys = normalise_cover_request(&keys);
    let mut covers = Vec::new();
    for batch in keys.chunks(MAX_COVER_KEYS) {
        let response = remote
            .request(ClientRequest::AlbumCovers {
                keys: batch.to_vec(),
            })
            .await?;
        match response {
            ServerResponse::AlbumCovers {
                covers: batch_covers,
            } => covers.extend(batch_covers),
            response => return Err(unexpected_response(&response)),
        }
    }
    Ok(covers)
}

#[tauri::command]
async fn remote_covers(
    keys: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Vec<RemoteAlbumCover>, String> {
    companion_covers(&state.remote, keys).await
}

/// Artwork this phone has just been sent, before it is written down.
struct FetchedArt {
    hash: String,
    bytes: Vec<u8>,
}

/// Where to draw one rendition of one album's art from, fetching it from the
/// paired computer if this phone is not holding it yet.
///
/// `hash` is what the host said it would serve, as it appears in the cover
/// answer, and it is what makes a screen of albums cheap: an album this phone
/// already holds costs no request at all, and one it does not costs a single
/// transfer. An empty address means there is nothing to draw yet, and the caller
/// keeps whatever it has.
#[tauri::command]
async fn remote_art(
    key: String,
    rendition: String,
    hash: String,
    state: State<'_, AppState>,
) -> Result<AlbumArtwork, String> {
    let rendition = match rendition.as_str() {
        "thumb" => ArtRendition::Thumb,
        "full" => ArtRendition::Full,
        other => return Err(format!("{other} is not a rendition of anything")),
    };
    if key.is_empty() || key.chars().count() > MAX_ART_KEY_CHARS {
        return Err("that is not an album key".into());
    }
    // Held already: not one byte crosses the wire, which is the ordinary case
    // for a screen that is drawn twice.
    if art_store::path(&state.art_root, &hash).is_some() {
        return Ok(AlbumArtwork {
            url: state.media.art_url(&hash),
            hash,
        });
    }
    let Some(art) = state.remote.fetch_art(&key, rendition).await? else {
        return Ok(AlbumArtwork {
            url: String::new(),
            hash: String::new(),
        });
    };
    // Written only after its hash checks out, and only if it is really an image.
    art_store::store(&state.art_root, &art.hash, &art.bytes)?;
    Ok(AlbumArtwork {
        url: state.media.art_url(&art.hash),
        hash: art.hash,
    })
}

/// The address to draw fetched artwork from, and the name of what is at it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AlbumArtwork {
    /// Empty when there is nothing to draw.
    url: String,
    /// What this phone now holds for that rendition. Empty with an empty url.
    hash: String,
}

/// Cover keys are `trim(artist)|trim(album)`, lowercased, exactly one
/// separator, at most 300 characters, exactly as the cover NIP defines them.
/// Rewriting here means the phone and the host always agree on the key, and a
/// sloppy caller cannot silently match nothing.
fn normalise_cover_request(keys: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut normalised = Vec::new();
    for key in keys {
        let Some(key) = normalise_cover_key(key) else {
            continue;
        };
        if seen.insert(key.clone()) {
            normalised.push(key);
        }
        if normalised.len() >= MAX_COVER_KEYS * 4 {
            break;
        }
    }
    normalised
}

fn normalise_cover_key(value: &str) -> Option<String> {
    let mut halves = value.split('|');
    let artist = halves.next()?.trim().to_lowercase();
    let album = halves.next()?.trim().to_lowercase();
    if halves.next().is_some() || artist.is_empty() || album.is_empty() {
        return None;
    }
    let key = format!("{artist}|{album}");
    (key.chars().count() <= 300).then_some(key)
}

/// What the paired Napstr desktop is playing, if anything.
///
/// A read-only pairing may ask this too: seeing what the computer is doing is
/// not a way of changing it.
/// The conversation around one track, a page at a time.
///
/// Reading costs the phone nothing but a request, and reading is all a lent phone
/// may do: the messages are public, and the computer is the one fetching them.
#[tauri::command]
async fn remote_track_discussion(
    file_id: String,
    before: Option<u64>,
    state: State<'_, AppState>,
) -> Result<Vec<RemoteDiscussionMessage>, String> {
    match state
        .remote
        .request(ClientRequest::TrackDiscussion {
            file_id: file_id.clone(),
            before,
        })
        .await
    {
        Ok(ServerResponse::TrackDiscussion { messages }) => Ok(messages),
        Ok(response) => Err(unexpected_response(&response)),
        Err(error) => Err(friendly_if_missing(error, DISCUSSION_UNAVAILABLE)),
    }
}

/// Say something in that conversation.
///
/// The computer signs it with the user's own key and publishes it under their
/// name, so a read-only pairing is refused there rather than here - and this
/// still refuses to offer the box in the first place, because a phone that can
/// only fail should not invite the attempt.
#[tauri::command]
async fn remote_send_track_discussion(
    file_id: String,
    content: String,
    reply_to: Option<String>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    match state
        .remote
        .request(ClientRequest::SendTrackDiscussion {
            file_id: file_id.clone(),
            content,
            reply_to,
        })
        .await
    {
        Ok(ServerResponse::TrackDiscussionSent { event_id }) => Ok(event_id),
        Ok(response) => Err(unexpected_response(&response)),
        Err(error) => Err(friendly_if_missing(error, DISCUSSION_UNAVAILABLE)),
    }
}

/// How many people have commented on each of these files, for the marks on rows.
///
/// The host answers about the files it is willing to name to a relay, so a file
/// this computer holds without having published is simply absent from the answer.
#[tauri::command]
async fn remote_track_discussion_activity(
    file_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Vec<RemoteDiscussionActivity>, String> {
    match state
        .remote
        .request(ClientRequest::TrackDiscussionActivity { file_ids })
        .await
    {
        Ok(ServerResponse::TrackDiscussionActivity { activity }) => Ok(activity),
        Ok(response) => Err(unexpected_response(&response)),
        // A mark is a courtesy: an older computer that does not know the question
        // leaves the rows quiet rather than reporting an error for something
        // nobody asked for.
        Err(_) => Ok(Vec::new()),
    }
}

#[tauri::command]
async fn remote_playback_state(
    state: State<'_, AppState>,
) -> Result<RemotePlaybackState, String> {
    let response = state
        .remote
        .request(ClientRequest::PlaybackState)
        .await
        .map_err(|error| friendly_if_missing(error, PLAYBACK_UNAVAILABLE))?;
    match response {
        ServerResponse::Playback { state: playing } => Ok(playing),
        response => Err(unexpected_response(&response)),
    }
}

/// Drive the desktop's own player from this phone.
#[tauri::command]
async fn remote_playback(
    command: PlaybackCommand,
    state: State<'_, AppState>,
) -> Result<RemotePlaybackState, String> {
    // Refuse the nonsense here rather than letting the host guess: a seek past a
    // day, a volume over 100%, or more of a queue than one request carries, is a
    // bug in the caller and not a preference.
    match &command {
        PlaybackCommand::Seek { position_ms } if *position_ms > MAX_SEEK_MS => {
            return Err("That position is out of range".into());
        }
        PlaybackCommand::Volume { percent } if *percent > 100 => {
            return Err("Volume is a percentage".into());
        }
        PlaybackCommand::PlayTrack { queue, .. } if queue.len() > MAX_PLAY_QUEUE => {
            return Err("That is more tracks than the computer can take at once".into());
        }
        _ => {}
    }
    if state.remote.stream_only().await {
        return Err("This pairing is read only. It cannot control the computer.".into());
    }
    let response = state
        .remote
        .request(ClientRequest::Playback { command })
        .await
        .map_err(|error| friendly_if_missing(error, PLAYBACK_UNAVAILABLE))?;
    match response {
        ServerResponse::Playback { state: playing } => Ok(playing),
        response => Err(unexpected_response(&response)),
    }
}

/// Ask the host for a read-only pairing code to hand to another device.
///
/// Only a host that knows this phone has write access will mint one, and what it
/// mints is read-only, so access can be lent on but never widened.
#[tauri::command]
async fn remote_read_only_ticket(
    state: State<'_, AppState>,
) -> Result<ReadOnlyTicketOffer, String> {
    let response = state
        .remote
        .request(ClientRequest::ReadOnlyTicket)
        .await
        .map_err(|error| friendly_if_missing(error, READ_ONLY_CODE_UNAVAILABLE))?;
    match response {
        ServerResponse::ReadOnlyTicket {
            uri,
            qr_svg,
            expires_at,
            desktop_name,
        } => Ok(ReadOnlyTicketOffer {
            uri,
            qr_svg: safe_qr_svg(&qr_svg),
            expires_at,
            desktop_name,
        }),
        response => Err(unexpected_response(&response)),
    }
}

/// A track URI is a scheme, a path and a SHA-256, so anything of this length or
/// more is not one.
const MAX_TRACK_URI_BYTES: usize = 256;

/// Draw the code that carries a track's own URI, for another client to scan.
///
/// Unlike a pairing code this one never crosses the network: the page builds
/// the URI and this process draws the markup, so it is trusted by construction.
/// It still goes through the same sanitiser as a host's code, because that is
/// what guarantees only a QR renderer's own elements ever reach the page.
#[tauri::command]
fn track_code(uri: String) -> Result<String, String> {
    let trimmed = uri.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_TRACK_URI_BYTES {
        return Err("invalid track code".into());
    }
    let drawn = QrCode::new(trimmed.as_bytes())
        .map_err(|error| format!("could not create the track code: {error}"))?
        .render::<svg::Color>()
        .min_dimensions(240, 240)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build();
    let safe = safe_qr_svg(&drawn);
    if safe.is_empty() {
        return Err("could not draw the track code".into());
    }
    Ok(safe)
}

/// Ask the host to sign and publish a NIP-56 `1984` report about an album cover.
///
/// This app holds no Nostr keys, and that is worth keeping: the report is the
/// user's words, but the signature is the host's to make.
#[tauri::command]
async fn remote_report_cover(
    key: String,
    reason: String,
    note: String,
    state: State<'_, AppState>,
) -> Result<CoverReport, String> {
    let key = normalise_cover_key(&key).ok_or("That album has no cover key")?;
    let reason = reason.trim().to_lowercase();
    if !REPORT_REASONS.contains(&reason.as_str()) {
        return Err("Choose a reason for the report".into());
    }
    let note = note.trim().to_string();
    if note.chars().count() > MAX_REPORT_NOTE_CHARS {
        return Err(format!(
            "Keep the note under {MAX_REPORT_NOTE_CHARS} characters"
        ));
    }
    let response = state
        .remote
        .request(ClientRequest::ReportCover { key, reason, note })
        .await
        .map_err(|error| friendly_if_missing(error, REPORT_UNAVAILABLE))?;
    match response {
        ServerResponse::CoverReported { report } => Ok(CoverReport {
            report_id: report.report_id,
            queued: report.queued,
        }),
        response => Err(unexpected_response(&response)),
    }
}

#[tauri::command]
async fn reconcile_audio_cache(
    protected_file_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let protected = protected_file_ids
        .into_iter()
        .filter(|file_id| validate_file_id(file_id).is_ok())
        .collect();
    state
        .remote
        .reconcile_cache(state.media.clone(), protected)
        .await
}

#[tauri::command]
async fn remote_search(
    query: String,
    source: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<RemoteTrack>, String> {
    // No computer named is everyone: a search is a question worth asking every
    // computer this phone may read from, which is how a friend's library turns up
    // in results. Naming one asks that one alone.
    if source.is_none() {
        return state.remote.search_everywhere(&query).await;
    }
    match state
        .remote
        .request_from(source.as_deref(), ClientRequest::Search { query })
        .await?
    {
        ServerResponse::Search { tracks } => {
            if let Some(source) = source.as_deref() {
                state.remote.remember_origins(source, &tracks).await;
            }
            Ok(tracks)
        }
        response => Err(unexpected_response(&response)),
    }
}

/// The computer's own list of what is live on the network.
///
/// The computer answers this from the catalogue it already mirrors rather than by
/// asking the relays, so it is the one network read that costs the far end
/// nothing. Nothing is searched: `mode` says how to choose, `seed` fixes the order
/// inside a tier of seeders so a page is an offset into one list, and anything the
/// computer already holds is left out — what you have is not a discovery.
#[tauri::command]
async fn remote_discover(
    mode: String,
    seed: u64,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<DiscoverPage, String> {
    let mode = match mode.as_str() {
        "mostSeeded" => DiscoverMode::MostSeeded,
        other => {
            return Err(format!(
                "that is not a way this phone knows to choose a list: {other}"
            ))
        }
    };
    // The primary computer, the way a library browse asks it: the list is one
    // computer's view of the network, and a list stitched from several would be
    // three different rankings interleaved.
    match state
        .remote
        .request_from(None, ClientRequest::Discover {
            mode,
            seed,
            offset,
            limit,
        })
        .await?
    {
        ServerResponse::Discover { tracks, total } => Ok(DiscoverPage { tracks, total }),
        response => Err(unexpected_response(&response)),
    }
}

/// One page of that list, and how many the whole of it holds.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscoverPage {
    tracks: Vec<RemoteTrack>,
    total: usize,
}

#[tauri::command]
async fn remote_audiobooks(
    query: String,
    state: State<'_, AppState>,
) -> Result<Vec<RemoteAudiobook>, String> {
    match state
        .remote
        .request(ClientRequest::Audiobooks { query })
        .await?
    {
        ServerResponse::Audiobooks { audiobooks } => Ok(audiobooks),
        response => Err(unexpected_response(&response)),
    }
}

#[tauri::command]
async fn remote_audiobook_library(
    query: String,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<AudiobookLibraryPage, String> {
    match state
        .remote
        .request(ClientRequest::AudiobookLibrary {
            query: query.clone(),
            offset,
            limit,
        })
        .await?
    {
        ServerResponse::AudiobookLibrary { audiobooks, total } => {
            Ok(AudiobookLibraryPage { audiobooks, total })
        }
        _ => {
            // Compatibility with Napstr versions that predate paged summaries.
            let audiobooks = remote_audiobooks(query, state).await?;
            let total = audiobooks.len();
            let audiobooks = audiobooks
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(|book| RemoteAudiobookSummary {
                    audiobook_id: book.audiobook_id,
                    title: book.title,
                    author: book.author,
                    narrator: book.narrator,
                    total_size: book.total_size,
                    chapter_count: book.chapters.len(),
                })
                .collect();
            Ok(AudiobookLibraryPage { audiobooks, total })
        }
    }
}

#[tauri::command]
async fn remote_audiobook(
    audiobook_id: String,
    state: State<'_, AppState>,
) -> Result<RemoteAudiobook, String> {
    match state
        .remote
        .request(ClientRequest::Audiobook {
            audiobook_id: audiobook_id.clone(),
        })
        .await?
    {
        ServerResponse::Audiobook { audiobook } => Ok(audiobook),
        _ => remote_audiobooks(String::new(), state)
            .await?
            .into_iter()
            .find(|book| book.audiobook_id == audiobook_id)
            .ok_or("That audiobook is no longer available".into()),
    }
}

#[tauri::command]
fn podcast_parse_search(payload: String, limit: usize) -> Result<Vec<PodcastFeed>, String> {
    parse_public_podcast_search(&payload, limit)
}

#[tauri::command]
async fn podcast_episodes(
    feed: PodcastFeed,
    directory_payload: String,
    state: State<'_, AppState>,
) -> Result<Vec<PodcastEpisode>, String> {
    let episodes = parse_public_podcast_episodes(&feed, &directory_payload, 50)?;
    if !episodes.is_empty() {
        return Ok(episodes);
    }
    state.podcasts.feed_episodes(&feed, 50).await
}

#[tauri::command]
async fn podcast_download(
    episode: PodcastEpisode,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.podcasts.start(episode).await
}

#[tauri::command]
async fn podcast_downloads(state: State<'_, AppState>) -> Result<Vec<PodcastDownload>, String> {
    Ok(state.podcasts.list().await)
}

#[tauri::command]
async fn podcast_playback_url(
    episode: PodcastEpisode,
    state: State<'_, AppState>,
) -> Result<CachedPodcastAudio, String> {
    state
        .podcasts
        .playback_url(episode, state.media.clone())
        .await
}

#[tauri::command]
async fn remote_download(
    file_id: String,
    source_pubkeys: Vec<String>,
    destination_folder: Option<String>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    match state
        .remote
        .request(ClientRequest::RequestDownload {
            file_id,
            source_pubkeys,
            destination_folder,
        })
        .await?
    {
        ServerResponse::DownloadRequested { request_id } => Ok(request_id),
        response => Err(unexpected_response(&response)),
    }
}

#[tauri::command]
async fn remote_transfers(state: State<'_, AppState>) -> Result<Vec<RemoteTransfer>, String> {
    match state.remote.request(ClientRequest::Transfers).await? {
        ServerResponse::Transfers { transfers } => Ok(transfers),
        response => Err(unexpected_response(&response)),
    }
}

#[tauri::command]
async fn cache_remote_audio(
    track: RemoteTrack,
    library_visible: Option<bool>,
    state: State<'_, AppState>,
) -> Result<CachedAudio, String> {
    state
        .remote
        .cache_audio(track, state.media.clone(), library_visible.unwrap_or(true))
        .await
}

#[tauri::command]
async fn prefetch_remote_audio(
    after_file_id: String,
    track: RemoteTrack,
    library_visible: Option<bool>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    validate_file_id(&after_file_id)?;
    validate_cache_track(&track)?;
    // Keyed by the file being fetched rather than by what is playing: warming
    // several tracks ahead means the same target can be asked for from two
    // different current tracks, and it is still one download.
    let key = track.file_id.clone();
    if !state
        .media
        .scheduled_prefetches
        .lock()
        .await
        .insert(key.clone())
    {
        return Ok(());
    }
    let remote = state.remote.clone();
    let media = state.media.clone();
    tauri::async_runtime::spawn(async move {
        if media.wait_until_complete(&after_file_id).await.is_ok() {
            let _ = remote
                .cache_audio(track, media.clone(), library_visible.unwrap_or(true))
                .await;
        }
        media.scheduled_prefetches.lock().await.remove(&key);
    });
    Ok(())
}

async fn exchange_on(
    connection: &iroh::endpoint::Connection,
    request: ClientRequest,
) -> Result<(ServerResponse, iroh::endpoint::RecvStream), String> {
    let (mut send, mut receive) = connection
        .open_bi()
        .await
        .map_err(|error| format!("Could not open an Iroh request: {error}"))?;
    let payload = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
    if payload.len() > MAX_CONTROL_FRAME_BYTES {
        return Err("The request is too large".into());
    }
    send.write_all(&(payload.len() as u32).to_be_bytes())
        .await
        .map_err(|error| error.to_string())?;
    send.write_all(&payload)
        .await
        .map_err(|error| error.to_string())?;
    send.finish().map_err(|error| error.to_string())?;
    let response = read_response(&mut receive).await?;
    Ok((response, receive))
}

async fn read_response(receive: &mut iroh::endpoint::RecvStream) -> Result<ServerResponse, String> {
    let mut length = [0u8; 4];
    receive
        .read_exact(&mut length)
        .await
        .map_err(|error| format!("Could not read Napstr's response: {error}"))?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_CONTROL_FRAME_BYTES {
        return Err("Napstr returned an invalid response size".into());
    }
    let mut payload = vec![0u8; length];
    receive
        .read_exact(&mut payload)
        .await
        .map_err(|error| format!("Could not read Napstr's response: {error}"))?;
    serde_json::from_slice(&payload).map_err(|_| "Napstr returned an invalid response".into())
}

fn decode_endpoint_addr(host: &SavedHost) -> Result<EndpointAddr, String> {
    serde_json::from_str(&host.endpoint_addr)
        .or_else(|_| {
            host.endpoint_id
                .parse::<EndpointId>()
                .map(EndpointAddr::new)
                .map_err(|_| serde_json::Error::io(std::io::Error::other("invalid endpoint ID")))
        })
        .map_err(|_| "The saved Napstr Iroh address is invalid".into())
}

fn validate_file_id(value: &str) -> Result<(), String> {
    if hex::decode(value)
        .map(|bytes| bytes.len() == 32)
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err("invalid SHA-256 file ID".into())
    }
}

fn safe_extension(format: &str) -> Result<&'static str, String> {
    match format.to_ascii_uppercase().as_str() {
        "MP3" => Ok("mp3"),
        "FLAC" => Ok("flac"),
        "WAV" => Ok("wav"),
        "OGG" => Ok("ogg"),
        "OPUS" => Ok("opus"),
        "M4A" => Ok("m4a"),
        _ => Err("Napstr returned an unsupported audio format".into()),
    }
}

fn audio_mime(extension: &str) -> Option<&'static str> {
    match extension {
        "mp3" => Some("audio/mpeg"),
        "flac" => Some("audio/flac"),
        "wav" => Some("audio/wav"),
        "ogg" | "opus" => Some("audio/ogg"),
        "m4a" => Some("audio/mp4"),
        _ => None,
    }
}

fn validate_cache_track(track: &RemoteTrack) -> Result<(), String> {
    validate_file_id(&track.file_id)?;
    safe_extension(&track.format)?;
    if !track.local || track.size == 0 || track.size > 2 * 1024 * 1024 * 1024 {
        return Err("Napstr returned an invalid local audio track".into());
    }
    Ok(())
}

fn validate_matching_track(expected: &RemoteTrack, actual: &RemoteTrack) -> Result<(), String> {
    if expected.file_id != actual.file_id
        || expected.size != actual.size
        || !expected.format.eq_ignore_ascii_case(&actual.format)
        || expected.mime != actual.mime
    {
        return Err("Napstr returned different audio than Napstrfy requested".into());
    }
    Ok(())
}

fn requested_audio_range(value: Option<&str>, len: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.strip_prefix("bytes=").ok_or(())?;
    if value.contains(',') || len == 0 {
        return Err(());
    }
    let (start, end) = value.split_once('-').ok_or(())?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        return Ok(Some((len.saturating_sub(suffix), len - 1)));
    }
    let start = start.parse::<u64>().map_err(|_| ())?;
    if start >= len {
        return Err(());
    }
    let end = if end.is_empty() {
        len - 1
    } else {
        end.parse::<u64>().map_err(|_| ())?.min(len - 1)
    };
    if end < start {
        return Err(());
    }
    Ok(Some((start, end)))
}

fn clean_device_name(value: &str) -> String {
    let cleaned = value
        .chars()
        .filter(|character| !character.is_control() && !is_bidi_control(*character))
        .take(64)
        .collect::<String>();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "Napstrfy phone".into()
    } else {
        cleaned.into()
    }
}

fn is_bidi_control(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

fn chrono_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// The message an older Napstr answers with when it cannot parse a request it
/// has never heard of.
const UNKNOWN_REQUEST: &str = "invalid Napstrfy request";
/// The longest seek that can be meant: nothing Napstr plays is a day long.
const MAX_SEEK_MS: u64 = 24 * 60 * 60 * 1000;
const PLAYBACK_UNAVAILABLE: &str =
    "This Napstr cannot be driven from a phone yet. Update Napstr on your computer.";
const READ_ONLY_CODE_UNAVAILABLE: &str =
    "This Napstr cannot create read-only codes yet. Update Napstr on your computer.";
const REPORT_UNAVAILABLE: &str =
    "This Napstr cannot publish reports yet. Update Napstr on your computer.";
const DISCUSSION_UNAVAILABLE: &str =
    "This Napstr cannot show track discussions yet. Update Napstr on your computer.";

/// A host that does not know a request answers with a parse error, which says
/// nothing useful to the person holding the phone.
fn friendly_if_missing(error: String, suggestion: &str) -> String {
    if error.contains(UNKNOWN_REQUEST) || error.contains("unknown variant") {
        suggestion.to_string()
    } else {
        error
    }
}

/// Accept only the elements a QR renderer emits.
///
/// The markup is drawn by the host, but it arrives over the network and is about
/// to be inserted into this app's own page, so anything with a script, a link or
/// an event handler in it is dropped. The caller falls back to the code as text.
fn safe_qr_svg(value: &str) -> String {
    let trimmed = value.trim();
    // The renderer prefixes an XML prolog, which means nothing once the markup
    // is part of a page, so drop it before looking at the rest.
    let body = match trimmed.strip_prefix("<?xml") {
        Some(_) => match trimmed.find("?>") {
            Some(end) => trimmed[end + 2..].trim_start(),
            None => return String::new(),
        },
        None => trimmed,
    };
    if body.is_empty()
        || body.len() > MAX_QR_SVG_BYTES
        || !body.starts_with("<svg")
        || !body.ends_with("</svg>")
    {
        return String::new();
    }
    let lowercase = body.to_lowercase();
    if ["<script", "<!--", "href", "xlink", " on", "&#"]
        .iter()
        .any(|forbidden| lowercase.contains(forbidden))
    {
        return String::new();
    }
    let mut rest = body;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('>') else {
            return String::new();
        };
        let name = rest[..end]
            .trim_start_matches('/')
            .split(|character: char| character.is_whitespace() || character == '/')
            .next()
            .unwrap_or("")
            .to_lowercase();
        if !matches!(
            name.as_str(),
            "svg" | "path" | "rect" | "g" | "circle" | "polygon" | "polyline"
        ) {
            return String::new();
        }
        rest = &rest[end + 1..];
    }
    body.to_string()
}

fn unexpected_response(response: &ServerResponse) -> String {
    match response {
        ServerResponse::Error { message } => message.clone(),
        _ => "Napstr returned an unexpected response".into(),
    }
}

fn save_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    fs::write(path, bytes).map_err(|error| error.to_string())
}

fn remove_if_present(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn load_or_create_key(path: &Path) -> Result<SecretKey, String> {
    if let Ok(bytes) = fs::read(path) {
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "The saved Iroh identity has an invalid length")?;
        return Ok(SecretKey::from_bytes(&bytes));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let key = SecretKey::generate();
    write_private_key(path, &key.to_bytes())?;
    Ok(key)
}

#[cfg(unix)]
fn write_private_key(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    match options.open(path) {
        Ok(mut file) => file.write_all(bytes).map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(not(unix))]
fn write_private_key(path: &Path, bytes: &[u8]) -> Result<(), String> {
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            std::io::Write::write_all(&mut file, bytes).map_err(|error| error.to_string())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
fn client_platform() -> &'static str {
    std::env::consts::OS
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Iroh intentionally uses reqwest's bring-your-own-provider Rustls mode.
    // Mobile processes do not install a provider on our behalf, so do this
    // before Tauri or any Iroh background task can construct a TLS client.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let builder = tauri::Builder::default();
    // Single instance has to be registered before anything else, so that a link
    // opened while the companion is running reaches that window instead of
    // starting a second copy of it. The plugin's deep-link feature is what turns
    // the second launch's `napstrfy://` argument into an open-url event.
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|_app, argv, _cwd| {
        // The integration has already delivered any link in `argv`; this only
        // keeps the launch visible while developing.
        eprintln!("Napstrfy is already running; opened with {argv:?}");
    }));
    builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_os::init())
        .setup(|app| {
            #[cfg(mobile)]
            app.handle().plugin(tauri_plugin_barcode_scanner::init())?;
            #[cfg(desktop)]
            {
                // A link only reaches an installed app, so registering on every
                // launch is what makes `napstrfy://` work while developing.
                use tauri_plugin_deep_link::DeepLinkExt;
                if let Err(error) = app.deep_link().register_all() {
                    eprintln!("could not register the napstrfy link scheme: {error}");
                }
            }
            let app_data = app
                .path()
                .app_data_dir()
                .map_err(|error| error.to_string())?;
            let podcasts = PodcastStore::new(&app_data)?;
            // One path, decided once: the server serves from it and the command
            // that fetches writes into it.
            let art_root = app_data.join(art_store::ART_DIRECTORY);
            app.manage(AppState {
                remote: RemoteClient::new(app_data),
                media: MediaServer::start(art_root.clone())?,
                podcasts,
                art_root,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            client_platform,
            companion_status,
            pair_desktop,
            forget_desktop,
            remote_hosts,
            forget_mobile_host,
            set_mobile_host_included,
            remote_file_hosts,
            remote_library,
            remote_library_by_ids,
            remote_playlists,
            remote_playlist,
            remote_playlists_containing,
            remote_new_playlist_id,
            remote_save_playlist,
            remote_publish_playlist,
            remote_delete_playlist,
            remote_withdraw_playlist,
            cached_library,
            remote_covers,
            remote_art,
            remote_track_discussion,
            remote_send_track_discussion,
            remote_track_discussion_activity,
            remote_playback_state,
            remote_playback,
            remote_read_only_ticket,
            track_code,
            remote_report_cover,
            reconcile_audio_cache,
            remote_discover,
            remote_search,
            remote_audiobooks,
            remote_audiobook_library,
            remote_audiobook,
            podcast_parse_search,
            podcast_episodes,
            podcast_download,
            podcast_downloads,
            podcast_playback_url,
            remote_download,
            remote_transfers,
            cache_remote_audio,
            prefetch_remote_audio
        ])
        .run(tauri::generate_context!())
        .expect("error while running Napstrfy");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this guards: the reachability probe ran on every connect and then
    /// every thirty seconds, and any failed attempt dropped the tunnel - so asking
    /// whether a computer was there could take down the connection that was
    /// playing music, and the phone then reported itself offline.
    #[test]
    fn only_a_broken_transport_drops_a_tunnel() {
        assert!(!attempt_drops_tunnel(false, AttemptFailure::Transport));
        assert!(!attempt_drops_tunnel(false, AttemptFailure::Timeout));
        assert!(attempt_drops_tunnel(true, AttemptFailure::Transport));
        // A slow answer is not a broken connection, so the retry has somewhere to
        // go instead of paying for a fresh connect.
        assert!(!attempt_drops_tunnel(true, AttemptFailure::Timeout));
    }

    fn host(endpoint: &str, rights: DeviceRights, name: &str) -> SavedHost {
        SavedHost {
            endpoint_id: endpoint.into(),
            endpoint_addr: format!("{{\"id\":\"{endpoint}\",\"addrs\":[]}}"),
            desktop_name: name.into(),
            stream_only: rights.is_read_only(),
            rights: Some(rights),
            included: true,
            pubkey: String::new(),
        }
    }

    /// One library row, of the shape a computer sends.
    ///
    /// Only the parts a mix looks at are interesting here: the file id a row is
    /// placed by, and who answered, which is how two rows for one file are told
    /// apart.
    fn library_row(file_id: &str, local: bool) -> RemoteTrack {
        RemoteTrack {
            file_id: file_id.into(),
            filename: format!("{}.mp3", &file_id[..8]),
            title: "A track".into(),
            artist: if local { "Mine".into() } else { "Theirs".into() },
            album: "An album".into(),
            format: "MP3".into(),
            mime: "audio/mpeg".into(),
            size: 4_000_000,
            tags: String::new(),
            local,
            sources: Vec::new(),
            bitrate_kbps: 320,
            sample_rate_hz: 44_100,
            channels: 2,
            lossless: false,
            duration_ms: 200_000,
        }
    }

    /// Rows for whole files, named by one character each so a library can be
    /// written down as a short string.
    fn library_rows(ids: &[char]) -> Vec<RemoteTrack> {
        ids.iter()
            .map(|id| library_row(&id.to_string().repeat(64), true))
            .collect()
    }

    /// The order a page came back in, as those characters.
    fn places(tracks: &[RemoteTrack]) -> String {
        tracks
            .iter()
            .map(|track| track.file_id.chars().next().unwrap())
            .collect()
    }

    fn mixed_from(endpoint_id: &str, rows: Vec<RemoteTrack>, total: usize) -> MixedHost {
        MixedHost {
            endpoint_id: endpoint_id.into(),
            rows,
            total: Some(total),
            failed: false,
        }
    }

    /// A file written before this phone could hold more than one computer is one
    /// computer, not a list of none.
    ///
    /// This is why the list is a bare array: serde ignores fields it does not
    /// know, so a wrapper object would have read the old flat shape as a valid
    /// wrapper with no computers in it, and the pairing would vanish quietly.
    #[test]
    fn the_single_computer_file_is_read_as_the_only_computer() {
        let directory = std::env::temp_dir().join(format!("napstr-hosts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let legacy = directory.join(LEGACY_PAIRED_FILE);
        let list = directory.join(PAIRED_HOSTS_FILE);

        let old = host("legacy", DeviceRights::full(), "Old Napstr");
        fs::write(&legacy, serde_json::to_vec(&old).unwrap()).unwrap();
        let hosts = load_hosts(&list, &legacy);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].desktop_name, "Old Napstr");
        assert_eq!(hosts[0].grant(), DeviceRights::full());

        // Once the list is there it is what is read, even with the old file still
        // lying about - which is also why forgetting removes both.
        save_hosts(&list, &[]).unwrap();
        assert!(load_hosts(&list, &legacy).is_empty());
        let _ = fs::remove_dir_all(&directory);
    }

    /// A second code for a computer this phone knows changes what it may do
    /// rather than adding it twice, which is what makes a fresh code a way to
    /// widen a friend's access without unpairing anything.
    #[test]
    fn pairing_a_known_computer_replaces_what_was_known_about_it() {
        let mut hosts = vec![host("a", DeviceRights::read_only(), "Ada's Napstr")];
        upsert_host(&mut hosts, host("a", DeviceRights::full(), "Ada's Napstr"));
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].grant(), DeviceRights::full());

        upsert_host(&mut hosts, host("b", DeviceRights::read_only(), "Bob's Napstr"));
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[1].grant(), DeviceRights::read_only());
    }

    /// The phone acts through the computer that lets it act as its owner,
    /// whatever order they were paired in - and through the first it has when
    /// none of them do, which is the single computer a lent phone has ever had.
    #[test]
    fn the_phone_acts_through_the_computer_that_lets_it_own() {
        let friend = host("friend", DeviceRights::read_only(), "Ada's Napstr");
        let own = host("own", DeviceRights::full(), "My Napstr");

        assert_eq!(
            primary_host(&[friend.clone(), own.clone()]).unwrap().endpoint_id,
            "own"
        );
        assert_eq!(
            primary_host(&[own, friend.clone()]).unwrap().endpoint_id,
            "own"
        );
        assert_eq!(primary_host(&[friend]).unwrap().endpoint_id, "friend");
        assert!(primary_host(&[]).is_none());
    }

    /// A computer that has never named its grant is taken at the word of the old
    /// boolean, which is what that boolean has always meant.
    #[test]
    fn a_computer_that_never_said_keeps_the_meaning_of_the_boolean() {
        let mut saved = host("legacy", DeviceRights::full(), "Old Napstr");
        saved.rights = None;
        saved.stream_only = false;
        assert_eq!(saved.grant(), DeviceRights::full());
        assert!(!saved.grant().is_read_only());

        // And read-only stays read-only, so every gate on this phone decides
        // what it decided before grants existed.
        saved.stream_only = true;
        assert_eq!(saved.grant(), DeviceRights::read_only());
        assert!(saved.grant().is_read_only());
    }

    /// Only the computers this phone may read are asked, and it asks its own
    /// first whatever order they were paired in.
    #[test]
    fn the_computers_this_phone_may_read_are_asked_own_first() {
        let friend = host("friend", DeviceRights::read_only(), "Ada's Napstr");
        let own = host("own", DeviceRights::full(), "My Napstr");
        let nothing = host("none", DeviceRights::default(), "A Laptop");

        let asked = readable_hosts(&[friend.clone(), nothing.clone(), own.clone()]).unwrap();
        assert_eq!(
            asked
                .iter()
                .map(|host| host.endpoint_id.as_str())
                .collect::<Vec<_>>(),
            ["own", "friend"]
        );
        // A computer that allows nothing is not a library to read, and a phone
        // with no computer at all is told which of the two it is looking at.
        assert!(readable_hosts(&[nothing]).is_err());
        assert_eq!(
            readable_hosts(&[]).unwrap_err(),
            "Pair Napstrfy with Napstr first"
        );
    }

    /// A file two computers hold is one row, and it is the row of the computer
    /// that was asked first.
    #[test]
    fn a_file_two_computers_hold_is_one_row_from_the_first_of_them() {
        let merged = merge_search_results(vec![
            vec![library_row(&"b".repeat(64), true)],
            vec![
                library_row(&"b".repeat(64), false),
                library_row(&"d".repeat(64), false),
            ],
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(places(&merged), "bd");
        // The phone's own computer answered first, so its row is the one kept.
        assert_eq!(merged[0].artist, "Mine");
        assert!(merged[0].local);
    }

    /// A mix is one order over every computer's rows, and the order is the shared
    /// key: the same files come back in the same places however the rows were
    /// shared out, so a scroll through a shuffle keeps going rather than
    /// repeating.
    ///
    /// The orders here are worked out away from this code, from the formula both
    /// applications use.
    #[test]
    fn a_mix_is_one_order_over_every_computers_rows() {
        let hosts = vec![
            mixed_from("own", library_rows(&['a', 'b']), 2),
            mixed_from("friend", library_rows(&['b', 'c']), 2),
        ];

        let (page, total) = mixed_page(&hosts, Some(7), 0, 10);
        // Each file once: the one both computers hold is one row.
        assert_eq!(page.len(), 3);
        assert_eq!(places(&page), "cba");

        // A page at a time continues where the last left off rather than
        // repeating what has already gone by.
        let (first, _) = mixed_page(&hosts, Some(7), 0, 1);
        let (second, _) = mixed_page(&hosts, Some(7), 1, 2);
        assert_eq!(places(&first), "c");
        assert_eq!(places(&second), "ba");

        // Another seed is another order.
        let (other, _) = mixed_page(&hosts, Some(9), 0, 10);
        assert_eq!(places(&other), "acb");

        // What the computers together say they hold, which counts the file they
        // both hold twice: an upper bound, and exactly right with one computer.
        assert_eq!(total, 4);
    }

    /// A page deeper than one answer can carry is asked for in parts, and a
    /// computer that has given everything it holds is asked no more.
    #[test]
    fn a_deep_page_is_asked_for_in_parts_and_stops_when_asked_out() {
        // The first part of anything is at most one answer.
        assert_eq!(next_mix_request(0, None, 100), Some((0, 100)));
        assert_eq!(
            next_mix_request(0, None, MAX_PAGE_SIZE + 50),
            Some((0, MAX_PAGE_SIZE))
        );
        assert_eq!(
            next_mix_request(MAX_PAGE_SIZE, Some(400), MAX_PAGE_SIZE + 50),
            Some((MAX_PAGE_SIZE, 50))
        );
        // Holding what the page needs is nothing to ask for, and neither is a
        // computer that has given all it has.
        assert_eq!(next_mix_request(200, Some(400), 100), None);
        assert_eq!(next_mix_request(30, Some(30), 100), None);
        assert_eq!(next_mix_request(30, Some(40), 100), Some((30, 10)));
    }

    /// The order a file is asked for in: the computer that answered with it, then
    /// the phone's own, then the others - and only computers that let this phone
    /// take audio.
    #[test]
    fn a_file_is_asked_for_from_the_computer_that_answered_with_it() {
        let own = host("own", DeviceRights::full(), "My Napstr");
        let friend = host("friend", DeviceRights::read_only(), "Ada's Napstr");
        let reader = host(
            "reader",
            DeviceRights {
                browse: true,
                ..DeviceRights::default()
            },
            "A Laptop",
        );
        // A nested function rather than a closure: the answer borrows the hosts
        // it was read from, which a closure cannot say.
        fn ordered(hosts: &[SavedHost]) -> Vec<&str> {
            hosts
                .iter()
                .map(|host| host.endpoint_id.as_str())
                .collect()
        }

        // A friend's row is asked of the friend, then of the phone's own computer,
        // which may well hold the same file.
        assert_eq!(
            ordered(&fetch_order(Some("friend"), &[own.clone(), friend.clone()]).unwrap()),
            ["friend", "own"]
        );
        // A row from the phone's own computer, or from nowhere in particular.
        assert_eq!(
            ordered(&fetch_order(None, &[friend.clone(), own.clone()]).unwrap()),
            ["own", "friend"]
        );
        // A computer that may be read but not taken audio from is not asked for a
        // file, however it is named.
        assert_eq!(
            ordered(&fetch_order(Some("reader"), &[own.clone(), reader.clone()]).unwrap()),
            ["own"]
        );
        // A computer this phone no longer holds is not asked at all, and a phone
        // that may take audio from nobody at all is told which of the two it is.
        assert_eq!(
            ordered(&fetch_order(Some("gone"), &[own]).unwrap()),
            ["own"]
        );
        assert!(fetch_order(None, &[reader]).is_err());
        assert!(fetch_order(None, &[]).is_err());
    }

    /// Forgetting one computer is not forgetting the others, and a computer this
    /// phone never held is refused rather than quietly leaving it a host short.
    #[test]
    fn forgetting_a_computer_keeps_the_others() {
        let own = host("own", DeviceRights::full(), "My Napstr");
        let friend = host("friend", DeviceRights::read_only(), "Ada's Napstr");

        let remaining = without_host(vec![own.clone(), friend.clone()], "friend").unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].endpoint_id, "own");
        // The last computer can be forgotten as well: that is what leaves a
        // phone holding nothing, with the pairing screen back.
        assert!(without_host(vec![friend], "friend").unwrap().is_empty());
        assert!(without_host(vec![own], "friend").is_err());
    }

    /// A computer left out in Settings is not asked for anything, and a phone
    /// that has left every one of them out says so rather than offering nothing
    /// as though it had never been paired.
    #[test]
    fn a_computer_left_out_is_not_read_from() {
        let own = host("own", DeviceRights::full(), "My Napstr");
        let mut left_out = host("friend", DeviceRights::read_only(), "Ada's Napstr");
        left_out.included = false;

        let asked = readable_hosts(&[own.clone(), left_out.clone()]).unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].endpoint_id, "own");
        // Left out of being read, and out of being fetched from, which is the
        // same answer a track that came from it gets.
        assert_eq!(
            fetch_order(None, &[own, left_out.clone()]).unwrap().len(),
            1
        );
        assert_eq!(
            readable_hosts(&[left_out]).unwrap_err(),
            "Every computer is left out in Settings"
        );
    }

    /// A computer written down before one could be left out is read from, which
    /// is what every computer has always been.
    #[test]
    fn a_computer_file_without_the_answer_is_included() {
        let legacy = r#"{"endpointId":"legacy","endpointAddr":"{}","desktopName":"Old Napstr","streamOnly":false,"pubkey":""}"#;
        let host: SavedHost = serde_json::from_str(legacy).unwrap();
        assert!(host.included);
        assert_eq!(host.grant(), DeviceRights::full());
    }

    /// A plain list is each computer's own order, one after another, with a file
    /// two of them hold appearing once. A seed is what mixes them.
    #[test]
    fn a_plain_list_keeps_each_computers_own_order() {
        let hosts = vec![
            mixed_from("own", library_rows(&['a', 'b']), 2),
            mixed_from("friend", library_rows(&['c', 'a']), 2),
        ];
        let (page, total) = mixed_page(&hosts, None, 0, 10);
        assert_eq!(places(&page), "abc");
        assert_eq!(total, 4);
        // The same rows under a seed are in the seed's order, which is the mix
        // the phone's own music and a friend's are drawn from.
        let (shuffled, _) = mixed_page(&hosts, Some(7), 0, 10);
        assert_eq!(places(&shuffled), "cba");
        // And paging either of them goes on rather than repeating.
        let (second, _) = mixed_page(&hosts, None, 1, 2);
        assert_eq!(places(&second), "bc");
    }

    /// A playlist is edited on a phone by sending the whole of it, so the one
    /// size the phone can get wrong is its own: an edit too large to travel in a
    /// control frame. The refusal names the real problem instead of leaving the
    /// computer to answer with a protocol error the person cannot act on.
    #[test]
    fn a_playlist_too_large_for_one_frame_is_refused_here() {
        let playlist = |members: Vec<napstr_remote_protocol::RemotePlaylistTrack>| RemotePlaylist {
            playlist_id: "77abf082-7075-4d36-afe2-e9710ac6b33c".into(),
            title: "rock".into(),
            author: String::new(),
            display_name: String::new(),
            artist: String::new(),
            mbid: String::new(),
            image: String::new(),
            tags: String::new(),
            private: false,
            published: false,
            updated_at: 0,
            total: members.len(),
            tracks: members,
        };
        let member = |position: u32, hint: &str| napstr_remote_protocol::RemotePlaylistTrack {
            position,
            file_id: format!("{position:064x}"),
            title: hint.to_string(),
            artist: hint.to_string(),
            album: hint.to_string(),
        };
        // Hints of an ordinary length always fit, even with the playlist full.
        let ordinary = playlist(
            (1..=MAX_PLAYLIST_MEMBERS as u32)
                .map(|position| member(position, "Enter Sandman"))
                .collect(),
        );
        assert!(
            bounded_playlist_request(ClientRequest::SavePlaylist {
                playlist: ordinary.clone()
            })
            .is_ok(),
            "a full playlist of ordinary hints has to be editable from a phone"
        );
        // Hints at their maximum length are what makes a save too large - three
        // of them per member, 500 members - and the answer has to say so rather
        // than fail as an unexplained protocol error.
        let longest = "t".repeat(256);
        let enormous = playlist(
            (1..=MAX_PLAYLIST_MEMBERS as u32)
                .map(|position| member(position, &longest))
                .collect(),
        );
        let refusal = bounded_playlist_request(ClientRequest::PublishPlaylist {
            playlist: enormous,
            suggest_tags: false,
        })
        .unwrap_err();
        assert!(refusal.contains("too large"), "{refusal}");
        // Over the member limit is a different refusal, and it names the limit.
        let over = playlist(
            (1..=MAX_PLAYLIST_MEMBERS as u32 + 1)
                .map(|position| member(position, "Enter Sandman".into()))
                .collect(),
        );
        let refusal = bounded_playlist_request(ClientRequest::SavePlaylist { playlist: over })
            .unwrap_err();
        assert!(refusal.contains(&MAX_PLAYLIST_MEMBERS.to_string()), "{refusal}");
    }

    /// The QR the host draws has to survive the sanitiser, and nothing that
    /// could run in this page may.
    #[test]
    fn only_qr_markup_reaches_the_page() {
        // The hash in a colour literal needs a two-hash raw string: `"#` would
        // otherwise end it early.
        let drawn = r##"<?xml version="1.0" standalone="yes"?><svg xmlns="http://www.w3.org/2000/svg" version="1.1" width="4" height="4" viewBox="0 0 4 4" shape-rendering="crispEdges"><rect x="0" y="0" width="4" height="4" fill="#ffffff"/><path fill="#000000" d="M0 0h1v1H0V0"/></svg>"##;
        let accepted = safe_qr_svg(drawn);
        assert!(accepted.starts_with("<svg"));
        assert!(accepted.ends_with("</svg>"));
        assert!(!accepted.contains("<?xml"));
        assert!(safe_qr_svg(
            r#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#
        )
        .is_empty());
        assert!(safe_qr_svg(
            r#"<svg xmlns="http://www.w3.org/2000/svg"><path onload="x" d=""/></svg>"#
        )
        .is_empty());
        assert!(safe_qr_svg(
            r#"<svg xmlns="http://www.w3.org/2000/svg"><a href="https://x">y</a></svg>"#
        )
        .is_empty());
        assert!(safe_qr_svg("<html></html>").is_empty());
        assert!(safe_qr_svg("").is_empty());
    }

    /// A track code is drawn by this process rather than fetched, but it reaches
    /// the page through the same sanitiser a host's code does, so it has to come
    /// out of that intact or there would be no code to show.
    #[test]
    fn track_codes_are_drawn_and_validated() {
        let uri = format!("napstrfy://track/{}", "a".repeat(64));
        let svg = track_code(uri).expect("a track code");
        assert!(svg.starts_with("<svg"), "the sanitiser kept the markup");
        assert!(svg.ends_with("</svg>"));
        assert!(!svg.contains("<?xml"));
        assert!(track_code(String::new()).is_err());
        assert!(track_code("   ".to_string()).is_err());
        assert!(track_code("x".repeat(MAX_TRACK_URI_BYTES + 1)).is_err());
    }

    #[test]
    fn read_only_playback_caches_verified_audio_and_works_offline() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let root = std::env::temp_dir().join(format!(
                    "napstrfy-stream-{}",
                    hex::encode(SecretKey::generate().to_bytes())
                ));
                fs::create_dir_all(&root).unwrap();
                let bytes: Vec<u8> = (0..1024 * 1024 + 512)
                    .map(|index| (index % 251) as u8)
                    .collect();
                let track = RemoteTrack {
                    file_id: hex::encode(Sha256::digest(&bytes)),
                    filename: "song.mp3".into(),
                    title: "Song".into(),
                    artist: String::new(),
                    album: String::new(),
                    format: "MP3".into(),
                    mime: "audio/mpeg".into(),
                    size: bytes.len() as u64,
                    tags: String::new(),
                    local: true,
                    sources: Vec::new(),
                    bitrate_kbps: 128,
                    sample_rate_hz: 44_100,
                    channels: 2,
                    lossless: false,
                    duration_ms: 0,
                };
                let desktop_endpoint = Endpoint::builder(presets::Minimal)
                    .clear_ip_transports()
                    .alpns(vec![ALPN.to_vec()])
                    .bind_addr("127.0.0.1:0")
                    .unwrap()
                    .bind()
                    .await
                    .unwrap();
                let phone_endpoint = Endpoint::builder(presets::Minimal)
                    .clear_ip_transports()
                    .bind_addr("127.0.0.1:0")
                    .unwrap()
                    .bind()
                    .await
                    .unwrap();
                let server_track = track.clone();
                let server_bytes = bytes.clone();
                let endpoint = desktop_endpoint.clone();
                let server = tokio::spawn(async move {
                    let connection = endpoint.accept().await.unwrap().await.unwrap();
                    while let Ok((mut send, mut receive)) = connection.accept_bi().await {
                        let mut length = [0; 4];
                        receive.read_exact(&mut length).await.unwrap();
                        let mut payload = vec![0; u32::from_be_bytes(length) as usize];
                        receive.read_exact(&mut payload).await.unwrap();
                        let request: ClientRequest = serde_json::from_slice(&payload).unwrap();
                        let ClientRequest::FetchAudio { file_id } = request else {
                            panic!("read-only playback requested a host operation: {request:?}");
                        };
                        assert_eq!(file_id, server_track.file_id);
                        let response = ServerResponse::AudioReady { track: server_track.clone() };
                        let payload = serde_json::to_vec(&response).unwrap();
                        send.write_all(&(payload.len() as u32).to_be_bytes())
                            .await
                            .unwrap();
                        send.write_all(&payload).await.unwrap();
                        send.write_all(&server_bytes).await.unwrap();
                        send.finish().unwrap();
                    }
                });
                let connection = phone_endpoint
                    .connect(desktop_endpoint.addr(), ALPN)
                    .await
                    .unwrap();
                let remote = RemoteClient::new(root.clone());
                {
                    let endpoint_id = desktop_endpoint.id().to_string();
                    remote.hosts.write().await.push(SavedHost {
                        endpoint_id: endpoint_id.clone(),
                        endpoint_addr: String::new(),
                        desktop_name: "Test Napstr".into(),
                        stream_only: true,
                        rights: Some(DeviceRights::read_only()),
                        included: true,
                        pubkey: String::new(),
                    });
                    remote.connections.write().await.insert(endpoint_id, connection);
                }
                let media = MediaServer::start(root.join(art_store::ART_DIRECTORY)).unwrap();
                let playback = remote
                    .cache_audio(track.clone(), media.clone(), true)
                    .await
                    .unwrap();
                let http = reqwest::Client::builder().no_proxy().build().unwrap();
                let response = http
                    .get(&playback.url)
                    .header("Range", "bytes=123-456")
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), 206);
                assert_eq!(
                    response.headers()["content-range"],
                    format!("bytes 123-456/{}", bytes.len())
                );
                assert_eq!(response.headers()["cache-control"], "private, no-store");
                assert_eq!(&response.bytes().await.unwrap()[..], &bytes[123..457]);
                let response = http.get(&playback.url).send().await.unwrap();
                assert_eq!(response.status(), 200);
                assert_eq!(&response.bytes().await.unwrap()[..], bytes.as_slice());
                let response = http
                    .get(&playback.url)
                    .header("Range", "bytes=999999999-")
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), 416);
                media.wait_until_complete(&track.file_id).await.unwrap();
                let offline = remote.offline_library().await.unwrap();
                assert!(offline.stream_only);
                assert_eq!(offline.tracks, vec![track.clone()]);
                let cached = root.join("audio").join(format!("{}.mp3", track.file_id));
                assert_eq!(fs::read(cached).unwrap(), bytes);
                remote.disconnect().await;
                let replay = remote.cache_audio(track.clone(), media.clone(), true).await.unwrap();
                assert_eq!(http.get(replay.url).send().await.unwrap().bytes().await.unwrap().as_ref(), bytes.as_slice());
                tokio::time::timeout(Duration::from_secs(5), server)
                    .await
                    .unwrap()
                    .unwrap();
                phone_endpoint.close().await;
                desktop_endpoint.close().await;
                fs::remove_dir_all(root).unwrap();
            });
    }

    #[test]
    fn only_hashes_and_known_audio_extensions_become_cache_paths() {
        assert!(validate_file_id(&"a".repeat(64)).is_ok());
        assert!(validate_file_id("../../secret").is_err());
        assert_eq!(safe_extension("FLAC").unwrap(), "flac");
        assert!(safe_extension("EXE").is_err());
    }

    #[test]
    fn completed_audio_cache_rehydrates_without_a_desktop_connection() {
        let root = std::env::temp_dir().join(format!(
            "napstrfy-offline-cache-{}-{}",
            std::process::id(),
            chrono_timestamp()
        ));
        let audio = root.join("audio");
        fs::create_dir_all(&audio).unwrap();
        let bytes = b"verified offline audio";
        let file_id = hex::encode(Sha256::digest(bytes));
        let track = RemoteTrack {
            file_id: file_id.clone(),
            filename: "offline.mp3".into(),
            title: "Offline".into(),
            artist: "Napstr".into(),
            album: String::new(),
            format: "MP3".into(),
            mime: "audio/mpeg".into(),
            size: bytes.len() as u64,
            tags: String::new(),
            local: true,
            sources: Vec::new(),
            bitrate_kbps: 128,
            sample_rate_hz: 44_100,
            channels: 2,
            lossless: false,
            duration_ms: 0,
        };
        fs::write(audio.join(format!("{file_id}.mp3")), bytes).unwrap();
        save_json(
            &audio.join(format!("{file_id}.json")),
            &CachedRemoteAudio {
                track: track.clone(),
                library_visible: true,
            },
        )
        .unwrap();

        let cached = cached_entries_in(&root).unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].track, track);

        fs::write(audio.join(format!("{file_id}.mp3")), b"short").unwrap();
        assert!(cached_entries_in(&root).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    /// Three cached tracks in a directory of its own, written oldest first so the
    /// sidecar timestamps are the order they were last listened to in.
    fn staged_audio_cache(name: &str) -> (PathBuf, Vec<CachedRemoteAudio>) {
        let root = std::env::temp_dir().join(format!("napstrfy-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let directory = root.join("audio");
        fs::create_dir_all(&directory).unwrap();
        let mut cached = Vec::new();
        for file_id in ["oldest", "middle", "newest"] {
            let track = RemoteTrack {
                file_id: file_id.to_string(),
                filename: format!("{file_id}.mp3"),
                title: file_id.to_string(),
                artist: "Napstr".into(),
                album: String::new(),
                format: "MP3".into(),
                mime: "audio/mpeg".into(),
                size: 100,
                tags: String::new(),
                local: true,
                sources: Vec::new(),
                bitrate_kbps: 128,
                sample_rate_hz: 44_100,
                channels: 2,
                lossless: false,
                duration_ms: 0,
            };
            fs::write(directory.join(format!("{file_id}.mp3")), vec![0u8; 100]).unwrap();
            save_json(
                &directory.join(format!("{file_id}.json")),
                &CachedRemoteAudio {
                    track: track.clone(),
                    library_visible: true,
                },
            )
            .unwrap();
            // Apart in time, because the eviction order *is* the timestamp.
            std::thread::sleep(std::time::Duration::from_millis(20));
            cached.push(CachedRemoteAudio {
                track,
                library_visible: true,
            });
        }
        (directory, cached)
    }

    #[test]
    fn the_audio_cache_drops_what_has_not_been_listened_to_for_longest() {
        let (directory, cached) = staged_audio_cache("budget");
        // 300 bytes held, a 250 byte budget: one track goes, and it is the oldest.
        assert_eq!(
            evict_beyond_budget(&directory, &cached, &HashSet::new(), 250).unwrap(),
            1
        );
        assert!(!directory.join("oldest.mp3").exists());
        // The sidecar goes with it: a row without its bytes is a cache that lies.
        assert!(!directory.join("oldest.json").exists());
        assert!(directory.join("middle.mp3").exists());
        assert!(directory.join("newest.mp3").exists());
        fs::remove_dir_all(directory.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_audio_cache_never_evicts_what_the_phone_protects() {
        let (directory, cached) = staged_audio_cache("protected");
        // A budget nothing could fit in: the protected track still survives — it is
        // what the phone is playing or about to play, which is the whole point of
        // pre-loading — and the rest go oldest first.
        let protected = HashSet::from(["middle".to_string()]);
        assert_eq!(
            evict_beyond_budget(&directory, &cached, &protected, 1).unwrap(),
            2
        );
        assert!(directory.join("middle.mp3").exists());
        assert!(!directory.join("oldest.mp3").exists());
        assert!(!directory.join("newest.mp3").exists());
        fs::remove_dir_all(directory.parent().unwrap()).unwrap();
    }

    #[test]
    fn mobile_names_drop_direction_overrides() {
        assert_eq!(clean_device_name("My\u{202e}Phone"), "MyPhone");
    }

    #[test]
    fn cover_keys_match_the_cover_nip_normalisation() {
        assert_eq!(
            normalise_cover_key("  Pink Floyd |Animals ").as_deref(),
            Some("pink floyd|animals")
        );
        assert_eq!(
            normalise_cover_key("BEYONCÉ|Lemonade").as_deref(),
            Some("beyoncé|lemonade")
        );
        // Edition markers are preserved: matching is against the catalogue's own
        // display strings, and the host handles the canonical alias.
        assert_eq!(
            normalise_cover_key("Artist|Album (Deluxe Edition)").as_deref(),
            Some("artist|album (deluxe edition)")
        );
        // A key that is not addressable is dropped rather than sent on.
        assert_eq!(normalise_cover_key("artist"), None);
        assert_eq!(normalise_cover_key("artist|"), None);
        assert_eq!(normalise_cover_key("|album"), None);
        assert_eq!(normalise_cover_key("a|b|c"), None);
        assert_eq!(normalise_cover_key(&format!("{}|album", "a".repeat(299))), None);

        let request = normalise_cover_request(&[
            " Artist | Album ".into(),
            "artist|album".into(),
            "junk".into(),
        ]);
        assert_eq!(request, vec!["artist|album".to_string()]);

        // A caller cannot buy an unbounded amount of relay work in one go.
        let many = (0..MAX_COVER_KEYS * 5)
            .map(|index| format!("artist|album{index}"))
            .collect::<Vec<_>>();
        assert_eq!(normalise_cover_request(&many).len(), MAX_COVER_KEYS * 4);
    }

    #[test]
    fn public_podcast_search_keeps_clean_unique_genres() {
        let payload = r#"{"results":[{"collectionId":42,"collectionName":"A Show","artistName":"A Host","feedUrl":"https://example.com/feed.xml","genres":["Technology","Podcasts","technology"],"primaryGenreName":"Science","trackCount":10}]}"#;
        let feeds = parse_public_podcast_search(payload, 10).unwrap();
        assert_eq!(feeds.len(), 1);
        assert_eq!(feeds[0].genres, vec!["Science", "Technology", "Podcasts"]);
    }

    #[test]
    fn public_podcast_lookup_skips_the_show_and_keeps_audio_episodes() {
        let feed = PodcastFeed {
            id: 42,
            title: "A Show".into(),
            author: "A Host".into(),
            description: String::new(),
            feed_url: "https://example.com/feed.xml".into(),
            image: "https://example.com/show.jpg".into(),
            language: "en".into(),
            episode_count: 1,
            genres: vec!["Technology".into()],
        };
        let payload = r#"{"results":[
          {"wrapperType":"track","kind":"podcast","trackId":42,"collectionId":42,"trackName":"A Show"},
          {"wrapperType":"podcastEpisode","kind":"podcast-episode","trackId":99,"collectionId":42,"collectionName":"A Show","trackName":"First episode","description":"Hello","episodeUrl":"https://cdn.example.com/one.mp3","episodeContentType":"audio","episodeFileExtension":"mp3","trackTimeMillis":3723000,"releaseDate":"2026-08-25T12:00:00Z","artworkUrl600":"https://example.com/episode.jpg"}
        ]}"#;
        let episodes = parse_public_podcast_episodes(&feed, payload, 50).unwrap();
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].id, 99);
        assert_eq!(episodes[0].title, "First episode");
        assert_eq!(episodes[0].enclosure_type, "audio/mpeg");
        assert_eq!(episodes[0].duration, 3_723);
    }

    #[test]
    fn audio_ranges_are_not_truncated_to_a_preview_chunk() {
        assert_eq!(
            requested_audio_range(Some("bytes=0-"), 18_000_000),
            Ok(Some((0, 17_999_999)))
        );
        assert_eq!(
            requested_audio_range(Some("bytes=100-199"), 1_000),
            Ok(Some((100, 199)))
        );
        assert_eq!(
            requested_audio_range(Some("bytes=-50"), 1_000),
            Ok(Some((950, 999)))
        );
        assert!(requested_audio_range(Some("bytes=1000-"), 1_000).is_err());
    }

    #[test]
    fn podcast_rss_is_parsed_without_napstr_protocol_data() {
        let feed = PodcastFeed {
            id: 42,
            title: "Independent show".into(),
            author: "Host".into(),
            description: String::new(),
            feed_url: "https://example.com/feed.xml".into(),
            image: "https://example.com/show.jpg".into(),
            language: "en".into(),
            episode_count: 1,
            genres: vec!["Technology".into()],
        };
        let rss = br#"<?xml version="1.0"?><rss><channel><item>
          <guid>episode-one</guid><title>First episode</title>
          <pubDate>Tue, 25 Aug 2026 12:00:00 +0000</pubDate>
          <itunes:duration>01:02:03</itunes:duration>
          <enclosure url="https://cdn.example.com/one.mp3" type="audio/mpeg" length="1234" />
        </item></channel></rss>"#;
        let episodes = parse_podcast_feed(&feed, rss, 50).unwrap();
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].feed_title, "Independent show");
        assert_eq!(episodes[0].duration, 3_723);
        assert_eq!(episodes[0].enclosure_length, 1_234);
        assert_eq!(episodes[0].image, feed.image);
    }

    #[test]
    fn podcast_feeds_accept_relative_media_content_and_upgrade_public_http() {
        let feed = PodcastFeed {
            id: 42,
            title: "Show".into(),
            author: String::new(),
            description: String::new(),
            feed_url: "https://feeds.example.com/show/rss.xml".into(),
            image: String::new(),
            language: String::new(),
            episode_count: 2,
            genres: Vec::new(),
        };
        let rss = br#"<rss><channel>
          <item><title>Relative</title><media:content url="/audio/one.mp3" type="audio/mpeg" /></item>
          <item><title>Legacy HTTP</title><enclosure url="http://cdn.example.com/two.mp3" type="application/octet-stream" /></item>
        </channel></rss>"#;
        let episodes = parse_podcast_feed(&feed, rss, 50).unwrap();
        assert_eq!(episodes.len(), 2);
        assert_eq!(
            episodes[0].enclosure_url,
            "https://feeds.example.com/audio/one.mp3"
        );
        assert_eq!(episodes[1].enclosure_url, "https://cdn.example.com/two.mp3");
        assert_eq!(episodes[1].enclosure_type, "audio/mpeg");
    }

    #[test]
    fn podcast_feeds_reject_private_enclosures() {
        let feed = PodcastFeed {
            id: 42,
            title: "Show".into(),
            author: String::new(),
            description: String::new(),
            feed_url: "https://example.com/feed.xml".into(),
            image: String::new(),
            language: String::new(),
            episode_count: 1,
            genres: Vec::new(),
        };
        let rss = br#"<rss><channel><item><title>Unsafe</title>
          <enclosure url="http://192.168.1.1/private.mp3" type="audio/mpeg" />
        </item></channel></rss>"#;
        assert!(parse_podcast_feed(&feed, rss, 50).unwrap().is_empty());
    }
}
