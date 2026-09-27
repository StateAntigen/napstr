use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};

pub const ALPN: &[u8] = b"/napstr/mobile/1";
pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_CONTROL_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_PAGE_SIZE: usize = 200;
/// Album covers per request. Bounded so a full answer always fits in one
/// control frame even when every URL is at its maximum length.
pub const MAX_COVER_KEYS: usize = 40;
/// Longest cover key a phone may ask for art by. The NIP bounds a `d` value at
/// 300 characters, so anything longer is not a key this host ever stored and is
/// refused rather than searched for.
pub const MAX_ART_KEY_CHARS: usize = 300;

/// Which of an album's two renditions a phone is asking for.
///
/// Named rather than a boolean because the difference is a policy about what
/// this computer asks a third party about: a thumbnail is fetched for every
/// album that comes on screen, and the full rendition only for one somebody
/// engaged with. A call site should have to say which it means.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum ArtRendition {
    /// What a grid tile, a list row or a backdrop draws.
    Thumb,
    /// What the player and the album sheet draw.
    Full,
}
/// Longest queue a phone may hand to the host when it asks the host to play
/// something. 200 file ids of the 64 characters a SHA-256 takes is about 13 KB,
/// so a full queue always fits in one control frame.
pub const MAX_PLAY_QUEUE: usize = 200;
/// NIP-56 report types a cover report may use. `other` exists so a phone is
/// never forced to mislabelled something to be able to report it at all.
pub const REPORT_REASONS: [&str; 7] = [
    "illegal",
    "malware",
    "spam",
    "impersonation",
    "nudity",
    "profanity",
    "other",
];
pub const MAX_REPORT_NOTE_CHARS: usize = 500;
/// Longest pairing-code QR image the host will render, in bytes. A pairing code
/// is a few hundred bytes, which is a QR of roughly a hundred modules a side,
/// and the renderer draws one path segment per dark module - so the image is a
/// little over 100 KB in practice. This sits below `MAX_CONTROL_FRAME_BYTES` so
/// a ticket and its QR always travel in a single control frame.
pub const MAX_QR_SVG_BYTES: usize = 192 * 1024;
const PAIRING_URI_PREFIX: &str = "napstrfy://pair/";
const LEGACY_PAIRING_URI_PREFIX: &str = "nostrfy://pair/";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PairingTicket {
    pub version: u16,
    pub endpoint_id: String,
    /// JSON-encoded Iroh EndpointAddr. Keeping this opaque prevents the shared
    /// protocol crate from taking a networking dependency.
    pub endpoint_addr: String,
    pub token: String,
    pub expires_at: i64,
    pub desktop_name: String,
}

impl PairingTicket {
    pub fn to_uri(&self) -> Result<String, String> {
        let json = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        Ok(format!(
            "{PAIRING_URI_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(json)
        ))
    }

