export type RemoteSource = { pubkey: string; displayName: string };

/**
 * One public message about one track.
 *
 * The name and the npub arrive resolved, because this phone holds no Nostr
 * identity: it cannot ask a relay for a profile, so the computer sends the names
 * it has already seen.
 */
export type RemoteDiscussionMessage = {
  eventId: string;
  pubkey: string;
  npub: string;
  displayName: string;
  content: string;
  /** The event's own timestamp, in seconds since the epoch. */
  createdAt: number;
  /** The message this one answers, when it says. */
  replyTo?: string;
  /**
   * What the parent said, resolved by the computer: this phone holds no relay pool
   * and should not have to fetch a parent to draw one line of context.
   */
  reply?: RemoteDiscussionReply;
};

/** What a message in a conversation is answering. */
export type RemoteDiscussionReply = {
  author: string;
  excerpt: string;
};

/**
 * How much conversation a track has attracted.
 *
 * `authors` is what a mark shows: distinct people, not messages, because one
 * person can post as many messages as they like.
 */
export type RemoteDiscussionActivity = {
  fileId: string;
  authors: number;
  messages: number;
  lastAt: number;
};

export type RemoteTrack = {
  fileId: string;
  filename: string;
  title: string;
  artist: string;
  album: string;
  format: string;
  mime: string;
  size: number;
  tags: string;
  local: boolean;
  sources: RemoteSource[];
  /**
   * What the audio is, as the file itself declares it, so this phone can decide
   * before it spends data on it. Zero means the host could not tell, which is
   * what an older host sends and what a track from a peer's catalogue entry
   * looks like, because a catalogue entry says nothing about the audio in it.
   */
  bitrateKbps: number;
  sampleRateHz: number;
  channels: number;
  lossless: boolean;
  durationMs: number;
};

export type RemoteAudiobook = {
  audiobookId: string;
  title: string;
  author: string;
  narrator: string;
  totalSize: number;
  chapters: RemoteTrack[];
};

export type RemoteAudiobookSummary = Omit<RemoteAudiobook, 'chapters'> & {
  chapterCount: number;
};

export type AudiobookLibraryPage = {
  audiobooks: RemoteAudiobookSummary[];
  total: number;
};

/**
 * One computer this phone may talk to.
 *
 * There may be more than one: one of them is home - the one this phone signs,
 * downloads and browses as - and the others are libraries to read from.
 */
export type RemoteHost = {
  endpointId: string;
  desktopName: string;
  rights: {
    browse: boolean;
    fetch: boolean;
    control: boolean;
    /** Reach the network through this computer: search, browse, and download. */
    download: boolean;
    /** Sign and publish in the owner's name. */
    privileged: boolean;
  };
  /**
   * The home computer: the one whose status, player and download queue the app
   * is drawn from. Chosen in Settings when a phone holds more than one.
   */
  home: boolean;
  /** Whether this phone reads from it. A computer left out keeps its pairing. */
  included: boolean;
  /** Whether it answered just now. */
  online: boolean;
  /** Whether it may reach the network for this phone. */
  mayDownload: boolean;
};

export type CompanionStatus = {
  streamOnly: boolean;
  /**
   * Whether the home computer may reach the network for this phone.
   *
   * Beside `streamOnly` because they are two different questions: a phone lent
   * the network but not the owner's name is "read only" and may still download.
   */
  mayDownload: boolean;
  /** Whether the home computer may drive its own player for this phone. */
  mayControl: boolean;
  paired: boolean;
  connected: boolean;
  /**
   * A tunnel to the computer is being opened right now.
   *
   * Its own state rather than a flavour of `connected`: "connecting" is the app
   * doing something and asking to be waited for, and "offline" is nothing
   * happening at all. A cold start spends its first seconds in the first.
   */
  connecting: boolean;
  desktopName: string;
  endpointId: string;
  libraryRevision: number;
  /** Moves when the host's album art changes, so cached covers are re-asked. */
  coverRevision: number;
  /**
   * The computer's own public key, or empty while it has not said.
   *
   * A phone holds no key of its own, so this is the only thing that can tell a
   * playlist its computer wrote down from a public one somebody else published.
   */
  pubkey: string;
  error: string;
};

export type LibraryPage = { tracks: RemoteTrack[]; total: number };
export type CachedAudio = { url: string; track: RemoteTrack };

export type PodcastFeed = {
  id: number;
  title: string;
  author: string;
  description: string;
  feedUrl: string;
  image: string;
  language: string;
  episodeCount: number;
  genres: string[];
};

export type PodcastEpisode = {
  id: number;
  feedId: number;
  feedTitle: string;
  title: string;
  description: string;
  enclosureUrl: string;
  enclosureType: string;
  enclosureLength: number;
  datePublished: number;
  duration: number;
  image: string;
};

export type PodcastDownload = {
  episode: PodcastEpisode;
  progress: number;
  status: string;
  ready: boolean;
};
export type RemoteTransfer = { id: string; fileId: string; filename: string; size: number; progress: number; status: string; speed: string };

