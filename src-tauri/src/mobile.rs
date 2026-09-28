use crate::{
    build_local_audiobooks, build_local_audiobooks_from_files, cover_publish::CoverAlbumNote,
    cover_publish::CoverPublisher, load_files, load_files_by_id, load_transfers, network,
    open_connection, search_matches, SharedFile,
};
use chrono::Utc;
use iroh::{endpoint::presets, Endpoint, SecretKey};
use crate::art_fetch::ArtWant;
use napstr_remote_protocol::{
    ArtRendition, ClientRequest, CoverReportResult, DeviceRights, PairingTicket, PlaybackCommand,
    RemoteAlbumCover,
    RemoteAudiobook, RemoteAudiobookSummary, RemoteDiscussionActivity, RemoteDiscussionMessage,
    RemoteDiscussionReply, RemoteSource, RemoteTrack, RemoteTransfer, ServerResponse, ALPN,
    MAX_ART_KEY_CHARS, MAX_CONTROL_FRAME_BYTES, MAX_COVER_KEYS, MAX_PAGE_SIZE, MAX_PLAYLIST_PAGE,
    MAX_PLAY_QUEUE, MAX_POSITION_MS, MAX_TRACKS_BY_ID, PROTOCOL_VERSION,
};
use qrcode::{render::svg, QrCode};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::io::AsyncReadExt;

