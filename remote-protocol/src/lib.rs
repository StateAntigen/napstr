use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};

pub const ALPN: &[u8] = b"/napstr/mobile/1";
pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_CONTROL_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_PAGE_SIZE: usize = 200;
/// Album covers per request. Bounded so a full answer always fits in one
/// control frame even when every URL is at its maximum length.
pub const MAX_COVER_KEYS: usize = 40;
/// Longest queue either side may hand to the other. 200 file ids of the 64
/// characters a SHA-256 takes is about 13 KB, so a full queue always fits in one
/// control frame, in a request or in the answer a handoff gets back.
pub const MAX_PLAY_QUEUE: usize = 200;
/// Longest track a handoff can be asked to start inside. Nothing either side
/// plays is a day long, and a position past this is a bug in the caller rather
/// than a preference.
pub const MAX_POSITION_MS: u64 = 24 * 60 * 60 * 1000;
/// Catalogue records per by-id request. Smaller than a library page because every
/// field of every track may be at its maximum length and a full answer still has
/// to fit in one control frame - which a page of 200 such tracks would not.
pub const MAX_TRACKS_BY_ID: usize = 100;
/// Members per playlist page, for the same reason as `MAX_TRACKS_BY_ID`: a
/// playlist may name 500 members, so a page of them is what keeps an answer
/// inside one control frame.
pub const MAX_PLAYLIST_PAGE: usize = 100;
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
    /// Hand playback over to the phone that asked: stop, and answer with what
    /// this computer was doing as it was at the moment it stopped.
    ///
    /// One request rather than "read the state, then stop", because between two
    /// requests this host may have moved on to the next track by itself and the
    /// phone would take over the wrong one.
    Handoff,
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
        /// Where inside the track to start, so handing playback over resumes at
        /// the second the other device was at rather than starting again. Older
        /// senders omit it, which reads as the beginning of the track.
        #[serde(default)]
        position_ms: u64,
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
    /// The host's own record of the file it is playing, when this computer holds
    /// it. A phone needs the real record - format, mime, size - to fetch the
    /// audio and take playback over; one built from the title alone cannot be
    /// played. Absent when the host is playing something it does not index.
    #[serde(default)]
    pub track: Option<RemoteTrack>,
    /// File ids of the host's queue, in the order it would play them. Only the
    /// answer to a handoff fills this: a queue is far too big to repeat on every
    /// poll, and a phone that needs it is taking the whole queue over.
    #[serde(default)]
    pub queue: Vec<String>,
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

/// What a playlist is called and who published it, without its members. A phone
/// lists these first and asks for the members of the one it is about to play, so
/// that browsing never carries the members of playlists nobody opened.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct RemotePlaylistSummary {
    /// The stable id from the `d` tag: a canonical lowercase UUID, which stays
    /// the same across edits. Every playlist kind uses it, so it is also what a
    /// phone sends back to ask for one.
    pub playlist_id: String,
    pub title: String,
    /// Author of a public playlist. Empty for a playlist only this computer
    /// holds.
    pub author: String,
    pub display_name: String,
    /// Members the playlist names, which is also how many rows the members of it
    /// can be paged through.
    pub track_count: usize,
    /// True when the playlist is private. A private playlist is never published
    /// to a relay at all: its owner's computer stores it and serves it to a
    /// paired companion over the companion channel, and nowhere else. A relay
    /// would see the coordinate, the size and the edit time even with the body
    /// encrypted, which is exactly what a private playlist is for avoiding.
    pub private: bool,
    pub updated_at: i64,
}

/// One member of a playlist, in the order the playlist puts it in.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct RemotePlaylistTrack {
    /// Where the member sits in the playlist. The first member is 1.
    pub position: u32,
    pub file_id: String,
    /// Display hints, which exist so a member whose catalogue entry cannot be
    /// found still renders as something a person recognises. A catalogue entry
    /// always wins over them, and they are never authoritative.
    pub title: String,
    pub artist: String,
    pub album: String,
}

