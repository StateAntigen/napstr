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
/// Members one playlist may name, which is the spec's own limit. A phone edits a
/// playlist by sending the whole of it, so this is also the most a save can
/// carry - see `a_full_playlist_save_fits_in_one_control_frame`.
pub const MAX_PLAYLIST_MEMBERS: usize = 500;
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
    /// What the audio is, so a phone can decide before it spends a phone's data
    /// on it: "is this lossless, and how many kilobits is it" is a question only
    /// the file answers, and the answer decides whether somebody on a metered
    /// connection wants it at all.
    ///
    /// Each of these defaults, because a host older than this sends none of them
    /// and a phone must read that as "not known" rather than as a track with no
    /// bitrate. Bitrate is in kilobits per second and duration in milliseconds;
    /// zero in either means the host could not tell.
    #[serde(default)]
    pub bitrate_kbps: u32,
    #[serde(default)]
    pub sample_rate_hz: u32,
    #[serde(default)]
    pub channels: u32,
    #[serde(default)]
    pub lossless: bool,
    #[serde(default)]
    pub duration_ms: u64,
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

/// One public message about one file, as a phone draws it.
///
/// The name and the npub come resolved, because the phone holds no Nostr
/// identity of its own: it cannot ask a relay for a profile, and the host already
/// keeps the names it has seen.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDiscussionMessage {
    pub event_id: String,
    pub pubkey: String,
    pub npub: String,
    pub display_name: String,
    pub content: String,
    /// The event's own timestamp, in seconds since the epoch.
    pub created_at: u64,
    /// The message this one answers, when it says.
    #[serde(default)]
    pub reply_to: Option<String>,
    /// What the parent said, resolved by the host: a phone holds no relay pool and
    /// should not have to fetch a parent to draw one line of context.
    #[serde(default)]
    pub reply: Option<RemoteDiscussionReply>,
}

/// What a message in a conversation is answering.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDiscussionReply {
    /// A display name, or a short key when the host has never seen the author.
    pub author: String,
    /// The opening of the parent, bounded and sanitised by the host.
    pub excerpt: String,
}

/// How much conversation a file has attracted, as a row's mark reads it.
///
/// `authors` is the number of distinct people, which is not the number of
/// messages: one author talking to themselves is one author.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDiscussionActivity {
    pub file_id: String,
    pub authors: u32,
    pub messages: u32,
    /// The newest comment, in seconds since the epoch.
    pub last_at: u64,
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

/// A playlist named by its coordinate: its author and its id together.
///
/// The id is chosen by the author, so two authors may choose the same one, which
/// is why the author travels with it wherever a playlist is named rather than
/// being assumed from whoever is asking.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct RemotePlaylistCoordinate {
    pub author: String,
    pub playlist_id: String,
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
    /// The playlist's own artwork, when it has any, so a list can draw a picture
    /// without fetching every playlist's members.
    pub image: String,
    /// The first member's file id.
    ///
    /// A list row draws the album the playlist opens with, and asking each
    /// playlist for its members to find that out would be a page of them per row.
    /// Only the file id travels: a full page of names has to fit in one control
    /// frame, and the artist and album it is drawn from come off the library,
    /// which already answers for a hundred file ids at a time. Empty for a
    /// playlist that names nothing yet.
    pub first_file_id: String,
    /// Members the playlist names, which is also how many rows the members of it
    /// can be paged through.
    pub track_count: usize,
    /// True when the playlist is private. A private playlist is never published
    /// to a relay at all: its owner's computer stores it and serves it to a
    /// paired companion over the companion channel, and nowhere else. A relay
    /// would see the coordinate, the size and the edit time even with the body
    /// encrypted, which is exactly what a private playlist is for avoiding.
    pub private: bool,
    /// True once this coordinate has a revision the relays can answer with.
    ///
    /// It is not "this is the newest thing I hold": editing a playlist that was
    /// published keeps the flag, because the edit lives on this computer until
    /// the author publishes it again. It answers one question - has this
    /// coordinate ever been signed and sent - which is also the question that
    /// decides whether getting rid of the playlist means withdrawing it.
    pub published: bool,
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
    /// The playlist's own artwork, as the file id of a picture, or empty when it
    /// has none. A file id and never a URL: the picture travels as bytes over the
    /// ordinary transfer path, so no reader has to tell a third party that it is
    /// looking at this playlist.
    pub image: String,
    /// The author's own search words, comma-separated in the format a catalogue
    /// entry uses for its `tags`: what this playlist is meant to be found by.
    ///
    /// These are the author's choice and outrank anything a client would
    /// suggest, including the choice of having none. A publisher MUST NOT add
    /// words of its own on top of them.
    pub tags: String,
    pub private: bool,
    /// True once this coordinate has a revision on the relays, so a reader can
    /// tell a playlist it published from one it has only written down.
    pub published: bool,
    pub updated_at: i64,
    /// Members in `position` order, this page of them.
    pub tracks: Vec<RemotePlaylistTrack>,
    /// Members the whole playlist names, so a paged answer says how many are
    /// still to come.
    pub total: usize,
}