const PAIRING_LIFETIME_SECONDS: i64 = 5 * 60;
/// What a comment may be: the same ceiling the desktop's own composer enforces,
/// because both write the same kind of event into the same conversation.
const MAX_DISCUSSION_CHARS: usize = 500;
/// Files one row-mark question may name. The host caps it again in the network
/// layer; this is the wire's own limit, refused before any work is done.
const MAX_DISCUSSION_ACTIVITY_IDS: usize = 200;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECTION_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const RESPONSE_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// How much art travels per frame. Smaller than the audio chunk on purpose: art
/// is tens of kilobytes, so this is one frame for a thumbnail and a handful for
/// a full rendition, and a smaller frame keeps a slow link from holding a large
/// buffer between permission checks.
const ART_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedDevice {
    endpoint_id: String,
    name: String,
    paired_at: String,
    last_seen: String,
    rights: DeviceRights,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileStatus {
    running: bool,
    online: bool,
    endpoint_id: String,
    error: String,
    devices: Vec<PairedDevice>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobilePairingOffer {
    ticket: String,
    qr_svg: String,
    expires_at: i64,
    endpoint_id: String,
}

struct PairingSession {
    token: String,
    expires_at: i64,
    rights: DeviceRights,
}

#[derive(Default)]
struct RuntimeStatus {
    running: bool,
    online: bool,
    endpoint_id: String,
    error: String,
}

struct MusicLibraryCache {
    revision: u64,
    tracks: Arc<Vec<RemoteTrack>>,
    audiobook_chapter_ids: Arc<std::collections::HashSet<String>>,
}

pub struct MobileService {
    db_path: PathBuf,
    /// Where the art this service hands to phones lives. The app's cache
    /// directory, because these bytes are evictable by design: every one of them
    /// can be fetched again, and a person must be able to clear them.
    art_root: PathBuf,
    key_path: PathBuf,
    network: Arc<crate::network::NetworkService>,
    /// The cover worker, so a phone's own results join the queue the window
    /// fills. The phone has no MusicBrainz client and resolves art only from
    /// kind `30427`, so an album that only it has shown would otherwise never be
    /// looked up at all.
    covers: Arc<CoverPublisher>,
    playback: Arc<crate::playback_bridge::PlaybackBridge>,
    endpoint: tokio::sync::RwLock<Option<Endpoint>>,
    start_lock: tokio::sync::Mutex<()>,
    pairing: Mutex<Vec<PairingSession>>,
    status: Mutex<RuntimeStatus>,
    audiobook_cache: Mutex<std::collections::HashMap<String, RemoteAudiobook>>,
    music_library_cache: Mutex<Option<MusicLibraryCache>>,
    last_seen_updates: Mutex<std::collections::HashMap<String, Instant>>,
    connection_slots: Arc<tokio::sync::Semaphore>,
    request_slots: Arc<tokio::sync::Semaphore>,
}

impl MobileService {
    pub fn new(
        db_path: PathBuf,
        app_data: PathBuf,
        art_root: PathBuf,
        network: Arc<crate::network::NetworkService>,
        covers: Arc<CoverPublisher>,
        playback: Arc<crate::playback_bridge::PlaybackBridge>,
    ) -> Result<Arc<Self>, String> {
        initialise_schema(&db_path)?;
        Ok(Arc::new(Self {
            db_path,
            art_root,
            key_path: app_data.join("iroh-identity"),
            network,
            covers,
            playback,
            endpoint: tokio::sync::RwLock::new(None),
            start_lock: tokio::sync::Mutex::new(()),
            pairing: Mutex::new(Vec::new()),
            status: Mutex::new(RuntimeStatus::default()),
            audiobook_cache: Mutex::new(std::collections::HashMap::new()),
            music_library_cache: Mutex::new(None),
            last_seen_updates: Mutex::new(std::collections::HashMap::new()),
            connection_slots: Arc::new(tokio::sync::Semaphore::new(16)),
            request_slots: Arc::new(tokio::sync::Semaphore::new(32)),
        }))
    }

    pub fn has_devices(&self) -> bool {
        load_devices(&self.db_path)
            .map(|devices| !devices.is_empty())
            .unwrap_or(false)
    }

    pub async fn start(self: &Arc<Self>) -> Result<(), String> {
        let _guard = self.start_lock.lock().await;
        if self.endpoint.read().await.is_some() {
            return Ok(());
        }
        if let Ok(mut status) = self.status.lock() {
            status.error.clear();
        }
        let key = load_or_create_key(&self.key_path)?;
        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(key)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .map_err(|error| self.remember_error(format!("Iroh failed to start: {error}")))?;
        let endpoint_id = endpoint.id().to_string();
        {
            let mut slot = self.endpoint.write().await;
            *slot = Some(endpoint.clone());
        }
        if let Ok(mut status) = self.status.lock() {
            status.running = true;
            status.endpoint_id = endpoint_id;
            status.error.clear();
        }

        let accept_service = self.clone();
        let accept_endpoint = endpoint.clone();
        tokio::spawn(async move {
            while let Some(incoming) = accept_endpoint.accept().await {
                let permit = match accept_service.connection_slots.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        incoming.refuse();
                        continue;
                    }
                };
                let service = accept_service.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    match tokio::time::timeout(HANDSHAKE_TIMEOUT, incoming).await {
                        Ok(Ok(connection)) => service.handle_connection(connection).await,
                        Ok(Err(error)) => eprintln!("Napstrfy connection failed: {error}"),
                        Err(_) => eprintln!("Napstrfy connection handshake timed out"),
                    }
                });
            }
        });

        let online_service = self.clone();
        tokio::spawn(async move {
            endpoint.online().await;
            if let Ok(mut status) = online_service.status.lock() {
                status.online = true;
            }
        });
        Ok(())
    }

    pub async fn stop(&self) {
        if let Some(endpoint) = self.endpoint.write().await.take() {
            endpoint.close().await;
        }
        if let Ok(mut status) = self.status.lock() {
            status.running = false;
            status.online = false;
        }
    }

    pub async fn status(self: &Arc<Self>) -> MobileStatus {
        if self.endpoint.read().await.is_none() {
            let _ = self.start().await;
        }
        let runtime = self.status.lock().ok();
        MobileStatus {
            running: runtime.as_ref().map(|value| value.running).unwrap_or(false),
            online: runtime.as_ref().map(|value| value.online).unwrap_or(false),
            endpoint_id: runtime
                .as_ref()
                .map(|value| value.endpoint_id.clone())
                .unwrap_or_default(),
            error: runtime
                .as_ref()
                .map(|value| value.error.clone())
                .unwrap_or_else(|| "Mobile service state is unavailable".into()),
            devices: load_devices(&self.db_path).unwrap_or_default(),
        }
    }

    pub async fn create_pairing(
        self: &Arc<Self>,
        rights: DeviceRights,
    ) -> Result<MobilePairingOffer, String> {
        self.start().await?;
        if let Some(endpoint) = self.endpoint.read().await.clone() {
            // Waiting briefly gives the ticket a relay path as well as the
            // endpoint identity. A DNS lookup remains available if it times out.
            let _ = tokio::time::timeout(Duration::from_secs(12), endpoint.online()).await;
        }
        self.issue_pairing(rights).await
    }

    /// Mint a one-use code for the endpoint that is already running.
    ///
    /// This starts nothing on purpose: it is also reached from request handling,
    /// and a request that arrived over Iroh has already proved the endpoint is
    /// up. Calling `start` from there would make `start` reachable through its
    /// own spawned tasks, which the compiler cannot type (E0391).
    async fn issue_pairing(&self, rights: DeviceRights) -> Result<MobilePairingOffer, String> {
        let endpoint = self
            .endpoint
            .read()
            .await
            .clone()
            .ok_or("Iroh is not running")?;
        let endpoint_addr = serde_json::to_string(&endpoint.addr())
            .map_err(|error| format!("could not encode the Iroh address: {error}"))?;
        let token = hex::encode(rand::random::<[u8; 32]>());
        let expires_at = Utc::now().timestamp() + PAIRING_LIFETIME_SECONDS;
        {
            let mut pairing = self
                .pairing
                .lock()
                .map_err(|_| "pairing state lock was poisoned")?;
            pairing.retain(|session| {
                // One live code per distinct grant, so minting a second read-only
                // code replaces the first rather than leaving two valid ones.
                session.rights != rights && session.expires_at >= Utc::now().timestamp()
            });
            pairing.push(PairingSession {
                token: token.clone(),
                expires_at,
                rights,
            });
        }
        let desktop_name = self.desktop_name();
        let ticket = PairingTicket {
            version: PROTOCOL_VERSION,
            endpoint_id: endpoint.id().to_string(),
            endpoint_addr,
            token,
            expires_at,
            desktop_name,
        }
        .to_uri()?;
        let qr_svg = QrCode::new(ticket.as_bytes())
            .map_err(|error| format!("could not create pairing QR: {error}"))?
            .render::<svg::Color>()
            .min_dimensions(280, 280)
            .dark_color(svg::Color("#000000"))
            .light_color(svg::Color("#ffffff"))
            .build();
        Ok(MobilePairingOffer {
            endpoint_id: endpoint.id().to_string(),
            ticket,
            qr_svg,
            expires_at,
        })
    }

    pub fn revoke(&self, endpoint_id: &str) -> Result<(), String> {
        let parsed = endpoint_id
            .parse::<iroh::EndpointId>()
            .map_err(|_| "invalid Iroh endpoint ID")?;
        open_connection(&self.db_path)?
            .execute(
                "DELETE FROM mobile_devices WHERE endpoint_id=?1",
                [parsed.to_string()],
            )
            .map_err(|error| error.to_string())?;
        if let Ok(mut updates) = self.last_seen_updates.lock() {
            updates.remove(endpoint_id);
        }
        Ok(())
    }

    /// What this computer may do for this device, set after it has paired.
    ///
    /// The grant used to be fixed by the code that let a device in, so changing
    /// your mind meant pairing again. It belongs to the device now, and this is the
    /// one place it changes. An endpoint that is not paired is refused rather than
    /// written: a row here is what makes a device real, so writing one would be a
    /// way of pairing without a code.
    pub fn set_rights(&self, endpoint_id: &str, rights: DeviceRights) -> Result<(), String> {
        set_device_rights(&self.db_path, endpoint_id, rights)
    }

    /// The name shown to a phone. One place, so every answer agrees.
    fn desktop_name(&self) -> String {
        open_connection(&self.db_path)
            .and_then(|connection| crate::get_setting(&connection, "display_name"))
            .unwrap_or_else(|_| "Napstr".into())
    }

    fn remember_error(&self, message: String) -> String {
        if let Ok(mut status) = self.status.lock() {
            status.error = message.clone();
            status.running = false;
            status.online = false;
        }
        message
    }

    async fn handle_connection(self: Arc<Self>, connection: iroh::endpoint::Connection) {
        let remote_id = connection.remote_id().to_string();
        if !self.connection_is_allowed(&remote_id) {
            return;
        }
        loop {
            if !self.connection_is_allowed(&remote_id) {
                break;
            }
            let (mut send, mut receive) =
                match tokio::time::timeout(CONNECTION_IDLE_TIMEOUT, connection.accept_bi()).await {
                    Ok(Ok(streams)) => streams,
                    Ok(Err(_)) | Err(_) => break,
                };
            let service = self.clone();
            let remote_id = remote_id.clone();
            let permit = match self.request_slots.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    // Never let an overloaded peer block the connection's accept loop.
                    let _ = send.reset(1u32.into());
                    let _ = receive.stop(1u32.into());
                    continue;
                }
            };
            tokio::spawn(async move {
                let _permit = permit;
                let request =
                    match tokio::time::timeout(Duration::from_secs(15), read_request(&mut receive))
                        .await
                    {
                        Ok(Ok(request)) => request,
                        Ok(Err(error)) => {
                            let _ = write_response(
                                &mut send,
                                &ServerResponse::Error { message: error },
                            )
                            .await;
                            let _ = send.finish();
                            return;
                        }
                        Err(_) => {
                            let _ = write_response(
                                &mut send,
                                &ServerResponse::Error {
                                    message: "Napstrfy request timed out".into(),
                                },
                            )
                            .await;
                            let _ = send.finish();
                            return;
                        }
                    };
                if let Err(error) = service.serve_request(&remote_id, request, &mut send).await {
                    let _ =
                        write_response(&mut send, &ServerResponse::Error { message: error }).await;
                }
                let _ = send.finish();
            });
        }
    }

    fn connection_is_allowed(&self, remote_id: &str) -> bool {
        if self.authorise(remote_id).is_ok() {
            return true;
        }
        let now = Utc::now().timestamp();
        self.pairing
            .lock()
            .ok()
            .map(|mut pairing| {
                pairing.retain(|session| session.expires_at >= now);
                !pairing.is_empty()
            })
            .unwrap_or(false)
    }

    async fn audiobook_catalogue(
        &self,
        query: &str,
        rights: DeviceRights,
    ) -> Result<Vec<RemoteAudiobook>, String> {
        let (local_books, local_tracks) = load_local_remote_audiobooks(&self.db_path)?;
        let mut books = std::collections::HashMap::new();
        for book in local_books.into_iter().filter(|book| {
            search_matches(
                query,
                &book
                    .chapters
                    .iter()
                    .flat_map(|chapter| [chapter.title.as_str(), chapter.filename.as_str()])
                    .chain([
                        book.title.as_str(),
                        book.author.as_str(),
                        book.narrator.as_str(),
                    ])
                    .collect::<Vec<_>>(),
            )
        }) {
            books.insert(book.audiobook_id.clone(), book);
        }
        let remote = if rights.privileged {
            self.network.search_audiobooks(query).await?
        } else {
            Vec::new()
        };
        for book in remote {
            books
                .entry(book.audiobook_id.clone())
                .or_insert_with(|| remote_audiobook(book, &local_tracks));
        }
        let mut audiobooks = books.into_values().collect::<Vec<_>>();
        audiobooks.sort_by(|left, right| left.title.cmp(&right.title));
        audiobooks.truncate(MAX_PAGE_SIZE);
        if let Ok(mut cache) = self.audiobook_cache.lock() {
            cache.clear();
            cache.extend(
                audiobooks
                    .iter()
                    .cloned()
                    .map(|book| (book.audiobook_id.clone(), book)),
            );
        }
        Ok(audiobooks)
    }

    fn music_library(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
        shuffle_seed: Option<u64>,
    ) -> Result<
        (
            Vec<RemoteTrack>,
            usize,
            Arc<std::collections::HashSet<String>>,
        ),
        String,
    > {
        let revision = library_revision(&self.db_path)?;
        let cached = self.music_library_cache.lock().ok().and_then(|cache| {
            cache.as_ref().and_then(|cached| {
                (cached.revision == revision)
                    .then(|| (cached.tracks.clone(), cached.audiobook_chapter_ids.clone()))
            })
        });
        let (tracks, audiobook_chapter_ids) = match cached {
            Some(cached) => cached,
            None => {
                let (tracks, audiobook_chapter_ids) = build_music_library(&self.db_path)?;
                let tracks = Arc::new(tracks);
                let audiobook_chapter_ids = Arc::new(audiobook_chapter_ids);
                if let Ok(mut cache) = self.music_library_cache.lock() {
                    *cache = Some(MusicLibraryCache {
                        revision,
                        tracks: tracks.clone(),
                        audiobook_chapter_ids: audiobook_chapter_ids.clone(),
                    });
                }
                (tracks, audiobook_chapter_ids)
            }
        };
        let (page, total) = match (query.is_empty(), shuffle_seed) {
            (true, Some(seed)) => page_shuffled_music_library(&tracks, seed, offset, limit),
            _ => page_music_library(&tracks, query, offset, limit),
        };
        Ok((page, total, audiobook_chapter_ids))
    }

    /// Write a playlist down on this computer, on behalf of a phone.
    ///
    /// The author and the edit time are stamped here rather than taken from the
    /// phone, for the same reason the desktop's own command does it: the
    /// coordinate has to be the one a later publication will use, and a phone
    /// must not be able to file a playlist under somebody else's key. What comes
    /// back is what was stored, so a phone that keeps the returned copy and the
    /// list the host will answer with cannot disagree about a title or a member.
    ///
    /// A playlist the phone opened is somebody else's public playlist as often
    /// as not, now that the host reads them from the relays. Editing one of
    /// those is copying it: it is filed under an id of its own, and the phone
    /// learns that from the answer rather than from a flag - the copy it gets
    /// back names itself.
    fn save_playlist(
        &self,
        playlist: napstr_remote_protocol::RemotePlaylist,
    ) -> Result<napstr_remote_protocol::RemotePlaylist, String> {
        crate::playlist::file_revision(
            &open_connection(&self.db_path)?,
            playlist,
            &crate::network::own_pubkey()?,
            Utc::now().timestamp(),
        )
    }

    async fn serve_request(
        &self,
        remote_id: &str,
        request: ClientRequest,
        send: &mut iroh::endpoint::SendStream,
    ) -> Result<(), String> {
        if let ClientRequest::Pair {
            token,
            device_name,
        } = request
        {
            let rights =
                self.accept_pairing(remote_id, &token, &device_name)?;
            return write_response(
                send,
                &ServerResponse::Paired {
                    stream_only: rights.is_read_only(),
                    rights: Some(rights),
                    desktop_name: self.desktop_name(),
                },
            )
            .await;
        }
        let rights = self.authorise(remote_id)?;
        check_request_permission(rights, &request)?;
        self.touch_device(remote_id);
        match request {
            ClientRequest::Library {
                query,
                offset,
                limit,
                shuffle_seed,
            } => {
                if query.chars().count() > 120 {
                    return Err("Library searches are limited to 120 characters".into());
                }
                let (tracks, total, _) = self.music_library(
                    &query,
                    offset,
                    limit.clamp(1, MAX_PAGE_SIZE),
                    shuffle_seed,
                )?;
                write_response(send, &ServerResponse::Library { tracks, total }).await
            }
            ClientRequest::Search { query } => {
                let query = query.trim();
                if query.is_empty() || query.chars().count() > 120 {
                    return Err("Search for between 1 and 120 characters".into());
                }
                let (mut tracks, _, audiobook_chapter_ids) =
                    self.music_library(query, 0, MAX_PAGE_SIZE, None)?;
                let remote = if rights.privileged {
                    self.network.search(query).await?
                } else {
                    Vec::new()
                };
                for result in remote {
                    if audiobook_chapter_ids.contains(&result.file_id)
                        || tracks.iter().any(|track| track.file_id == result.file_id)
                    {
                        continue;
                    }
                    tracks.push(RemoteTrack {
                        file_id: result.file_id,
                        filename: result.filename,
                        title: result.title,
                        artist: result.artist,
                        album: result.album,
                        format: result.format,
                        mime: result.mime,
                        size: result.size,
                        tags: result.tags,
                        local: false,
                        sources: result
                            .sources
                            .into_iter()
                            .map(|source| RemoteSource {
                                pubkey: source.pubkey,
                                display_name: source.display_name,
                            })
                            .collect(),
                        // A catalogue result is somebody else's event, and a
                        // catalogue entry says what a file is called and how big
                        // it is, not what the audio in it is. Reporting zero is
                        // the honest answer here: the only row that knows is the
                        // library row, and a file this host holds is taken from
                        // there and never from the catalogue.
                        bitrate_kbps: 0,
                        sample_rate_hz: 0,
                        channels: 0,
                        lossless: false,
                        duration_ms: 0,
                    });
                }
                tracks.sort_by(|left, right| {
                    right
                        .local
                        .cmp(&left.local)
                        .then_with(|| right.sources.len().cmp(&left.sources.len()))
                        .then_with(|| left.filename.cmp(&right.filename))
                });
                tracks.truncate(MAX_PAGE_SIZE);
                // Hand the albums this phone is about to see to the cover worker,
                // which is what lets a phone get art by proxy. It has no
                // MusicBrainz client of its own, so an album that only this phone
                // has shown is never looked up unless the search says so here.
                // Reporting is a local write and does nothing while both cover
                // switches are off; a failure to queue must not fail the search.
                // It is still said out loud, because a queue write that fails
                // silently is exactly how a phone ends up with no art and no
                // reason why.
                if let Err(error) = self.covers.note_visible(
                    &tracks
                        .iter()
                        .map(|track| CoverAlbumNote {
                            artist: track.artist.clone(),
                            album: track.album.clone(),
                        })
                        .collect::<Vec<_>>(),
                ) {
                    eprintln!("Could not queue the albums a phone searched for: {error}");
                }
                write_response(send, &ServerResponse::Search { tracks }).await
            }
            ClientRequest::Audiobooks { query } => {
                let query = query.trim();
                if query.chars().count() > 120 {
                    return Err("Audiobook searches are limited to 120 characters".into());
                }
                // Legacy full response retained for older Napstrfy installs.
                let audiobooks = self.audiobook_catalogue(query, rights).await?;
                write_response(send, &ServerResponse::Audiobooks { audiobooks }).await
            }
            ClientRequest::AudiobookLibrary {
                query,
                offset,
                limit,
            } => {
                let query = query.trim();
                if query.chars().count() > 120 {
                    return Err("Audiobook searches are limited to 120 characters".into());
                }
                let audiobooks = self.audiobook_catalogue(query, rights).await?;
                let total = audiobooks.len();
                let summaries = audiobooks
                    .into_iter()
                    .skip(offset)
                    .take(limit.clamp(1, MAX_PAGE_SIZE))
                    .map(|book| RemoteAudiobookSummary {
                        audiobook_id: book.audiobook_id,
                        title: book.title,
                        author: book.author,
                        narrator: book.narrator,
                        total_size: book.total_size,
                        chapter_count: book.chapters.len(),
                    })
                    .collect();
                write_response(
                    send,
                    &ServerResponse::AudiobookLibrary {
                        audiobooks: summaries,
                        total,
                    },
                )
                .await
            }
            ClientRequest::Audiobook { audiobook_id } => {
                // Napstrfy may retain a library summary while Napstr restarts or
                // while another search replaces this process's detail cache.
                // Local files and audiobook configuration are authoritative, so
                // resolve those from the database before consulting the cache.
                let local_audiobook = load_local_remote_audiobooks(&self.db_path)?
                    .0
                    .into_iter()
                    .find(|book| book.audiobook_id == audiobook_id);
                if let Some(audiobook) = local_audiobook {
                    if let Ok(mut cache) = self.audiobook_cache.lock() {
                        cache.insert(audiobook_id, audiobook.clone());
                    }
                    return write_response(send, &ServerResponse::Audiobook { audiobook }).await;
                }
                // An audiobook this computer does not hold locally has to be read
                // out of the relays in the user's own name, which is acting
                // through this computer rather than reading it.
                if !rights.privileged {
                    return Err("This audiobook is not in Napstr's local library".into());
                }
                let mut audiobook = self
                    .audiobook_cache
                    .lock()
                    .map_err(|_| "audiobook cache lock poisoned".to_string())?
                    .get(&audiobook_id)
                    .cloned()
                    .ok_or("That audiobook is no longer available; refresh the list")?;
                let chapter_ids = audiobook
                    .chapters
                    .iter()
                    .map(|chapter| chapter.file_id.clone())
                    .collect::<Vec<_>>();
                let local_tracks =
                    load_files_by_id(&open_connection(&self.db_path)?, &chapter_ids)?
                        .into_iter()
                        .map(|file| {
                            let file_id = file.file_id.clone();
                            (file_id, remote_track(file))
                        })
                        .collect::<std::collections::HashMap<_, _>>();
                for chapter in &mut audiobook.chapters {
                    if let Some(local) = local_tracks.get(&chapter.file_id) {
                        *chapter = local.clone();
                    }
                }
                if let Ok(mut cache) = self.audiobook_cache.lock() {
                    cache.insert(audiobook_id, audiobook.clone());
                }
                write_response(send, &ServerResponse::Audiobook { audiobook }).await
            }
            ClientRequest::RequestDownload {
                file_id,
                source_pubkeys,
                destination_folder,
            } => {
                let request_id = self
                    .network
                    .request_download(file_id, source_pubkeys, destination_folder)
                    .await?;
                write_response(send, &ServerResponse::DownloadRequested { request_id }).await
            }
            ClientRequest::Transfers => {
                let transfers = load_remote_transfers(&self.db_path)?;
                write_response(send, &ServerResponse::Transfers { transfers }).await
            }
            ClientRequest::FetchAudio { file_id } => {
                let track = local_track(&self.db_path, &file_id)?;
                let path = secure_audio_path(&self.db_path, &file_id)?;
                write_response(send, &ServerResponse::AudioReady { track }).await?;
                let mut file = tokio::fs::File::open(path)
                    .await
                    .map_err(|error| format!("could not open the audio: {error}"))?;
                let mut buffer = vec![0u8; 256 * 1024];
                loop {
                    check_request_permission(
                        self.authorise(remote_id)?,
                        &ClientRequest::FetchAudio {
                            file_id: file_id.clone(),
                        },
                    )?;
                    let count = file
                        .read(&mut buffer)
                        .await
                        .map_err(|error| format!("could not read the audio: {error}"))?;
                    if count == 0 {
                        break;
                    }
                    write_bytes(send, &buffer[..count]).await?;
                }
                Ok(())
            }
            ClientRequest::Available { file_ids } => {
                if file_ids.len() > MAX_PAGE_SIZE
                    || file_ids.iter().any(|file_id| !is_sha256_file_id(file_id))
                {
                    return Err("Invalid cached-file availability request".into());
                }
                let available = load_files_by_id(&open_connection(&self.db_path)?, &file_ids)?
                    .into_iter()
                    .map(|file| file.file_id)
                    .collect();
                write_response(
                    send,
                    &ServerResponse::Available {
                        file_ids: available,
                    },
                )
                .await
            }
            ClientRequest::LibraryByIds { file_ids } => {
                if file_ids.len() > MAX_TRACKS_BY_ID
                    || file_ids.iter().any(|file_id| !is_sha256_file_id(file_id))
                {
                    return Err("Invalid track lookup request".into());
                }
                // In the order asked for, and only the files this computer still
                // holds: a queue or playlist that names a file it no longer has
                // is answered with the rest of itself rather than with nothing.
                let tracks = load_files_by_id(&open_connection(&self.db_path)?, &file_ids)?
                    .into_iter()
                    .map(remote_track)
                    .collect();
                write_response(send, &ServerResponse::LibraryByIds { tracks }).await
            }
            ClientRequest::Playlists {
                offset,
                limit,
                own_only,
            } => {
                let connection = open_connection(&self.db_path)?;
                // The picker asks for this computer's own playlists alone: a
                // public one somebody else published is a playlist to play, not
                // a list to add a track to.
                let owned_by = if own_only {
                    Some(crate::network::own_pubkey()?)
                } else {
                    None
                };
                let (playlists, total) = crate::playlist::list_owned_by(
                    &connection,
                    owned_by.as_deref(),
                    offset,
                    limit.clamp(1, MAX_PAGE_SIZE),
                )?;
                write_response(send, &ServerResponse::Playlists { playlists, total }).await
            }
            ClientRequest::Playlist {
                author,
                playlist_id,
                offset,
                limit,
            } => {
                let page = crate::playlist::page(
                    &open_connection(&self.db_path)?,
                    &author,
                    &playlist_id,
                    offset,
                    limit.clamp(1, MAX_PLAYLIST_PAGE),
                )?;
                match page {
                    Some(playlist) => {
                        write_response(send, &ServerResponse::Playlist { playlist }).await
                    }
                    // A playlist this computer does not have is not an empty
                    // one: saying so is what lets a phone drop it from a list
                    // it is holding.
                    None => Err("That playlist is not on this computer".into()),
                }
            }
            ClientRequest::PlaylistsContaining { file_id } => {
                // The picker's ticks, answered in one go: one indexed lookup
                // here rather than a page of members for every playlist in the
                // list the phone is already showing.
                let playlists =
                    crate::playlist::containing(&open_connection(&self.db_path)?, &file_id)?;
                write_response(send, &ServerResponse::PlaylistsContaining { playlists }).await
            }
            ClientRequest::NewPlaylistId => {
                write_response(
                    send,
                    &ServerResponse::PlaylistId {
                        playlist_id: crate::playlist::new_playlist_id(),
                    },
                )
                .await
            }
            ClientRequest::SavePlaylist { playlist } => {
                let stored = self.save_playlist(playlist)?;
                write_response(send, &ServerResponse::Playlist { playlist: stored }).await
            }
            ClientRequest::PublishPlaylist {
                playlist,
                suggest_tags,
            } => {
                // Written down first, so a publication that fails at the relay
                // leaves the edit here rather than throwing it away: the phone
                // would otherwise lose whatever the author had just typed.
                let stored = self.save_playlist(playlist)?;
                let published = self.network.publish_playlist(&stored, suggest_tags).await?;
                write_response(
                    send,
                    &ServerResponse::Playlist {
                        playlist: published,
                    },
                )
                .await
            }
            ClientRequest::DeletePlaylist {
                author,
                playlist_id,
            } => {
                let connection = open_connection(&self.db_path)?;
                // An empty author means "the one this computer holds under this
                // id", which is the coordinate a phone was shown in the first
                // place. A withdrawal is a different act and is asked for
                // separately, because a relay has to be told about it.
                let author = if author.is_empty() {
                    match crate::playlist::page(&connection, "", &playlist_id, 0, 1)? {
                        Some(playlist) => playlist.author,
                        None => return Err("That playlist is not on this computer".into()),
                    }
                } else {
                    author
                };
                crate::playlist::remove(&connection, &author, &playlist_id)?;
                write_response(send, &ServerResponse::PlaylistRemoved).await
            }
            ClientRequest::WithdrawPlaylist { playlist_id } => {
                self.network.withdraw_playlist(&playlist_id).await?;
                write_response(send, &ServerResponse::PlaylistRemoved).await
            }
            ClientRequest::AlbumCovers { keys } => {
                if keys.len() > MAX_COVER_KEYS {
                    return Err("Too many album covers were requested at once".into());
                }
                // The rendering view, not the assertion view: a phone should
                // see the art this computer resolved for itself, exactly as the
                // desktop's own window does. Reporting one of those is refused
                // by `report_cover`, because there is no claim to report.
                let covers = self.network.best_known_covers(keys).await?;
                // ...and the part that makes a phone private: the hashes of the
                // pictures this computer can hand over, which is what it draws
                // from now. A rendition it is not holding yet is answered with
                // no hash, which is not a failure - the claim's own URL stays on
                // the cover, so the phone can tell "no art exists" from "this
                // computer has not fetched it yet" and keep the copy it already
                // has until the bytes arrive and the cover revision moves.
                let connection = open_connection(&self.db_path)?;
                let mut answer = Vec::with_capacity(covers.len());
                for cover in covers {
                    let mut remote = remote_album_cover(cover);
                    let (full, thumb) =
                        cached_art_hashes(&connection, &self.art_root, &remote.key)?;
                    // A hash is only ever reported for a picture this computer
                    // is still willing to show. The domain list already decided
                    // that `art` and `thumb` survive, and the bytes behind a
                    // hash are held to the same promise.
                    remote.art_hash = if remote.art.is_empty() {
                        String::new()
                    } else {
                        full
                    };
                    remote.thumb_hash = if remote.thumb.is_empty() {
                        String::new()
                    } else {
                        thumb
                    };
                    // ...and, having just been asked about exactly these
                    // albums, this is the moment to go and get the thumbnails that
                    // are missing. It is bounded and deduped by the fetcher, so a
                    // phone opening a screen is a small number of downloads
                    // rather than a small number of requests per album, and the
                    // answer above goes out without waiting for any of them.
                    answer.push(remote);
                }
                self.covers.ensure_art(
                    &answer
                        .iter()
                        .map(|cover| ArtWant {
                            key: &cover.key,
                            // Thumbnails only. Every tile and row on a phone draws
                            // one, and nothing on a screen of albums draws the
                            // full picture: a full one is asked for by the screen
                            // that draws it, per rendition, which is the only
                            // thing that knows whether it wants it.
                            art: "",
                            thumb: &cover.thumb,
                            source: &cover.source,
                        })
                        .collect::<Vec<_>>(),
                );
                write_response(send, &ServerResponse::AlbumCovers { covers: answer }).await
            }
            ClientRequest::TrackDiscussion { file_id, before } => {
                if !is_sha256_file_id(&file_id.trim()) {
                    return Err("That track has no valid file ID".into());
                }
                // A cursor rather than an offset, because a conversation grows at
                // the end: the oldest message of the last page is what the phone
                // asks from, and the boundary stays inclusive so a message written
                // in that same second is not skipped.
                let cursor = before.map(network::PublicChatCursor::written_before);
                let messages = self
                    .network
                    .track_discussion_messages(file_id.trim().to_string(), false, cursor)
                    .await?
                    .into_iter()
                    .map(remote_discussion_message)
                    .collect();
                write_response(send, &ServerResponse::TrackDiscussion { messages }).await
            }
            ClientRequest::SendTrackDiscussion {
                file_id,
                content,
                reply_to,
            } => {
                if !is_sha256_file_id(&file_id.trim()) {
                    return Err("That track has no valid file ID".into());
                }
                if content.trim().is_empty() || content.chars().count() > MAX_DISCUSSION_CHARS {
                    return Err(format!(
                        "Comments are between 1 and {MAX_DISCUSSION_CHARS} characters"
                    ));
                }
                // Signed and published by the computer's identity, so this speaks
                // in the user's own name on a public relay. The network service
                // announces the result itself, so the desktop's own chat view
                // hears about a comment sent from a phone.
                let event_id = self
                    .network
                    .send_track_discussion_message(
                        file_id.trim().to_string(),
                        content,
                        reply_to,
                    )
                    .await?;
                write_response(send, &ServerResponse::TrackDiscussionSent { event_id }).await
            }
            ClientRequest::TrackDiscussionActivity { file_ids } => {
                if file_ids.len() > MAX_DISCUSSION_ACTIVITY_IDS {
                    return Err("Too many files were asked about at once".into());
                }
                let activity = self
                    .network
                    .track_discussion_activity(file_ids)
                    .await?
                    .into_iter()
                    .map(|row| RemoteDiscussionActivity {
                        file_id: row.file_id,
                        authors: row.authors.min(u32::MAX as usize) as u32,
                        messages: row.messages.min(u32::MAX as usize) as u32,
                        last_at: row.last_at,
                    })
                    .collect();
                write_response(send, &ServerResponse::TrackDiscussionActivity { activity }).await
            }
            ClientRequest::FetchArt { key, rendition } => {
                if key.is_empty() || key.chars().count() > MAX_ART_KEY_CHARS {
                    return Err("Invalid art request".into());
                }
                let connection = open_connection(&self.db_path)?;
                let Some(art) =
                    crate::art_cache::lookup(&connection, &self.art_root, &key, rendition)?
                else {
                    // A considered answer, not an error: a phone may ask about an
                    // album this computer has not fetched art for yet, and the
                    // right thing for it to do is paint its placeholder and ask
                    // again when the cover revision moves.
                    //
                    // Asking is also how this computer learns which rendition a
                    // screen wants, which is the one thing a phone knows and a
                    // host cannot: a thumbnail is drawn by every row that comes
                    // on screen, and the full picture only by an album somebody
                    // engaged with. So the ask is taken as the request it is - in
                    // the background, because the phone is waiting for an answer
                    // and not for a download - and the bytes landing are what move
                    // that revision.
                    let covers = self.covers.clone();
                    let wanted = key.clone();
                    tauri::async_runtime::spawn(async move {
                        covers.ensure_rendition(&wanted, rendition).await;
                    });
                    return write_response(send, &ServerResponse::ArtMissing { key }).await;
                };
                // Handing them out is what "used" means, and it is what eviction
                // orders by — so a library's own art outlives art for albums this
                // computer merely browsed past.
                let _ = crate::art_cache::touch(&connection, &key, rendition);
                write_response(
                    send,
                    &ServerResponse::ArtReady {
                        key: key.clone(),
                        rendition,
                        hash: art.hash.clone(),
                        mime: art.mime.clone(),
                        length: art.bytes,
                    },
                )
                .await?;
                let mut file = tokio::fs::File::open(&art.path)
                    .await
                    .map_err(|error| format!("could not open the artwork: {error}"))?;
                let mut buffer = vec![0u8; ART_CHUNK_BYTES];
                loop {
                    // Asked again for every chunk, exactly as audio is: a pairing
                    // that is revoked mid-transfer stops at the next frame rather
                    // than at the end of the file.
                    check_request_permission(
                        self.authorise(remote_id)?,
                        &ClientRequest::FetchArt {
                            key: key.clone(),
                            rendition,
                        },
                    )?;
                    let count = file
                        .read(&mut buffer)
                        .await
                        .map_err(|error| format!("could not read the artwork: {error}"))?;
                    if count == 0 {
                        break;
                    }
                    write_bytes(send, &buffer[..count]).await?;
                }
                Ok(())
            }
            ClientRequest::Status => {
                write_response(
                    send,
                    &ServerResponse::Status {
                        library_revision: library_revision(&self.db_path)?,
                        cover_revision: cover_revision(&self.db_path)?,
                        stream_only: rights.is_read_only(),
                        rights: Some(rights),
                        // Who this computer is, which is the only way the phone
                        // can tell its own playlists from public ones. It is a
                        // public key: nothing secret travels, and the phone
                        // holds no key of its own to compare against otherwise.
                        pubkey: self
                            .network
                            .own_pubkey()
                            .await
                            .unwrap_or_default(),
                    },
                )
                .await
            }
            ClientRequest::Playback { command } => {
                let state = self
                    .playback
                    .apply(&self.db_path, bounded_playback(command)?)?;
                write_response(send, &ServerResponse::Playback { state }).await
            }
            ClientRequest::PlaybackState => {
                write_response(
                    send,
                    &ServerResponse::Playback {
                        state: self.playback.state(&self.db_path),
                    },
                )
                .await
            }
            ClientRequest::ReadOnlyTicket => {
                // Only a read-only code can come out of here, which is what lets
                // a phone with write access lend its access on without ever
                // widening it: whoever scans this browses and plays, no more.
                let offer = self.issue_pairing(DeviceRights::read_only()).await?;
                let desktop_name = self.desktop_name();
                let mut qr_svg = offer.qr_svg;
                let candidate = ServerResponse::ReadOnlyTicket {
                    uri: offer.ticket.clone(),
                    qr_svg: qr_svg.clone(),
                    expires_at: offer.expires_at,
                    desktop_name: desktop_name.clone(),
                };
                if serde_json::to_vec(&candidate)
                    .map(|encoded| encoded.len())
                    .unwrap_or(usize::MAX)
                    > MAX_CONTROL_FRAME_BYTES
                {
                    // The QR is around a hundred kilobytes of path data. Drop
                    // the image rather than failing the request that carries
                    // the code itself.
                    qr_svg.clear();
                }
                write_response(
                    send,
                    &ServerResponse::ReadOnlyTicket {
                        uri: offer.ticket,
                        qr_svg,
                        expires_at: offer.expires_at,
                        desktop_name,
                    },
                )
                .await
            }
            ClientRequest::ReportCover { key, reason, note } => {
                let report_id = self.network.report_cover(key, reason, note).await?;
                write_response(
                    send,
                    &ServerResponse::CoverReported {
                        report: CoverReportResult {
                            report_id,
                            queued: false,
                        },
                    },
                )
                .await
            }
            ClientRequest::Ping => write_response(send, &ServerResponse::Pong).await,
            ClientRequest::Pair { .. } => unreachable!(),
        }
    }

    fn accept_pairing(
        &self,
        remote_id: &str,
        token: &str,
        name: &str,
    ) -> Result<DeviceRights, String> {
        let mut pairing = self
            .pairing
            .lock()
            .map_err(|_| "pairing state lock was poisoned")?;
        accept_pairing(
            &self.db_path,
            &mut pairing,
            remote_id,
            token,
            name,
        )
    }

    /// What this device may do, which is also what proves it is paired at all:
    /// there is no such thing as a device in the table with no grant, because a
    /// grant of nothing is a device that can do nothing.
    fn authorise(&self, remote_id: &str) -> Result<DeviceRights, String> {
        device_rights(&self.db_path, remote_id)
    }

    fn touch_device(&self, remote_id: &str) {
        let now = Instant::now();
        if let Ok(mut updates) = self.last_seen_updates.lock() {
            if updates
                .get(remote_id)
                .is_some_and(|updated| now.duration_since(*updated) < Duration::from_secs(60))
            {
                return;
            }
            updates.insert(remote_id.to_string(), now);
        }
        if let Ok(connection) = open_connection(&self.db_path) {
            let _ = connection.execute(
                "UPDATE mobile_devices SET last_seen=?1 WHERE endpoint_id=?2",
                params![Utc::now().to_rfc3339(), remote_id],
            );
        }
    }
}