/** Repeating is a choice of three, matching the drawer and the desktop. */
export type RemoteRepeat = 'off' | 'all' | 'one';

/**
 * One transport instruction for the computer's own player. The phone never
 * plays the desktop's audio, so every one of these can still be refused.
 */
export type PlaybackCommand =
  | { type: 'play' }
  | { type: 'pause' }
  | { type: 'toggle' }
  | { type: 'stop' }
  | { type: 'next' }
  | { type: 'previous' }
  | { type: 'seek'; positionMs: number }
  | { type: 'volume'; percent: number }
  | { type: 'repeat'; mode: RemoteRepeat }
  | { type: 'shuffle'; enabled: boolean }
  /**
   * Take playback over from the computer: it stops, and answers with exactly
   * what it was doing, queue and all, so this phone can carry on from there.
   */
  | { type: 'handoff' }
  /**
   * Play one track, with the list the phone was showing as the queue.
   * `positionMs` is where to start inside the track, which is how a handover
   * resumes rather than restarting.
   */
  | { type: 'playTrack'; fileId: string; queue: string[]; positionMs?: number };

export type RemotePlaybackState = {
  active: boolean;
  playing: boolean;
  fileId: string;
  title: string;
  artist: string;
  album: string;
  positionMs: number;
  durationMs: number;
  volume: number;
  queueLen: number;
  queueIndex: number;
  /**
   * The computer's own record of the file it is playing, when it holds it.
   * Fetching the audio needs the real record's size, format and MIME, so a
   * track named by title alone could be shown here but never played.
   */
  track?: RemoteTrack | null;
  /**
   * File ids of the computer's queue, in its order. Only the answer to a
   * handoff carries them: a queue is far too big to repeat on every poll.
   */
  queue?: string[];
  repeat: RemoteRepeat;
  shuffle: boolean;
  /** True while a phone has driven the host recently. */
  remoteControl: boolean;
  error: string;
  updatedAt: number;
};

/** What a playlist is called and who published it, without its members. */
export type RemotePlaylistSummary = {
  playlistId: string;
  title: string;
  /** Author of a public playlist; empty for one only this computer holds. */
  author: string;
  displayName: string;
  /** The picture the playlist names, as a file id, or empty when it has none. */
  image: string;
  /**
   * The file the playlist opens with, so the row can draw that album's cover.
   * Only the id travels: the artist and album it is drawn from come off the
   * library, which answers for many file ids at once. Empty while the playlist
   * names nothing.
   */
  firstFileId: string;
  trackCount: number;
  /** True when it is private, so it only ever arrives from its own computer. */
  private: boolean;
  /** True once the coordinate has a revision the relays can answer with. */
  published: boolean;
  updatedAt: number;
};

/** A playlist named by its coordinate: the author and the id together. */
export type RemotePlaylistCoordinate = {
  author: string;
  playlistId: string;
};

/** A page of playlist names, which is what a browse costs. */
export type PlaylistPage = {
  playlists: RemotePlaylistSummary[];
  total: number;
};

/** One member of a playlist, in the order the playlist puts it in. */
export type RemotePlaylistTrack = {
  /** Where it sits in the playlist. The first member is 1. */
  position: number;
  fileId: string;
  /** Display hints, which a catalogue entry always wins over. */
  title: string;
  artist: string;
  album: string;
};

/** A playlist and one page of its members. */
export type RemotePlaylist = {
  playlistId: string;
  title: string;
  author: string;
  displayName: string;
  /** Album artist and release-group MBID, when it describes one release. */
  artist: string;
  mbid: string;
  /** The picture it names, as a file id, or empty when it has none. */
  image: string;
  /**
   * The author's own search words, comma-separated in the shape a catalogue
   * entry uses. These are the author's choice and outrank anything a client
   * would suggest, including the choice of having none.
   */
  tags: string;
  private: boolean;
  published: boolean;
  updatedAt: number;
  tracks: RemotePlaylistTrack[];
  /** Members the whole playlist names, so a page says how many are still to come. */
  total: number;
};

/** A read-only pairing code, minted by the computer, for another device. */
export type ReadOnlyTicketOffer = {
  uri: string;
  /** Empty when the host drew no QR, or drew something unsafe to insert. */
  qrSvg: string;
  expiresAt: number;
  desktopName: string;
};

export type CoverReport = { reportId: string; queued: boolean };

/** The NIP-56 reasons a cover report may use. */
export const reportReasons = [
  { value: 'spam', label: 'Spam or advertising' },
  { value: 'illegal', label: 'Illegal content' },
  { value: 'malware', label: 'Malware or a scam' },
  { value: 'impersonation', label: 'Wrong artist or album' },
  { value: 'nudity', label: 'Nudity' },
  { value: 'profanity', label: 'Profanity' },
  { value: 'other', label: 'Something else' }
] as const;
export type ReportReason = (typeof reportReasons)[number]['value'];