impl RemotePlaylist {
    /// The same playlist without its members, for a list that only needs names.
    ///
    /// The artwork hints come from the first member of the page this playlist is
    /// holding, so a summary is built from a page that starts at the top: a page
    /// taken from the middle would name the member it starts at.
    pub fn summary(&self) -> RemotePlaylistSummary {
        let first = self.tracks.first();
        RemotePlaylistSummary {
            playlist_id: self.playlist_id.clone(),
            title: self.title.clone(),
            author: self.author.clone(),
            display_name: self.display_name.clone(),
            image: self.image.clone(),
            first_file_id: first.map(|track| track.file_id.clone()).unwrap_or_default(),
            track_count: self.total,
            private: self.private,
            published: self.published,
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
        /// A seed for a shuffled browse order, or nothing for the stored one.
        ///
        /// A phone mints one when it starts and sends it with every page, so the
        /// pages it walks are one single order - and the order is derived from
        /// the seed rather than kept anywhere, which is why a desktop restart
        /// cannot change the order under a phone that is halfway through it, and
        /// why two paired phones never see the same one. It applies to the
        /// whole-library browse alone: a search that names something has an
        /// order of its own, and the closest match is what belongs first.
        #[serde(default)]
        shuffle_seed: Option<u64>,
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
        /// Answer with this computer's own playlists only.
        ///
        /// A phone's "add this track to a playlist" picker asks this way, because
        /// a playlist somebody else published is not a list to add to: editing one
        /// makes a copy of it, which is a different action from a tick in a
        /// picker. At library scale the picker also cannot afford to page past
        /// playlists the phone may not edit in order to reach the ones it may.
        #[serde(default)]
        own_only: bool,
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
    /// Which playlists name a file, as coordinates.
    ///
    /// A track's menu has to draw the playlists that already hold it, and that is
    /// one indexed lookup in the store rather than a page of members for every
    /// playlist in the list. Only the coordinates travel: the names, the counts
    /// and the artwork come from the playlist list the picker is already
    /// showing. This is a read, so it is also the one thing a read-only pairing
    /// can ask of a playlist.
    PlaylistsContaining {
        file_id: String,
    },
    /// An id for a playlist that has not been written down yet.
    ///
    /// The host mints it, because a playlist's identity is its name and role for
    /// its author rather than its contents, so it cannot be derived from
    /// anything: whoever creates a playlist has to be handed an id, and the host
    /// is the side that files it. An id on its own changes nothing anywhere, so
    /// this is the one write in this group that is safe to ask for idly.
    NewPlaylistId,
    /// Write a playlist down on the host without publishing it.
    ///
    /// The whole playlist travels, because a revision **is** the whole list: an
    /// edit that drops a member has to say so, and an incremental "remove the
    /// third one" would be a second way of describing a playlist that could
    /// disagree with the first. The host stamps the author and the edit time,
    /// exactly as it does for its own window, so a phone never gets to claim a
    /// coordinate that is not its owner's.
    SavePlaylist {
        playlist: RemotePlaylist,
    },
    /// Forget a playlist the host holds and has never published.
    ///
    /// An empty `author` means "whichever playlist this computer holds under
    /// that id". A published playlist is withdrawn instead, because forgetting it
    /// here alone would leave the revision on the relays standing.
    DeletePlaylist {
        #[serde(default)]
        author: String,
        playlist_id: String,
    },
    /// Sign a playlist and send it to the host's relays, keeping what comes
    /// back rather than what was sent.
    PublishPlaylist {
        playlist: RemotePlaylist,
        /// The author's answer to "suggest search words from the title?". It
        /// only applies while they have written no words of their own.
        #[serde(default)]
        suggest_tags: bool,
    },
    /// Take a published playlist back off the relays.
    WithdrawPlaylist {
        playlist_id: String,
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
    /// The conversation around one file, a page at a time, oldest of the page
    /// last.
    ///
    /// Reading is reading: a read-only pairing may ask, because these messages
    /// are public and the host is only fetching them. Posting is a different
    /// message, and a different permission.
    TrackDiscussion {
        file_id: String,
        /// Only messages written before this second. The oldest message of the
        /// last page is the cursor, so nothing is repeated - and a message written
        /// in the same second as the cursor is not skipped, because the host keeps
        /// the boundary inclusive rather than guessing.
        #[serde(default)]
        before: Option<u64>,
    },
    /// Say something in that conversation.
    ///
    /// Signed by the computer's identity, so it is published under the user's own
    /// name: a public act rather than a read, and one a read-only pairing is
    /// refused.
    SendTrackDiscussion {
        file_id: String,
        content: String,
        /// The message this one answers, when it answers one. NIP-C7 replies quote
        /// their parent, so this becomes a `q` tag and a reference at the front of
        /// the text rather than a second kind of event.
        #[serde(default)]
        reply_to: Option<String>,
    },
    /// How much conversation a page of files has attracted, for the marks on rows.
    /// The host answers about the files it is willing to name to a relay; a file
    /// it holds without having published is left out of the answer as well as out
    /// of the question.
    TrackDiscussionActivity {
        file_ids: Vec<String>,
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
    /// The playlists that name a file, by coordinate.
    PlaylistsContaining {
        playlists: Vec<RemotePlaylistCoordinate>,
    },
    /// An id for a playlist that does not exist yet.
    PlaylistId {
        playlist_id: String,
    },
    /// A playlist is gone: forgotten here, and withdrawn from the relays when it
    /// had ever been published.
    PlaylistRemoved,
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
    TrackDiscussion {
        messages: Vec<RemoteDiscussionMessage>,
    },
    TrackDiscussionSent {
        event_id: String,
    },
    TrackDiscussionActivity {
        activity: Vec<RemoteDiscussionActivity>,
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
        /// This computer's own public key: the author half of every playlist it
        /// wrote down.
        ///
        /// A companion has no key of its own and no other way to learn this one,
        /// and without it a playlist's `author` is just an opaque string: a phone
        /// could not tell its computer's own playlist from a public one somebody
        /// else published. It compares the two to decide what it may edit and
        /// what it may only read and copy.
        #[serde(default)]
        pubkey: String,
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

    /// The bytes a phone needs to decide about data, as the host writes them.
    ///
    /// Both directions of one shape: a host that sends the facts is read, and a
    /// host too old to send them is read as "not known" rather than as a track
    /// with no bitrate - which is what lets the two be updated apart.
    #[test]
    fn a_track_carries_what_its_audio_is_and_tolerates_a_host_that_says_nothing() {
        let sent = RemoteTrack {
            file_id: "a".repeat(64),
            filename: "song.flac".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            format: "FLAC".into(),
            mime: "audio/flac".into(),
            size: 42_000_000,
            tags: String::new(),
            local: true,
            sources: Vec::new(),
            bitrate_kbps: 900,
            sample_rate_hz: 48_000,
            channels: 2,
            lossless: true,
            duration_ms: 240_000,
        };
        let json = serde_json::to_string(&sent).unwrap();
        // These names are the contract: the phone is the other end of this type,
        // and its own copy reads exactly these keys.
        for field in [
            "bitrateKbps",
            "sampleRateHz",
            "channels",
            "lossless",
            "durationMs",
        ] {
            assert!(json.contains(field), "the wire is missing {field}: {json}");
        }
        assert_eq!(serde_json::from_str::<RemoteTrack>(&json).unwrap(), sent);

        let older = r#"{"fileId":"a","filename":"song.flac","title":"Song","artist":"Artist",
            "album":"Album","format":"FLAC","mime":"audio/flac","size":42000000,"tags":"",
            "local":true,"sources":[]}"#;
        let read = serde_json::from_str::<RemoteTrack>(older).unwrap();
        assert_eq!(read.format, "FLAC");
        assert_eq!(read.bitrate_kbps, 0, "a host that says nothing is not a 0 kb/s file");
        assert!(!read.lossless, "and it says nothing about losslessness either");
    }

    /// A phone reads a conversation, asks which rows are worth reading, and - if it
    /// was not lent read-only - says something in one.
    ///
    /// The wire names are the contract, so they are asserted rather than assumed,
    /// and so is the biggest answer this can produce: a page of the longest
    /// comments a host may carry, which has to fit in one control frame.
    #[test]
    fn discussion_messages_round_trip_and_a_page_of_them_fits_one_control_frame() {
        let file_id = "a".repeat(64);
        assert_eq!(
            serde_json::to_value(ClientRequest::TrackDiscussion {
                file_id: file_id.clone(),
                before: Some(1_800_000_000),
            })
            .unwrap(),
            serde_json::json!({
                "type": "trackDiscussion",
                "fileId": file_id,
                "before": 1_800_000_000u64,
            })
        );
        // An older phone sends no cursor at all, and asking for the newest page is
        // exactly what that means.
        let newest: ClientRequest = serde_json::from_str(&format!(
            r#"{{"type":"trackDiscussion","fileId":"{file_id}"}}"#
        ))
        .unwrap();
        assert_eq!(
            newest,
            ClientRequest::TrackDiscussion {
                file_id: file_id.clone(),
                before: None
            }
        );

        let messages = (0..100u32)
            .map(|index| RemoteDiscussionMessage {
                event_id: format!("{index:064x}"),
                pubkey: "b".repeat(64),
                npub: format!("npub1{}", "c".repeat(58)),
                display_name: "d".repeat(64),
                content: "e".repeat(500),
                created_at: 1_800_000_000,
                // A page of replies is the biggest this answer gets: every message
                // quoting the one before it.
                reply_to: Some(format!("{:064x}", index.saturating_sub(1))),
                reply: Some(RemoteDiscussionReply {
                    author: "f".repeat(64),
                    excerpt: "g".repeat(160),
                }),
            })
            .collect::<Vec<_>>();
        let payload = serde_json::to_vec(&ServerResponse::TrackDiscussion {
            messages: messages.clone(),
        })
        .unwrap();
        assert!(String::from_utf8_lossy(&payload).contains("replyTo"));
        assert!(
            payload.len() <= MAX_CONTROL_FRAME_BYTES,
            "a page of comments is {} bytes, over the {MAX_CONTROL_FRAME_BYTES} byte frame limit",
            payload.len()
        );
        assert_eq!(
            serde_json::from_slice::<ServerResponse>(&payload).unwrap(),
            ServerResponse::TrackDiscussion { messages }
        );

        // The marks on rows travel as counts rather than as conversations.
        assert_eq!(
            serde_json::to_value(ServerResponse::TrackDiscussionActivity {
                activity: vec![RemoteDiscussionActivity {
                    file_id: file_id.clone(),
                    authors: 3,
                    messages: 7,
                    last_at: 1_800_000_000,
                }]
            })
            .unwrap(),
            serde_json::json!({
                "type": "trackDiscussionActivity",
                "activity": [{ "fileId": file_id, "authors": 3, "messages": 7, "lastAt": 1_800_000_000u64 }],
            })
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
                stream_only: false,
                pubkey: String::new()
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
            pubkey: "c".repeat(64),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert_eq!(
            serde_json::from_str::<ServerResponse>(&json).unwrap(),
            response
        );
    }

    /// A phone that asks for the computer's own playlists alone.
    ///
    /// The flag is additive: a picker that does not send it is answered with
    /// every playlist, exactly as before it existed.
    #[test]
    fn the_playlist_list_may_be_asked_for_its_owners_own() {
        assert_eq!(
            serde_json::from_str::<ClientRequest>(r#"{"type":"playlists","offset":0,"limit":100}"#)
                .unwrap(),
            ClientRequest::Playlists {
                offset: 0,
                limit: 100,
                own_only: false
            }
        );
        let own = ClientRequest::Playlists {
            offset: 100,
            limit: 50,
            own_only: true,
        };
        assert_eq!(
            serde_json::from_str::<ClientRequest>(&serde_json::to_string(&own).unwrap()).unwrap(),
            own
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
                pubkey: String::new(),
            }
        );
    }

    #[test]
    fn a_host_that_does_not_say_who_it_is_answers_with_no_key() {
        // An older desktop sends no `pubkey`. Reading it as empty is what lets a
        // newer phone treat every playlist as somebody else's - the safe way
        // round, because the actions that are wrong to offer are the writes.
        assert_eq!(
            serde_json::from_str::<ServerResponse>(
                r#"{"type":"status","libraryRevision":3,"streamOnly":false}"#
            )
            .unwrap(),
            ServerResponse::Status {
                library_revision: 3,
                cover_revision: 0,
                stream_only: false,
                pubkey: String::new(),
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
                limit: 100,
                own_only: false
            })
            .unwrap(),
            r#"{"type":"playlists","offset":0,"limit":100,"ownOnly":false}"#
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

    #[test]
    fn playlists_round_trip() {
        let playlist = RemotePlaylist {
            playlist_id: "77abf082-7075-4d36-afe2-e9710ac6b33c".into(),
            title: "rock".into(),
            author: "a".repeat(64),
            display_name: "Sean Parker".into(),
            artist: "Metallica".into(),
            mbid: "60691bed-fdd7-32f9-92dc-b151aac9e271".into(),
            image: "c".repeat(64),
            tags: "driving, late night".into(),
            private: false,
            published: true,
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
        assert_eq!(summary.image, playlist.image, "a list can draw the picture it is told about");
        // The artwork a list row draws comes from the member the playlist opens
        // with, so the summary carries that one file id and not the members.
        assert_eq!(summary.first_file_id, playlist.tracks[0].file_id);
        let empty = RemotePlaylist::default().summary();
        assert!(empty.first_file_id.is_empty(), "a playlist that names nothing has no artwork");
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
        // The membership question a track's "add to playlist" picker asks, and
        // the coordinates it is answered with: the author is half the identity,
        // so it has to survive the wire with the id rather than being guessed
        // from whoever happens to be asking.
        let request = ClientRequest::PlaylistsContaining {
            file_id: "b".repeat(64),
        };
        assert_eq!(
            serde_json::from_str::<ClientRequest>(&serde_json::to_string(&request).unwrap())
                .unwrap(),
            request
        );
        let response = ServerResponse::PlaylistsContaining {
            playlists: vec![RemotePlaylistCoordinate {
                author: "a".repeat(64),
                playlist_id: "77abf082-7075-4d36-afe2-e9710ac6b33c".into(),
            }],
        };
        let wire = serde_json::to_string(&response).unwrap();
        assert_eq!(
            wire,
            format!(
                r#"{{"type":"playlistsContaining","playlists":[{{"author":"{}","playlistId":"77abf082-7075-4d36-afe2-e9710ac6b33c"}}]}}"#,
                "a".repeat(64)
            )
        );
        assert_eq!(
            serde_json::from_str::<ServerResponse>(&wire).unwrap(),
            response
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
        // "This coordinate is on the relays" is the difference between a
        // withdrawal and simply forgetting a playlist, so it travels too.
        let response: ServerResponse = serde_json::from_str(
            r#"{"type":"playlist","playlist":{"playlistId":"id","published":true}}"#,
        )
        .unwrap();
        assert!(matches!(
            response,
            ServerResponse::Playlist { playlist } if playlist.published
        ));
    }

    /// The write half of the playlist contract: a phone with write access can
    /// say all of this, and every one of them has to survive the wire.
    #[test]
    fn playlist_writes_round_trip() {
        let playlist = RemotePlaylist {
            playlist_id: "77abf082-7075-4d36-afe2-e9710ac6b33c".into(),
            title: "rock".into(),
            author: "a".repeat(64),
            display_name: "Sean Parker".into(),
            artist: String::new(),
            mbid: String::new(),
            image: String::new(),
            tags: "driving".into(),
            private: false,
            published: false,
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
        let requests = vec![
            ClientRequest::NewPlaylistId,
            ClientRequest::SavePlaylist {
                playlist: playlist.clone(),
            },
            ClientRequest::DeletePlaylist {
                author: playlist.author.clone(),
                playlist_id: playlist.playlist_id.clone(),
            },
            ClientRequest::PublishPlaylist {
                playlist: playlist.clone(),
                suggest_tags: true,
            },
            ClientRequest::WithdrawPlaylist {
                playlist_id: playlist.playlist_id.clone(),
            },
        ];
        for request in requests {
            assert_eq!(
                serde_json::from_str::<ClientRequest>(&serde_json::to_string(&request).unwrap())
                    .unwrap(),
                request
            );
        }
        // A phone that says nothing about suggested words is asking for none,
        // which is the direction that cannot put our words in someone's mouth.
        let quiet: ClientRequest = serde_json::from_str(
            r#"{"type":"publishPlaylist","playlist":{"playlistId":"id"}}"#,
        )
        .unwrap();
        assert!(matches!(
            quiet,
            ClientRequest::PublishPlaylist { suggest_tags, .. } if !suggest_tags
        ));
        // And a phone that names no author is asking about the one this computer
        // holds under that id, rather than about nobody's.
        let unowned: ClientRequest =
            serde_json::from_str(r#"{"type":"deletePlaylist","playlistId":"id"}"#).unwrap();
        assert!(matches!(
            unowned,
            ClientRequest::DeletePlaylist { author, .. } if author.is_empty()
        ));
        for response in [
            ServerResponse::PlaylistId {
                playlist_id: playlist.playlist_id,
            },
            ServerResponse::PlaylistRemoved,
        ] {
            assert_eq!(
                serde_json::from_str::<ServerResponse>(&serde_json::to_string(&response).unwrap())
                    .unwrap(),
                response
            );
        }
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
                // The picture is a member's worth of bytes on the wire, and the
                // list answer carries it too, so both are sized here.
                image: "c".repeat(64),
                // The author's own words, at the catalogue's maximum of 12.
                tags: (1..=12)
                    .map(|index| format!("w{index}"))
                    .collect::<Vec<_>>()
                    .join(", "),
                private: false,
                published: true,
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
                image: "c".repeat(64),
                // The one artwork hint a list row is given is a member's file id.
                first_file_id: "b".repeat(64),
                track_count: 500,
                private: false,
                published: true,
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
                bitrate_kbps: 4_608,
                sample_rate_hz: 96_000,
                channels: 2,
                lossless: true,
                duration_ms: 3_600_000,
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