fn accept_pairing(
    db_path: &Path,
    pairing: &mut Vec<PairingSession>,
    remote_id: &str,
    token: &str,
    name: &str,
) -> Result<DeviceRights, String> {
    let now = Utc::now();
    pairing.retain(|session| session.expires_at >= now.timestamp());
    let index = pairing
        .iter()
        .position(|session| session.token.as_bytes() == token.as_bytes())
        .ok_or("The pairing code is invalid or expired")?;
    // The grant travels with the code that was scanned, and a code minted by a
    // phone can only ever carry the lent gift: see `ReadOnlyTicket`.
    let rights = pairing[index].rights;
    let endpoint = remote_id
        .parse::<iroh::EndpointId>()
        .map_err(|_| "invalid mobile Iroh identity")?;
    let name = clean_device_name(name);
    let connection = open_connection(db_path)?;
    connection
            .execute(
                "INSERT INTO mobile_devices(endpoint_id,name,paired_at,last_seen,stream_only,rights)
                 VALUES(?1,?2,?3,?3,?4,?5)
                 ON CONFLICT(endpoint_id) DO UPDATE SET name=excluded.name,last_seen=excluded.last_seen,stream_only=excluded.stream_only,rights=excluded.rights",
                params![
                    endpoint.to_string(),
                    name,
                    now.to_rfc3339(),
                    rights.is_read_only(),
                    rights.bits() as i64
                ],
            )
            .map_err(|error| error.to_string())?;
    pairing.remove(index);
    Ok(rights)
}

