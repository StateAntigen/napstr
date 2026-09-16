use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};

pub const ALPN: &[u8] = b"/napstr/mobile/1";
pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_CONTROL_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_PAGE_SIZE: usize = 200;
const PAIRING_URI_PREFIX: &str = "napstrfy://pair/";
const LEGACY_PAIRING_URI_PREFIX: &str = "nostrfy://pair/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackUri {
    pub file_id: String,
}

impl TrackUri {
    pub fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        let file_id = value
            .strip_prefix("napstr://track/")
            .ok_or("This is not a Napstr track URI")?;
        if file_id.len() != 64
            || !file_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || file_id.bytes().any(|byte| byte.is_ascii_uppercase())
            || file_id.contains('/')
            || file_id.contains('?')
            || file_id.contains('#')
        {
            return Err("The Napstr track URI contains an invalid file ID".into());
        }
        Ok(Self {
            file_id: file_id.to_string(),
        })
    }
}

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
    /// Exact lookup of one file by its SHA-256 ID.
    ///
    /// A file ID is not a text query: `napstr://track/<file-id>` deep links
    /// must resolve the single file that hashes to it, including files the
    /// desktop holds but has not published with searchable words. Companions
    /// send this first and fall back to [`ClientRequest::Search`] when the
    /// desktop answers that the request is unknown.
    Track {
        file_id: String,
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
    /// Result of [`ClientRequest::Track`].
    ///
    /// `track` is `None` when neither the desktop library nor the catalogue
    /// knows the file ID. A track with `local: false` is only available from
    /// the catalogue, so it can be streamed but not downloaded from the host.
    Track {
        track: Option<RemoteTrack>,
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
    fn track_uri_round_trips_and_rejects_ambiguous_forms() {
        let file_id = "a".repeat(64);
        assert_eq!(
            TrackUri::parse(&format!("napstr://track/{file_id}"))
                .unwrap()
                .file_id,
            file_id
        );
        for value in [
            "napstrfy://track/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "napstr://track/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "napstr://track/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "napstr://track/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/extra",
            "napstr://track/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa?download=1",
            "napstr://track/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa#player",
        ] {
            assert!(TrackUri::parse(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn track_lookup_requests_and_responses_keep_their_wire_shape() {
        let file_id = "3f".repeat(32);
        let request = ClientRequest::Track {
            file_id: file_id.clone(),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(json, format!(r#"{{"type":"track","fileId":"{file_id}"}}"#));
        assert_eq!(serde_json::from_str::<ClientRequest>(&json).unwrap(), request);

        // An unknown file is an ordinary answer, not an error, so a deep link
        // can report "not in your library" instead of a protocol failure.
        let missing = ServerResponse::Track { track: None };
        let json = serde_json::to_string(&missing).unwrap();
        assert_eq!(json, r#"{"type":"track","track":null}"#);
        assert_eq!(serde_json::from_str::<ServerResponse>(&json).unwrap(), missing);
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
}
