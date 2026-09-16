use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};

pub const ALPN: &[u8] = b"/napstr/mobile/1";
pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_CONTROL_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_PAGE_SIZE: usize = 200;
/// Album covers per request. Bounded so a full answer always fits in one
/// control frame even when every URL is at its maximum length.
pub const MAX_COVER_KEYS: usize = 40;
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
    pub art: String,
    /// HTTPS URL of a smaller rendition of the same image, when published.
    pub thumb: String,
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
    Available {
        file_ids: Vec<String>,
    },
    /// Album artwork asserted by kind `30427` events. The host resolves the
    /// keys because it owns the relay pool, the catalogue, and the availability
    /// heartbeats that decide which claim wins.
    AlbumCovers {
        keys: Vec<String>,
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
    Status {
        library_revision: u64,
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
            stream_only: true,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert_eq!(
            serde_json::from_str::<ServerResponse>(&json).unwrap(),
            response
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
}