/// What this device may do. `None` for the column means a row written before
/// grants existed and read without the migration having run: the old boolean's
/// default was the owner's own device, so that is what it reads as.
fn device_rights(db_path: &Path, remote_id: &str) -> Result<DeviceRights, String> {
    let stored = open_connection(db_path)?
        .query_row(
            "SELECT rights FROM mobile_devices WHERE endpoint_id=?1",
            [remote_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    match stored {
        None => Err("This phone is not paired with Napstr".into()),
        Some(None) => Ok(DeviceRights::full()),
        Some(Some(bits)) => Ok(DeviceRights::from_bits(bits as u32)),
    }
}

/// Write a device's grant. One place, so the window, any future menu and the
/// tests all go through the same rule: the endpoint must be one this computer
/// paired with, because writing a row here is what makes a device real, and a
/// device that was never paired must not become one by being written.
fn set_device_rights(
    db_path: &Path,
    endpoint_id: &str,
    rights: DeviceRights,
) -> Result<(), String> {
    let parsed = endpoint_id
        .parse::<iroh::EndpointId>()
        .map_err(|_| "invalid Iroh endpoint ID")?;
    let changed = open_connection(db_path)?
        .execute(
            "UPDATE mobile_devices SET stream_only=?2, rights=?3 WHERE endpoint_id=?1",
            params![
                parsed.to_string(),
                rights.is_read_only(),
                rights.bits() as i64
            ],
        )
        .map_err(|error| error.to_string())?;
    if changed == 0 {
        return Err("That device is not paired with Napstr".into());
    }
    Ok(())
}

/// What one request needs, and what to say when the device was not given it.
///
/// Reads come first because they are the larger half: a device lent only the
/// index may still search it, read its playlists, covers and comments, and watch
/// what this computer is playing without being able to change any of it.
fn check_request_permission(rights: DeviceRights, request: &ClientRequest) -> Result<(), String> {
    if rights.is_full() {
        // The owner's own device. Nothing to weigh.
        return Ok(());
    }
    match request {
        // Reading this computer's index, and the public things attached to it.
        ClientRequest::Library { .. }
        | ClientRequest::LibraryByIds { .. }
        | ClientRequest::Search { .. }
        | ClientRequest::Audiobooks { .. }
        | ClientRequest::AudiobookLibrary { .. }
        | ClientRequest::Audiobook { .. }
        | ClientRequest::Available { .. }
        // A playlist is a list of names, and reading it is reading the library.
        | ClientRequest::Playlists { .. }
        | ClientRequest::Playlist { .. }
        // Which playlists hold a file is one more way of reading a playlist.
        | ClientRequest::PlaylistsContaining { .. }
        | ClientRequest::AlbumCovers { .. }
        // A conversation is public and reading it is a read. Posting is not: it
        // is published in the user's own name, which is why it is not here.
        | ClientRequest::TrackDiscussion { .. }
        | ClientRequest::TrackDiscussionActivity { .. }
        // Art is a read, and a device lent the index should see covers: the bytes
        // come from this computer, so this reveals nothing the device could not
        // already ask for by key.
        | ClientRequest::FetchArt { .. }
        // Seeing what the computer is playing is not a way of changing it.
        | ClientRequest::PlaybackState => require(
            rights,
            DeviceRights::BROWSE,
            "This device was not given access to this computer's library.",
        ),
        // The channel's own business rather than the library's: a device has to be
        // able to ask where it is and what it may do, or a grant of nothing would
        // look like a computer that is not there at all.
        ClientRequest::Status | ClientRequest::Ping => Ok(()),
        ClientRequest::FetchAudio { .. } => require(
            rights,
            DeviceRights::FETCH,
            "This device was not given access to this computer's audio.",
        ),
        ClientRequest::Playback { .. } => require(
            rights,
            DeviceRights::CONTROL,
            "This phone has read-only access, so it cannot control Napstr on the computer.",
        ),
        // A playlist a phone edits is the same playlist the author could edit on
        // the computer, so browsing access cannot reach any of these: an id for
        // a playlist that does not exist yet included, because a listing minted
        // there would only be a promise this phone could not keep.
        ClientRequest::NewPlaylistId
        | ClientRequest::SavePlaylist { .. }
        | ClientRequest::DeletePlaylist { .. }
        | ClientRequest::PublishPlaylist { .. }
        | ClientRequest::WithdrawPlaylist { .. } => require(
            rights,
            DeviceRights::PRIVILEGED,
            "This phone has read-only access, so it cannot create or edit playlists.",
        ),
        ClientRequest::ReadOnlyTicket => require(
            rights,
            DeviceRights::PRIVILEGED,
            "This phone has read-only access, so it cannot lend access to another device.",
        ),
        ClientRequest::ReportCover { .. } => require(
            rights,
            DeviceRights::PRIVILEGED,
            "This phone has read-only access, so it cannot publish reports.",
        ),
        // A comment is signed with the user's own key and published to public
        // relays, under their name. That is an act rather than a read, and it is
        // the same line read-only access already draws for reports and playlists.
        ClientRequest::SendTrackDiscussion { .. } => require(
            rights,
            DeviceRights::PRIVILEGED,
            "This phone has read-only access, so it cannot post comments.",
        ),
        // Asking this computer to fetch something, or to account for what it is
        // fetching, is acting through it rather than reading it.
        ClientRequest::RequestDownload { .. } | ClientRequest::Transfers => require(
            rights,
            DeviceRights::PRIVILEGED,
            "This phone has read-only access. Downloads on the Napstr host are not permitted.",
        ),
        // Pairing is answered before this is reached, and a code is not a grant,
        // so there is nothing here to weigh.
        ClientRequest::Pair { .. } => {
            Err("Pairing is not a request a paired device makes.".into())
        }
    }
}

fn require(rights: DeviceRights, right: u32, message: &str) -> Result<(), String> {
    if rights.grants(right) {
        Ok(())
    } else {
        Err(message.to_string())
    }
}

fn is_sha256_file_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A playback command with anything a phone should not be able to say removed.
///
/// A queue a phone sends is a list it built from what it could see, so entries
/// that are not file ids are dropped rather than refused: the queue still makes
/// sense without them, and refusing the whole request would be a worse answer
/// than playing the part of it that does. The track being asked for is not
/// dropped, because a request that cannot play its own track is a broken one.
fn bounded_playback(command: PlaybackCommand) -> Result<PlaybackCommand, String> {
    match command {
        PlaybackCommand::PlayTrack {
            file_id,
            queue,
            position_ms,
        } => {
            if !is_sha256_file_id(&file_id) {
                return Err("That is not a track this computer can look up".into());
            }
            if queue.len() > MAX_PLAY_QUEUE {
                return Err(format!(
                    "A queue of more than {MAX_PLAY_QUEUE} tracks is more than one request can carry"
                ));
            }
            Ok(PlaybackCommand::PlayTrack {
                file_id,
                queue: queue
                    .into_iter()
                    .filter(|candidate| is_sha256_file_id(candidate))
                    .collect(),
                // Clamped rather than refused: a position past any track is a
                // broken sender, but playing the track from the beginning of it
                // is a better answer than an error the phone cannot act on.
                position_ms: position_ms.min(MAX_POSITION_MS),
            })
        }
        other => Ok(other),
    }
}

/// The hashes of the art this computer is holding for an album: the full
/// rendition first, the smaller one second. Either is empty when that rendition
/// has not been fetched here yet.
fn cached_art_hashes(
    connection: &rusqlite::Connection,
    root: &std::path::Path,
    key: &str,
) -> Result<(String, String), String> {
    let hash = |rendition| -> Result<String, String> {
        Ok(crate::art_cache::lookup(connection, root, key, rendition)?
            .map(|art| art.hash)
            .unwrap_or_default())
    };
    Ok((hash(ArtRendition::Full)?, hash(ArtRendition::Thumb)?))
}

/// Trim the host's bookkeeping (event ids, timestamps) from a resolved cover
/// before it crosses the wire.
/// A public message as the phone draws it.
///
/// The name and the npub come resolved rather than empty: a phone holds no Nostr
/// identity, so it cannot ask a relay for a profile, and the host already keeps
/// the names it has seen for the conversation it is rendering.
fn remote_discussion_message(
    message: crate::network::TrollboxMessage,
) -> RemoteDiscussionMessage {
    RemoteDiscussionMessage {
        event_id: message.event_id,
        pubkey: message.pubkey,
        npub: message.npub,
        display_name: message.display_name,
        content: message.content,
        created_at: message.created_at,
        reply_to: message.reply_to,
        reply: message.reply.map(|reply| RemoteDiscussionReply {
            author: reply.author,
            excerpt: reply.excerpt,
        }),
    }
}

fn remote_album_cover(cover: crate::network::AlbumCover) -> RemoteAlbumCover {
    RemoteAlbumCover {
        key: cover.key,
        art: cover.art,
        thumb: cover.thumb,
        art_hash: String::new(),
        thumb_hash: String::new(),
        mbid: cover.mbid,
        year: cover.year,
        genre: cover.genre,
        collection: cover.collection,
        source: cover.source,
        cover_file_id: cover.cover_file_id,
        mime: cover.mime,
        author: cover.author,
        seeder: cover.seeder,
    }
}

fn remote_audiobook(
    book: crate::network::AudiobookResult,
    local_tracks: &std::collections::HashMap<String, RemoteTrack>,
) -> RemoteAudiobook {
    let sources = book
        .sources
        .iter()
        .map(|source| RemoteSource {
            pubkey: source.pubkey.clone(),
            display_name: source.display_name.clone(),
        })
        .collect::<Vec<_>>();
    let chapters = book
        .chapters
        .into_iter()
        .map(|chapter| {
            local_tracks
                .get(&chapter.file_id)
                .cloned()
                .unwrap_or_else(|| RemoteTrack {
                    file_id: chapter.file_id,
                    filename: chapter.filename,
                    title: chapter.title,
                    artist: book.author.clone(),
                    album: book.title.clone(),
                    format: chapter.format,
                    mime: chapter.mime,
                    size: chapter.size,
                    tags: "audiobook".into(),
                    local: false,
                    sources: sources.clone(),
                    // A chapter the library does not hold is only known by its
                    // manifest, which says nothing about the audio in it.
                    bitrate_kbps: 0,
                    sample_rate_hz: 0,
                    channels: 0,
                    lossless: false,
                    duration_ms: 0,
                })
        })
        .collect();
    RemoteAudiobook {
        audiobook_id: book.audiobook_id,
        title: book.title,
        author: book.author,
        narrator: book.narrator,
        total_size: book.total_size,
        chapters,
    }
}

fn load_local_remote_audiobooks(
    db_path: &Path,
) -> Result<
    (
        Vec<RemoteAudiobook>,
        std::collections::HashMap<String, RemoteTrack>,
    ),
    String,
> {
    let connection = open_connection(db_path)?;
    let local_tracks = load_files(&connection, None)?
        .into_iter()
        .map(|file| {
            let file_id = file.file_id.clone();
            (file_id, remote_track(file))
        })
        .collect::<std::collections::HashMap<_, _>>();
    let audiobooks = build_local_audiobooks(&connection)?
        .into_iter()
        .map(|book| remote_audiobook(book, &local_tracks))
        .collect();
    Ok((audiobooks, local_tracks))
}

fn initialise_schema(db_path: &Path) -> Result<(), String> {
    let connection = open_connection(db_path)?;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS mobile_devices (
               endpoint_id TEXT PRIMARY KEY,
               name TEXT NOT NULL,
               paired_at TEXT NOT NULL,
               last_seen TEXT NOT NULL
             );",
        )
        .map_err(|error| error.to_string())?;
    let has_permission: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('mobile_devices') WHERE name='stream_only')",
        [], |row| row.get(0),
    ).map_err(|error| error.to_string())?;
    if !has_permission {
        connection.execute_batch("ALTER TABLE mobile_devices ADD COLUMN stream_only INTEGER NOT NULL DEFAULT 0 CHECK(stream_only IN (0,1));")
            .map_err(|error| error.to_string())?;
    }
    // A device's grant, held as one integer. `stream_only` above is the summary of
    // it that a companion older than grants still reads.
    let has_rights: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('mobile_devices') WHERE name='rights')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !has_rights {
        connection
            .execute_batch("ALTER TABLE mobile_devices ADD COLUMN rights INTEGER;")
            .map_err(|error| error.to_string())?;
        // Every device paired before this had the boolean, and it meant exactly one
        // of two things: browse and play, or the owner's own.
        connection
            .execute_batch(&format!(
                "UPDATE mobile_devices SET rights = CASE WHEN stream_only = 1 THEN {} ELSE {} END WHERE rights IS NULL;",
                DeviceRights::read_only().bits(),
                DeviceRights::full().bits()
            ))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn load_devices(db_path: &Path) -> Result<Vec<PairedDevice>, String> {
    let connection = open_connection(db_path)?;
    let mut statement = connection
        .prepare(
            "SELECT endpoint_id,name,paired_at,last_seen,rights FROM mobile_devices ORDER BY last_seen DESC",
        )
        .map_err(|error| error.to_string())?;
    let devices = statement
        .query_map([], |row| {
            Ok(PairedDevice {
                endpoint_id: row.get(0)?,
                name: row.get(1)?,
                paired_at: row.get(2)?,
                last_seen: row.get(3)?,
                // A row written before grants existed reads as the owner's own,
                // which is what the old boolean's default meant.
                rights: DeviceRights::from_bits(
                    row.get::<_, Option<i64>>(4)?
                        .unwrap_or(DeviceRights::FULL as i64) as u32,
                ),
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(devices)
}

#[cfg(test)]
fn load_library(
    db_path: &Path,
    query: &str,
    offset: usize,
    limit: usize,
) -> Result<(Vec<RemoteTrack>, usize, std::collections::HashSet<String>), String> {
    let (tracks, audiobook_chapter_ids) = build_music_library(db_path)?;
    let (page, total) = page_music_library(&tracks, query, offset, limit);
    Ok((page, total, audiobook_chapter_ids))
}

fn build_music_library(
    db_path: &Path,
) -> Result<(Vec<RemoteTrack>, std::collections::HashSet<String>), String> {
    let connection = open_connection(db_path)?;
    let files = load_files(&connection, None)?;
    let audiobook_chapter_ids = build_local_audiobooks_from_files(&connection, &files)?
        .into_iter()
        .flat_map(|book| book.chapters.into_iter().map(|chapter| chapter.file_id))
        .collect::<std::collections::HashSet<_>>();
    let tracks = files
        .into_iter()
        .filter(|file| !audiobook_chapter_ids.contains(&file.file_id))
        .map(remote_track)
        .collect::<Vec<_>>();
    Ok((tracks, audiobook_chapter_ids))
}

/// One page of the library in the order one seed produces.
///
/// The order is a property of the seed and the file ids rather than of anything
/// kept here: a file's key depends on its own id alone, so paging a hundred at a
/// time walks one permutation from beginning to end, and the same seed asks for
/// the same order however long ago it was minted. That is what makes a phone's
/// browse a shuffle without the host holding a list per device, and what keeps a
/// desktop restart from reshuffling a list the phone is halfway down.
fn page_shuffled_music_library(
    tracks: &[RemoteTrack],
    seed: u64,
    offset: usize,
    limit: usize,
) -> (Vec<RemoteTrack>, usize) {
    let mut order = (0..tracks.len()).collect::<Vec<_>>();
    order.sort_by_cached_key(|index| shuffle_key(seed, &tracks[*index].file_id));
    let page = order
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|index| tracks[index].clone())
        .collect::<Vec<_>>();
    (page, tracks.len())
}

/// Where one file falls in a seeded order.
///
/// A hash of the seed and the file id, truncated: the file id is already a
/// hash, so any mixing would do, and hashing the two together is the version of
/// "any mixing" that can be checked by reading it.
fn shuffle_key(seed: u64, file_id: &str) -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update(seed.to_le_bytes());
    hasher.update(file_id.as_bytes());
    let digest = hasher.finalize();
    let mut key = [0u8; 8];
    key.copy_from_slice(&digest[..8]);
    key
}

fn page_music_library(
    tracks: &[RemoteTrack],
    query: &str,
    offset: usize,
    limit: usize,
) -> (Vec<RemoteTrack>, usize) {
    let mut page = Vec::with_capacity(limit.min(tracks.len()));
    let mut total = 0usize;
    for track in tracks {
        if !search_matches(
            query,
            &[
                &track.filename,
                &track.title,
                &track.artist,
                &track.album,
                &track.tags,
            ],
        ) {
            continue;
        }
        if total >= offset && page.len() < limit {
            page.push(track.clone());
        }
        total += 1;
    }
    (page, total)
}

fn library_revision(db_path: &Path) -> Result<u64, String> {
    let revision = open_connection(db_path)?
        .query_row("SELECT revision FROM library_state WHERE id=1", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| error.to_string())?;
    Ok(revision.max(0) as u64)
}

/// How many times the art this host would report has changed.
///
/// A phone caches covers, including "this host has none", so this is what tells
/// it that a cached answer may be stale rather than making it guess or ask
/// again on every render.
fn cover_revision(db_path: &Path) -> Result<u64, String> {
    let connection = open_connection(db_path)?;
    crate::cover::cover_revision(&connection)
}

/// One catalogue row as the wire describes it.
///
/// Every file this produces is one this computer holds, so `local` is always
/// true and no seeders are named: a track this computer does not hold is
/// described by the catalogue's own copy of it instead.
fn remote_track(file: SharedFile) -> RemoteTrack {
    RemoteTrack {
        file_id: file.file_id,
        filename: file.filename,
        title: file.title,
        artist: file.artist,
        album: file.album,
        format: file.format,
        mime: file.mime,
        size: file.size,
        tags: file.tags,
        local: true,
        sources: Vec::new(),
        bitrate_kbps: file.bitrate_kbps,
        sample_rate_hz: file.sample_rate_hz,
        channels: file.channels,
        lossless: file.lossless,
        duration_ms: file.duration_ms,
    }
}

fn local_track(db_path: &Path, file_id: &str) -> Result<RemoteTrack, String> {
    load_files_by_id(&open_connection(db_path)?, &[file_id.to_string()])?
        .into_iter()
        .next()
        .map(remote_track)
        .ok_or_else(|| "This track is no longer in the Napstr folder".into())
}

/// The same record, for callers that only want to describe what they are
/// playing.
///
/// The playback bridge needs it in both directions of a handoff: a phone can
/// only take playback over if it is told the size, format and MIME of what the
/// computer is playing, because fetching the audio is what needs them.
/// A computer that no longer holds the track answers `None`, and the phone shows
/// what the state already says about it.
pub(crate) fn local_track_for(db_path: &Path, file_id: &str) -> Option<RemoteTrack> {
    local_track(db_path, file_id).ok()
}

fn secure_audio_path(db_path: &Path, file_id: &str) -> Result<PathBuf, String> {
    if hex::decode(file_id)
        .map(|bytes| bytes.len() == 32)
        .unwrap_or(false)
        == false
    {
        return Err("invalid SHA-256 file ID".into());
    }
    let connection = open_connection(db_path)?;
    let root = crate::get_setting(&connection, "shared_folder")?;
    let path: Option<String> = connection
        .query_row(
            "SELECT path FROM files WHERE file_id=?1 AND format IN ('MP3','FLAC','WAV','OGG','OPUS')",
            [file_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let root = PathBuf::from(root)
        .canonicalize()
        .map_err(|_| "The Napstr folder is unavailable")?;
    let path = PathBuf::from(path.ok_or("This track is no longer available")?)
        .canonicalize()
        .map_err(|_| "This track is no longer available")?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err("Napstr refused a path outside the selected folder".into());
    }
    Ok(path)
}

fn load_remote_transfers(db_path: &Path) -> Result<Vec<RemoteTransfer>, String> {
    Ok(load_transfers(&open_connection(db_path)?)?
        .into_iter()
        .map(|transfer| RemoteTransfer {
            id: transfer.id.to_string(),
            file_id: transfer.file_id,
            filename: transfer.filename,
            size: transfer.size,
            progress: transfer.progress,
            status: transfer.status,
            speed: transfer.speed,
        })
        .collect())
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

async fn read_request(receive: &mut iroh::endpoint::RecvStream) -> Result<ClientRequest, String> {
    let mut length = [0u8; 4];
    receive
        .read_exact(&mut length)
        .await
        .map_err(|error| format!("could not read request length: {error}"))?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_CONTROL_FRAME_BYTES {
        return Err("invalid Napstrfy request size".into());
    }
    let mut payload = vec![0u8; length];
    receive
        .read_exact(&mut payload)
        .await
        .map_err(|error| format!("could not read request: {error}"))?;
    serde_json::from_slice(&payload).map_err(|_| "invalid Napstrfy request".into())
}

async fn write_response(
    send: &mut iroh::endpoint::SendStream,
    response: &ServerResponse,
) -> Result<(), String> {
    let payload = serde_json::to_vec(response).map_err(|error| error.to_string())?;
    if payload.len() > MAX_CONTROL_FRAME_BYTES {
        return Err("Napstrfy response is too large".into());
    }
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&payload);
    write_bytes(send, &frame).await
}

async fn write_bytes(send: &mut iroh::endpoint::SendStream, bytes: &[u8]) -> Result<(), String> {
    let result = write_with_timeout(send, bytes, RESPONSE_WRITE_TIMEOUT).await;
    if result.is_err() {
        // A cancelled write may have sent part of a frame. Reset it instead of
        // appending an error frame or waiting again on the same blocked stream.
        let _ = send.reset(1u32.into());
    }
    result
}

async fn write_with_timeout<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    bytes: &[u8],
    timeout: Duration,
) -> Result<(), String> {
    tokio::time::timeout(timeout, tokio::io::AsyncWriteExt::write_all(writer, bytes))
        .await
        .map_err(|_| "Napstrfy stopped reading the response".to_string())?
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stalled_response_writes_release_request_slots() {
        let slots = Arc::new(tokio::sync::Semaphore::new(32));
        let mut readers = Vec::new();
        let mut requests = Vec::new();
        for _ in 0..32 {
            let permit = slots.clone().try_acquire_owned().unwrap();
            let (mut writer, reader) = tokio::io::duplex(1);
            readers.push(reader); // Connected peers deliberately never read.
            requests.push(tokio::spawn(async move {
                let _permit = permit;
                write_with_timeout(&mut writer, b"response", Duration::from_millis(50)).await
            }));
        }
        assert_eq!(slots.available_permits(), 0);
        tokio::time::timeout(Duration::from_secs(2), async {
            for request in requests {
                assert!(request.await.unwrap().unwrap_err().contains("stopped reading"));
            }
        }).await.unwrap();
        assert_eq!(slots.available_permits(), 32);
        drop(readers);
    }

    #[tokio::test]
    async fn response_writes_preserve_bytes_for_reading_clients() {
        let (mut writer, mut reader) = tokio::io::duplex(1);
        let received = tokio::spawn(async move {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).await.unwrap();
            bytes
        });
        write_with_timeout(&mut writer, b"complete response", Duration::from_secs(1)).await.unwrap();
        drop(writer);
        assert_eq!(received.await.unwrap(), b"complete response");
    }

    #[test]
    fn browsing_access_can_cache_but_cannot_download_on_host_or_inspect_transfers() {
        for request in [
            ClientRequest::RequestDownload {
                file_id: "a".repeat(64),
                source_pubkeys: vec![],
                destination_folder: None,
            },
            ClientRequest::Transfers,
        ] {
            assert!(check_request_permission(DeviceRights::read_only(), &request).is_err());
            assert!(check_request_permission(DeviceRights::full(), &request).is_ok());
        }
        assert!(check_request_permission(
            DeviceRights::read_only(),
            &ClientRequest::FetchAudio { file_id: "a".repeat(64) }
        ).is_ok());
        assert!(check_request_permission(
            DeviceRights::read_only(),
            &ClientRequest::Library {
                query: String::new(),
                offset: 0,
                limit: 100,
                shuffle_seed: Some(7)
            }
        )
        .is_ok());
    }

    /// The three rights that are not "may read" are separable, which is the whole
    /// point of naming them: a device can be lent the index without the audio,
    /// or the audio without the owner's identity, and each refusal says which
    /// right is missing rather than describing the device as read-only.
    #[test]
    fn each_right_is_enforced_on_its_own() {
        let audio = ClientRequest::FetchAudio {
            file_id: "a".repeat(64),
        };
        let library = ClientRequest::Library {
            query: String::new(),
            offset: 0,
            limit: 100,
            shuffle_seed: None,
        };
        let control = ClientRequest::Playback {
            command: PlaybackCommand::Toggle,
        };
        let publish = ClientRequest::NewPlaylistId;
        let comment = ClientRequest::SendTrackDiscussion {
            file_id: "a".repeat(64),
            content: "Nice track".into(),
            reply_to: None,
        };

        // Browse only: the index, and nothing behind it.
        let index_only = DeviceRights {
            browse: true,
            fetch: false,
            control: false,
            privileged: false,
        };
        assert!(check_request_permission(index_only, &library).is_ok());
        assert!(check_request_permission(index_only, &audio).is_err());
        assert!(check_request_permission(index_only, &control).is_err());
        assert!(check_request_permission(index_only, &publish).is_err());

        // Browse and play: what a lent phone has always been given.
        let lent = DeviceRights::read_only();
        assert!(check_request_permission(lent, &audio).is_ok());
        assert!(check_request_permission(lent, &control).is_err());
        assert!(check_request_permission(lent, &comment).is_err());

        // Browse, play and drive this computer, but not to act as its owner.
        let remote = DeviceRights {
            browse: true,
            fetch: true,
            control: true,
            privileged: false,
        };
        assert!(check_request_permission(remote, &control).is_ok());
        assert!(check_request_permission(remote, &publish).is_err());
        assert!(check_request_permission(remote, &comment).is_err());

        // Nothing at all is a device that may only ask who it is talking to.
        let nothing = DeviceRights::default();
        assert!(check_request_permission(nothing, &library).is_err());
        assert!(check_request_permission(nothing, &ClientRequest::Ping).is_ok());
        assert!(check_request_permission(nothing, &ClientRequest::Status).is_ok());
    }

    /// A grant is a property of the device now, so changing your mind must not
    /// mean pairing again - and an endpoint that was never paired must not be
    /// writable, or a row could be created without a code.
    #[test]
    fn a_devices_rights_can_be_changed_without_pairing_again() {
        let directory =
            std::env::temp_dir().join(format!("napstr-rights-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let db = directory.join("napstr.sqlite3");
        initialise_schema(&db).unwrap();
        let endpoint = SecretKey::generate().public().to_string();

        assert!(set_device_rights(&db, &endpoint, DeviceRights::read_only()).is_err());
        assert!(set_device_rights(&db, "not-an-endpoint", DeviceRights::read_only()).is_err());

        let mut sessions = vec![PairingSession {
            token: "code".into(),
            rights: DeviceRights::read_only(),
            expires_at: Utc::now().timestamp() + 60,
        }];
        assert_eq!(
            accept_pairing(&db, &mut sessions, &endpoint, "code", "Ada's phone").unwrap(),
            DeviceRights::read_only()
        );
        set_device_rights(&db, &endpoint, DeviceRights::full()).unwrap();
        assert_eq!(device_rights(&db, &endpoint).unwrap(), DeviceRights::full());
        assert_eq!(
            load_devices(&db).unwrap().first().unwrap().rights,
            DeviceRights::full()
        );
        set_device_rights(&db, &endpoint, DeviceRights::default()).unwrap();
        assert_eq!(device_rights(&db, &endpoint).unwrap(), DeviceRights::default());
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_read_only_phone_may_read_a_conversation_but_not_speak_in_one() {
        let file_id = "a".repeat(64);
        // Reading is reading: the messages are public and the computer is only
        // fetching them, so a lent phone may show a conversation and the marks on
        // rows that say which ones are worth showing.
        for request in [
            ClientRequest::TrackDiscussion {
                file_id: file_id.clone(),
                before: Some(1_800_000_000),
            },
            ClientRequest::TrackDiscussionActivity {
                file_ids: vec![file_id.clone()],
            },
        ] {
            assert!(check_request_permission(DeviceRights::read_only(), &request).is_ok());
        }
        // Posting is signed with the user's own key and published under their
        // name, which is the same line read-only access already draws for reports
        // and for playlists. A reply is the same act with a parent attached.
        let comment = ClientRequest::SendTrackDiscussion {
            file_id: file_id.clone(),
            content: "Nice track".into(),
            reply_to: None,
        };
        assert!(check_request_permission(DeviceRights::read_only(), &comment).is_err());
        assert!(check_request_permission(DeviceRights::full(), &comment).is_ok());
        let reply = ClientRequest::SendTrackDiscussion {
            file_id,
            content: "Yes it is".into(),
            reply_to: Some("b".repeat(64)),
        };
        assert!(check_request_permission(DeviceRights::read_only(), &reply).is_err());
        assert!(check_request_permission(DeviceRights::full(), &reply).is_ok());
    }

    #[test]
    fn a_play_queue_is_bounded_and_filtered_to_file_ids() {
        let track = "a".repeat(64);
        let queued = "b".repeat(64);
        let PlaybackCommand::PlayTrack {
            file_id,
            queue,
            position_ms,
        } = bounded_playback(PlaybackCommand::PlayTrack {
            file_id: track.clone(),
            queue: vec![queued.clone(), "not-a-file".into(), String::new()],
            position_ms: 12_000,
        })
        .unwrap()
        else {
            panic!("a play command must stay a play command")
        };
        assert_eq!(file_id, track);
        assert_eq!(queue, vec![queued]);
        // Where a handover resumes is carried through untouched.
        assert_eq!(position_ms, 12_000);

        // A position past anything this computer could be playing is clamped
        // rather than refused, because there is nothing a phone could do about
        // an error but the track itself is still playable.
        let PlaybackCommand::PlayTrack { position_ms, .. } = bounded_playback(
            PlaybackCommand::PlayTrack {
                file_id: "a".repeat(64),
                queue: Vec::new(),
                position_ms: MAX_POSITION_MS + 1,
            },
        )
        .unwrap()
        else {
            panic!("a play command must stay a play command")
        };
        assert_eq!(position_ms, MAX_POSITION_MS);

        // The track being asked for is never quietly swapped for another.
        assert!(bounded_playback(PlaybackCommand::PlayTrack {
            file_id: "nope".into(),
            queue: Vec::new(),
            position_ms: 0,
        })
        .is_err());
        assert!(bounded_playback(PlaybackCommand::PlayTrack {
            file_id: track,
            queue: vec!["c".repeat(64); MAX_PLAY_QUEUE + 1],
            position_ms: 0,
        })
        .is_err());

        // Anything that is not about a queue passes through untouched.
        assert_eq!(
            bounded_playback(PlaybackCommand::Next).unwrap(),
            PlaybackCommand::Next
        );
    }

    #[test]
    fn pairing_grants_are_separate_single_use_and_persisted() {
        let directory =
            std::env::temp_dir().join(format!("napstr-pairing-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let db = directory.join("napstr.sqlite3");
        let connection = open_connection(&db).unwrap();
        // An existing installation must retain its previous full access.
        connection.execute_batch("CREATE TABLE mobile_devices(endpoint_id TEXT PRIMARY KEY,name TEXT NOT NULL,paired_at TEXT NOT NULL,last_seen TEXT NOT NULL);
            INSERT INTO mobile_devices VALUES('legacy','Old phone','now','now');").unwrap();
        initialise_schema(&db).unwrap();
        initialise_schema(&db).unwrap();
        // The column this predates said "full access", and it still means that.
        assert_eq!(device_rights(&db, "legacy").unwrap(), DeviceRights::full());
        let endpoint = SecretKey::generate().public().to_string();
        let mut sessions = vec![
            PairingSession {
                token: "full".into(),
                rights: DeviceRights::full(),
                expires_at: Utc::now().timestamp() + 60,
            },
            PairingSession {
                token: "stream".into(),
                rights: DeviceRights::read_only(),
                expires_at: Utc::now().timestamp() + 60,
            },
            PairingSession {
                token: "expired".into(),
                rights: DeviceRights::read_only(),
                expires_at: Utc::now().timestamp() - 1,
            },
        ];
        assert!(accept_pairing(&db, &mut sessions, &endpoint, "expired", "Guest").is_err());
        assert!(accept_pairing(&db, &mut sessions, &endpoint, "wrong", "Guest").is_err());
        // Older clients can also fetch/cache audio; the host enforces their read-only grant.
        assert_eq!(
            accept_pairing(&db, &mut sessions, &endpoint, "stream", "Guest").unwrap(),
            DeviceRights::read_only()
        );
        assert_eq!(device_rights(&db, &endpoint).unwrap(), DeviceRights::read_only());
        assert!(accept_pairing(&db, &mut sessions, &endpoint, "stream", "Guest").is_err());
        assert_eq!(
            accept_pairing(&db, &mut sessions, &endpoint, "full", "Owner").unwrap(),
            DeviceRights::full()
        );
        assert_eq!(device_rights(&db, &endpoint).unwrap(), DeviceRights::full());
        assert!(sessions.is_empty());
        sessions.push(PairingSession {
            token: "downgrade".into(),
            rights: DeviceRights::read_only(),
            expires_at: Utc::now().timestamp() + 60,
        });
        assert_eq!(
            accept_pairing(&db, &mut sessions, &endpoint, "downgrade", "Guest").unwrap(),
            DeviceRights::read_only()
        );
        assert_eq!(
            load_devices(&db)
                .unwrap()
                .iter()
                .find(|device| device.endpoint_id == endpoint)
                .unwrap()
                .rights,
            DeviceRights::read_only()
        );
        connection
            .execute(
                "DELETE FROM mobile_devices WHERE endpoint_id=?1",
                [&endpoint],
            )
            .unwrap();
        // A revoked device has no rights to read, which is the same call that
        // authorises it: one question, asked once.
        assert!(device_rights(&db, &endpoint).is_err());
        drop(connection);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn device_names_cannot_include_control_characters() {
        assert_eq!(clean_device_name("  My\nPhone\u{202e}  "), "MyPhone");
        assert_eq!(clean_device_name("\n\r"), "Napstrfy phone");
    }

    #[test]
    fn music_library_excludes_local_audiobook_chapters() {
        let directory =
            std::env::temp_dir().join(format!("napstr-mobile-music-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let db_path = directory.join("napstr.sqlite3");
        crate::initialise_database(&db_path, &directory).unwrap();
        let original_revision = library_revision(&db_path).unwrap();
        let connection = open_connection(&db_path).unwrap();
        for (file_id, filename, folder, title) in [
            ("11".repeat(32), "Song.mp3", "Music", "A Song"),
            ("22".repeat(32), "Part 1.mp3", "Audiobooks/A Book", "Part 1"),
            ("33".repeat(32), "Part 2.mp3", "Audiobooks/A Book", "Part 2"),
        ] {
            connection
                .execute(
                    "INSERT INTO files(file_id,filename,path,size,format,indexed_at,mime,folder,title,artist)
                     VALUES(?1,?2,?3,1,'MP3','now','audio/mpeg',?4,?5,'Author')",
                    params![
                        file_id,
                        filename,
                        directory.join(filename).to_string_lossy(),
                        folder,
                        title
                    ],
                )
                .unwrap();
        }
        drop(connection);
        assert!(library_revision(&db_path).unwrap() > original_revision);

        let (tracks, total, audiobook_chapter_ids) =
            load_library(&db_path, "", 0, MAX_PAGE_SIZE).unwrap();

        assert_eq!(total, 1);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "A Song");
        assert_eq!(audiobook_chapter_ids.len(), 2);
        assert!(audiobook_chapter_ids.contains(&"22".repeat(32)));
        assert!(audiobook_chapter_ids.contains(&"33".repeat(32)));

        // Audiobook detail lookup must be reconstructable from the database.
        // Napstrfy can retain summaries while the desktop process restarts, so
        // correctness cannot depend on an in-memory catalogue cache.
        let (audiobooks, _) = load_local_remote_audiobooks(&db_path).unwrap();
        assert_eq!(audiobooks.len(), 1);
        assert_eq!(audiobooks[0].title, "A Book");
        assert_eq!(audiobooks[0].chapters.len(), 2);
        assert!(audiobooks[0].chapters.iter().all(|chapter| chapter.local));
        fs::remove_dir_all(directory).unwrap();
    }

    /// A seeded browse is one order, whatever page asks for it and whenever.
    ///
    /// This is what lets a phone shuffle the whole library without the host
    /// keeping a list per device: the order is a function of the seed and the
    /// file ids, so paging walks it, a restart of either side reproduces it, and
    /// a library that changed keeps every file it still has in the same place.
    #[test]
    fn a_seeded_browse_walks_one_shuffled_order() {
        fn track(index: usize) -> RemoteTrack {
            RemoteTrack {
                file_id: format!("{index:064x}"),
                filename: format!("track-{index}.flac"),
                title: format!("Track {index}"),
                artist: "Someone".into(),
                album: "Something".into(),
                format: "FLAC".into(),
                mime: "audio/flac".into(),
                size: 1_000,
                tags: String::new(),
                local: true,
                sources: Vec::new(),
                bitrate_kbps: 900,
                sample_rate_hz: 44_100,
                channels: 2,
                lossless: true,
                duration_ms: 60_000,
            }
        }
        let tracks = (0..40).map(track).collect::<Vec<_>>();
        let ids = |page: &[RemoteTrack]| {
            page.iter()
                .map(|track| track.file_id.clone())
                .collect::<Vec<_>>()
        };
        let stored = ids(&tracks);
        let seed = 0x5eed_1234_5678_9abc;

        // The whole library, seven at a time, is the library: every file once,
        // in an order that is not the stored one.
        let mut walked = Vec::new();
        for offset in (0..tracks.len()).step_by(7) {
            let (page, total) = page_shuffled_music_library(&tracks, seed, offset, 7);
            assert_eq!(total, tracks.len());
            assert!(page.len() <= 7);
            walked.extend(ids(&page));
        }
        assert_eq!(walked.len(), stored.len());
        let mut unique = walked.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), stored.len(), "a page repeated or lost a file");
        assert_ne!(walked, stored, "the seeded order is the stored order");

        // The same seed asks for the same order; a different one does not.
        let (first, _) = page_shuffled_music_library(&tracks, seed, 0, 10);
        let (again, _) = page_shuffled_music_library(&tracks, seed, 0, 10);
        assert_eq!(ids(&first), ids(&again));
        let (other, _) = page_shuffled_music_library(&tracks, seed.wrapping_add(1), 0, 10);
        assert_ne!(ids(&first), ids(&other));

        // Where a file falls depends on its own id, so a library that gained or
        // lost something does not reorder the rest of itself.
        let mut subset = tracks.clone();
        let removed = subset.remove(3).file_id;
        let (subset_order, _) = page_shuffled_music_library(&subset, seed, 0, subset.len());
        let expected = walked
            .iter()
            .filter(|file_id| **file_id != removed)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(ids(&subset_order), expected);
    }
}