/// A playlist and one page of its members.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct RemotePlaylist {
    pub playlist_id: String,
    pub title: String,
    pub author: String,
    pub display_name: String,
    /// Album artist and release-group MBID, when the playlist describes one
    /// release group rather than a mix. Untrusted display metadata.
    pub artist: String,
    pub mbid: String,
    /// The author's own search words, comma-separated in the format a catalogue
    /// entry uses for its `tags`: what this playlist is meant to be found by.
    ///
    /// These are the author's choice and outrank anything a client would
    /// suggest, including the choice of having none. A publisher MUST NOT add
    /// words of its own on top of them.
    pub tags: String,
    pub private: bool,
    pub updated_at: i64,
    /// Members in `position` order, this page of them.
    pub tracks: Vec<RemotePlaylistTrack>,
    /// Members the whole playlist names, so a paged answer says how many are
    /// still to come.
    pub total: usize,
}

impl RemotePlaylist {
    /// The same playlist without its members, for a list that only needs names.
    pub fn summary(&self) -> RemotePlaylistSummary {
        RemotePlaylistSummary {
            playlist_id: self.playlist_id.clone(),
            title: self.title.clone(),
            author: self.author.clone(),
            display_name: self.display_name.clone(),
            track_count: self.total,
            private: self.private,
            updated_at: self.updated_at,
        }
    }
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
    /// The catalogue records for particular file ids, in the order asked for.
    /// A handoff answer carries the host's queue as ids, so this is how a phone
    /// turns that queue into tracks it can actually play.
    LibraryByIds {
        file_ids: Vec<String>,
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
    /// Playlists this computer can see: the ones it published, the ones it found
    /// on relays, and its own private ones. Discovery is by name here rather
    /// than by member, because "which playlists contain any of my N files" does
    /// not survive library scale.
    Playlists {
        offset: usize,
        limit: usize,
    },
    /// One playlist, a page of its members at a time in `position` order.
    ///
    /// A playlist is named by its coordinate: its author and its id together.
    /// The id is chosen by the author, so two authors may choose the same one,
    /// and a playlist must never be answerable with a stranger's under that id.
    /// An empty `author` means "whichever playlist this computer holds under that
    /// id", which is only unambiguous while there is one.
    Playlist {
        #[serde(default)]
        author: String,
        playlist_id: String,
        offset: usize,
        limit: usize,
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
    LibraryByIds {
        tracks: Vec<RemoteTrack>,
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
    Playlists {
        playlists: Vec<RemotePlaylistSummary>,
        total: usize,
    },
    Playlist {
        playlist: RemotePlaylist,
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
        assert_eq!(
            serde_json::to_string(&ClientRequest::Playback {
                command: PlaybackCommand::Handoff
            })
            .unwrap(),
            r#"{"type":"playback","command":{"type":"handoff"}}"#
        );
        assert_eq!(
            serde_json::to_string(&ClientRequest::LibraryByIds {
                file_ids: vec!["a".repeat(64)]
            })
            .unwrap(),
            format!(
                r#"{{"type":"libraryByIds","fileIds":["{}"]}}"#,
                "a".repeat(64)
            )
        );
        assert_eq!(
            serde_json::to_string(&ClientRequest::Playlists {
                offset: 0,
                limit: 100
            })
            .unwrap(),
            r#"{"type":"playlists","offset":0,"limit":100}"#
        );
        assert_eq!(
            serde_json::to_string(&ClientRequest::Playlist {
                author: "a".repeat(64),
                playlist_id: "id".into(),
                offset: 0,
                limit: 100
            })
            .unwrap(),
            format!(
                r#"{{"type":"playlist","author":"{}","playlistId":"id","offset":0,"limit":100}}"#,
                "a".repeat(64)
            )
        );
        // A companion that predates the coordinate still asks by id, which the
        // host reads as "the playlist you hold under this id".
        let by_id: ClientRequest =
            serde_json::from_str(r#"{"type":"playlist","playlistId":"id","offset":0,"limit":100}"#)
                .unwrap();
        assert!(matches!(by_id, ClientRequest::Playlist { author, .. } if author.is_empty()));
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
            PlaybackCommand::Handoff,
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
                position_ms: 12_000,
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
        // A host that predates the handoff fields sends neither, and a phone
        // must read that as "no record, no queue" rather than as a failure.
        assert!(state.track.is_none());
        assert!(state.queue.is_empty());
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

    #[test]
    fn playlists_round_trip() {
        let playlist = RemotePlaylist {
            playlist_id: "77abf082-7075-4d36-afe2-e9710ac6b33c".into(),
            title: "rock".into(),
            author: "a".repeat(64),
            display_name: "Sean Parker".into(),
            artist: "Metallica".into(),
            mbid: "60691bed-fdd7-32f9-92dc-b151aac9e271".into(),
            tags: "driving, late night".into(),
            private: false,
            updated_at: 1_787_680_200,
            tracks: vec![RemotePlaylistTrack {
                position: 1,
                file_id: "b".repeat(64),
                title: "Enter Sandman".into(),
                artist: "Metallica".into(),
                album: "Metallica".into(),
            }],
            total: 1,
        };
        let response = ServerResponse::Playlist {
            playlist: playlist.clone(),
        };
        assert_eq!(
            serde_json::from_str::<ServerResponse>(&serde_json::to_string(&response).unwrap())
                .unwrap(),
            response
        );
        // The list view is the same object with the members left out.
        let summary = playlist.summary();
        assert_eq!(summary.playlist_id, playlist.playlist_id);
        assert_eq!(summary.track_count, 1);
        let request = ClientRequest::Playlist {
            author: playlist.author.clone(),
            playlist_id: playlist.playlist_id.clone(),
            offset: 0,
            limit: MAX_PLAYLIST_PAGE,
        };
        assert_eq!(
            serde_json::from_str::<ClientRequest>(&serde_json::to_string(&request).unwrap())
                .unwrap(),
            request
        );
        // A private playlist never leaves its owner's own devices, so the flag
        // has to survive the wire rather than being assumed false.
        let response: ServerResponse = serde_json::from_str(
            r#"{"type":"playlists","playlists":[{"playlistId":"id","private":true}],"total":1}"#,
        )
        .unwrap();
        assert!(matches!(
            response,
            ServerResponse::Playlists { playlists, .. } if playlists[0].private
        ));
    }

    /// A playlist may name 500 members and every hint may be at its maximum
    /// length, so a page of them still has to fit in one control frame.
    #[test]
    fn a_full_playlist_page_fits_in_one_control_frame() {
        let members = (0..MAX_PLAYLIST_PAGE)
            .map(|index| RemotePlaylistTrack {
                position: index as u32 + 1,
                file_id: "a".repeat(64),
                title: "t".repeat(256),
                artist: "a".repeat(256),
                album: "l".repeat(256),
            })
            .collect::<Vec<_>>();
        let payload = serde_json::to_vec(&ServerResponse::Playlist {
            playlist: RemotePlaylist {
                playlist_id: "77abf082-7075-4d36-afe2-e9710ac6b33c".into(),
                title: "t".repeat(256),
                author: "a".repeat(64),
                display_name: "d".repeat(256),
                artist: "a".repeat(256),
                mbid: "60691bed-fdd7-32f9-92dc-b151aac9e271".into(),
                // The author's own words, at the catalogue's maximum of 12.
                tags: (1..=12)
                    .map(|index| format!("w{index}"))
                    .collect::<Vec<_>>()
                    .join(", "),
                private: false,
                updated_at: 1_787_680_200,
                tracks: members,
                total: 500,
            },
        })
        .unwrap();
        assert!(
            payload.len() <= MAX_CONTROL_FRAME_BYTES,
            "a full playlist page is {} bytes, over the {MAX_CONTROL_FRAME_BYTES} byte frame limit",
            payload.len()
        );
        let summaries = (0..MAX_PAGE_SIZE)
            .map(|_| RemotePlaylistSummary {
                playlist_id: "77abf082-7075-4d36-afe2-e9710ac6b33c".into(),
                title: "t".repeat(256),
                author: "a".repeat(64),
                display_name: "d".repeat(256),
                track_count: 500,
                private: false,
                updated_at: 1_787_680_200,
            })
            .collect::<Vec<_>>();
        let payload =
            serde_json::to_vec(&ServerResponse::Playlists {
                playlists: summaries,
                total: 500,
            })
            .unwrap();
        assert!(
            payload.len() <= MAX_CONTROL_FRAME_BYTES,
            "a full playlist list is {} bytes, over the {MAX_CONTROL_FRAME_BYTES} byte frame limit",
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
                position_ms: 3_600_000,
            },
        };
        let payload = serde_json::to_vec(&request).unwrap();
        assert!(
            payload.len() <= MAX_CONTROL_FRAME_BYTES,
            "a full play queue is {} bytes, over the {MAX_CONTROL_FRAME_BYTES} byte frame limit",
            payload.len()
        );
    }

    /// A handoff answers with the track the host was playing and its whole
    /// queue, and that answer has to fit in one control frame too.
    #[test]
    fn a_full_handoff_answer_fits_in_one_control_frame() {
        let state = RemotePlaybackState {
            active: true,
            playing: true,
            file_id: "a".repeat(64),
            title: "t".repeat(256),
            artist: "a".repeat(256),
            album: "l".repeat(256),
            position_ms: 3_600_000,
            duration_ms: 3_600_000,
            volume: 1.0,
            queue_len: MAX_PLAY_QUEUE,
            queue_index: 0,
            track: Some(RemoteTrack {
                file_id: "a".repeat(64),
                filename: "f".repeat(256),
                title: "t".repeat(256),
                artist: "a".repeat(256),
                album: "l".repeat(256),
                format: "FLAC".into(),
                mime: "audio/flac".into(),
                size: u64::MAX,
                tags: "g".repeat(256),
                local: true,
                sources: Vec::new(),
            }),
            queue: (0..MAX_PLAY_QUEUE)
                .map(|index| format!("{index:064x}"))
                .collect(),
            remote_control: true,
            error: String::new(),
            updated_at: 1_787_680_200,
            ..RemotePlaybackState::default()
        };
        let payload =
            serde_json::to_vec(&ServerResponse::Playback { state: state.clone() }).unwrap();
        assert!(
            payload.len() <= MAX_CONTROL_FRAME_BYTES,
            "a full handoff answer is {} bytes, over the {MAX_CONTROL_FRAME_BYTES} byte frame limit",
            payload.len()
        );
        // And the largest queue a phone may hand over can be looked up again,
        // which is how it becomes playable tracks on the phone.
        let request = ClientRequest::LibraryByIds {
            file_ids: (0..MAX_TRACKS_BY_ID)
                .map(|index| format!("{index:064x}"))
                .collect(),
        };
        assert!(serde_json::to_vec(&request).unwrap().len() <= MAX_CONTROL_FRAME_BYTES);
        let answer = ServerResponse::LibraryByIds {
            tracks: (0..MAX_TRACKS_BY_ID)
                .map(|_| state.track.clone().unwrap())
                .collect(),
        };
        let payload = serde_json::to_vec(&answer).unwrap();
        assert!(
            payload.len() <= MAX_CONTROL_FRAME_BYTES,
            "a full page of tracks by id is {} bytes, over the {MAX_CONTROL_FRAME_BYTES} byte frame limit",
            payload.len()
        );
    }
}