    pub fn from_uri(value: &str) -> Result<Self, String> {
        let value = value.trim();
        let encoded = value
            .strip_prefix(PAIRING_URI_PREFIX)
            .or_else(|| value.strip_prefix(LEGACY_PAIRING_URI_PREFIX))
            .ok_or("This is not a Napstrfy pairing code")?;
        let json = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| "The Napstrfy pairing code is damaged")?;
        let ticket: Self =
            serde_json::from_slice(&json).map_err(|_| "The Napstrfy pairing code is invalid")?;
        if ticket.version != PROTOCOL_VERSION {
            return Err("This pairing code uses an unsupported protocol version".into());
        }
        Ok(ticket)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSource {
    pub pubkey: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTrack {
    pub file_id: String,
    pub filename: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub format: String,
    pub mime: String,
    pub size: u64,
    pub tags: String,
    pub local: bool,
    pub sources: Vec<RemoteSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAudiobook {
    pub audiobook_id: String,
    pub title: String,
    pub author: String,
    pub narrator: String,
    pub total_size: u64,
    /// Ordered chapter tracks. A one-file audiobook contains one entry.
    pub chapters: Vec<RemoteTrack>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAudiobookSummary {
    pub audiobook_id: String,
    pub title: String,
    pub author: String,
    pub narrator: String,
    pub total_size: u64,
    pub chapter_count: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAlbumCover {
    /// The verbatim `artist|album` key, lowercased and trimmed. This is what a
    /// track's own key is computed into, so the phone can match without
    /// understanding the canonical alias.
    pub key: String,
    /// HTTPS URL of the front cover. Empty when the publisher only shared an
    /// embedded copy through an `x` tag.
    ///
    /// Superseded for display by [`RemoteAlbumCover::art_hash`]. A current phone
    /// draws the picture the host holds rather than fetching from a publisher,
    /// so it ignores this, and it is kept on the wire only so a phone built
    /// before the art channel still finds something it can use.
    pub art: String,
    /// HTTPS URL of a smaller rendition of the same image, when published.
    /// Superseded by [`RemoteAlbumCover::thumb_hash`], for the same reason.
    pub thumb: String,
    /// SHA-256 of the full-size picture as this host holds it, askable for with
    /// [`ClientRequest::FetchArt`] at [`ArtRendition::Full`].
    ///
    /// Empty is an ordinary state rather than a failure: this host may not have
    /// downloaded the picture yet. A phone draws its placeholder for an empty
    /// hash and asks again when the cover revision moves, which is exactly what
    /// the host reports when the bytes arrive.
    #[serde(default)]
    pub art_hash: String,
    /// SHA-256 of the smaller rendition, for [`ArtRendition::Thumb`].
    #[serde(default)]
    pub thumb_hash: String,
    pub mbid: String,
    pub year: String,
    pub genre: String,
    pub collection: String,
    /// Provenance hint such as `itunes` or `embedded`. Informational only.
    pub source: String,
    /// SHA-256 of the image as an ordinary catalogue entry, when the publisher
    /// shared the bytes themselves. Fetch it through the normal transfer path
    /// and verify the hash and the image magic bytes before display.
    pub cover_file_id: String,
    pub mime: String,
    /// Author of the winning claim.
    pub author: String,
    /// True when the winning author is (or was) an active seeder of a track of
    /// this album, which is the strongest trust signal in the cover NIP.
    pub seeder: bool,
}
/// Repeating is a choice of three, matching what the phone's drawer offers.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RemoteRepeat {
    #[default]
    Off,
    All,
    One,
}

/// One transport instruction from a write-capable phone to the host's own
/// player. The phone never plays the desktop's audio itself, so every variant
/// here is a request the host may still refuse.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PlaybackCommand {
    Play,
    Pause,
    /// Pause when playing, resume when paused: what a single button wants.
    Toggle,
    Stop,
    Next,
    Previous,
    Seek {
        position_ms: u64,
    },
    /// A percentage rather than a fraction: it keeps the command comparable and
    /// keeps floating point off the wire.
    Volume {
        percent: u8,
    },
    Repeat {
        mode: RemoteRepeat,
    },
    Shuffle {
        enabled: bool,
    },
    /// Play one particular track on the host, adopting the list the phone was
    /// showing as the queue so that "next" goes where the phone would have
    /// gone. The host finds `file_id` inside `queue`; an empty queue means
    /// "this track on its own".
    PlayTrack {
        file_id: String,
        #[serde(default)]
        queue: Vec<String>,
    },
}

/// What the host's own player is doing right now, plus the queue facts only the
/// desktop's frontend knows. Every field is optional on the wire so a newer
/// phone can still talk to an older host and vice versa.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct RemotePlaybackState {
    /// False when the host has not played anything this session.
    pub active: bool,
    pub playing: bool,
    pub file_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub volume: f64,
    pub queue_len: usize,
    /// Index of the playing track in the host's queue, or -1.
    pub queue_index: i64,
    pub repeat: RemoteRepeat,
    pub shuffle: bool,
    /// True while a phone has driven the host recently, so the desktop can say
    /// so rather than having tracks appear to move on their own.
    pub remote_control: bool,
    /// Why the host cannot play anything, when that is the case - a phone that
    /// presses play on an idle computer deserves the reason, not silence.
    pub error: String,
    pub updated_at: i64,
}

/// What the host signed and published on this phone's behalf.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct CoverReportResult {
    /// Event id of the `1984` report.
    pub report_id: String,
    /// True when the host accepted it into its publish queue rather than
    /// having already written it to a relay.
    pub queued: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTransfer {
    pub id: String,
    pub file_id: String,
    pub filename: String,
    pub size: u64,
    pub progress: f64,
    pub status: String,
    pub speed: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ClientRequest {
    Pair {
        token: String,
        device_name: String,
    },
    Library {
        query: String,
        offset: usize,
        limit: usize,
    },
    Search {
        query: String,
    },
    Audiobooks {
        query: String,
    },
    AudiobookLibrary {
        query: String,
        offset: usize,
        limit: usize,
    },
    Audiobook {
        audiobook_id: String,
    },
    RequestDownload {
        file_id: String,
        source_pubkeys: Vec<String>,
        #[serde(default)]
        destination_folder: Option<String>,
    },
    Transfers,
    FetchAudio {
        file_id: String,
    },
    /// The pixels of one album's art, which this host holds. The phone asks for
    /// the rendition it is about to draw instead of fetching the URL itself: the
    /// album it is looking at, and its own address, are not a third party's
    /// business.
    FetchArt {
        key: String,
        rendition: ArtRendition,
    },
    Available {
        file_ids: Vec<String>,
    },
    /// Album artwork asserted by kind `30427` events. The host resolves the
    /// keys because it owns the relay pool, the catalogue, and the availability
    /// heartbeats that decide which claim wins.
    AlbumCovers {
        keys: Vec<String>,
    },
    /// A write-capable phone driving the host's own player.
    Playback {
        command: PlaybackCommand,
    },
    /// What the host's player is doing. Reading it is harmless, so a phone with
    /// read-only access may ask too.
    PlaybackState,
    /// Ask the host to mint a read-only pairing code this phone can hand to
    /// another device. Only a phone with write access may ask, so read-only
    /// access can never re-delegate itself and widen.
    ReadOnlyTicket,
    /// NIP-56: ask the host to sign and publish a `1984` report about an album
    /// cover. The phone holds no Nostr keys, so the host is the only one that
    /// can speak on the user's behalf.
    ReportCover {
        key: String,
        reason: String,
        note: String,
    },
    Status,
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ServerResponse {
    Paired {
        desktop_name: String,
        #[serde(default)]
        stream_only: bool,
    },
    Library {
        tracks: Vec<RemoteTrack>,
        total: usize,
    },
    Search {
        tracks: Vec<RemoteTrack>,
    },
    Audiobooks {
        audiobooks: Vec<RemoteAudiobook>,
    },
    AudiobookLibrary {
        audiobooks: Vec<RemoteAudiobookSummary>,
        total: usize,
    },
    Audiobook {
        audiobook: RemoteAudiobook,
    },
    DownloadRequested {
        request_id: String,
    },
    Transfers {
        transfers: Vec<RemoteTransfer>,
    },
    AudioReady {
        track: RemoteTrack,
    },
    Available {
        file_ids: Vec<String>,
    },
    AlbumCovers {
        covers: Vec<RemoteAlbumCover>,
    },
    /// The header an art transfer begins with: the bytes follow it in chunks,
    /// exactly as audio does. `hash` is what the phone stores them under, so two
    /// albums sharing one picture cost one of them, and `length` is how much to
    /// expect before the stream is done.
    ArtReady {
        key: String,
        rendition: ArtRendition,
        /// Lowercase SHA-256 of the bytes that follow.
        hash: String,
        mime: String,
        length: u64,
    },
    /// This host holds no art for that album — nothing claimed one, or the pass
    /// that fills the cache has not reached it. A considered answer rather than
    /// an error: the phone paints its placeholder and asks again when the host's
    /// cover revision moves, which is the signal it already uses for "the host
    /// has none".
    ArtMissing {
        key: String,
    },
    Playback {
        state: RemotePlaybackState,
    },
    /// A read-only pairing code, minted by the host, for this phone to show.
    ReadOnlyTicket {
        /// A `napstrfy://pair/...` code, exactly as the desktop's own QR holds.
        uri: String,
        /// The same code drawn as a QR image by the host so the other phone can
        /// scan it instead of typing it.
        #[serde(default)]
        qr_svg: String,
        expires_at: i64,
        desktop_name: String,
    },
    CoverReported {
        report: CoverReportResult,
    },
    Status {
        library_revision: u64,
        /// Moves whenever what the host would report about album art changes.
        /// A companion caches covers - including "the host has none" - so this
        /// is what tells it a cached answer may be stale. An older host omits
        /// it, which reads as `0` and simply never invalidates.
        #[serde(default)]
        cover_revision: u64,
        #[serde(default)]
        stream_only: bool,
    },
    Pong,
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_art_request_and_its_header_round_trip() {
        // The phone names the rendition, because that is what decides how much
        // this computer asks a third party about.
        for (rendition, name) in [(ArtRendition::Thumb, "thumb"), (ArtRendition::Full, "full")] {
            let asked = format!(
                r#"{{"type":"fetchArt","key":"KREAM|Annihilation","rendition":"{name}"}}"#
            );
            assert_eq!(
                serde_json::from_str::<ClientRequest>(&asked).unwrap(),
                ClientRequest::FetchArt {
                    key: "KREAM|Annihilation".into(),
                    rendition
                },
                "{asked}"
            );
        }
        // The header a phone reads before the bytes: the hash it stores them
        // under, and the length to expect.
        let header = ServerResponse::ArtReady {
            key: "KREAM|Annihilation".into(),
            rendition: ArtRendition::Thumb,
            hash: "a".repeat(64),
            mime: "image/jpeg".into(),
            length: 24_576,
        };
        let round_tripped: ServerResponse =
            serde_json::from_str(&serde_json::to_string(&header).unwrap()).unwrap();
        assert_eq!(round_tripped, header);
        // And "this host has none" is an answer, not an error.
        let missing = ServerResponse::ArtMissing {
            key: "KREAM|Annihilation".into(),
        };
        let round_tripped: ServerResponse =
            serde_json::from_str(&serde_json::to_string(&missing).unwrap()).unwrap();
        assert_eq!(round_tripped, missing);
    }

    #[test]
    fn an_art_header_fits_in_one_control_frame() {
        // The bytes stream after the header, in chunks, so only the header has
        // to fit one frame. Worst case: a maximum-length key, a full hash and a
        // mime.
        let worst = ServerResponse::ArtReady {
            key: "k".repeat(MAX_ART_KEY_CHARS),
            rendition: ArtRendition::Full,
            hash: "f".repeat(64),
            mime: "image/webp".into(),
            length: u64::MAX,
        };
        let payload = serde_json::to_vec(&worst).unwrap();
        assert!(
            payload.len() <= MAX_CONTROL_FRAME_BYTES,
            "an art header is {} bytes, over the {MAX_CONTROL_FRAME_BYTES} byte frame limit",
            payload.len()
        );
    }

    #[test]
    fn older_pairing_and_status_messages_keep_full_access() {
        assert_eq!(
            serde_json::from_str::<ServerResponse>(
                r#"{"type":"paired","desktopName":"Old Napstr"}"#
            )
            .unwrap(),
            ServerResponse::Paired {
                desktop_name: "Old Napstr".into(),
                stream_only: false
            }
        );
        assert_eq!(
            serde_json::from_str::<ServerResponse>(r#"{"type":"status","libraryRevision":1}"#)
                .unwrap(),
            ServerResponse::Status {
                library_revision: 1,
                cover_revision: 0,
                stream_only: false
            }
        );
        assert_eq!(
            serde_json::from_str::<ClientRequest>(
                r#"{"type":"pair","token":"secret","deviceName":"Old phone"}"#
            )
            .unwrap(),
            ClientRequest::Pair {
                token: "secret".into(),
                device_name: "Old phone".into()
            }
        );
    }

    #[test]
    fn pairing_ticket_round_trips_without_exposing_raw_json() {
        let ticket = PairingTicket {
            version: PROTOCOL_VERSION,
            endpoint_id: "abc123".into(),
            endpoint_addr: "{\"id\":\"abc123\",\"addrs\":[]}".into(),
            token: "one-time-secret".into(),
            expires_at: 123456,
            desktop_name: "Living room".into(),
        };
        let uri = ticket.to_uri().unwrap();
        assert!(uri.starts_with(PAIRING_URI_PREFIX));
        assert!(!uri.contains("one-time-secret"));
        assert_eq!(PairingTicket::from_uri(&uri).unwrap(), ticket);
    }

    #[test]
    fn library_status_round_trips() {
        let response = ServerResponse::Status {
            library_revision: 42,
            cover_revision: 9,
            stream_only: true,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert_eq!(
            serde_json::from_str::<ServerResponse>(&json).unwrap(),
            response
        );
    }

    #[test]
    fn a_host_without_a_cover_revision_reports_none() {
        // An older desktop sends no `coverRevision`. Reading it as zero is what
        // lets a newer phone treat "never invalidated" as the status quo rather
        // than an answer that keeps changing underneath it.
        assert_eq!(
            serde_json::from_str::<ServerResponse>(
                r#"{"type":"status","libraryRevision":3,"streamOnly":true}"#
            )
            .unwrap(),
            ServerResponse::Status {
                library_revision: 3,
                cover_revision: 0,
                stream_only: true,
            }
        );
    }

    #[test]
    fn download_destination_is_optional_for_older_companions() {
        let legacy = r#"{"type":"requestDownload","fileId":"abc","sourcePubkeys":["def"]}"#;
        assert_eq!(
            serde_json::from_str::<ClientRequest>(legacy).unwrap(),
            ClientRequest::RequestDownload {
                file_id: "abc".into(),
                source_pubkeys: vec!["def".into()],
                destination_folder: None,
            }
        );
        let audiobook = ClientRequest::RequestDownload {
            file_id: "abc".into(),
            source_pubkeys: vec!["def".into()],
            destination_folder: Some("A Book [12345678]".into()),
        };
        assert_eq!(
            serde_json::from_str::<ClientRequest>(&serde_json::to_string(&audiobook).unwrap())
                .unwrap(),
            audiobook
        );
    }

    #[test]
    fn cache_availability_round_trips() {
        let request = ClientRequest::Available {
            file_ids: vec!["a".repeat(64), "b".repeat(64)],
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<ClientRequest>(&json).unwrap(),
            request
        );
    }

    #[test]
    fn album_covers_round_trip() {
        let request = ClientRequest::AlbumCovers {
            keys: vec!["artist|album".into()],
        };
        assert_eq!(
            serde_json::from_str::<ClientRequest>(&serde_json::to_string(&request).unwrap())
                .unwrap(),
            request
        );
        let response = ServerResponse::AlbumCovers {
            covers: vec![RemoteAlbumCover {
                key: "artist|album".into(),
                art: "https://example.com/cover.jpg".into(),
                thumb: String::new(),
                art_hash: "c".repeat(64),
                thumb_hash: String::new(),
                mbid: String::new(),
                year: "2007".into(),
                genre: "Rock".into(),
                collection: String::new(),
                source: "itunes".into(),
                cover_file_id: String::new(),
                mime: String::new(),
                author: "a".repeat(64),
                seeder: true,
            }],
        };
        assert_eq!(
            serde_json::from_str::<ServerResponse>(&serde_json::to_string(&response).unwrap())
                .unwrap(),
            response
        );
    }

    /// The phone and the desktop are released independently, so these strings
    /// are part of the contract rather than an implementation detail.
    #[test]
    fn playback_wire_names_are_stable() {
        assert_eq!(
            serde_json::to_string(&ClientRequest::Playback {
                command: PlaybackCommand::Seek { position_ms: 1 }
            })
            .unwrap(),
            r#"{"type":"playback","command":{"type":"seek","positionMs":1}}"#
        );
        assert_eq!(
            serde_json::to_string(&ClientRequest::PlaybackState).unwrap(),
            r#"{"type":"playbackState"}"#
        );
        assert_eq!(serde_json::to_string(&RemoteRepeat::One).unwrap(), r#""one""#);
        assert_eq!(
            serde_json::to_string(&ClientRequest::ReadOnlyTicket).unwrap(),
            r#"{"type":"readOnlyTicket"}"#
        );
    }

    #[test]
    fn every_playback_command_round_trips() {
        for command in [
            PlaybackCommand::Play,
            PlaybackCommand::Pause,
            PlaybackCommand::Toggle,
            PlaybackCommand::Stop,
            PlaybackCommand::Next,
            PlaybackCommand::Previous,
            PlaybackCommand::Seek {
                position_ms: 42_000,
            },
            PlaybackCommand::Volume { percent: 50 },
            PlaybackCommand::Repeat {
                mode: RemoteRepeat::One,
            },
            PlaybackCommand::Shuffle { enabled: true },
            PlaybackCommand::PlayTrack {
                file_id: "a".repeat(64),
                queue: vec!["b".repeat(64), "c".repeat(64)],
            },
        ] {
            let request = ClientRequest::Playback {
                command: command.clone(),
            };
            let json = serde_json::to_string(&request).unwrap();
            assert_eq!(
                serde_json::from_str::<ClientRequest>(&json).unwrap(),
                request
            );
        }
    }

    /// A host that predates a field must still be understood, rather than
    /// making the phone show an error for a state it could partly render.
    #[test]
    fn a_partial_playback_state_still_decodes() {
        let state: RemotePlaybackState = serde_json::from_str(r#"{"playing":true}"#).unwrap();
        assert!(state.playing);
        assert!(!state.active);
        assert_eq!(state.repeat, RemoteRepeat::Off);
        assert_eq!(state.queue_index, 0);
        let response: ServerResponse =
            serde_json::from_str(r#"{"type":"playback","state":{"title":"Song"}}"#).unwrap();
        assert_eq!(
            response,
            ServerResponse::Playback {
                state: RemotePlaybackState {
                    title: "Song".into(),
                    ..RemotePlaybackState::default()
                }
            }
        );
    }

    #[test]
    fn read_only_tickets_and_reports_round_trip() {
        let response = ServerResponse::ReadOnlyTicket {
            uri: "napstrfy://pair/abc".into(),
            qr_svg: "<svg></svg>".into(),
            expires_at: 123,
            desktop_name: "Living room".into(),
        };
        assert_eq!(
            serde_json::from_str::<ServerResponse>(&serde_json::to_string(&response).unwrap())
                .unwrap(),
            response
        );
        // An older host renders no QR, which must not be an error.
        let bare: ServerResponse =
            serde_json::from_str(r#"{"type":"readOnlyTicket","uri":"napstrfy://pair/abc","expiresAt":1,"desktopName":"N"}"#)
                .unwrap();
        assert!(matches!(
            bare,
            ServerResponse::ReadOnlyTicket { qr_svg, .. } if qr_svg.is_empty()
        ));
        let request = ClientRequest::ReportCover {
            key: "artist|album".into(),
            reason: "spam".into(),
            note: "not this record".into(),
        };
        assert_eq!(
            serde_json::from_str::<ClientRequest>(&serde_json::to_string(&request).unwrap())
                .unwrap(),
            request
        );
    }

    /// A host may answer with `MAX_COVER_KEYS` covers whose URL fields are all
    /// at their documented maximum, and the answer still has to fit in one
    /// control frame rather than being truncated mid-flight.
    #[test]
    fn a_full_cover_answer_fits_in_one_control_frame() {
        let key = format!("{}|{}", "a".repeat(148), "b".repeat(150));
        let covers = (0..MAX_COVER_KEYS)
            .map(|index| RemoteAlbumCover {
                key: key.clone(),
                // 2048 is the accepted maximum for `art` and `thumb`.
                art: format!("https://example.com/{}.jpg", "x".repeat(2020)),
                thumb: format!("https://example.com/{}.jpg", "y".repeat(2020)),
                art_hash: format!("{index:064x}"),
                thumb_hash: format!("{index:064x}"),
                mbid: "0".repeat(36),
                year: "2007".into(),
                genre: "g".repeat(120),
                collection: "c".repeat(120),
                source: "s".repeat(32),
                cover_file_id: format!("{index:064x}"),
                mime: "image/jpeg".into(),
                author: "a".repeat(64),
                seeder: true,
            })
            .collect::<Vec<_>>();
        let payload = serde_json::to_vec(&ServerResponse::AlbumCovers { covers }).unwrap();
        assert!(
            payload.len() <= MAX_CONTROL_FRAME_BYTES,
            "a full cover answer is {} bytes, over the {MAX_CONTROL_FRAME_BYTES} byte frame limit",
            payload.len()
        );
    }

    /// A phone may hand the desktop the whole list it was showing, and that
    /// request still has to fit in one control frame.
    #[test]
    fn a_full_play_queue_fits_in_one_control_frame() {
        let request = ClientRequest::Playback {
            command: PlaybackCommand::PlayTrack {
                file_id: "a".repeat(64),
                queue: (0..MAX_PLAY_QUEUE)
                    .map(|index| format!("{index:064x}"))
                    .collect(),
            },
        };
        let payload = serde_json::to_vec(&request).unwrap();
        assert!(
            payload.len() <= MAX_CONTROL_FRAME_BYTES,
            "a full play queue is {} bytes, over the {MAX_CONTROL_FRAME_BYTES} byte frame limit",
            payload.len()
        );
    }
}
