<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import {
    Format,
    checkPermissions,
    openAppSettings,
    requestPermissions,
    scan
  } from '@tauri-apps/plugin-barcode-scanner';
  import TrackArtwork from './lib/TrackArtwork.svelte';
  import TrackBadge from './lib/TrackBadge.svelte';
  import CoverDebug from './lib/CoverDebug.svelte';
  import { artworkHue, coverFor, coverKey, invalidateCoverNegatives, type AlbumCover } from './lib/artwork';
  import { reportReasons } from './lib/types';
  import type { AudiobookLibraryPage, CachedAudio, CompanionStatus, CoverReport, LibraryPage, PlaybackCommand, PodcastDownload, PodcastEpisode, PodcastFeed, ReadOnlyTicketOffer, RemoteAudiobook, RemoteAudiobookSummary, RemotePlaybackState, RemoteRepeat, RemoteTrack, RemoteTransfer, ReportReason } from './lib/types';

  const musicChips = ['Rock', 'Soundtrack', 'Punk', 'Folk', 'Upbeat'];
  const musicHistoryKey = 'napstrfy-played-albums';
  /** Matches the CSS transition, so the drawer unmounts once it has slid away. */
  const SHEET_ANIMATION_MS = 280;
  /** Fraction of the drawer's height a drag must cover to dismiss it. */
  const SHEET_DISMISS_RATIO = 0.2;
  /** Fraction of the screen a drag up on the collapsed bar must cover to open it. */
  const BAR_OPEN_RATIO = 0.2;
  /** Upward speed, in px/ms, that opens the drawer even on a short pull. */
  const BAR_FLING_SPEED = 0.35;
  /** Movement below this is a tap on the bar, not a pull. */
  const BAR_DRAG_SLOP = 6;
  /** Temporary: cover-art diagnostics overlay. Delete with CoverDebug.svelte. */
  const COVER_DEBUG = true;
  /** The host caps a library page at 200, so one album always fits. */
  const MAX_ALBUM_TRACKS = 200;
  /** Albums grouped out of the tracks this phone has loaded. */
  type AlbumShelf = {
    key: string;
    artist: string;
    album: string;
    representative: RemoteTrack;
    tracks: RemoteTrack[];
  };
  type ArtistShelf = { name: string; representative: RemoteTrack; count: number };
  type PlayedAlbum = { key: string; artist: string; album: string };
  const podcastGenres = ['Comedy', 'News', 'True Crime', 'Society & Culture', 'Technology', 'History', 'Business', 'Science', 'Arts', 'Sports', 'Education', 'Music'];
  const likedMusicKey = 'napstrfy-liked-music';
  const likedPodcastsKey = 'napstrfy-liked-podcasts';
  type AppTab = 'music' | 'search' | 'podcasts' | 'audiobooks';
  /** Repeating is a choice of three, and shuffling is independent of it. */
  type LoopMode = 'off' | 'all' | 'one';
  const LOOP_MODES: LoopMode[] = ['off', 'all', 'one'];
  const LOOP_LABELS: Record<LoopMode, string> = {
    off: 'Repeat off',
    all: 'Repeat all',
    one: 'Repeat this track'
  };
  const playModeKey = 'napstrfy-play-mode';
  type SleepOption = { value: string; label: string; minutes?: number; endsTrack?: boolean };
  const SLEEP_OPTIONS: SleepOption[] = [
    { value: '5', label: '5 minutes', minutes: 5 },
    { value: '10', label: '10 minutes', minutes: 10 },
    { value: '15', label: '15 minutes', minutes: 15 },
    { value: '30', label: '30 minutes', minutes: 30 },
    { value: '45', label: '45 minutes', minutes: 45 },
    { value: '60', label: '1 hour', minutes: 60 },
    { value: 'track', label: 'End of track', endsTrack: true }
  ];
  /** Everything the album view needs once its tracks have been gathered. */
  type AlbumView = {
    key: string;
    artist: string;
    album: string;
    year: string;
    art: string;
    tracks: RemoteTrack[];
    more: AlbumShelf[];
  };
  let activeTab = $state<AppTab>('music');
  let status = $state<CompanionStatus>({ streamOnly: false, paired: false, connected: false, desktopName: '', endpointId: '', libraryRevision: 0, coverRevision: 0, error: '' });
  let statusLoading = $state(true);
  let statusPending = $state(false);
  let pairingCode = $state('');
  let pairing = $state(false);
  let scanning = $state(false);
  let cameraPermissionDenied = $state(false);
  let error = $state('');
  let notice = $state('');
  let query = $state('');
  let tracks = $state<RemoteTrack[]>([]);
  let likedMusic = $state<RemoteTrack[]>([]);
  let showingLikedMusic = $state(false);
  let total = $state(0);
  let loading = $state(false);
  let loadingMore = $state(false);
  let musicViewVersion = 0;
  let loadedLibraryRevision = 0;
  let silentLibraryRefresh = false;
  let cacheReconciliationKey = '';
  let cacheReconciliationPending = false;
  let selected = $state<RemoteTrack | null>(null);
  let current = $state<RemoteTrack | null>(null);
  let playerQueue = $state<RemoteTrack[]>([]);
  let playerQueueLibraryVisible = true;
  let playerIndex = $state(-1);
  let loopMode = $state<LoopMode>('all');
  let shuffle = $state(false);
  let randomHistory = $state<number[]>([]);
  let randomHistoryIndex = $state(-1);
  let randomUpcoming = $state(-1);
  let playing = $state(false);
  let caching = $state(false);
  let currentTime = $state(0);
  let duration = $state(0);
  let volume = $state(0.85);
  let pending = $state(new Map<string, string>());
  let pendingAudiobooks = $state(new Map<string, string>());
  let transfers = $state<RemoteTransfer[]>([]);
  let audiobookQuery = $state('');
  let audiobooks = $state<RemoteAudiobookSummary[]>([]);
  let audiobookTotal = $state(0);
  let selectedAudiobook = $state<RemoteAudiobook | null>(null);
  let audiobookLoading = $state(false);
  let podcastQuery = $state('');
  let podcastFeeds = $state<PodcastFeed[]>([]);
  let likedPodcasts = $state<PodcastFeed[]>([]);
  let showingLikedPodcasts = $state(false);
  let podcastGenre = $state('');
  let selectedPodcast = $state<PodcastFeed | null>(null);
  let podcastEpisodes = $state<PodcastEpisode[]>([]);
  let podcastHistory = $state<PodcastEpisode[]>([]);
  let podcastDownloads = $state<PodcastDownload[]>([]);
  let podcastLoading = $state(false);
  let podcastViewVersion = 0;
  let currentPodcast = $state<PodcastEpisode | null>(null);
  let activeMedia = $state<'music' | 'podcast'>('music');
  // The expandable now-playing sheet. Audiobook chapters run through the same
  // player as music, so the library flag is what distinguishes "music only".
  let showNowPlaying = $state(false);
  let nowCover = $state<AlbumCover | null>(null);
  let nowArtFailed = $state(false);
  let sheetDragY = $state(0);
  let sheetDragging = $state(false);
  let sheetDragPending = $state(false);
  /** False when the gesture began inside a queue that is scrolled down. */
  let sheetDragAllowed = false;
  let sheetDragStart = 0;
  /** Scroll offset when the gesture began: a scrolled list scrolls, it does not drag. */
  let sheetScrollTop = 0;
  /** Set for one frame on open so the drawer slides up instead of appearing. */
  let sheetEntering = $state(false);
  /** Set while sliding away; the drawer unmounts when the animation ends. */
  let sheetClosing = $state(false);
  /** The playlist lives in its own view so the drawer never scrolls. */
  let showQueue = $state(false);
  /** The track menu, opened from the drawer or from any album track. */
  let showActions = $state(false);
  let showSleepOptions = $state(false);
  let actionTrack = $state<RemoteTrack | null>(null);
  let sleepValue = $state('');
  let sleepEndsAt = $state(0);
  let sleepClock = $state('');
  /** Album preview, split into distinct sections. */
  let showAlbumView = $state(false);
  let albumView = $state<AlbumView | null>(null);
  /** File ids held in this phone's audio cache, for the storage badge. */
  let cachedFileIds = $state<Set<string>>(new Set());
  let showSettings = $state(false);
  /** The computer's own player, when this phone is allowed to drive it. */
  let showRemote = $state(false);
  let remoteState = $state<RemotePlaybackState | null>(null);
  let remoteBusy = $state(false);
  let remoteError = $state('');
  /** A read-only code the computer minted for somebody else to scan. */
  let readOnlyTicket = $state<ReadOnlyTicketOffer | null>(null);
  let ticketBusy = $state(false);
  let ticketError = $state('');
  /** NIP-56: reporting the cover on an album, opened from the track menu. */
  let showReport = $state(false);
  let reportKey = $state('');
  let reportLabel = $state('');
  let reportReason = $state<ReportReason>('spam');
  let reportNote = $state('');
  let reportBusy = $state(false);
  let reportError = $state('');
  /** The album view's scroller, so opening another album can jump to the top. */
  let albumScroll = $state<HTMLDivElement | undefined>(undefined);

  $effect(() => {
    // A different album means a fresh page, not the last one's scroll position.
    const key = albumView?.key;
    const element = albumScroll;
    if (key && element) element.scrollTop = 0;
  });
  let sheetElement = $state<HTMLDivElement | undefined>(undefined);
  let sheetScroller = $state<HTMLDivElement | undefined>(undefined);
  let sheetCloseTimer = 0;
  /** Dragging the collapsed bar upward pulls the drawer into view. */
  let barElement = $state<HTMLButtonElement | undefined>(undefined);
  let barDragging = $state(false);
  let barDragTravelled = $state(0);
  let barDragStart = 0;
  let barDragLastY = 0;
  let barDragLastAt = 0;
  let barDragSpeed = 0;
  /** Where the bar rests: the drawer's top edge starts level with it. */
  let barRestTop = $state(0);
  let barSwallowClick = false;

  /** How far the finger must pull to open, which is also how far the bar fades. */
  let barOpenTravel = $derived(Math.max(1, barRestTop * BAR_OPEN_RATIO));
  /** The bar rides up with the drawer and is gone by the time it would open. */
  let barShift = $derived(barDragging ? -barDragTravelled : 0);
  let barFade = $derived(barDragging ? Math.max(0, 1 - barDragTravelled / barOpenTravel) : 1);
  /** The played portion of the card, starting where the artwork ends. */
  let barProgress = $derived(duration > 0 ? Math.min(1, Math.max(0, currentTime / duration)) : 0);
  /** Whichever track the open menu applies to: the chosen one, else the current. */
  let menuTrack = $derived(actionTrack ?? (activeMedia === 'music' ? current : null));
  let nowTitle = $derived(
    activeMedia === 'podcast' && currentPodcast
      ? currentPodcast.title
      : current
        ? title(current)
        : 'Choose something to play'
  );
  let nowArtist = $derived(
    activeMedia === 'podcast' && currentPodcast
      ? currentPodcast.feedTitle
      : current
        ? artist(current)
        : 'Music and podcasts, wherever you are'
  );
  /** A title wider than the card scrolls rather than being cut in half. */
  let titleClipper = $state<HTMLDivElement | undefined>(undefined);
  let titleText = $state<HTMLSpanElement | undefined>(undefined);
  let titleOverflows = $state(false);

  $effect(() => {
    const text = nowTitle;
    if (!text) {
      titleOverflows = false;
      return;
    }
    const clipper = titleClipper;
    const element = titleText;
    if (!clipper || !element) return;
    // Measure once the new title has been laid out.
    const frame = window.requestAnimationFrame(() => {
      titleOverflows = element.scrollWidth > clipper.clientWidth + 1;
    });
    return () => window.cancelAnimationFrame(frame);
  });
  /** The navigation bar slides away with the drawer instead of being covered. */
  let navShift = $derived(
    showNowPlaying ? Math.min(1, Math.max(0, 1 - sheetDragY / Math.max(1, barRestTop))) : 0
  );
  let discoverAlbums = $state<AlbumShelf[]>([]);
  let discoverSeed = '';
  let playedAlbums = $state<PlayedAlbum[]>(readPlayedAlbums());
  let audio: HTMLAudioElement;
  let lastSystemMediaSync = 0;

  type AndroidMediaBridge = {
    update(payload: string): void;
    clear(): void;
  };

  function androidMediaBridge(): AndroidMediaBridge | undefined {
    return (window as Window & { NapstrfyMedia?: AndroidMediaBridge }).NapstrfyMedia;
  }

  type AndroidBackBridge = {
    setDrawerOpen(open: boolean): void;
  };

  function androidBackBridge(): AndroidBackBridge | undefined {
    return (window as Window & { NapstrfyBack?: AndroidBackBridge }).NapstrfyBack;
  }

  /** The hardware back button arrives as an event, not a callback. */
  function handleSystemBack() {
    if (showReport) {
      showReport = false;
      return;
    }
    if (showRemote) {
      showRemote = false;
      return;
    }
    if (showSettings) {
      showSettings = false;
      return;
    }
    if (showActions) {
      closeActions();
      return;
    }
    if (showAlbumView) {
      closeAlbumView();
      return;
    }
    if (showQueue) {
      showQueue = false;
      return;
    }
    if (showNowPlaying) closeNowPlaying();
  }

  function title(track: RemoteTrack) {
    return track.title || track.filename;
  }

  function artist(track: RemoteTrack) {
    return track.artist || 'Unknown artist';
  }

  function isStoredTrack(value: unknown): value is RemoteTrack {
    if (!value || typeof value !== 'object') return false;
    const item = value as Partial<RemoteTrack>;
    return typeof item.fileId === 'string' && item.fileId.length <= 128 &&
      typeof item.filename === 'string' && item.filename.length <= 500 &&
      typeof item.title === 'string' && typeof item.artist === 'string' &&
      typeof item.album === 'string' && typeof item.format === 'string' &&
      typeof item.mime === 'string' && typeof item.size === 'number' &&
      typeof item.tags === 'string' && typeof item.local === 'boolean' &&
      Array.isArray(item.sources);
  }

  function isStoredPodcast(value: unknown): value is PodcastFeed {
    if (!value || typeof value !== 'object') return false;
    const item = value as Partial<PodcastFeed>;
    return typeof item.id === 'number' && Number.isFinite(item.id) &&
      typeof item.title === 'string' && item.title.length <= 500 &&
      typeof item.author === 'string' && typeof item.description === 'string' &&
      typeof item.feedUrl === 'string' && typeof item.image === 'string' &&
      typeof item.language === 'string' && typeof item.episodeCount === 'number';
  }

  function saveLikes(key: string, value: unknown) {
    try {
      window.localStorage.setItem(key, JSON.stringify(value));
    } catch {
      error = 'Napstrfy could not save that favourite on this phone.';
    }
  }

  function isTrackLiked(track: RemoteTrack) {
    return likedMusic.some((item) => item.fileId === track.fileId);
  }

  function toggleTrackLike(track: RemoteTrack) {
    likedMusic = isTrackLiked(track)
      ? likedMusic.filter((item) => item.fileId !== track.fileId)
      : [track, ...likedMusic.filter((item) => item.fileId !== track.fileId)].slice(0, 1000);
    saveLikes(likedMusicKey, likedMusic);
    if (showingLikedMusic) {
      tracks = [...likedMusic];
      total = tracks.length;
      if (!tracks.some((item) => item.fileId === selected?.fileId)) selected = tracks[0] ?? null;
    }
  }

  function isPodcastLiked(feed: PodcastFeed) {
    return likedPodcasts.some((item) => item.id === feed.id);
  }

  function togglePodcastLike(feed: PodcastFeed) {
    likedPodcasts = isPodcastLiked(feed)
      ? likedPodcasts.filter((item) => item.id !== feed.id)
      : [feed, ...likedPodcasts.filter((item) => item.id !== feed.id)].slice(0, 500);
    saveLikes(likedPodcastsKey, likedPodcasts);
    if (showingLikedPodcasts) podcastFeeds = [...likedPodcasts];
  }

  function usePodcastArtwork(event: Event, fallback: string) {
    const image = event.currentTarget as HTMLImageElement;
    if (fallback && image.getAttribute('src') !== fallback) {
      image.src = fallback;
    } else {
      image.remove();
    }
  }

  function showLikedTracks() {
    musicViewVersion += 1;
    activeTab = 'music';
    showingLikedMusic = !showingLikedMusic;
    if (!showingLikedMusic) {
      void searchTracks(query);
      return;
    }
    loading = false;
    loadingMore = false;
    tracks = [...likedMusic];
    total = tracks.length;
    selected = tracks[0] ?? null;
  }

  function randomIndexExcept(currentIndex: number) {
    if (playerQueue.length < 2) return -1;
    const played = new Set(randomHistory);
    const pool: number[] = [];
    for (let index = 0; index < playerQueue.length; index += 1) {
      if (index === currentIndex || played.has(index)) continue;
      pool.push(index);
    }
    // A finished cycle only starts again when the queue is set to repeat.
    if (pool.length === 0 && loopMode !== 'off') {
      for (let index = 0; index < playerQueue.length; index += 1) {
        if (index !== currentIndex) pool.push(index);
      }
    }
    if (pool.length === 0) return -1;
    return pool[Math.floor(Math.random() * pool.length)];
  }

  function resetRandomOrder() {
    randomHistory = playerIndex >= 0 ? [playerIndex] : [];
    randomHistoryIndex = randomHistory.length - 1;
    randomUpcoming = randomIndexExcept(playerIndex);
  }

  function savePlaySettings() {
    try {
      window.localStorage.setItem(playModeKey, JSON.stringify({ loop: loopMode, shuffle }));
    } catch {
      // A preference that cannot be stored is only a lost convenience.
    }
  }

  function cycleLoopMode() {
    loopMode = LOOP_MODES[(LOOP_MODES.indexOf(loopMode) + 1) % LOOP_MODES.length];
    // The unplayed pool depends on whether a finished cycle may start again.
    resetRandomOrder();
    savePlaySettings();
    syncSystemMedia(true);
  }

  function toggleShuffle() {
    shuffle = !shuffle;
    resetRandomOrder();
    savePlaySettings();
    syncSystemMedia(true);
  }

  function readableSize(size: number) {
    if (size < 1024 * 1024) return `${Math.max(1, Math.round(size / 1024))} KB`;
    return `${(size / 1024 / 1024).toFixed(size >= 10 * 1024 * 1024 ? 0 : 1)} MB`;
  }

  function clock(seconds: number) {
    if (!Number.isFinite(seconds)) return '0:00';
    const whole = Math.max(0, Math.floor(seconds));
    return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, '0')}`;
  }

  async function refreshStatus(showError = false, syncLibrary = true) {
    if (statusPending) return;
    statusPending = true;
    try {
      const wasConnected = status.connected;
      status = await invoke<CompanionStatus>('companion_status');
      if (showError && status.error) error = status.error;
      if (status.connected) {
        void reconcileAudioCache();
        // Covers are cached here, the albums the host had nothing for included,
        // so the host's own count of how often its art changed is what tells
        // this phone to ask again instead of trusting an answer that has aged.
        invalidateCoverNegatives(status.coverRevision);
        if (syncLibrary && (!wasConnected || (status.libraryRevision > 0
          && loadedLibraryRevision > 0 && status.libraryRevision !== loadedLibraryRevision))) {
          void refreshLibrarySilently(status.libraryRevision);
        }
      }
    } catch (nextError) {
      if (showError) error = String(nextError);
    } finally {
      statusLoading = false;
      statusPending = false;
    }
  }

  async function loadCachedLibrary() {
    try {
      const offline = await invoke<LibraryPage & { paired: boolean; desktopName: string; streamOnly: boolean }>('cached_library');
      if (offline.paired) {
        status = { ...status, paired: true, desktopName: offline.desktopName, streamOnly: offline.streamOnly };
      }
      tracks = offline.tracks;
      total = offline.total;
      // These are exactly the files this phone holds, which is the first of
      // the three storage states the badges describe.
      cachedFileIds = new Set(offline.tracks.map((track) => track.fileId));
      if (!selected || !tracks.some((track) => track.fileId === selected?.fileId)) selected = tracks[0] ?? null;
    } catch {
      // A damaged cache must never prevent pairing or normal online use.
    }
  }

  /** Re-reads the phone's audio cache after a download lands. */
  async function refreshCachedIds() {
    try {
      const offline = await invoke<LibraryPage>('cached_library');
      cachedFileIds = new Set(offline.tracks.map((track) => track.fileId));
    } catch {
      // Offline or a damaged cache: the badge falls back to host and network.
    }
  }

  async function reconcileAudioCache() {
    if (!status.connected || cacheReconciliationPending) return;
    const key = `${status.endpointId}:${status.libraryRevision}`;
    if (cacheReconciliationKey === key) return;
    cacheReconciliationPending = true;
    try {
      const complete = await invoke<boolean>('reconcile_audio_cache', {
        protectedFileIds: playing && activeMedia === 'music' && current ? [current.fileId] : []
      });
      if (complete) cacheReconciliationKey = key;
    } catch (nextError) {
      // Older Napstr versions do not implement cache reconciliation. Preserve
      // every offline file and avoid repeatedly asking during this session.
      if (/invalid Napstrfy request|unexpected response/i.test(String(nextError))) {
        cacheReconciliationKey = key;
      }
    } finally {
      cacheReconciliationPending = false;
    }
  }

  async function reconnect() {
    await refreshStatus(true, false);
    if (status.connected) await loadLibrary();
  }

  async function pair(code = pairingCode) {
    if (!code.trim() || pairing) return;
    pairing = true;
    error = '';
    try {
      const platform = /iPhone|iPad|iPod/i.test(navigator.userAgent) ? 'iPhone' : 'Android phone';
      const desktop = await invoke<string>('pair_desktop', { code: code.trim(), deviceName: `Napstrfy on ${platform}` });
      pairingCode = '';
      notice = `Connected to ${desktop}`;
      await refreshStatus();
      await loadLibrary();
    } catch (nextError) {
      error = String(nextError);
    } finally {
      pairing = false;
    }
  }

  async function scanCode() {
    if (scanning || pairing) return;
    error = '';
    cameraPermissionDenied = false;
    scanning = true;
    try {
      let permission = await checkPermissions();
      if (permission !== 'granted') permission = await requestPermissions();
      if (permission !== 'granted') {
        cameraPermissionDenied = true;
        error = 'Camera access is required to scan the Napstr pairing code.';
        return;
      }

      const result = await scan({
        cameraDirection: 'back',
        formats: [Format.QRCode],
        windowed: false
      });
      pairingCode = result.content;
      await pair(result.content);
    } catch (nextError) {
      const message = String(nextError);
      if (!/cancel/i.test(message)) {
        cameraPermissionDenied = /permission/i.test(message);
        error = cameraPermissionDenied
          ? 'Camera access is required to scan the Napstr pairing code.'
          : `Could not open the QR scanner: ${message}`;
      }
    } finally {
      scanning = false;
    }
  }

  async function showCameraSettings() {
    try {
      await openAppSettings();
    } catch (nextError) {
      error = `Could not open Android settings: ${String(nextError)}`;
    }
  }

  async function forgetDesktop() {
    if (!window.confirm('Disconnect this phone from Napstr? You will need to scan a new QR code.')) return;
    await invoke('forget_desktop');
    status = { streamOnly: false, paired: false, connected: false, desktopName: '', endpointId: '', libraryRevision: 0, coverRevision: 0, error: '' };
    tracks = [];
    current = null;
    audio?.pause();
  }

  async function loadLibrary(append = false) {
    if (!status.paired || loading || loadingMore) return;
    const viewVersion = ++musicViewVersion;
    showingLikedMusic = false;
    append ? (loadingMore = true) : (loading = true);
    error = '';
    try {
      const page = await invoke<LibraryPage>('remote_library', {
        query: query.trim(),
        offset: append ? tracks.length : 0,
        limit: 100
      });
      if (viewVersion !== musicViewVersion) return;
      tracks = append ? [...tracks, ...page.tracks] : page.tracks;
      total = page.total;
      loadedLibraryRevision = status.libraryRevision;
      if (!selected || !tracks.some((track) => track.fileId === selected?.fileId)) selected = tracks[0] ?? null;
    } catch (nextError) {
      if (viewVersion === musicViewVersion) error = String(nextError);
    } finally {
      if (viewVersion === musicViewVersion) {
        loading = false;
        loadingMore = false;
      }
    }
  }

  async function refreshLibrarySilently(revision: number) {
    if (silentLibraryRefresh || loading || loadingMore || !status.connected) return;
    if (showingLikedMusic || query.trim()) {
      // These views issue a fresh request when the user opens or submits them.
      loadedLibraryRevision = revision;
      return;
    }
    silentLibraryRefresh = true;
    try {
      const page = await invoke<LibraryPage>('remote_library', { query: '', offset: 0, limit: 100 });
      tracks = page.tracks;
      total = page.total;
      loadedLibraryRevision = revision;
      if (!selected || !tracks.some((track) => track.fileId === selected?.fileId)) selected = tracks[0] ?? null;
    } catch {
      // Keep the current list visible and retry after the next status check.
    } finally {
      silentLibraryRefresh = false;
    }
  }

  async function searchTracks(nextQuery = query) {
    query = nextQuery;
    showingLikedMusic = false;
    if (!query.trim()) return loadLibrary();
    if (loading) return;
    const viewVersion = ++musicViewVersion;
    loading = true;
    error = '';
    try {
      // Two passes. The host answers for its own folder straight away, so those
      // results appear before the relay round trip has finished; the network
      // search then only adds what the local pass did not already have.
      const local = await invoke<LibraryPage>('remote_library', {
        query: query.trim(),
        offset: 0,
        limit: MAX_ALBUM_TRACKS
      });
      if (viewVersion !== musicViewVersion) return;
      const known = new Set(local.tracks.map((track) => track.fileId));
      tracks = local.tracks;
      total = local.tracks.length;
      selected = tracks[0] ?? null;
      try {
        const found = await invoke<RemoteTrack[]>('remote_search', { query: query.trim() });
        if (viewVersion !== musicViewVersion) return;
        const merged = [
          ...local.tracks,
          ...found.filter((track) => !known.has(track.fileId))
        ];
        tracks = merged;
        total = merged.length;
        if (!selected || !merged.some((track) => track.fileId === selected?.fileId)) {
          selected = merged[0] ?? null;
        }
      } catch (networkError) {
        // The host's own files are still worth showing when the network is out.
        if (viewVersion === musicViewVersion) {
          notice = `Showing results from Napstr only: ${String(networkError)}`;
        }
      }
    } catch (nextError) {
      if (viewVersion === musicViewVersion) error = String(nextError);
    } finally {
      if (viewVersion === musicViewVersion) loading = false;
    }
  }

  async function showAudiobooks() {
    activeTab = 'audiobooks';
    if (audiobooks.length === 0) await loadAudiobooks();
  }

  async function loadAudiobooks() {
    if (!status.connected || audiobookLoading) return;
    audiobookLoading = true;
    selectedAudiobook = null;
    error = '';
    try {
      const page = await invoke<AudiobookLibraryPage>('remote_audiobook_library', {
        query: audiobookQuery.trim(), offset: 0, limit: 100
      });
      audiobooks = page.audiobooks;
      audiobookTotal = page.total;
    } catch (nextError) {
      error = String(nextError);
    } finally {
      audiobookLoading = false;
    }
  }

  async function openAudiobook(book: RemoteAudiobookSummary) {
    if (audiobookLoading) return;
    audiobookLoading = true;
    error = '';
    try {
      selectedAudiobook = await invoke<RemoteAudiobook>('remote_audiobook', { audiobookId: book.audiobookId });
    } catch (nextError) {
      error = String(nextError);
    } finally {
      audiobookLoading = false;
    }
  }

  async function activateAudiobookChapter(book: RemoteAudiobook, track: RemoteTrack) {
    activeMedia = 'music';
    selected = track;
    if (!track.local) {
      await requestDownload(track, audiobookDestinationFolder(book), book.audiobookId);
      return;
    }
    playerQueue = book.chapters.filter((chapter) => chapter.local);
    playerQueueLibraryVisible = false;
    playerIndex = playerQueue.findIndex((chapter) => chapter.fileId === track.fileId);
    resetRandomOrder();
    await playTrack(track);
  }

  async function activateTrack(track: RemoteTrack) {
    activeMedia = 'music';
    selected = track;
    if (!track.local) {
      await requestDownload(track);
      return;
    }
    const queue = tracks.filter((item) => item.local);
    playerQueue = queue;
    playerQueueLibraryVisible = true;
    playerIndex = queue.findIndex((item) => item.fileId === track.fileId);
    resetRandomOrder();
    await playTrack(track);
  }

  async function playTrack(track: RemoteTrack, libraryVisible = playerQueueLibraryVisible) {
    if (caching) return;
    caching = true;
    error = '';
    current = track;
    activeMedia = 'music';
    try {
      audio?.pause();
      const cached = await invoke<CachedAudio>('cache_remote_audio', { track, libraryVisible });
      current = cached.track;
      await tick();
      audio.src = cached.url;
      audio.volume = volume;
      await audio.play();
      playing = true;
      rememberPlayedAlbum(cached.track);
      const nextIndex = shuffle
        ? randomUpcoming
        : playerQueue.length > 1
          ? (playerIndex + 1 < playerQueue.length ? playerIndex + 1 : loopMode === 'off' ? -1 : 0)
          : -1;
      const next = nextIndex >= 0 ? playerQueue[nextIndex] : undefined;
      if (next?.local) {
        void invoke('prefetch_remote_audio', {
          afterFileId: cached.track.fileId,
          track: next,
          libraryVisible
        });
      }
    } catch (nextError) {
      playing = false;
      error = `Could not play ${title(track)}: ${String(nextError)}`;
    } finally {
      caching = false;
    }
  }

  function audiobookDestinationFolder(book: RemoteAudiobook) {
    const title = book.title
      .replace(/[\x00-\x1f/\\:*?"<>|]/g, '_')
      .replace(/^[.\s]+|[.\s]+$/g, '')
      .slice(0, 86) || 'Audiobook';
    return `${title} [${book.audiobookId.slice(0, 8)}]`;
  }

  async function requestDownload(
    track: RemoteTrack,
    destinationFolder: string | null = null,
    audiobookId: string | null = null
  ) {
    if (status.streamOnly) {
      error = 'This pairing is read only. It cannot ask Napstr to download songs.';
      return;
    }
    if (pending.has(track.fileId)) return;
    pending = new Map(pending).set(track.fileId, track.filename);
    if (audiobookId) pendingAudiobooks = new Map(pendingAudiobooks).set(track.fileId, audiobookId);
    error = '';
    try {
      await invoke<string>('remote_download', {
        fileId: track.fileId,
        sourcePubkeys: track.sources.map((source) => source.pubkey),
        destinationFolder
      });
      notice = `Napstr is downloading ${title(track)} over Tor`;
      await refreshTransfers();
    } catch (nextError) {
      const next = new Map(pending);
      next.delete(track.fileId);
      pending = next;
      const nextAudiobooks = new Map(pendingAudiobooks);
      nextAudiobooks.delete(track.fileId);
      pendingAudiobooks = nextAudiobooks;
      error = String(nextError);
    }
  }

  async function refreshTransfers() {
    if (!status.connected || status.streamOnly || pending.size === 0) return;
    try {
      transfers = await invoke<RemoteTransfer[]>('remote_transfers');
      for (const fileId of [...pending]) {
        const [pendingFileId, pendingFilename] = fileId;
        const transfer = transfers.find((item) => item.fileId === pendingFileId);
        if (transfer && /failed|cancel/i.test(transfer.status)) {
          const next = new Map(pending);
          next.delete(pendingFileId);
          pending = next;
          const nextAudiobooks = new Map(pendingAudiobooks);
          nextAudiobooks.delete(pendingFileId);
          pendingAudiobooks = nextAudiobooks;
          error = `${transfer.filename}: ${transfer.status}`;
          continue;
        }
        if (transfer && transfer.progress < 100 && !/complete|verified/i.test(transfer.status)) continue;
        const original = tracks.find((item) => item.fileId === pendingFileId)
          ?? selectedAudiobook?.chapters.find((item) => item.fileId === pendingFileId);
        const audiobookId = pendingAudiobooks.get(pendingFileId);
        let local: RemoteTrack | undefined;
        if (audiobookId) {
          const refreshed = await invoke<RemoteAudiobook>('remote_audiobook', { audiobookId });
          local = refreshed.chapters.find((item) => item.fileId === pendingFileId && item.local);
          if (selectedAudiobook?.audiobookId === audiobookId) selectedAudiobook = refreshed;
        } else {
          const page = await invoke<LibraryPage>('remote_library', { query: original?.filename || transfer?.filename || pendingFilename, offset: 0, limit: 20 });
          local = page.tracks.find((item) => item.fileId === pendingFileId);
        }
        if (!local) continue;
        tracks = tracks.map((item) => item.fileId === pendingFileId ? local : item);
        if (selectedAudiobook) selectedAudiobook = {
          ...selectedAudiobook,
          chapters: selectedAudiobook.chapters.map((chapter) => chapter.fileId === pendingFileId ? local : chapter)
        };
        if (likedMusic.some((item) => item.fileId === pendingFileId)) {
          likedMusic = likedMusic.map((item) => item.fileId === pendingFileId ? local : item);
          saveLikes(likedMusicKey, likedMusic);
        }
        if (selected?.fileId === pendingFileId) selected = local;
        const next = new Map(pending);
        next.delete(pendingFileId);
        pending = next;
        const nextAudiobooks = new Map(pendingAudiobooks);
        nextAudiobooks.delete(pendingFileId);
        pendingAudiobooks = nextAudiobooks;
        notice = `${title(local)} is ready to play`;
        void refreshCachedIds();
      }
    } catch { /* the next foreground poll retries */ }
  }

  function togglePlayer() {
    if (!current && !currentPodcast) return;
    if (audio.paused) audio.play().catch((nextError) => (error = String(nextError)));
    else audio.pause();
  }

  function syncSystemMedia(force = false) {
    const bridge = androidMediaBridge();
    const media = activeMedia === 'podcast' ? currentPodcast : current;
    if (!bridge || !media) return;
    const now = performance.now();
    if (!force && now - lastSystemMediaSync < 900) return;
    lastSystemMediaSync = now;
    bridge.update(JSON.stringify({
      title: activeMedia === 'podcast' ? currentPodcast?.title : current ? title(current) : '',
      artist: activeMedia === 'podcast' ? currentPodcast?.feedTitle : current ? artist(current) : '',
      artwork: activeMedia === 'podcast'
        ? currentPodcast?.image ?? ''
        : nowCover && !nowArtFailed ? nowCover.art || nowCover.thumb : '',
      playing,
      position: Number.isFinite(currentTime) ? currentTime : 0,
      duration: Number.isFinite(duration) ? duration : 0,
      canPrevious: activeMedia === 'music' && playerQueue.length > 1 && (!shuffle || randomHistoryIndex > 0),
      canNext: activeMedia === 'music' && playerQueue.length > 1,
      liked: activeMedia === 'music' && !!current && isTrackLiked(current),
      looping: activeMedia === 'music' && loopMode !== 'off'
    }));
  }

  function handleSystemMediaAction(event: Event) {
    const action = (event as CustomEvent<string>).detail;
    if (!audio) return;
    if (action === 'play') {
      if (audio.paused) audio.play().catch((nextError) => (error = String(nextError)));
    } else if (action === 'pause') {
      if (!audio.paused) audio.pause();
    } else if (action === 'previous') {
      void moveTrack(-1);
    } else if (action === 'next') {
      void moveTrack(1);
    } else if (action === 'like') {
      if (current && activeMedia === 'music') toggleTrackLike(current);
    } else if (action === 'repeat') {
      // The notification toggles repeat; the drawer owns the three-way choice.
      if (activeMedia === 'music') {
        loopMode = loopMode === 'off' ? 'all' : 'off';
        resetRandomOrder();
        savePlaySettings();
        syncSystemMedia(true);
      }
    } else if (action.startsWith('seek:')) {
      const milliseconds = Number(action.slice(5));
      if (Number.isFinite(milliseconds)) seek(milliseconds / 1000);
    }
  }

  function seek(value: number) {
    if (!audio || !Number.isFinite(audio.duration)) return;
    audio.currentTime = value;
    currentTime = value;
  }

  // Track the sheet's cover alongside playback. The batched cache means this is
  // free when the library list already resolved the same album.
  $effect(() => {
    const track = current;
    nowArtFailed = false;
    if (!track) {
      nowCover = null;
      return;
    }
    // Drop the previous album's cover straight away: the notification reads
    // this value, and a stale cover is worse than none.
    nowCover = null;
    let alive = true;
    void coverFor(track).then((cover) => { if (alive) nowCover = cover; });
    return () => { alive = false; };
  });

  $effect(() => {
    if (!current) {
      showNowPlaying = false;
      showQueue = false;
    }
  });

  // While a drag is in flight the browser must not claim the gesture as a
  // scroll: these listeners are deliberately non-passive so they can stop that.
  $effect(() => {
    const targets: HTMLElement[] = [];
    if (sheetElement) targets.push(sheetElement);
    if (barElement) targets.push(barElement);
    if (targets.length === 0) return;
    const hold: EventListener = (event) => {
      if (sheetDragging || barDragging) event.preventDefault();
    };
    for (const target of targets) target.addEventListener('touchmove', hold, { passive: false });
    return () => {
      for (const target of targets) target.removeEventListener('touchmove', hold);
    };
  });

  /** Albums this phone has loaded, which is what both shelves are built from. */
  let libraryAlbums = $derived(albumsFromTracks(tracks));
  /** Search results grouped the way the results view presents them. */
  let resultArtists = $derived(artistsFromTracks(tracks));
  let searching = $derived(Boolean(query.trim()) && !showingLikedMusic);

  let lastPlayed = $derived(
    playedAlbums
      .map((played) => libraryAlbums.find((album) => album.key === played.key))
      .filter((album): album is AlbumShelf => Boolean(album))
      .slice(0, 12)
  );

  // Reshuffle only when the album set itself changes, so the shelf does not
  // jump around while the user is looking at it.
  $effect(() => {
    const albums = libraryAlbums;
    const seed = albums.map((album) => album.key).sort().join('|');
    if (seed === discoverSeed) return;
    discoverSeed = seed;
    discoverAlbums = shuffled(albums).slice(0, 12);
  });

  $effect(() => {
    document.body.style.overflow = showNowPlaying ? 'hidden' : '';
    return () => { document.body.style.overflow = ''; };
  });

  // The drawer, and every view above it, own the hardware back button.
  $effect(() => {
    androidBackBridge()?.setDrawerOpen(
      showSettings ||
        showRemote ||
        showReport ||
        showActions ||
        showAlbumView ||
        showQueue ||
        (showNowPlaying && !sheetClosing)
    );
  });

  function nowPlayingAvailable() {
    return activeMedia === 'music' && playerQueueLibraryVisible && !!current;
  }

  function openNowPlaying() {
    if (!nowPlayingAvailable()) return;
    // Tapping the bar mid-close should bring the drawer back, not be ignored.
    window.clearTimeout(sheetCloseTimer);
    if (showNowPlaying && !sheetClosing) return;
    sheetClosing = false;
    sheetDragY = 0;
    if (showNowPlaying) return;
    // Remember where the bar sits: the drawer's travel and the navigation bar's
    // slide are both measured against it.
    barRestTop = barElement?.getBoundingClientRect().top ?? window.innerHeight;
    sheetEntering = true;
    showNowPlaying = true;
    // Remove the start position on the next frame so the transition runs.
    window.requestAnimationFrame(() => { sheetEntering = false; });
  }

  function closeNowPlaying() {
    if (!showNowPlaying || sheetClosing) return;
    sheetClosing = true;
    sheetDragging = false;
    sheetCloseTimer = window.setTimeout(() => {
      showNowPlaying = false;
      sheetClosing = false;
      sheetDragY = 0;
    }, SHEET_ANIMATION_MS);
  }

  function sheetCoverUrl() {
    if (!nowCover || nowArtFailed) return '';
    return nowCover.art || nowCover.thumb;
  }

  async function playFromQueue(index: number) {
    const track = playerQueue[index];
    if (!track) return;
    playerIndex = index;
    selected = track;
    resetRandomOrder();
    await playTrack(track);
  }

  function startSheetDrag(event: PointerEvent) {
    if (sheetClosing) return;
    // Sliders own their own gestures.
    if ((event.target as HTMLElement | null)?.closest('input')) return;
    const target = (event.target as Node | null) ?? null;
    sheetDragPending = true;
    sheetDragging = false;
    sheetDragStart = event.clientY;
    sheetScrollTop = sheetScroller?.scrollTop ?? 0;
    // A gesture starting inside the queue belongs to the queue until it is back
    // at its top; everything above it can be pulled away at once.
    sheetDragAllowed = !(target && sheetScroller?.contains(target)) || sheetScrollTop <= 0;
  }

  function moveSheetDrag(event: PointerEvent) {
    const travel = event.clientY - sheetDragStart;
    if (sheetDragPending) {
      if (!sheetDragAllowed && travel > 6) {
        sheetDragPending = false;
        return;
      }
      // Wait for a real downward pull before stealing the gesture.
      if (travel < 6) {
        if (travel > -6) return;
        sheetDragPending = false;
        return;
      }
      sheetDragPending = false;
      sheetDragging = true;
    }
    if (!sheetDragging) return;
    sheetDragY = Math.max(0, travel);
  }

  function endSheetDrag() {
    sheetDragPending = false;
    if (!sheetDragging) return;
    sheetDragging = false;
    const height = sheetElement?.offsetHeight ?? window.innerHeight;
    // Pulling the drawer a fifth of the way down commits to closing.
    if (sheetDragY > height * SHEET_DISMISS_RATIO) closeNowPlaying();
    else sheetDragY = 0;
  }

  function startBarDrag(event: PointerEvent) {
    if (!nowPlayingAvailable() || showNowPlaying) return;
    barDragging = true;
    barDragTravelled = 0;
    barDragSpeed = 0;
    barDragStart = event.clientY;
    barDragLastY = event.clientY;
    barDragLastAt = performance.now();
    barRestTop = barElement?.getBoundingClientRect().top ?? window.innerHeight;
    // Keep every move on the bar even once the drawer covers it.
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  }

  function moveBarDrag(event: PointerEvent) {
    if (!barDragging) return;
    const now = performance.now();
    const elapsed = now - barDragLastAt;
    if (elapsed > 0) barDragSpeed = (barDragLastY - event.clientY) / elapsed;
    barDragLastY = event.clientY;
    barDragLastAt = now;
    barDragTravelled = barDragStart - event.clientY;
    if (barDragTravelled <= BAR_DRAG_SLOP) return;
    if (!showNowPlaying) {
      // Mount the drawer with its top edge level with the bar, so it rises out
      // from underneath it rather than appearing at the foot of the page.
      sheetClosing = false;
      sheetEntering = false;
      sheetDragging = true;
      showNowPlaying = true;
    }
    sheetDragY = Math.max(0, barRestTop - barDragTravelled);
  }

  function endBarDrag() {
    if (!barDragging) return;
    barDragging = false;
    const travelled = barDragTravelled;
    barDragTravelled = 0;
    // A deliberate pull should not also fire the button's click handler.
    barSwallowClick = travelled > 8;
    if (!showNowPlaying) return;
    sheetDragging = false;
    // The pull that fades the bar out is the pull that opens the drawer, so
    // the release decision matches what the finger just saw.
    if (travelled > barOpenTravel || barDragSpeed > BAR_FLING_SPEED) sheetDragY = 0;
    else closeNowPlaying();
  }

  function handleBarClick() {
    if (barSwallowClick) {
      barSwallowClick = false;
      return;
    }
    openNowPlaying();
  }

  /** Seek by a relative amount, staying inside the track. */
  function nudge(seconds: number) {
    if (!current && !currentPodcast) return;
    const limit = duration > 0 ? duration : Number.POSITIVE_INFINITY;
    seek(Math.min(Math.max(0, currentTime + seconds), limit));
  }

  /** Albums among the loaded tracks, grouped by the key covers are addressed by. */
  function albumsFromTracks(source: RemoteTrack[]): AlbumShelf[] {
    const groups = new Map<string, AlbumShelf>();
    for (const track of source) {
      const key = coverKey(track.artist ?? '', track.album ?? '');
      if (!key) continue;
      const existing = groups.get(key);
      if (existing) existing.tracks.push(track);
      else groups.set(key, {
        key,
        artist: track.artist,
        album: track.album,
        representative: track,
        tracks: [track]
      });
    }
    return [...groups.values()];
  }

  /**
   * Artists among a set of tracks. There is no artist record on the wire, so a
   * search groups the results it already has rather than asking the host for
   * something it does not model.
   */
  function artistsFromTracks(source: RemoteTrack[]): ArtistShelf[] {
    const groups = new Map<string, ArtistShelf>();
    for (const track of source) {
      const name = (track.artist ?? '').trim();
      if (!name) continue;
      const key = name.toLocaleLowerCase();
      const existing = groups.get(key);
      if (existing) existing.count += 1;
      else groups.set(key, { name, representative: track, count: 1 });
    }
    return [...groups.values()].sort((left, right) => right.count - left.count);
  }

  /** Other albums by the same artist, for the "More by" carousel. */
  async function albumsByArtist(album: AlbumShelf): Promise<AlbumShelf[]> {
    const name = album.artist.trim();
    if (!name) return [];
    const wanted = name.toLocaleLowerCase();
    const known = libraryAlbums.filter(
      (entry) => entry.key !== album.key && entry.artist.trim().toLocaleLowerCase() === wanted
    );
    try {
      const page = await invoke<LibraryPage>('remote_library', {
        query: name, offset: 0, limit: MAX_ALBUM_TRACKS
      });
      const sameArtist = page.tracks.filter(
        (track) => (track.artist ?? '').trim().toLocaleLowerCase() === wanted
      );
      const found = albumsFromTracks(sameArtist).filter((entry) => entry.key !== album.key);
      for (const entry of known) {
        if (!found.some((other) => other.key === entry.key)) found.push(entry);
      }
      return found.slice(0, 12);
    } catch {
      // Offline: whatever the loaded page already holds.
      return known.slice(0, 12);
    }
  }

  /**
   * Opening an album previews it: the tracks, the year the cover NIP carries,
   * and what else the artist has here. Playback starts from the view.
   */
  async function openAlbum(album: AlbumShelf, prepared?: RemoteTrack[]) {
    // The album page is a full screen of its own. Leaving the drawer open would
    // stack two full-height views, and the album is what the tap asked for.
    if (showNowPlaying) closeNowPlaying();
    closeActions();
    showAlbumView = true;
    albumView = {
      key: album.key,
      artist: album.artist,
      album: album.album,
      year: '',
      art: '',
      tracks: prepared ?? album.tracks,
      more: []
    };
    const [tracks, cover, more] = await Promise.all([
      prepared ? Promise.resolve(prepared) : albumPlaylist(album),
      coverFor(album.representative).catch(() => null),
      albumsByArtist(album)
    ]);
    // A later open wins, so a slow response cannot overwrite a newer album.
    if (albumView?.key !== album.key) return;
    albumView = {
      key: album.key,
      artist: album.artist,
      album: album.album,
      year: cover?.year ?? '',
      art: cover ? cover.art || cover.thumb : '',
      tracks,
      more
    };
  }

  function closeAlbumView() {
    showAlbumView = false;
    albumView = null;
  }

  /** Opens the same album preview from a track, wherever the menu was opened. */
  async function goToAlbum(track: RemoteTrack) {
    closeActions();
    const owner = albumsFromTracks([track])[0];
    if (!owner) return;
    const full = await albumPlaylist(owner);
    await openAlbum({ ...owner, tracks: full.length > 0 ? full : owner.tracks }, full);
  }

  function goToArtist(name: string) {
    closeActions();
    closeAlbumView();
    if (showNowPlaying) closeNowPlaying();
    activeTab = 'music';
    void searchTracks(name.trim());
  }

  async function playAlbumNow() {
    const view = albumView;
    if (!view) return;
    const playable = view.tracks.filter((track) => track.local);
    if (playable.length === 0) {
      await activateTrack(view.tracks[0]);
      return;
    }
    playerQueue = playable;
    playerQueueLibraryVisible = true;
    playerIndex = 0;
    selected = playable[0];
    resetRandomOrder();
    await playTrack(playable[0]);
  }

  async function playAlbumTrack(index: number) {
    const view = albumView;
    const track = view?.tracks[index];
    if (!view || !track) return;
    if (!track.local) {
      await requestDownload(track);
      return;
    }
    const playable = view.tracks.filter((item) => item.local);
    playerQueue = playable;
    playerQueueLibraryVisible = true;
    playerIndex = playable.findIndex((item) => item.fileId === track.fileId);
    selected = track;
    resetRandomOrder();
    await playTrack(track);
  }

  /** Whether this phone has anything to say to the computer's player. */
  function remoteAvailable() {
    return status.paired && status.connected;
  }

  async function refreshRemote() {
    if (!remoteAvailable()) return;
    try {
      remoteState = await invoke<RemotePlaybackState>('remote_playback_state');
      remoteError = '';
    } catch (nextError) {
      remoteError = String(nextError);
    }
  }

  function openRemote() {
    showRemote = true;
    void refreshRemote();
  }

  /** Every transport button lands here, so the view never has to guess. */
  async function sendPlayback(command: PlaybackCommand) {
    if (remoteBusy) return;
    remoteBusy = true;
    try {
      remoteState = await invoke<RemotePlaybackState>('remote_playback', { command });
      remoteError = '';
    } catch (nextError) {
      remoteError = String(nextError);
    } finally {
      remoteBusy = false;
    }
  }

  /**
   * Volume follows a drag, so it cannot be gated on one command being in
   * flight; the last answer to arrive is the one shown.
   */
  async function sendVolume(percent: number) {
    if (!remoteAvailable()) return;
    try {
      remoteState = await invoke<RemotePlaybackState>('remote_playback', {
        command: { type: 'volume', percent: Math.round(percent) } satisfies PlaybackCommand
      });
      remoteError = '';
    } catch (nextError) {
      remoteError = String(nextError);
    }
  }

  function remotePosition() {
    if (!remoteState || remoteState.durationMs <= 0) return 0;
    return Math.min(1, Math.max(0, remoteState.positionMs / remoteState.durationMs));
  }

  function remoteClock(milliseconds: number) {
    return clock(Math.max(0, milliseconds) / 1000);
  }

  /** Ask the computer for a code this phone can hand to another device. */
  async function requestReadOnlyCode() {
    if (ticketBusy) return;
    ticketBusy = true;
    ticketError = '';
    try {
      readOnlyTicket = await invoke<ReadOnlyTicketOffer>('remote_read_only_ticket');
      if (!readOnlyTicket.qrSvg) {
        notice = 'Your computer drew no QR for this code. Copy it instead.';
      }
    } catch (nextError) {
      readOnlyTicket = null;
      ticketError = String(nextError);
    } finally {
      ticketBusy = false;
    }
  }

  async function copyReadOnlyCode() {
    const uri = readOnlyTicket?.uri;
    if (!uri) return;
    try {
      await navigator.clipboard.writeText(uri);
      notice = 'Read-only code copied';
    } catch {
      // A refused clipboard still leaves the code on screen to copy by hand.
      notice = uri;
    }
  }

  function ticketMinutesLeft() {
    if (!readOnlyTicket) return 0;
    return Math.max(0, Math.round((readOnlyTicket.expiresAt * 1000 - Date.now()) / 60_000));
  }

  /**
   * A cover is reported by album key, never by event id: the computer decides
   * which claim to report, and the phone cannot name a pubkey it cannot verify.
   */
  function openCoverReport(track: RemoteTrack) {
    const key = coverKey(track.artist ?? '', track.album ?? '');
    if (!key) {
      error = 'This track names no artist and album to report.';
      return;
    }
    reportKey = key;
    reportLabel = `${track.album || 'Untitled'} · ${track.artist || 'Unknown artist'}`;
    reportReason = 'spam';
    reportNote = '';
    reportError = '';
    showReport = true;
  }

  async function submitCoverReport() {
    if (reportBusy || !reportKey) return;
    reportBusy = true;
    reportError = '';
    try {
      const report = await invoke<CoverReport>('remote_report_cover', {
        key: reportKey,
        reason: reportReason,
        note: reportNote.trim()
      });
      notice = report.reportId
        ? `Report published as ${report.reportId.slice(0, 12)}…`
        : 'Report sent to Napstr';
      showReport = false;
      closeActions();
    } catch (nextError) {
      reportError = String(nextError);
    } finally {
      reportBusy = false;
    }
  }

  function nextRemoteRepeat(mode: RemoteRepeat): RemoteRepeat {
    return mode === 'off' ? 'all' : mode === 'all' ? 'one' : 'off';
  }

  function remoteRepeatLabel(mode: RemoteRepeat) {
    return mode === 'off' ? 'Off' : mode === 'all' ? 'All' : 'One track';
  }

  function openActions(track: RemoteTrack | null) {
    actionTrack = track;
    showSleepOptions = false;
    showActions = true;
  }

  function closeActions() {
    showActions = false;
    showSleepOptions = false;
    actionTrack = null;
  }

  /** The track URI other Napstr clients understand. */
  function trackUri(track: RemoteTrack) {
    return `napstr://track/${track.fileId}`;
  }

  async function shareTrack(track: RemoteTrack) {
    const uri = trackUri(track);
    try {
      await navigator.clipboard.writeText(uri);
      notice = `Copied ${uri}`;
    } catch {
      // A refused clipboard still leaves the link on screen to copy by hand.
      notice = uri;
    }
    closeActions();
  }

  function sleepSummary() {
    if (!sleepValue) return 'Off';
    if (sleepValue === 'track') return 'After this track';
    return sleepClock ? `${sleepClock} left` : 'Running';
  }

  /** Choosing the running option again turns the timer off. */
  function chooseSleep(option: SleepOption) {
    if (sleepValue === option.value) {
      sleepValue = '';
      sleepEndsAt = 0;
      sleepClock = '';
      return;
    }
    sleepValue = option.value;
    if (option.endsTrack) {
      sleepEndsAt = 0;
      sleepClock = '';
      return;
    }
    const total = (option.minutes ?? 0) * 60;
    sleepEndsAt = Date.now() + total * 1000;
    sleepClock = `${Math.floor(total / 60)}:${String(total % 60).padStart(2, '0')}`;
  }

  function shuffled<T>(items: T[]): T[] {
    const copy = [...items];
    for (let index = copy.length - 1; index > 0; index -= 1) {
      const swap = Math.floor(Math.random() * (index + 1));
      [copy[index], copy[swap]] = [copy[swap], copy[index]];
    }
    return copy;
  }

  function readPlayedAlbums(): PlayedAlbum[] {
    try {
      const raw = window.localStorage.getItem(musicHistoryKey);
      const parsed = raw ? (JSON.parse(raw) as PlayedAlbum[]) : [];
      return Array.isArray(parsed)
        ? parsed.filter((entry) => entry && typeof entry.key === 'string')
        : [];
    } catch {
      return [];
    }
  }

  /** Recent albums live on the phone: the host has no play history to ask for. */
  function rememberPlayedAlbum(track: RemoteTrack) {
    const key = coverKey(track.artist ?? '', track.album ?? '');
    if (!key) return;
    playedAlbums = [
      { key, artist: track.artist, album: track.album },
      ...playedAlbums.filter((entry) => entry.key !== key)
    ].slice(0, 24);
    try {
      window.localStorage.setItem(musicHistoryKey, JSON.stringify(playedAlbums));
    } catch {
      // A history that cannot be stored is only a lost convenience.
    }
  }

  /**
   * The shelves are grouped out of the tracks this phone has loaded, which is
   * one page of the library. An album's other tracks are usually not in it, so
   * ask the host for the album before queueing anything.
   */
  async function albumPlaylist(album: AlbumShelf): Promise<RemoteTrack[]> {
    const name = album.album.trim();
    if (!name) return album.tracks;
    try {
      const page = await invoke<LibraryPage>('remote_library', {
        query: name, offset: 0, limit: MAX_ALBUM_TRACKS
      });
      const sameAlbum = (track: RemoteTrack) =>
        (track.album ?? '').trim().toLocaleLowerCase() === name.toLocaleLowerCase();
      // Prefer the exact artist|album key, but fall back to the album name so a
      // guest credit on one track does not silently drop it from the playlist.
      const byKey = page.tracks.filter(
        (track) => coverKey(track.artist ?? '', track.album ?? '') === album.key
      );
      const found = byKey.length > album.tracks.length ? byKey : page.tracks.filter(sameAlbum);
      return found.length > album.tracks.length ? found : album.tracks;
    } catch {
      // Offline, or a host that cannot answer: play what the shelf already had.
      return album.tracks;
    }
  }

  /** `MP3 · 320 kb/s`. The rate is the file's own average, not a claim. */
  function fileSummary(track: RemoteTrack): string {
    const parts: string[] = [];
    if (track.format) parts.push(track.format.toUpperCase());
    const bitrate = averageBitrate(track);
    if (bitrate > 0) parts.push(`${bitrate} kb/s`);
    return parts.join(' · ');
  }

  function averageBitrate(track: RemoteTrack): number {
    if (!track.size || duration <= 0) return 0;
    return Math.round((track.size * 8) / duration / 1000);
  }

  async function moveTrack(direction: -1 | 1) {
    if (activeMedia !== 'music' || playerQueue.length < 2) return;
    let next: number;
    if (shuffle) {
      if (direction === -1) {
        if (randomHistoryIndex <= 0) return;
        randomHistoryIndex -= 1;
        next = randomHistory[randomHistoryIndex];
        randomUpcoming = randomHistory[randomHistoryIndex + 1] ?? randomIndexExcept(next);
      } else if (randomHistoryIndex + 1 < randomHistory.length) {
        randomHistoryIndex += 1;
        next = randomHistory[randomHistoryIndex];
        randomUpcoming = randomHistory[randomHistoryIndex + 1] ?? randomIndexExcept(next);
      } else {
        next = randomUpcoming >= 0 ? randomUpcoming : randomIndexExcept(playerIndex);
        if (next < 0) return;
        randomHistory = [...randomHistory.slice(0, randomHistoryIndex + 1), next].slice(-100);
        randomHistoryIndex = randomHistory.length - 1;
        randomUpcoming = randomIndexExcept(next);
      }
    } else {
      next = (playerIndex + direction + playerQueue.length) % playerQueue.length;
    }
    playerIndex = next;
    selected = playerQueue[next];
    await playTrack(playerQueue[next]);
  }

  function handleTrackEnded() {
    playing = false;
    syncSystemMedia(true);
    // A sleep timer set to end of track stops here rather than advancing, for
    // podcasts as much as for music.
    if (sleepValue === 'track') {
      sleepValue = '';
      return;
    }
    if (activeMedia !== 'music') return;
    if (loopMode === 'one') {
      audio.currentTime = 0;
      audio.play().catch((nextError) => (error = String(nextError)));
      return;
    }
    if (shuffle) {
      // Every track has been played and the queue is not set to repeat.
      if (randomUpcoming < 0) return;
      void moveTrack(1);
      return;
    }
    if (playerIndex >= playerQueue.length - 1 && loopMode === 'off') return;
    void moveTrack(1);
  }

  function selectChip(chip: string) {
    activeTab = 'search';
    void searchTracks(query.toLocaleLowerCase() === chip.toLocaleLowerCase() ? '' : chip);
  }

  /** Switching to the library tab drops any search that was in flight. */
  function showMusic() {
    activeTab = 'music';
    if (query.trim()) {
      void searchTracks('');
      return;
    }
    if (!showingLikedMusic) void loadLibrary();
  }

  function showSearch() {
    activeTab = 'search';
  }

  function normalizeGenre(value: string) {
    return value.toLocaleLowerCase().replace(/[^\p{L}\p{N}]+/gu, ' ').trim();
  }

  function feedMatchesGenre(feed: PodcastFeed, genre: string) {
    const wanted = normalizeGenre(genre);
    return feed.genres.some((value) => {
      const candidate = normalizeGenre(value);
      return candidate === wanted || candidate.includes(wanted) || wanted.includes(candidate);
    });
  }

  function podcastDate(timestamp: number) {
    if (!timestamp) return '';
    return new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short', year: 'numeric' })
      .format(new Date(timestamp * 1000));
  }

  function podcastDownloadFor(episodeId: number) {
    return podcastDownloads.find((download) => download.episode.id === episodeId);
  }

  async function invokePodcast<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    let timeout = 0;
    try {
      return await Promise.race([
        invoke<T>(command, args),
        new Promise<T>((_, reject) => {
          timeout = window.setTimeout(
            () => reject(new Error('The podcast service did not respond. Check your connection and retry.')),
            20_000
          );
        })
      ]);
    } finally {
      window.clearTimeout(timeout);
    }
  }

  async function fetchPodcastDirectory(
    path: 'search' | 'lookup',
    parameters: Record<string, string>,
    maximumBytes: number
  ): Promise<string> {
    const url = new URL(`https://itunes.apple.com/${path}`);
    for (const [name, value] of Object.entries(parameters)) url.searchParams.set(name, value);
    const controller = new AbortController();
    const timeout = window.setTimeout(() => controller.abort(), 15_000);
    try {
      const response = await fetch(url, {
        headers: { Accept: 'application/json' },
        signal: controller.signal
      });
      if (!response.ok) throw new Error(`Podcast directory returned ${response.status}.`);
      const advertisedLength = Number(response.headers.get('content-length') || 0);
      if (advertisedLength > maximumBytes) throw new Error('Podcast directory response is too large.');
      const payload = await response.text();
      if (new TextEncoder().encode(payload).byteLength > maximumBytes) {
        throw new Error('Podcast directory response is too large.');
      }
      return payload;
    } catch (nextError) {
      if (controller.signal.aborted) {
        throw new Error('The podcast directory did not respond. Check your connection and retry.');
      }
      throw nextError;
    } finally {
      window.clearTimeout(timeout);
    }
  }

  async function searchPodcastDirectory(searchTerm: string, limit: number): Promise<PodcastFeed[]> {
    const query = searchTerm.trim();
    if (!query || query.length > 120) throw new Error('Search for between 1 and 120 characters.');
    const boundedLimit = Math.min(50, Math.max(1, limit));
    const payload = await fetchPodcastDirectory('search', {
      term: query,
      media: 'podcast',
      entity: 'podcast',
      limit: String(boundedLimit)
    }, 4 * 1024 * 1024);
    return invokePodcast<PodcastFeed[]>('podcast_parse_search', {
      payload,
      limit: boundedLimit
    });
  }

  async function showPodcasts() {
    activeTab = 'podcasts';
    error = '';
    if (podcastFeeds.length === 0 && !selectedPodcast) await loadTrendingPodcasts();
  }

  async function loadTrendingPodcasts() {
    if (podcastLoading) return;
    const viewVersion = ++podcastViewVersion;
    podcastLoading = true;
    showingLikedPodcasts = false;
    podcastGenre = '';
    error = '';
    selectedPodcast = null;
    podcastEpisodes = [];
    try {
      const results = await searchPodcastDirectory('podcast', 30);
      if (viewVersion === podcastViewVersion) podcastFeeds = results;
    } catch (nextError) {
      if (viewVersion === podcastViewVersion) {
        error = String(nextError);
        podcastFeeds = [];
      }
    } finally {
      if (viewVersion === podcastViewVersion) podcastLoading = false;
    }
  }

  async function searchPodcasts() {
    const query = podcastQuery.trim();
    if (!query) return loadTrendingPodcasts();
    if (podcastLoading) return;
    const viewVersion = ++podcastViewVersion;
    podcastLoading = true;
    showingLikedPodcasts = false;
    podcastGenre = '';
    error = '';
    selectedPodcast = null;
    podcastEpisodes = [];
    try {
      const results = await searchPodcastDirectory(query, 50);
      if (viewVersion === podcastViewVersion) podcastFeeds = results;
    } catch (nextError) {
      if (viewVersion === podcastViewVersion) {
        error = String(nextError);
        podcastFeeds = [];
      }
    } finally {
      if (viewVersion === podcastViewVersion) podcastLoading = false;
    }
  }

  function showLikedPodcastList() {
    podcastViewVersion += 1;
    podcastLoading = false;
    showingLikedPodcasts = !showingLikedPodcasts;
    selectedPodcast = null;
    podcastEpisodes = [];
    podcastGenre = '';
    podcastQuery = '';
    if (showingLikedPodcasts) {
      podcastFeeds = [...likedPodcasts];
    } else {
      void loadTrendingPodcasts();
    }
  }

  async function selectPodcastGenre(genre: string) {
    if (podcastLoading) return;
    if (podcastGenre === genre && !showingLikedPodcasts) {
      await loadTrendingPodcasts();
      return;
    }
    const viewVersion = ++podcastViewVersion;
    podcastLoading = true;
    showingLikedPodcasts = false;
    podcastGenre = genre;
    podcastQuery = '';
    selectedPodcast = null;
    podcastEpisodes = [];
    error = '';
    try {
      const results = await searchPodcastDirectory(genre, 50);
      if (viewVersion !== podcastViewVersion) return;
      const categoryMatches = results.filter((feed) => feedMatchesGenre(feed, genre));
      podcastFeeds = categoryMatches.length > 0 ? categoryMatches : results;
    } catch (nextError) {
      if (viewVersion === podcastViewVersion) {
        error = String(nextError);
        podcastFeeds = [];
      }
    } finally {
      if (viewVersion === podcastViewVersion) podcastLoading = false;
    }
  }

  async function openPodcast(feed: PodcastFeed) {
    if (podcastLoading) return;
    selectedPodcast = feed;
    podcastLoading = true;
    error = '';
    try {
      const directoryPayload = await fetchPodcastDirectory('lookup', {
        id: String(feed.id),
        media: 'podcast',
        entity: 'podcastEpisode',
        limit: '50'
      }, 8 * 1024 * 1024);
      podcastEpisodes = await invokePodcast<PodcastEpisode[]>('podcast_episodes', {
        feed,
        directoryPayload
      });
    } catch (nextError) {
      error = String(nextError);
      podcastEpisodes = [];
    } finally {
      podcastLoading = false;
    }
  }

  function rememberPodcast(episode: PodcastEpisode) {
    podcastHistory = [episode, ...podcastHistory.filter((item) => item.id !== episode.id)].slice(0, 10);
    window.localStorage.setItem('napstrfy-podcast-history', JSON.stringify(podcastHistory));
  }

  async function playPodcast(episode: PodcastEpisode) {
    if (caching) return;
    caching = true;
    error = '';
    try {
      audio?.pause();
      const source = await invoke<{ url: string; downloaded: boolean }>('podcast_playback_url', { episode });
      activeMedia = 'podcast';
      currentPodcast = episode;
      currentTime = 0;
      duration = episode.duration || 0;
      audio.src = source.url;
      audio.volume = volume;
      await audio.play();
      rememberPodcast(episode);
    } catch (nextError) {
      playing = false;
      error = `Could not play ${episode.title}: ${String(nextError)}`;
    } finally {
      caching = false;
    }
  }

  async function downloadPodcast(episode: PodcastEpisode) {
    const existing = podcastDownloadFor(episode.id);
    if (existing?.ready || existing?.status === 'Downloading') return;
    error = '';
    try {
      await invoke('podcast_download', { episode });
      await refreshPodcastDownloads();
      notice = `${episode.title} is downloading for offline listening`;
    } catch (nextError) {
      error = `Could not download ${episode.title}: ${String(nextError)}`;
    }
  }

  async function refreshPodcastDownloads() {
    try {
      podcastDownloads = await invoke<PodcastDownload[]>('podcast_downloads');
    } catch { /* the next foreground poll retries */ }
  }

  function hasActivePodcastDownload() {
    return podcastDownloads.some((download) => !download.ready && /downloading/i.test(download.status));
  }

  onMount(() => {
    // Older builds stored one of four mode names; newer ones store both
    // settings together. Either shape restores cleanly.
    try {
      const saved = window.localStorage.getItem(playModeKey) ?? '';
      if (saved.startsWith('{')) {
        const parsed = JSON.parse(saved) as { loop?: string; shuffle?: boolean };
        if (LOOP_MODES.includes(parsed.loop as LoopMode)) loopMode = parsed.loop as LoopMode;
        shuffle = parsed.shuffle === true;
      } else if (saved) {
        loopMode = saved === 'once' ? 'off' : saved === 'repeat' ? 'one' : saved === 'random' ? 'off' : 'all';
        shuffle = saved === 'random';
      }
    } catch {
      // An unreadable preference just means the defaults.
    }
    try {
      const saved = JSON.parse(window.localStorage.getItem(likedMusicKey) || '[]') as unknown;
      if (Array.isArray(saved)) likedMusic = saved.filter(isStoredTrack).slice(0, 1000);
    } catch { likedMusic = []; }
    try {
      const saved = JSON.parse(window.localStorage.getItem(likedPodcastsKey) || '[]') as unknown;
      if (Array.isArray(saved)) {
        likedPodcasts = saved.filter(isStoredPodcast).slice(0, 500).map((feed) => ({
          ...feed,
          genres: Array.isArray(feed.genres) ? feed.genres.filter((genre) => typeof genre === 'string').slice(0, 12) : []
        }));
      }
    } catch { likedPodcasts = []; }
    try {
      const saved = JSON.parse(window.localStorage.getItem('napstrfy-podcast-history') || window.localStorage.getItem('nostrfy-podcast-history') || '[]');
      if (Array.isArray(saved)) podcastHistory = saved.slice(0, 10);
    } catch { podcastHistory = []; }
    void loadCachedLibrary()
      .then(() => refreshStatus(true, false))
      .then(() => { if (status.connected) void loadLibrary(); });
    void refreshPodcastDownloads();
    const statusTimer = window.setInterval(() => {
      if (!document.hidden) void refreshStatus();
    }, 15000);
    const transferTimer = window.setInterval(() => {
      if (!document.hidden && pending.size > 0) void refreshTransfers();
    }, 3000);
    const podcastTimer = window.setInterval(() => {
      if (!document.hidden && hasActivePodcastDownload()) void refreshPodcastDownloads();
    }, 2500);
    const sleepTimer = window.setInterval(() => {
      if (!sleepEndsAt) return;
      const remaining = sleepEndsAt - Date.now();
      if (remaining <= 0) {
        sleepValue = '';
        sleepEndsAt = 0;
        sleepClock = '';
        audio?.pause();
        notice = 'Sleep timer finished';
        return;
      }
      const seconds = Math.ceil(remaining / 1000);
      sleepClock = `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
    }, 1000);
    const foreground = () => {
      if (document.hidden) return;
      void refreshStatus();
      void refreshTransfers();
      void refreshPodcastDownloads();
    };
    document.addEventListener('visibilitychange', foreground);
    window.addEventListener('napstrfy-media-action', handleSystemMediaAction);
    window.addEventListener('napstrfy-back', handleSystemBack);
    return () => {
      window.clearInterval(statusTimer);
      window.clearInterval(transferTimer);
      window.clearInterval(podcastTimer);
      window.clearInterval(sleepTimer);
      document.removeEventListener('visibilitychange', foreground);
      window.removeEventListener('napstrfy-media-action', handleSystemMediaAction);
      window.removeEventListener('napstrfy-back', handleSystemBack);
      androidBackBridge()?.setDrawerOpen(false);
      androidMediaBridge()?.clear();
    };
  });
</script>

<svelte:head><title>Napstrfy</title></svelte:head>

{#snippet trackList(emptyTitle: string, emptyHint: string, showLoadMore: boolean)}
  <section class="track-list" aria-busy={loading}>
    {#if !loading && tracks.length === 0}
      <div class="empty-library"><img src="/napstr-logo-small.png" alt="" /><h2>{emptyTitle}</h2><p>{emptyHint}</p></div>
    {/if}
    {#each tracks as track (track.fileId)}
      <div class:selected={selected?.fileId === track.fileId} class:remote={!track.local} class="track-row">
        <button class="track-open" disabled={status.streamOnly && !track.local} onclick={() => activateTrack(track)}>
          <TrackArtwork {track} lookup />
          <span class="track-copy">
            <strong>{title(track)}</strong>
            <small>{artist(track)}{track.album ? ` · ${track.album}` : ''}</small>
            <span class="track-meta">{readableSize(track.size)}</span>
          </span>
          <TrackBadge {track} cached={cachedFileIds.has(track.fileId)} pending={pending.has(track.fileId)} />
        </button>
        <button class:liked={isTrackLiked(track)} class="like-button" onclick={() => toggleTrackLike(track)} aria-label={`${isTrackLiked(track) ? 'Unlike' : 'Like'} ${title(track)}`}>{isTrackLiked(track) ? '♥' : '♡'}</button>
      </div>
    {/each}
    {#if showLoadMore && tracks.length < total}<button class="load-more" onclick={() => loadLibrary(true)} disabled={loadingMore}>{loadingMore ? 'Loading…' : `Load more · ${tracks.length} of ${total}`}</button>{/if}
  </section>
{/snippet}

{#if !status.paired && activeTab !== 'podcasts'}
  <main class="pair-screen">
    <div class="pair-glow"></div>
    <div class="pair-logo" aria-label="Napstrfy"><img src="/favicon.png" alt="" /><span>napstrfy</span></div>
    <p class="eyebrow">NAPSTR COMPANION</p>
    <h1>Your music.<br />Wherever you are.</h1>
    <p class="pair-copy">Pair securely with Napstr on your computer. Discovery and Tor downloads stay there; your music reaches this phone over encrypted Iroh.</p>
    {#if error}
      <div class="error-card">
        <span>{error}</span>
        {#if cameraPermissionDenied}<button onclick={showCameraSettings}>Open app settings</button>{/if}
      </div>
    {/if}
    <button class="scan-button" onclick={scanCode} disabled={scanning || pairing || statusLoading}><span>▦</span>{scanning ? 'Opening camera…' : pairing ? 'Pairing…' : 'Scan Napstr QR'}</button>
    <button class="browse-podcasts" onclick={showPodcasts}>Listen to podcasts without pairing</button>
    <details class="manual-pair">
      <summary>Enter a pairing code instead</summary>
      <textarea bind:value={pairingCode} placeholder="napstrfy://pair/…"></textarea>
      <button onclick={() => pair()} disabled={!pairingCode.trim() || pairing}>Connect</button>
    </details>
    <small class="pair-security">One-use pairing · no Nostr keys leave your computer</small>
  </main>
{:else}
  <main class="app-shell">
    <header class="mobile-header">
      {#if status.paired}
        <button class="status-chip" class:offline={!status.connected} onclick={reconnect} title={status.connected ? `Connected to ${status.desktopName || 'Napstr'}` : 'Reconnect to Napstr'}>
          <i></i><span>{statusPending ? 'Connecting…' : status.connected ? status.desktopName || 'Napstr' : 'Offline'}{status.streamOnly ? ' · Read only' : ''}</span>
        </button>
      {:else}
        <button class="status-chip offline" onclick={showMusic}><i></i><span>Pair Napstr</span></button>
      {/if}
      <button class="header-icon" onclick={() => (showSettings = true)} aria-label="Settings">
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="M3.4 7.6h9.4" /><path d="M17.6 7.6h3" /><circle cx="15.2" cy="7.6" r="2.4" />
          <path d="M3.4 16.4h4.2" /><path d="M12.4 16.4h8.2" /><circle cx="10" cy="16.4" r="2.4" />
        </svg>
      </button>
    </header>

    {#if error}<button class="error-banner" onclick={() => (error = '')}>{error}<span>×</span></button>{/if}
    {#if notice}<button class="notice-banner" onclick={() => (notice = '')}>{notice}<span>×</span></button>{/if}

    {#if activeTab === 'search'}
      <section class="search-area">
        <form onsubmit={(event) => { event.preventDefault(); event.currentTarget.querySelector('input')?.blur(); void searchTracks(); }}>
          <span>⌕</span><input bind:value={query} placeholder={status.streamOnly ? "Search Napstr’s music" : "Search your music and Nostr"} aria-label="Search tracks" />
          {#if loading}<i class="search-spinner" role="status" aria-label="Searching"></i>{/if}
          {#if query}<button type="button" class="clear-search" onclick={() => searchTracks('')}>×</button>{/if}
        </form>
        <div class="chips"><button class:active={showingLikedMusic} onclick={showLikedTracks}>♥ Liked</button>{#each musicChips as chip}<button class:active={!showingLikedMusic && query.toLocaleLowerCase() === chip.toLocaleLowerCase()} onclick={() => selectChip(chip)}>{chip}</button>{/each}</div>
      </section>

      {#if searching && (resultArtists.length > 0 || libraryAlbums.length > 0)}
        <section class="album-shelves">
          {#if resultArtists.length > 0}
            <div class="album-shelf-block">
              <div class="section-label"><b>Artists</b><span>{resultArtists.length} in these results</span></div>
              <div class="album-shelf">
                {#each resultArtists as entry (entry.name)}
                  <div class="artist-card">
                    <button class="artist-open" onclick={() => void searchTracks(entry.name)} aria-label={`Show tracks by ${entry.name}`}>
                      <TrackArtwork track={entry.representative} lookup />
                      <span class="album-play" aria-hidden="true">⌕</span>
                    </button>
                    <strong>{entry.name}</strong>
                    <small>{entry.count} {entry.count === 1 ? 'track' : 'tracks'}</small>
                  </div>
                {/each}
              </div>
            </div>
          {/if}
          {#if libraryAlbums.length > 0}
            <div class="album-shelf-block">
              <div class="section-label"><b>Albums</b><span>{libraryAlbums.length} in these results</span></div>
              <div class="album-shelf">
                {#each libraryAlbums as album (album.key)}
                  <div class="album-card">
                    <button class="album-open" onclick={() => void openAlbum(album)} aria-label={`Open ${album.album} by ${album.artist || 'an unknown artist'}`}>
                      <TrackArtwork track={album.representative} lookup />
                      <span class="album-play" aria-hidden="true">▶</span>
                    </button>
                    <strong>{album.album}</strong>
                    <small>{album.artist || 'Unknown artist'}</small>
                  </div>
                {/each}
              </div>
            </div>
          {/if}
        </section>
        <div class="section-label tracks-label"><b>Tracks</b><span>{tracks.length} {tracks.length === 1 ? 'result' : 'results'}</span></div>
      {:else if !query.trim()}
        <section class="library-heading"><div><p>SEARCH</p><h1>Find something</h1></div><span>Your library and the network</span></section>
      {/if}

      {@render trackList(
        'No tracks found',
        query.trim() ? 'Try different words or clear the search.' : 'Search your Napstr library and the network.',
        !showingLikedMusic && Boolean(query.trim())
      )}
    {:else if activeTab === 'music'}
      <section class="library-heading">
        <div><p>{showingLikedMusic ? 'FAVOURITES' : 'YOUR NAPSTR'}</p><h1>{showingLikedMusic ? 'Liked music' : 'Your music'}</h1></div>
        <span>{total} {total === 1 ? 'track' : 'tracks'}</span>
      </section>

      {#if !showingLikedMusic && (discoverAlbums.length > 0 || lastPlayed.length > 0)}
        <section class="album-shelves">
          {#if lastPlayed.length > 0}
            <div class="album-shelf-block">
              <div class="section-label"><b>Last played</b><span>Recent albums</span></div>
              <div class="album-shelf">
                {#each lastPlayed as album (album.key)}
                  <div class="album-card">
                    <button class="album-open" onclick={() => void openAlbum(album)} aria-label={`Open ${album.album} by ${album.artist || 'an unknown artist'}`}>
                      <TrackArtwork track={album.representative} lookup />
                      <span class="album-play" aria-hidden="true">▶</span>
                    </button>
                    <strong>{album.album}</strong>
                    <small>{album.artist || 'Unknown artist'}</small>
                  </div>
                {/each}
              </div>
            </div>
          {/if}
          {#if discoverAlbums.length > 0}
            <div class="album-shelf-block">
              <div class="section-label"><b>Discover albums</b><span>{libraryAlbums.length} in this library</span></div>
              <div class="album-shelf">
                {#each discoverAlbums as album (album.key)}
                  <div class="album-card">
                    <button class="album-open" onclick={() => void openAlbum(album)} aria-label={`Open ${album.album} by ${album.artist || 'an unknown artist'}`}>
                      <TrackArtwork track={album.representative} lookup />
                      <span class="album-play" aria-hidden="true">▶</span>
                    </button>
                    <strong>{album.album}</strong>
                    <small>{album.artist || 'Unknown artist'}</small>
                  </div>
                {/each}
              </div>
            </div>
          {/if}
        </section>
      {/if}

      {@render trackList(
        showingLikedMusic ? 'No liked tracks yet' : 'No tracks found',
        showingLikedMusic ? 'Tap the heart beside a song to keep it here.' : 'Add music to your Napstr folder on the computer.',
        !showingLikedMusic
      )}
    {:else if activeTab === 'podcasts'}
      <section class="search-area podcast-search">
        <form onsubmit={(event) => { event.preventDefault(); event.currentTarget.querySelector('input')?.blur(); void searchPodcasts(); }}>
          <span>⌕</span><input bind:value={podcastQuery} placeholder="Search podcasts" aria-label="Search podcasts" />
          {#if podcastQuery}<button type="button" class="clear-search" onclick={() => { podcastQuery = ''; void loadTrendingPodcasts(); }}>×</button>{/if}
        </form>
        <div class="chips podcast-genres"><button class:active={showingLikedPodcasts} onclick={showLikedPodcastList}>♥ Liked</button>{#each podcastGenres as genre}<button class:active={!showingLikedPodcasts && podcastGenre === genre} onclick={() => selectPodcastGenre(genre)}>{genre}</button>{/each}</div>
      </section>

      {#if selectedPodcast}
        <section class="podcast-show-heading">
          <button class="podcast-back" onclick={() => { selectedPodcast = null; podcastEpisodes = []; }}>‹</button>
          {#if selectedPodcast.image}<img src={selectedPodcast.image} alt="" />{:else}<div class="podcast-art-fallback">◉</div>{/if}
          <div><p>PODCAST</p><h1>{selectedPodcast.title}</h1><small>{selectedPodcast.author || 'Independent podcast'}</small></div>
          <button class:liked={isPodcastLiked(selectedPodcast)} class="like-button podcast-heading-like" onclick={() => togglePodcastLike(selectedPodcast!)} aria-label={`${isPodcastLiked(selectedPodcast) ? 'Unlike' : 'Like'} ${selectedPodcast.title}`}>{isPodcastLiked(selectedPodcast) ? '♥' : '♡'}</button>
        </section>
        <section class="episode-list" aria-busy={podcastLoading}>
          {#if podcastLoading}<div class="loading-list"><i></i><span>Loading episodes…</span></div>{/if}
          {#each podcastEpisodes as episode (episode.id)}
            {@const download = podcastDownloadFor(episode.id)}
            {@const episodeImage = episode.image || selectedPodcast.image}
            <article class="episode-row">
              <button class="episode-art" onclick={() => playPodcast(episode)} aria-label={`Play ${episode.title}`}>
                <span class="podcast-art-fallback">◉</span>
                {#if episodeImage}<img src={episodeImage} alt="" onerror={(event) => usePodcastArtwork(event, selectedPodcast!.image)} />{/if}
                <i aria-hidden="true">▶</i>
              </button>
              <button class="episode-copy" onclick={() => playPodcast(episode)}>
                <strong>{episode.title}</strong>
                {#if episode.description}<span>{episode.description}</span>{/if}
                <small>{podcastDate(episode.datePublished)}{episode.duration ? ` · ${clock(episode.duration)}` : ''}</small>
              </button>
              <button class:ready={download?.ready} class="episode-download" onclick={() => downloadPodcast(episode)} disabled={download?.status === 'Downloading'} aria-label={`Download ${episode.title}`} title={download?.status || 'Download for offline listening'}>{download?.ready ? '✓' : download?.status === 'Downloading' ? `${Math.round(download.progress)}%` : '⇩'}</button>
            </article>
          {/each}
          {#if !podcastLoading && podcastEpisodes.length === 0}<div class="empty-library"><h2>No playable episodes</h2><p>This feed may not currently expose supported HTTPS audio.</p></div>{/if}
        </section>
      {:else}
        {#if podcastHistory.length > 0 && !podcastQuery && !podcastGenre && !showingLikedPodcasts}
          <section class="podcast-history"><div class="section-label"><b>Recently played</b><span>Last 10</span></div><div class="history-scroller">{#each podcastHistory as episode (episode.id)}<button onclick={() => playPodcast(episode)}>{#if episode.image}<img src={episode.image} alt="" />{:else}<span>◉</span>{/if}<strong>{episode.title}</strong><small>{episode.feedTitle}</small></button>{/each}</div></section>
        {/if}
        <section class="library-heading">
          <div><p>{showingLikedPodcasts ? 'FAVOURITES' : 'POWERED BY PODCAST INDEX'}</p><h1>{showingLikedPodcasts ? 'Liked podcasts' : podcastGenre ? podcastGenre : podcastQuery ? `Results for “${podcastQuery}”` : 'Discover podcasts'}</h1></div>
          <span>{podcastFeeds.length} shows</span>
        </section>
        <section class="podcast-grid" aria-busy={podcastLoading}>
          {#if podcastLoading}<div class="loading-list"><i></i><span>Searching podcasts…</span></div>{/if}
          {#each podcastFeeds as feed (feed.id)}
            <article class="podcast-card">
              <button class="podcast-open" onclick={() => openPodcast(feed)}>
                {#if feed.image}<img src={feed.image} alt="" />{:else}<div class="podcast-art-fallback">◉</div>{/if}
                <span><strong>{feed.title}</strong><small>{feed.author || 'Independent podcast'}</small></span>
              </button>
              <button class:liked={isPodcastLiked(feed)} class="like-button podcast-like" onclick={() => togglePodcastLike(feed)} aria-label={`${isPodcastLiked(feed) ? 'Unlike' : 'Like'} ${feed.title}`}>{isPodcastLiked(feed) ? '♥' : '♡'}</button>
            </article>
          {/each}
          {#if !podcastLoading && podcastFeeds.length === 0}<div class="empty-library"><h2>{showingLikedPodcasts ? 'No liked podcasts yet' : 'Search podcasts'}</h2><p>{showingLikedPodcasts ? 'Tap the heart beside a podcast to keep it here.' : 'Napstrfy searches podcasts directly over this phone\'s internet connection.'}</p></div>{/if}
        </section>
      {/if}
    {:else}
      <section class="search-area audiobook-search">
        <form onsubmit={(event) => { event.preventDefault(); event.currentTarget.querySelector('input')?.blur(); void loadAudiobooks(); }}>
          <span>⌕</span><input bind:value={audiobookQuery} placeholder="Search audiobooks" aria-label="Search audiobooks" />
          {#if audiobookQuery}<button type="button" class="clear-search" onclick={() => { audiobookQuery = ''; void loadAudiobooks(); }}>×</button>{/if}
        </form>
      </section>

      {#if selectedAudiobook}
        <section class="audiobook-show-heading">
          <button class="podcast-back" onclick={() => (selectedAudiobook = null)}>‹</button>
          <div class="audiobook-cover">▥</div>
          <div><p>AUDIOBOOK</p><h1>{selectedAudiobook.title}</h1><small>{selectedAudiobook.author || 'Unknown author'}{selectedAudiobook.narrator ? ` · Read by ${selectedAudiobook.narrator}` : ''}</small></div>
        </section>
        <section class="audiobook-chapter-list" aria-busy={audiobookLoading}>
          {#each selectedAudiobook.chapters as chapter, index (chapter.fileId)}
            <button class="audiobook-chapter" disabled={status.streamOnly && !chapter.local} onclick={() => activateAudiobookChapter(selectedAudiobook!, chapter)}>
              <span>{chapter.local ? '▶' : status.streamOnly ? '—' : '⇩'}</span>
              <span><strong>{chapter.title || chapter.filename}</strong><small>Chapter {index + 1} · {readableSize(chapter.size)}</small></span>
            </button>
          {/each}
        </section>
      {:else}
        <section class="library-heading">
          <div><p>YOUR NAPSTR</p><h1>Audiobooks</h1></div>
          <span>{audiobookTotal} {audiobookTotal === 1 ? 'book' : 'books'}</span>
        </section>
        <section class="audiobook-list" aria-busy={audiobookLoading}>
          {#if audiobookLoading}<div class="loading-list"><i></i><span>Asking Napstr…</span></div>{/if}
          {#each audiobooks as book (book.audiobookId)}
            <button class="audiobook-card" onclick={() => openAudiobook(book)}>
              <span class="audiobook-cover">▥</span>
              <span><strong>{book.title}</strong><small>{book.author || 'Unknown author'}</small><i>{book.chapterCount} {book.chapterCount === 1 ? 'file' : 'chapters'} · {readableSize(book.totalSize)}</i></span>
              <b>›</b>
            </button>
          {/each}
          {#if !audiobookLoading && audiobooks.length === 0}<div class="empty-library"><h2>No audiobooks found</h2><p>Group a chapter folder in Napstr, or add the tag “audiobook” to a complete one-file book.</p></div>{/if}
        </section>
      {/if}
    {/if}

    <nav class:dragging={sheetDragging || barDragging} style={`--nav-shift:${navShift}`} class="bottom-nav" aria-label="Napstrfy navigation">
      <button class:active={activeTab === 'music'} onclick={showMusic}><span>♫</span>Music</button>
      <button class:active={activeTab === 'search'} onclick={showSearch}><span class="nav-icon"><svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="6.4" /><path d="M15.9 15.9 20.6 20.6" /></svg></span>Search</button>
      <button class:active={activeTab === 'podcasts'} onclick={showPodcasts}><span>◉</span>Podcasts</button>
      <button class:active={activeTab === 'audiobooks'} onclick={showAudiobooks}><span>▥</span>Audiobooks</button>
    </nav>

    <section
      class:dragging={barDragging}
      style={`--bar-shift:${barShift}px; --bar-opacity:${barFade}; --bar-progress:${barProgress}`}
      class:empty={activeMedia === 'music' ? !current : !currentPodcast}
      class="now-playing"
    >
      <span class="now-fill" aria-hidden="true"></span>
      <button
        class="now-open"
        bind:this={barElement}
        disabled={!nowPlayingAvailable()}
        onclick={handleBarClick}
        onpointerdown={startBarDrag}
        onpointermove={moveBarDrag}
        onpointerup={endBarDrag}
        onpointercancel={endBarDrag}
        aria-label="Open the now playing screen"
      >
        {#if activeMedia === 'podcast' && currentPodcast}
          {#if currentPodcast.image}<img class="podcast-player-art" src={currentPodcast.image} alt="" />{:else}<div class="empty-art">◉</div>{/if}
        {:else if current}<TrackArtwork track={current} large lookup />{:else}<div class="empty-art">♪</div>{/if}
        <div class="now-copy">
          <div class="now-title" bind:this={titleClipper}>
            {#key nowTitle}
              <div class:marquee={titleOverflows} class="now-title-row">
                <span bind:this={titleText}>{nowTitle}</span>
                {#if titleOverflows}<span aria-hidden="true">{nowTitle}</span>{/if}
              </div>
            {/key}
          </div>
          <small>{nowArtist}</small>
        </div>
      </button>
      <button class="now-play" onclick={togglePlayer} disabled={(!current && !currentPodcast) || caching} aria-label={playing ? 'Pause' : 'Play'}>
        {#if caching}<span class="icon-busy"></span>{:else if playing}<span class="icon-pause"></span>{:else}<span class="icon-play"></span>{/if}
      </button>
    </section>
  </main>
{/if}

{#if showNowPlaying && current && activeMedia === 'music'}
  <div
    class="now-sheet"
    class:entering={sheetEntering}
    class:closing={sheetClosing}
    class:dragging={sheetDragging}
    bind:this={sheetElement}
    style={`--sheet-drag:${sheetDragY}px; --cover-hue:${artworkHue(current.fileId)}`}
    role="dialog"
    aria-modal="true"
    tabindex="-1"
    aria-label="Now playing"
    onpointerdown={startSheetDrag}
    onpointermove={moveSheetDrag}
    onpointerup={endSheetDrag}
    onpointercancel={endSheetDrag}
  >
    <div class="now-sheet-hero">
      {#if sheetCoverUrl()}
        <div class="now-sheet-backdrop" style={`background-image:url(${sheetCoverUrl()})`}></div>
      {:else}
        <div class="now-sheet-backdrop empty"></div>
      {/if}
      <div class="now-sheet-scrim"></div>
      <div class="now-sheet-top">
        <button class="now-sheet-icon" onclick={closeNowPlaying} aria-label="Close the now playing screen">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 9.5 12 16l7-6.5" /></svg>
        </button>
        <div class="now-sheet-top-buttons">
          {#if remoteAvailable()}
            <button class="now-sheet-icon" onclick={openRemote} aria-label="Control the computer">
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <rect x="3" y="4.6" width="18" height="12" rx="2" /><path d="M12 16.6V20" /><path d="M9 20h6" />
              </svg>
            </button>
          {/if}
          <button class="now-sheet-icon" onclick={() => openActions(null)} aria-label="Track options">
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <circle class="filled" cx="12" cy="5.6" r="1.7" /><circle class="filled" cx="12" cy="12" r="1.7" /><circle class="filled" cx="12" cy="18.4" r="1.7" />
            </svg>
          </button>
        </div>
      </div>
      <div class="now-sheet-art">
        {#if sheetCoverUrl()}
          <img src={sheetCoverUrl()} alt="" onerror={() => (nowArtFailed = true)} />
        {:else}<div class="now-sheet-art-empty">♪</div>{/if}
      </div>
    </div>

    <div class="now-sheet-body" bind:this={sheetScroller}>
      <div class="now-sheet-timeline">
        <input type="range" min="0" max={duration || 0} step="0.1" value={currentTime} oninput={(event) => seek(Number(event.currentTarget.value))} aria-label="Seek" />
      </div>

      <div class="now-sheet-meta">
        <span>{clock(currentTime)}</span>
        <span class="now-sheet-speed">Speed: 1x</span>
        <span>{clock(duration)}</span>
      </div>

      <div class="now-sheet-copy">
        <h1>{title(current)}</h1>
        <p>{artist(current)}</p>
        {#if current.album}<small>{current.album}</small>{/if}
        {#if fileSummary(current)}<em>{fileSummary(current)}</em>{/if}
      </div>

      <div class="now-sheet-actions">
        <button onclick={() => moveTrack(-1)} disabled={playerQueue.length < 2 || (shuffle && randomHistoryIndex <= 0)} aria-label="Previous track">|◀</button>
        <button class="skip-button" onclick={() => nudge(-10)} aria-label="Back 10 seconds">
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <circle cx="12" cy="12" r="7.4" />
            <path class="filled" d="M12 1.9 8.4 5.2l3.6 3.3z" />
            <text class="filled" x="12" y="15.1" text-anchor="middle" font-size="8.4" font-weight="700">10</text>
          </svg>
        </button>
        <button class="play-main" class:square={playing} onclick={togglePlayer} disabled={caching} aria-label={playing ? 'Pause' : 'Play'}>
          {#if caching}<span class="icon-busy"></span>{:else if playing}<span class="icon-pause"></span>{:else}<span class="icon-play"></span>{/if}
        </button>
        <button class="skip-button" onclick={() => nudge(10)} aria-label="Forward 10 seconds">
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <circle cx="12" cy="12" r="7.4" />
            <path class="filled" d="M12 1.9 15.6 5.2 12 8.5z" />
            <text class="filled" x="12" y="15.1" text-anchor="middle" font-size="8.4" font-weight="700">10</text>
          </svg>
        </button>
        <button onclick={() => moveTrack(1)} disabled={playerQueue.length < 2} aria-label="Next track">▶|</button>
      </div>

      <div class="now-sheet-modes">
        <button class:active={loopMode !== 'off'} onclick={cycleLoopMode} aria-label={LOOP_LABELS[loopMode]} title={LOOP_LABELS[loopMode]}>
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M4.5 9.2A4.7 4.7 0 0 1 9.2 4.5H18" /><path d="M15.6 1.8 18.6 4.5 15.6 7.2" />
            <path d="M19.5 14.8a4.7 4.7 0 0 1-4.7 4.7H6" /><path d="M8.4 22.2 5.4 19.5 8.4 16.8" />
            {#if loopMode === 'one'}<path d="M11.7 11.4 12.9 10.3v5.2" /><path d="M11.2 15.5h3.4" />{/if}
          </svg>
        </button>
        <button class:active={shuffle} onclick={toggleShuffle} aria-label={shuffle ? 'Shuffle on' : 'Shuffle off'} title="Shuffle">
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M3.5 6.5h3.2l10.1 11h4" /><path d="M18.3 3.7 21 6.5l-2.7 2.8" />
            <path d="M3.5 17.5h3.2l10.1-11h4" /><path d="M18.3 14.7 21 17.5l-2.7 2.8" />
          </svg>
        </button>
        <button onclick={() => (showQueue = true)} aria-label="Open the playlist" title="Playlist">
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M4 6.5h16" /><path d="M4 12h16" /><path d="M4 17.5h9" />
            <path class="filled" d="M19.4 15.4a1.5 1.5 0 1 1-3 0 1.5 1.5 0 0 1 3 0z" />
            <path d="M16.4 15.4v-3.6l3-.7" />
          </svg>
        </button>
        <button disabled aria-label="Smart playlists, coming soon" title="Smart playlists, coming soon">
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M12 12h.01" /><path d="M8.4 8.4a5.1 5.1 0 0 0 0 7.2" /><path d="M15.6 8.4a5.1 5.1 0 0 1 0 7.2" />
          </svg>
        </button>
        <button class="now-mode-like" class:liked={current ? isTrackLiked(current) : false} onclick={() => current && toggleTrackLike(current)} aria-label="Like this track" disabled={!current}>
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M12 20.3c-1.4-1-7.2-5.2-7.2-9.4A4.2 4.2 0 0 1 12 8.2a4.2 4.2 0 0 1 7.2 2.7c0 4.2-5.8 8.4-7.2 9.4z" />
          </svg>
        </button>
      </div>
    </div>
  </div>
{/if}

{#if showQueue && activeMedia === 'music'}
  <div class="queue-view" role="dialog" aria-modal="true" aria-label="Playlist">
    <header class="queue-head">
      <div>
        <p>{shuffle ? 'SHUFFLED' : 'PLAYING NEXT'}</p>
        <h1>{playerQueue.length === 1 ? '1 track' : `${playerQueue.length} tracks`}</h1>
      </div>
      <button class="queue-close" onclick={() => (showQueue = false)} aria-label="Close the playlist">×</button>
    </header>
    <div class="queue-list">
      {#each playerQueue as track, index (track.fileId)}
        <button class:playing={index === playerIndex} class="queue-row" onclick={() => playFromQueue(index)}>
          <span class="queue-index">{index === playerIndex ? '▶' : index + 1}</span>
          <TrackArtwork track={track} lookup={index < 12} />
          <span class="queue-copy"><strong>{title(track)}</strong><small>{artist(track)}</small></span>
        </button>
      {/each}
      {#if playerQueue.length === 0}<p class="queue-empty">Nothing is queued yet.</p>{/if}
    </div>
  </div>
{/if}

{#if showSettings}
  <div class="settings-view" role="dialog" aria-modal="true" aria-label="Settings">
    <header class="view-head">
      <h1>Settings</h1>
      <button class="view-icon" onclick={() => (showSettings = false)} aria-label="Close settings">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6.5 6.5 17.5 17.5" /><path d="M17.5 6.5 6.5 17.5" /></svg>
      </button>
    </header>
    <div class="settings-scroll">
      <div class="settings-status">
        <i class:offline={!status.connected}></i>
        <div>
          <strong>{status.paired ? (status.connected ? status.desktopName || 'Napstr' : 'Not reachable') : 'Not paired'}</strong>
          <small>{status.paired ? (status.connected ? 'Connected over Iroh' : 'Tap reconnect to try again') : 'Pair with Napstr on your computer'}</small>
        </div>
      </div>
      {#if status.streamOnly}<p class="settings-note">This pairing is read only: it can browse and play, but cannot ask Napstr to download or publish.</p>{/if}

      {#if status.paired}
        <button class="settings-row" onclick={() => { showSettings = false; void reconnect(); }} disabled={statusPending}>
          <span>Reconnect</span><small>{statusPending ? 'Trying…' : 'Refresh the connection now'}</small>
        </button>
        <button class="settings-row danger" onclick={() => { showSettings = false; void forgetDesktop(); }}>
          <span>Disconnect this phone</span><small>You will need a new QR code</small>
        </button>
      {:else}
        <button class="settings-row" onclick={() => { showSettings = false; showMusic(); }}>
          <span>Pair Napstr</span><small>Scan a QR code from the computer</small>
        </button>
      {/if}

      {#if status.paired}
        <div class="settings-section">
          <p>The computer</p>
          <button class="settings-row" onclick={() => { showSettings = false; openRemote(); }} disabled={!status.connected}>
            <span>Control its player</span>
            <small>{status.connected ? (status.streamOnly ? 'See what it is playing' : 'Play, pause, skip and set the volume') : 'Offline'}</small>
          </button>
        </div>
      {/if}

      {#if status.paired && !status.streamOnly}
        <div class="settings-section">
          <p>Lend access</p>
          <button class="settings-row" onclick={() => void requestReadOnlyCode()} disabled={ticketBusy || !status.connected}>
            <span>Create a read-only code</span>
            <small>{ticketBusy ? 'Asking Napstr…' : 'For a guest phone'}</small>
          </button>
          {#if ticketError}<p class="settings-note">{ticketError}</p>{/if}
          {#if readOnlyTicket}
            <div class="ticket-card">
              {#if readOnlyTicket.qrSvg}
                <div class="ticket-qr">{@html readOnlyTicket.qrSvg}</div>
              {:else}
                <p class="ticket-note">Your computer drew no QR image. Send the code below instead.</p>
              {/if}
              <code>{readOnlyTicket.uri}</code>
              <small>Expires in about {ticketMinutesLeft()} minutes. Whoever scans this can browse, listen and keep what they play, and nothing else.</small>
              <div class="ticket-actions">
                <button onclick={() => void copyReadOnlyCode()}>Copy code</button>
                <button onclick={() => (readOnlyTicket = null)}>Done</button>
              </div>
            </div>
          {/if}
        </div>
      {/if}

      {#if COVER_DEBUG}
        <div class="settings-section">
          <p>Developer tools</p>
          <CoverDebug {tracks} {status} embedded />
        </div>
      {/if}
    </div>
  </div>
{/if}

{#if showAlbumView && albumView}
  <div class="album-view" style={`--cover-hue:${artworkHue(albumView.tracks[0]?.fileId ?? albumView.key)}`} role="dialog" aria-modal="true" aria-label={`${albumView.album} by ${albumView.artist}`}>
    <div class="album-glow" style={albumView.art ? `background-image:url(${albumView.art})` : ''}></div>
    <div class="album-glow-scrim"></div>
    <header class="view-head">
      <button class="view-icon" onclick={closeAlbumView} aria-label="Close the album">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M14.5 5 8 12l6.5 7" /></svg>
      </button>
      <button class="view-icon" onclick={() => openActions(albumView?.tracks[0] ?? null)} aria-label="Album options" disabled={albumView.tracks.length === 0}>
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <circle class="filled" cx="12" cy="5.6" r="1.7" /><circle class="filled" cx="12" cy="12" r="1.7" /><circle class="filled" cx="12" cy="18.4" r="1.7" />
        </svg>
      </button>
    </header>

    <div class="album-scroll">
      <div class="album-art">
        {#if albumView.art}<img src={albumView.art} alt="" />{:else}<div class="album-art-empty">♪</div>{/if}
      </div>

      <div class="album-title-row">
        <div class="album-title-copy">
          <h1>{albumView.album}</h1>
          <p>{albumView.artist || 'Unknown artist'}</p>
          <p class="album-meta">Album · {albumView.year || 'Year unknown'}</p>
        </div>
        <button class="album-play-all" onclick={() => void playAlbumNow()} disabled={albumView.tracks.length === 0} aria-label={`Play ${albumView.album}`}>
          {#if caching}<span class="icon-busy"></span>{:else}<span class="icon-play"></span>{/if}
        </button>
      </div>

      <ol class="album-tracks">
        {#each albumView.tracks as track, index (track.fileId)}
          <li class:playing={current?.fileId === track.fileId}>
            <button class="album-track" onclick={() => void playAlbumTrack(index)}>
              <span class="album-track-index">{current?.fileId === track.fileId ? '▶' : index + 1}</span>
              <span class="album-track-copy"><strong>{title(track)}</strong><small>{artist(track)}</small></span>
            </button>
            <button class="album-track-more" onclick={() => openActions(track)} aria-label={`Options for ${title(track)}`}>
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <circle class="filled" cx="12" cy="5.6" r="1.5" /><circle class="filled" cx="12" cy="12" r="1.5" /><circle class="filled" cx="12" cy="18.4" r="1.5" />
              </svg>
            </button>
          </li>
        {/each}
      </ol>
      {#if albumView.tracks.length === 0}<p class="queue-empty">No tracks for this album yet.</p>{/if}

      {#if albumView.more.length > 0}
        <div class="album-shelf-block">
          <div class="section-label"><b>More by {albumView.artist}</b><span>{albumView.more.length} albums</span></div>
          <div class="album-shelf">
            {#each albumView.more as album (album.key)}
              <div class="album-card">
                <button class="album-open" onclick={() => void openAlbum(album)} aria-label={`Open ${album.album}`}>
                  <TrackArtwork track={album.representative} lookup />
                  <span class="album-play" aria-hidden="true">▶</span>
                </button>
                <strong>{album.album}</strong>
                <small>{album.artist || 'Unknown artist'}</small>
              </div>
            {/each}
          </div>
        </div>
      {/if}
    </div>
  </div>
{/if}

{#if showRemote}
  <div
    class="remote-view"
    role="dialog"
    aria-modal="true"
    aria-label={`Playing on ${status.desktopName || 'the computer'}`}
  >
    <header class="view-head">
      <div class="remote-head-copy">
        <p>PLAYING ON</p>
        <h1>{status.desktopName || 'Napstr'}</h1>
      </div>
      <div class="now-sheet-top-buttons">
        <button class="view-icon" onclick={() => void refreshRemote()} aria-label="Refresh what the computer is playing">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M20.4 12a8.4 8.4 0 1 1-2.7-6.2" /><path d="M20.4 4.4v5.4H15" /></svg>
        </button>
        <button class="view-icon" onclick={() => (showRemote = false)} aria-label="Close">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6.5 6.5 17.5 17.5" /><path d="M17.5 6.5 6.5 17.5" /></svg>
        </button>
      </div>
    </header>

    <div class="remote-scroll">
      {#if remoteError}<div class="error-card"><span>{remoteError}</span></div>{/if}
      {#if remoteState?.error}<div class="error-card"><span>{remoteState.error}</span></div>{/if}

      {#if remoteState?.active}
        <div class="remote-now">
          <h2>{remoteState.title || 'Unknown track'}</h2>
          <p>{remoteState.artist || 'Unknown artist'}</p>
          {#if remoteState.album}<small>{remoteState.album}</small>{/if}
        </div>
        <input
          class="remote-seek"
          type="range"
          min="0"
          max={remoteState.durationMs || 0}
          step="1000"
          value={remoteState.positionMs}
          onchange={(event) => void sendPlayback({ type: 'seek', positionMs: Number(event.currentTarget.value) })}
          disabled={status.streamOnly}
          aria-label="Seek on the computer"
        />
        <div class="remote-times">
          <span>{remoteClock(remoteState.positionMs)}</span>
          <span>{remoteState.queueLen > 0 ? `Queue: ${remoteState.queueLen}` : 'One track'}</span>
          <span>{remoteClock(remoteState.durationMs)}</span>
        </div>
      {:else}
        <div class="remote-now">
          <h2>Nothing is playing there</h2>
          <p>
            {status.streamOnly
              ? 'This pairing is read only, so it cannot start anything.'
              : 'Napstr will pick up from wherever the computer left off.'}
          </p>
        </div>
      {/if}

      <div class="remote-transport">
        <button
          onclick={() => void sendPlayback({ type: 'previous' })}
          disabled={!remoteState?.active || (remoteState?.queueLen ?? 0) < 2 || status.streamOnly}
          aria-label="Previous track on the computer">|◀</button>
        <button
          class="remote-main"
          onclick={() => void sendPlayback(remoteState?.active ? { type: 'toggle' } : { type: 'play' })}
          disabled={remoteBusy || status.streamOnly}
          aria-label={remoteState?.playing ? 'Pause on the computer' : 'Play on the computer'}>
          {#if remoteBusy}<span class="icon-busy"></span>{:else if remoteState?.playing}<span class="icon-pause"></span>{:else}<span class="icon-play"></span>{/if}
        </button>
        <button
          onclick={() => void sendPlayback({ type: 'next' })}
          disabled={!remoteState?.active || (remoteState?.queueLen ?? 0) < 2 || status.streamOnly}
          aria-label="Next track on the computer">▶|</button>
      </div>

      <div class="remote-options">
        <button
          class:active={(remoteState?.repeat ?? 'off') !== 'off'}
          onclick={() => void sendPlayback({ type: 'repeat', mode: nextRemoteRepeat(remoteState?.repeat ?? 'off') })}
          disabled={status.streamOnly}>
          <span>Repeat</span><small>{remoteRepeatLabel(remoteState?.repeat ?? 'off')}</small>
        </button>
        <button
          class:active={remoteState?.shuffle === true}
          onclick={() => void sendPlayback({ type: 'shuffle', enabled: remoteState?.shuffle !== true })}
          disabled={status.streamOnly}>
          <span>Shuffle</span><small>{remoteState?.shuffle ? 'On' : 'Off'}</small>
        </button>
      </div>

      <label class="remote-volume">
        <span>Volume</span>
        <input
          type="range"
          min="0"
          max="100"
          step="1"
          value={Math.round((remoteState?.volume ?? 1) * 100)}
          oninput={(event) => void sendVolume(Number(event.currentTarget.value))}
          disabled={status.streamOnly}
        />
      </label>

      <button
        class="remote-stop"
        onclick={() => void sendPlayback({ type: 'stop' })}
        disabled={status.streamOnly || !remoteState?.active}>Stop on the computer</button>

      <p class="remote-note">
        {status.streamOnly
          ? 'This pairing is read only: you can see what is playing there, but not change it.'
          : 'Your own pairing signs every command. No Nostr keys leave your computer.'}
      </p>
    </div>
  </div>
{/if}

{#if showReport}
  <div class="report-view" role="dialog" aria-modal="true" aria-label="Report a cover">
    <button class="actions-scrim" onclick={() => (showReport = false)} aria-label="Close the report"></button>
    <div class="actions-panel report-panel">
      <div class="report-head">
        <h1>Report this cover</h1>
        <p>{reportLabel}</p>
      </div>
      <div class="actions-divider"></div>
      {#each reportReasons as reason (reason.value)}
        <button class:active={reportReason === reason.value} class="actions-row" onclick={() => (reportReason = reason.value)}>
          <span>{reason.label}</span>
          {#if reportReason === reason.value}<small>Chosen</small>{/if}
        </button>
      {/each}
      <label class="report-note">
        <span>Anything to add? (optional)</span>
        <textarea bind:value={reportNote} rows="3" maxlength="500" placeholder="Say what is wrong with this cover"></textarea>
      </label>
      {#if reportError}<p class="report-error">{reportError}</p>{/if}
      <p class="remote-note">
        Signed by {status.desktopName || 'your computer'} as a NIP-56 report. It tells other clients which cover to distrust.
      </p>
      <button class="report-send" onclick={() => void submitCoverReport()} disabled={reportBusy || !status.connected}>
        {reportBusy ? 'Sending…' : 'Send report'}
      </button>
    </div>
  </div>
{/if}

{#if showActions && menuTrack}
  <div class="actions-view" role="dialog" aria-modal="true" aria-label="Track options">
    <button class="actions-scrim" onclick={closeActions} aria-label="Close the track options"></button>
    <div class="actions-panel">
      <div class="actions-head">
        <TrackArtwork track={menuTrack} lookup />
        <div class="actions-head-copy"><strong>{title(menuTrack)}</strong><small>{artist(menuTrack)}</small></div>
      </div>
      <div class="actions-divider"></div>

      {#if showSleepOptions}
        <button class="actions-row back" onclick={() => (showSleepOptions = false)}>
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M14.5 5 8 12l6.5 7" /></svg>
          <span>Sleep timer</span><small>{sleepSummary()}</small>
        </button>
        {#each SLEEP_OPTIONS as option (option.value)}
          <button class:active={sleepValue === option.value} class="actions-row" onclick={() => chooseSleep(option)}>
            <span>{option.label}</span>
            {#if sleepValue === option.value}<small>On</small>{/if}
          </button>
        {/each}
      {:else}
        <button class="actions-row" onclick={() => void shareTrack(menuTrack)}>
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 16V4" /><path d="M8 7.5 12 3.5l4 4" /><path d="M5 14v6h14v-6" /></svg>
          <span>Share</span><small>{trackUri(menuTrack)}</small>
        </button>
        <button class="actions-row" onclick={() => toggleTrackLike(menuTrack)}>
          <svg class:filled={isTrackLiked(menuTrack)} viewBox="0 0 24 24" aria-hidden="true"><path d="M12 20.3c-1.4-1-7.2-5.2-7.2-9.4A4.2 4.2 0 0 1 12 8.2a4.2 4.2 0 0 1 7.2 2.7c0 4.2-5.8 8.4-7.2 9.4z" /></svg>
          <span>{isTrackLiked(menuTrack) ? 'Remove from Liked Songs' : 'Add to Liked Songs'}</span>
        </button>
        <button class="actions-row" disabled>
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 6.5h11" /><path d="M4 12h11" /><path d="M4 17.5h7" /><path d="M17 14v6" /><path d="M14 17h6" /></svg>
          <span>Add to playlist</span><small>Coming soon</small>
        </button>
        <button class="actions-row" onclick={() => void goToAlbum(menuTrack)}>
          <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="8" /><circle class="filled" cx="12" cy="12" r="2.4" /></svg>
          <span>Go to album</span>
        </button>
        <button class="actions-row" disabled={!menuTrack.artist.trim()} onclick={() => goToArtist(menuTrack.artist)}>
          <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="8.2" r="3.4" /><path d="M5.6 19.6a6.5 6.5 0 0 1 12.8 0" /></svg>
          <span>Go to artist</span>
        </button>
        <button class="actions-row" disabled={!remoteAvailable()} onclick={() => openCoverReport(menuTrack)}>
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 4.8 21 20H3z" /><path d="M12 10.6v4" /><circle class="filled" cx="12" cy="17.4" r="0.9" /></svg>
          <span>Report this cover</span><small>{remoteAvailable() ? '' : 'Needs a connection'}</small>
        </button>
        <button class="actions-row" onclick={() => (showSleepOptions = true)}>
          <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="8" /><path d="M12 7.4V12l3.1 2" /></svg>
          <span>Sleep timer</span><small>{sleepSummary()}</small>
        </button>
      {/if}
    </div>
  </div>
{/if}

<audio
  bind:this={audio}
  onplay={() => { playing = true; syncSystemMedia(true); }}
  onpause={() => { playing = false; syncSystemMedia(true); }}
  ontimeupdate={() => { currentTime = audio.currentTime; syncSystemMedia(); }}
  ondurationchange={() => { duration = Number.isFinite(audio.duration) ? audio.duration : 0; syncSystemMedia(true); }}
  onended={handleTrackEnded}
  onerror={() => {
    if (activeMedia === 'podcast' && currentPodcast) error = `This phone could not play ${currentPodcast.title}.`;
    else if (current) error = `This phone could not decode ${current.format} audio.`;
  }}
></audio>
