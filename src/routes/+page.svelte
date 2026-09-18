<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { getVersion } from '@tauri-apps/api/app';
  import { invoke } from '@tauri-apps/api/core';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { open } from '@tauri-apps/plugin-dialog';
  import CoverArt from '$lib/CoverArt.svelte';
  import CoverPicker from '$lib/CoverPicker.svelte';
  import CoverReport from '$lib/CoverReport.svelte';
  import { albumsFor, artUrl, clearCoverCache, coverFor, coverKey, type CoverAlbum } from '$lib/artwork';

  let appVersion = '…';
  const SEARCH_PAGE_SIZE = 100;
  const LOCAL_PAGE_SIZE = 100;
  const VISIBLE_SEEDER_LIMIT = 100;

  type View = 'Search' | 'Downloads' | 'Shared' | 'Profile' | 'Settings' | 'Trollbox' | 'Mobile' | 'Covers';
  type PlayerMode = 'single' | 'folder' | 'all';
  type PlayerOrigin = 'search' | 'downloads' | 'shared' | 'audiobook' | 'direct';
  type WindowResizeDirection = 'East' | 'North' | 'NorthEast' | 'NorthWest' | 'South' | 'SouthEast' | 'SouthWest' | 'West';
  type Result = {
    id: number;
    name: string;
    format: string;
    size: string;
    sources: number;
    speed: string;
    length: string;
    fileId: string;
    sourceDetails?: SourceDetail[];
    remote?: boolean;
    artist?: string;
    album?: string;
    license?: string;
    description?: string;
    tags?: string;
    audiobook?: Audiobook;
  };
  type CatalogueUser = { pubkey: string; npub: string; displayName: string };
  type SourceDetail = { pubkey: string; npub: string; displayName: string; relay: string; about: string; picture: string; eventId: string };
  type Transfer = {
    id: number;
    fileId: string;
    name: string;
    size: string;
    speed: string;
    progress: number;
    status: string;
    destination: string;
  };

  const views: { label: View; icon: string }[] = [
    { label: 'Search', icon: '⌕' },
    { label: 'Downloads', icon: '⇩' },
    { label: 'Shared', icon: '▤' },
    { label: 'Profile', icon: '☺' },
    { label: 'Settings', icon: '⚙' },
    { label: 'Trollbox', icon: '▣' },
    { label: 'Mobile', icon: '▯' },
    { label: 'Covers', icon: '▨' }
  ];

  type NativeFile = { fileId: string; filename: string; path: string; folder: string; size: number; format: string; status: string; title: string; artist: string; album: string; mime: string; license: string; description: string; tags: string };
  type NativeTransfer = { id: number; fileId: string; filename: string; size: number; progress: number; status: string; speed: string; destination: string };
  type NativeSettings = { napstrFolder: string; nostrRelays: string; displayName: string; profileAbout: string; profilePicture: string };
  type AudiobookChapter = { position: number; fileId: string; filename: string; title: string; format: string; mime: string; size: number };
  type Audiobook = { audiobookId: string; title: string; author: string; narrator: string; totalSize: number; chapters: AudiobookChapter[]; sources: SourceDetail[]; local: boolean; localFolder: string };
  type Snapshot = { files: NativeFile[]; audiobooks: Audiobook[]; transfers: NativeTransfer[]; settings: NativeSettings; indexedBytes: number; native: boolean };
  type NetworkStatus = { connected: boolean; npub: string; pubkey: string; relayCount: number; torRunning: boolean; torStarting: boolean; torProgress: number; torError: string; error: string };
  type NetworkResult = { fileId: string; filename: string; title: string; artist: string; album: string; format: string; mime: string; size: number; license: string; description: string; tags: string; sources: SourceDetail[] };
  type CatalogueBrowseCursor = { sessionId: string };
  type CatalogueBrowsePage = { results: NetworkResult[]; cursor: CatalogueBrowseCursor | null; totalAvailable: number };
  type PlayerTrack = { fileId: string; name: string; folder: string; artist: string; mime: string };
  type PlaybackStatus = { fileId: string; currentTime: number; duration: number; playing: boolean; ended: boolean; error: string };
  type ReleaseStatus = { version: string; url: string };
  type GitHubRelease = { tag_name?: unknown; html_url?: unknown };
  type TrollboxMessage = { eventId: string; pubkey: string; npub: string; displayName: string; content: string; createdAt: number };
  type IndexProgress = { scanning: boolean; processedFiles: number; indexedFiles: number; message: string };
  type IndexBatch = { files: NativeFile[]; fileCount: number; totalBytes: number };
  type MobileDevice = { endpointId: string; name: string; pairedAt: string; lastSeen: string; streamOnly: boolean };
  type MobileStatus = { running: boolean; online: boolean; endpointId: string; error: string; devices: MobileDevice[] };
  type MobilePairingOffer = { ticket: string; qrSvg: string; expiresAt: number; endpointId: string };
  type BlockConfirmation =
    | { kind: 'file'; fileId: string; label: string }
    | { kind: 'user'; pubkey: string; label: string };

  let activeView: View = 'Search';
  let results: Result[] = [];
  let resultPage = 0;
  let query = '';
  let searchUser: CatalogueUser | null = null;
  let resultUser: CatalogueUser | null = null;
  let matchingUsers: CatalogueUser[] = [];
  let format = 'Audio only';
  let minimumSources = 1;
  let maximumSize = '';
  let searchedQuery = 'All audio';
  let resultsAreNetwork = false;
  let selected: Result | null = null;
  let selectedResultIds = new Set<string>();
  let resultSelectionAnchor: string | null = null;
  let downloadingSelection = false;
  let clearingTransfers = false;
  let removingTransfers = new Set<number>();
  let downloadGeneration = 0;
  const pendingDownloadRequests = new Map<Promise<unknown>, string>();
  const downloadAttempts = new Map<string, { cancelled: boolean }>();
  const cancellingFiles = new Set<string>();
  let advanced = false;
  let paused = false;
  let aboutOpen = false;
  let sourceProfile: SourceDetail | null = null;
  let blockConfirmation: BlockConfirmation | null = null;
  let blockInProgress = false;
  let startingDownloads = new Set<string>();
  let clock = '';
  let desktopRuntime = false;
  let nativeReady = false;
  let activityMessage = 'Starting Napstr…';
  let napstrFolder = '';
  let nostrRelays = 'wss://relay.damus.io, wss://nos.lol, wss://relay.nostr.com, wss://relay.primal.net, wss://relay.snort.social, wss://nostr.mom, wss://relay.nostr.band';
  let displayName = 'napstr-user';
  let profileAbout = 'Sharing files privately with Napstr. napstr.net';
  let profilePicture = '';
  let indexedBytes = 0;
  let networkConnected = false;
  let torRunning = false;
  let torStarting = false;
  let torProgress = 0;
  let torError = '';
  let identityNpub = '';
  let networkError = '';
  let networkConnectPending = false;
  let newRelease: ReleaseStatus | null = null;
  let trollboxMessages: TrollboxMessage[] = [];
  let trollboxDraft = '';
  let trollboxLoading = false;
  let trollboxSending = false;
  let trollboxError = '';
  let trollboxPollPending = false;
  let trollboxRefreshAgain = false;
  let mobileStatusValue: MobileStatus | null = null;
  let mobilePairing: MobilePairingOffer | null = null;
  let mobileStreamPairing: MobilePairingOffer | null = null;
  let mobileStreamOnly = false;
  let mobileLoading = false;
  let mobileStatusPending = false;
  let mobileError = '';
  let trollboxLog: HTMLDivElement;
  let trackDiscussionFileId = '';
  let trackDiscussionMessages: TrollboxMessage[] = [];
  let trackDiscussionDraft = '';
  let trackDiscussionLoading = false;
  let trackDiscussionSending = false;
  let trackDiscussionError = '';
  let trackDiscussionPollPending = false;
  let trackDiscussionRefreshAgain = false;
  let trackDiscussionLog: HTMLDivElement;
  let searchAction: 'search' | 'surprise' | null = null;
  let browseCursor: CatalogueBrowseCursor | null = null;
  let browseLoading = false;
  let browseGeneration = 0;
  let loadedNetworkMatches: NetworkResult[] = [];
  let loadedNetworkAudiobooks: Audiobook[] = [];
  let browseTotalAvailable = 0;
  let rescanPending = false;
  let indexing = false;
  let downloadLibraryPage = 0;
  let sharedLibraryPage = 0;
  let selectedSource = 0;
  let selectedShared: NativeFile | null = null;
  let selectedTagFile: NativeFile | null = null;
  let tagDraft = '';
  let tagSaving = false;
  let libraryFolderView = '*';
  let libraryFolderMenuOpen = false;
  let playerMode: PlayerMode = 'single';
  let playerOrigin: PlayerOrigin = 'direct';
  let activePlayerAudiobook: Audiobook | null = null;
  let playerQueue: PlayerTrack[] = [];
  let playerQueueIndex = -1;
  let currentTrack: PlayerTrack | null = null;
  let playerPlaying = false;
  let playerLoading = false;
  let playerCurrentTime = 0;
  let playerDuration = 0;
  let playerVolume = 0.85;
  let playerEnded = false;
  let lastPlayerError = '';
  let transferPaneHeight = 119;
  let stopTransferResize = () => {};
  let transfers: Transfer[] = [];
  let localAudiobooks: Audiobook[] = [];
  let audiobookEditorOpen = false;
  let audiobookSaving = false;
  let audiobookTitle = '';
  let audiobookAuthor = '';
  let audiobookNarrator = '';
  type AudiobookDownload = { audiobookId: string; title: string; author: string; narrator: string; destinationFolder: string; chapters: AudiobookChapter[]; sources: SourceDetail[]; nextIndex: number; activeFileId: string; failed: number };
  let audiobookDownloads: AudiobookDownload[] = [];

  let sharedFiles: Array<NativeFile & { name: string; readableSize: string; peers: number }> = [];
  let localFileIds = new Set<string>();

  const readableSize = (bytes: number) => {
    if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
    if (bytes >= 1024 ** 2) return `${(bytes / 1024 ** 2).toFixed(bytes >= 100 * 1024 ** 2 ? 0 : 1)} MB`;
    if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`;
    return `${bytes} B`;
  };

  function mapFiles(files: NativeFile[]): Result[] {
    return files.map((file, index) => ({
      id: index + 1, name: file.title || file.filename, format: file.format, size: readableSize(file.size), sources: 1,
      speed: 'Local', length: '—', fileId: file.fileId, artist: file.artist, album: file.album, license: file.license, description: file.description, tags: file.tags
    }));
  }

  function mapNetworkFiles(files: NetworkResult[]): Result[] {
    return files.map((file, index) => {
      const local = localFileIds.has(file.fileId);
      return {
        id: index + 1, name: file.title || file.filename, format: file.format, size: readableSize(file.size),
        sources: file.sources.length, speed: local ? 'Local' : 'Tor', length: '—', fileId: file.fileId,
        sourceDetails: file.sources, remote: !local, artist: file.artist, album: file.album,
        license: file.license, description: file.description, tags: file.tags
      };
    });
  }

  function audiobookMatches(book: Audiobook, value: string) {
    if (/^audiobooks?$/i.test(value.trim())) return true;
    const fields = [book.title, book.author, book.narrator, ...book.chapters.flatMap((chapter) => [chapter.title, chapter.filename])]
      .join(' ')
      .toLowerCase();
    return value.trim().toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(Boolean).every((token) => fields.includes(token));
  }

  function mergeAudiobooks(trackResults: Result[], remoteBooks: Audiobook[], searchValue: string) {
    const audiobookKeyword = /^audiobooks?$/i.test(searchValue.trim());
    if (format !== 'Audiobooks' && !audiobookKeyword) return trackResults;
    const books = new Map<string, Audiobook>();
    for (const incoming of [...localAudiobooks.filter((book) => audiobookMatches(book, searchValue)), ...remoteBooks]) {
      const existing = books.get(incoming.audiobookId);
      if (!existing) {
        books.set(incoming.audiobookId, { ...incoming, sources: [...incoming.sources] });
        continue;
      }
      const sources = new Map(existing.sources.map((source) => [source.pubkey, source]));
      incoming.sources.forEach((source) => sources.set(source.pubkey, source));
      books.set(incoming.audiobookId, {
        ...(existing.local ? existing : incoming),
        local: existing.local || incoming.local,
        localFolder: existing.localFolder || incoming.localFolder,
        sources: [...sources.values()]
      });
    }
    const grouped = [...books.values()].map((book, index): Result => ({
      id: -(index + 1),
      name: book.title,
      format: 'AUDIOBOOK',
      size: readableSize(book.totalSize),
      sources: Math.max(book.local ? 1 : 0, book.sources.length),
      speed: book.local ? 'Local' : 'Tor',
      length: `${book.chapters.length} chapters`,
      fileId: `audiobook:${book.audiobookId}`,
      artist: book.author,
      album: book.narrator ? `Narrated by ${book.narrator}` : '',
      remote: !book.local,
      sourceDetails: book.sources,
      audiobook: book
    }));
    if (format === 'Audiobooks') {
      return grouped
        .sort((left, right) => right.sources - left.sources || left.name.localeCompare(right.name))
        .map((result, index) => ({ ...result, id: index + 1 }));
    }
    // The dedicated media browse presents each collection once. This is only
    // a display filter: normal title/artist searches retain every underlying
    // audio hash, so one publisher cannot hide ordinary tracks by grouping
    // them into a bogus collection.
    const chapterIds = audiobookKeyword
      ? new Set([...books.values()].flatMap((book) => book.chapters.map((chapter) => chapter.fileId)))
      : null;
    const displayedTracks = chapterIds
      ? trackResults.filter((track) => !chapterIds.has(track.fileId))
      : trackResults;
    return [...grouped, ...displayedTracks]
      .sort((left, right) => right.sources - left.sources || left.name.localeCompare(right.name))
      .map((result, index) => ({ ...result, id: index + 1 }));
  }

  function matchesType(mime: string, fileFormat: string) {
    return mime.startsWith('audio/') && ['MP3', 'FLAC', 'WAV', 'OGG', 'OPUS'].includes(fileFormat.toUpperCase());
  }

  function maximumBytes() {
    const match = maximumSize.trim().match(/^(\d+(?:\.\d+)?)\s*(B|KB|MB|GB|TB)?$/i);
    if (!match) return Number.POSITIVE_INFINITY;
    const units: Record<string, number> = { B: 1, KB: 1024, MB: 1024 ** 2, GB: 1024 ** 3, TB: 1024 ** 4 };
    return Number(match[1]) * units[(match[2] || 'B').toUpperCase()];
  }

  function eligibleNetworkMatches(matches: NetworkResult[]) {
    return matches.filter((item) =>
      item.sources.length >= minimumSources
      && item.size <= maximumBytes()
      && matchesType(item.mime, item.format)
    );
  }

  function mergeSearchResults(networkMatches: NetworkResult[], localMatches: NativeFile[]) {
    const merged = new Map<string, Result>();
    for (const result of mapNetworkFiles(eligibleNetworkMatches(networkMatches))) {
      merged.set(result.fileId, result);
    }
    if (minimumSources <= 1) {
      for (const local of mapFiles(localMatches.filter((item) =>
        item.size <= maximumBytes() && matchesType(item.mime, item.format)
      ))) {
        const existing = merged.get(local.fileId);
        merged.set(local.fileId, existing
          ? { ...existing, remote: false, speed: 'Local', sources: Math.max(1, existing.sources) }
          : local);
      }
    }
    return [...merged.values()]
      .sort((left, right) => right.sources - left.sources || left.name.localeCompare(right.name))
      .map((result, index) => ({ ...result, id: index + 1 }));
  }

  function mergeNetworkPages(existing: NetworkResult[], incoming: NetworkResult[]) {
    const merged = new Map(existing.map((item) => [item.fileId, item]));
    for (const item of incoming) {
      const previous = merged.get(item.fileId);
      if (!previous) {
        merged.set(item.fileId, item);
        continue;
      }
      const sources = new Map(previous.sources.map((source) => [source.pubkey, source]));
      for (const source of item.sources) sources.set(source.pubkey, source);
      merged.set(item.fileId, { ...previous, ...item, sources: [...sources.values()] });
    }
    return [...merged.values()];
  }

  function shuffled<T>(items: T[]) {
    const copy = [...items];
    for (let index = copy.length - 1; index > 0; index -= 1) {
      const random = new Uint32Array(1);
      window.crypto.getRandomValues(random);
      const swapIndex = random[0] % (index + 1);
      [copy[index], copy[swapIndex]] = [copy[swapIndex], copy[index]];
    }
    return copy;
  }

  function isActiveTransfer(transfer: Pick<Transfer, 'progress' | 'status'>) {
    return transfer.progress < 100 && !/^(Failed|Cancelled|Refused|All seeders refused)/.test(transfer.status);
  }

  function isCompleteTransfer(transfer: Transfer) {
    return transfer.progress >= 100 && transfer.status === 'Verified · Complete' && Boolean(transfer.destination);
  }

  function isFinishedTransfer(transfer: Transfer) {
    return isCompleteTransfer(transfer) || /^(Failed|Cancelled|Refused|All seeders refused)/.test(transfer.status);
  }

  function mapTransfers(items: NativeTransfer[]): Transfer[] {
    return items.map((transfer) => ({
      id: transfer.id,
      fileId: transfer.fileId,
      name: transfer.filename,
      size: readableSize(transfer.size),
      speed: transfer.speed,
      progress: transfer.progress,
      status: transfer.status,
      destination: transfer.destination
    }));
  }

  function isLocalFile(fileId: string) {
    return localFileIds.has(fileId);
  }

  function folderName(folder: string) {
    return folder || '(Napstr folder)';
  }

  function libraryFolders() {
    return [...new Set(sharedFiles.map((file) => file.folder))]
      .sort((left, right) => folderName(left).localeCompare(folderName(right)));
  }

  function visibleSharedFiles() {
    return libraryFolderView === '*'
      ? sharedFiles
      : sharedFiles.filter((file) => file.folder === libraryFolderView);
  }

  function audiobookFolderFiles() {
    if (libraryFolderView === '*') return [];
    const prefix = `${libraryFolderView}/`;
    return sharedFiles
      .filter((file) => file.folder === libraryFolderView || file.folder.startsWith(prefix))
      .sort((left, right) => `${left.folder}/${left.filename}`.localeCompare(`${right.folder}/${right.filename}`, undefined, { numeric: true, sensitivity: 'base' }));
  }

  function localPageCount(files: NativeFile[]) {
    return Math.max(1, Math.ceil(files.length / LOCAL_PAGE_SIZE));
  }

  function paginatedTagFiles() {
    const start = downloadLibraryPage * LOCAL_PAGE_SIZE;
    return sharedFiles.slice(start, start + LOCAL_PAGE_SIZE);
  }

  function paginatedSharedFiles() {
    const files = visibleSharedFiles();
    const start = sharedLibraryPage * LOCAL_PAGE_SIZE;
    return files.slice(start, start + LOCAL_PAGE_SIZE);
  }

  function localPageRange(page: number, total: number) {
    if (!total) return '0';
    const start = page * LOCAL_PAGE_SIZE + 1;
    return `${start}–${Math.min(start + LOCAL_PAGE_SIZE - 1, total)}`;
  }

  function changeDownloadLibraryPage(nextPage: number) {
    downloadLibraryPage = Math.max(0, Math.min(nextPage, localPageCount(sharedFiles) - 1));
  }

  function changeSharedLibraryPage(nextPage: number) {
    const files = visibleSharedFiles();
    sharedLibraryPage = Math.max(0, Math.min(nextPage, localPageCount(files) - 1));
    selectedShared = paginatedSharedFiles()[0] ?? null;
  }

  function changeLibraryFolder() {
    sharedLibraryPage = 0;
    selectedShared = visibleSharedFiles()[0] ?? null;
  }

  function selectLibraryFolder(folder: string) {
    libraryFolderView = folder;
    libraryFolderMenuOpen = false;
    changeLibraryFolder();
  }

  function containLibraryFolderMenu(node: HTMLElement) {
    const outside = (event: PointerEvent) => {
      if (libraryFolderMenuOpen && event.target instanceof Node && !node.contains(event.target)) {
        libraryFolderMenuOpen = false;
      }
    };
    const focusLeft = (event: FocusEvent) => {
      if (!(event.relatedTarget instanceof Node) || !node.contains(event.relatedTarget)) {
        libraryFolderMenuOpen = false;
      }
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      libraryFolderMenuOpen = false;
      node.querySelector<HTMLButtonElement>('.folder-picker-toggle')?.focus();
    };
    document.addEventListener('pointerdown', outside);
    node.addEventListener('focusout', focusLeft);
    node.addEventListener('keydown', escape);
    return {
      destroy() {
        document.removeEventListener('pointerdown', outside);
        node.removeEventListener('focusout', focusLeft);
        node.removeEventListener('keydown', escape);
      }
    };
  }

  // Every one of these takes its state as an argument rather than reading it
  // out of the component. Svelte decides what a template expression depends on
  // from the identifiers written in that expression, so `{resultRange()}` names
  // only a function and the state read inside the body is invisible to it: the
  // expression runs once and never again. The list would sit frozen while the
  // caption beside it counted thousands of results. Passing the state in makes
  // the dependency visible, and costs nothing over a hundred rows.
  function resultPageCount(source: Result[]) {
    return Math.max(1, Math.ceil(source.length / SEARCH_PAGE_SIZE));
  }

  function paginatedResults(source: Result[], page: number) {
    const start = page * SEARCH_PAGE_SIZE;
    return source.slice(start, start + SEARCH_PAGE_SIZE);
  }

  function resultRange(source: Result[], page: number) {
    if (!source.length) return '0';
    const start = page * SEARCH_PAGE_SIZE + 1;
    return `${start}–${Math.min(start + SEARCH_PAGE_SIZE - 1, source.length)}`;
  }

  function availableResultTotal(source: Result[], totalAvailable: number) {
    return Math.max(source.length, totalAvailable);
  }

  // What the template reads. Each derivation names the state it comes from, so
  // each one re-runs when that state changes.
  $: resultPageTotal = resultPageCount(results);
  $: resultPageItems = paginatedResults(results, resultPage);
  $: resultRangeLabel = resultRange(results, resultPage);
  $: resultAvailableTotal = availableResultTotal(results, browseTotalAvailable);

  async function changeResultPage(nextPage: number) {
    if (nextPage >= resultPageCount(results) && browseCursor && !browseLoading) {
      await loadNextBrowsePage();
    }
    resultPage = Math.max(0, Math.min(nextPage, resultPageCount(results) - 1));
    selectResult(paginatedResults(results, resultPage)[0] ?? null);
  }

  function toPlayerTrack(file: NativeFile): PlayerTrack {
    return {
      fileId: file.fileId,
      name: file.title || file.filename,
      folder: file.folder,
      artist: file.artist,
      mime: file.mime
    };
  }

  function selectTagFile(file: NativeFile) {
    selectedTagFile = { ...file };
    tagDraft = file.tags;
  }

  async function saveTags() {
    if (!nativeReady || !selectedTagFile || tagSaving) return;
    const fileId = selectedTagFile.fileId;
    tagSaving = true;
    try {
      applySnapshot(await invoke<Snapshot>('save_file_tags', { fileId, tags: tagDraft }));
      selectedTagFile = sharedFiles.find((file) => file.fileId === fileId) ?? null;
      tagDraft = selectedTagFile?.tags ?? '';
      activityMessage = 'Tags saved locally';
      if (networkConnected) {
        try {
          await invoke('publish_catalogue');
          activityMessage = 'Tags saved and queued for Nostr publication';
        } catch (error) {
          activityMessage = `Tags saved locally · Nostr publication will retry later: ${String(error)}`;
        }
      }
    } catch (error) {
      activityMessage = `Could not save tags: ${String(error)}`;
    } finally {
      tagSaving = false;
    }
  }

  function sortedLibraryTracks() {
    return sharedFiles
      .map(toPlayerTrack)
      .sort((left, right) => left.folder.localeCompare(right.folder) || left.name.localeCompare(right.name));
  }

  function contextualPlayerQueue(track: PlayerTrack, origin: PlayerOrigin) {
    let queue: PlayerTrack[] = [];
    if (origin === 'downloads') {
      queue = sharedFiles.map(toPlayerTrack);
    } else if (origin === 'search') {
      queue = results
        .filter((result) => isLocalFile(result.fileId))
        .flatMap((result) => {
          const file = sharedFiles.find((item) => item.fileId === result.fileId);
          return file ? [toPlayerTrack(file)] : [];
        });
    } else if (origin === 'shared') {
      queue = visibleSharedFiles().map(toPlayerTrack);
    } else if (origin === 'audiobook' && activePlayerAudiobook) {
      queue = activePlayerAudiobook.chapters.flatMap((chapter) => {
        const file = sharedFiles.find((candidate) => candidate.fileId === chapter.fileId);
        return file ? [toPlayerTrack(file)] : [];
      });
    }
    return queue.some((item) => item.fileId === track.fileId) ? queue : [track];
  }

  function queueForTrack(track: PlayerTrack, mode: PlayerMode, origin: PlayerOrigin = playerOrigin) {
    const library = sortedLibraryTracks();
    if (!library.some((item) => item.fileId === track.fileId)) return [track];
    const contextualQueue = contextualPlayerQueue(track, origin);
    if (origin === 'audiobook') return mode === 'single' ? [track] : contextualQueue;
    if (origin !== 'direct') {
      if (mode === 'folder') return contextualQueue.filter((item) => item.folder === track.folder);
      return contextualQueue;
    }
    if (mode === 'all') return library;
    if (mode === 'folder') return library.filter((item) => item.folder === track.folder);
    return [track];
  }

  function selectPlayingTrack(track: PlayerTrack) {
    if (playerOrigin === 'search') {
      const index = results.findIndex((item) => item.fileId === track.fileId);
      if (index >= 0) {
        resultPage = Math.floor(index / SEARCH_PAGE_SIZE);
        selectResult(results[index]);
      }
    } else if (playerOrigin === 'downloads') {
      const index = sharedFiles.findIndex((item) => item.fileId === track.fileId);
      if (index >= 0) {
        downloadLibraryPage = Math.floor(index / LOCAL_PAGE_SIZE);
        selectTagFile(sharedFiles[index]);
      }
    } else if (playerOrigin === 'shared') {
      const file = sharedFiles.find((item) => item.fileId === track.fileId);
      if (file) {
        const index = visibleSharedFiles().findIndex((item) => item.fileId === track.fileId);
        if (index >= 0) sharedLibraryPage = Math.floor(index / LOCAL_PAGE_SIZE);
        selectedShared = { ...file };
      }
    }
  }

  function formatPlayerTime(seconds: number) {
    if (!Number.isFinite(seconds) || seconds < 0) return '0:00';
    const whole = Math.floor(seconds);
    return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, '0')}`;
  }

  async function loadPlayerTrack(index: number) {
    const track = playerQueue[index];
    if (!track || playerLoading) return;
    playerLoading = true;
    playerQueueIndex = index;
    currentTrack = track;
    selectPlayingTrack(track);
    playerCurrentTime = 0;
    playerDuration = 0;
    playerEnded = false;
    try {
      applyPlaybackStatus(await invoke<PlaybackStatus>('play_audio', { fileId: track.fileId, volume: playerVolume }));
      if (!lastPlayerError) activityMessage = `Playing ${track.name}${track.folder ? ` · ${track.folder}` : ''}`;
    } catch (error) {
      playerPlaying = false;
      playerEnded = true;
      lastPlayerError = String(error);
      activityMessage = `Playback failed: ${lastPlayerError}`;
    } finally {
      playerLoading = false;
    }
  }

  async function playAudio(fileId: string, name: string, mode: PlayerMode = playerMode, origin: PlayerOrigin = 'direct') {
    if (!nativeReady || !fileId) return;
    const indexed = sharedFiles.find((file) => file.fileId === fileId);
    const track = indexed
      ? toPlayerTrack(indexed)
      : { fileId, name, folder: '', artist: '', mime: '' };
    activePlayerAudiobook = null;
    playerOrigin = origin;
    playerMode = indexed ? mode : 'single';
    playerQueue = queueForTrack(track, playerMode, playerOrigin);
    const index = Math.max(0, playerQueue.findIndex((item) => item.fileId === fileId));
    await loadPlayerTrack(index);
  }

  async function togglePlayer() {
    if (!currentTrack) {
      if (activeView === 'Downloads' && selectedTagFile) await playAudio(selectedTagFile.fileId, selectedTagFile.filename, playerMode, 'downloads');
      else if (activeView === 'Shared' && selectedShared) await playAudio(selectedShared.fileId, selectedShared.filename, playerMode, 'shared');
      else if (activeView === 'Search' && selected && isLocalFile(selected.fileId)) await playAudio(selected.fileId, selected.name, playerMode, 'search');
      else activityMessage = 'Select a local song to play';
      return;
    }
    if (playerEnded) {
      await loadPlayerTrack(playerQueueIndex);
      return;
    }
    try { applyPlaybackStatus(await invoke<PlaybackStatus>('toggle_audio')); }
    catch (error) { activityMessage = `Playback failed: ${String(error)}`; }
  }

  async function stopPlayer() {
    try { applyPlaybackStatus(await invoke<PlaybackStatus>('stop_audio')); }
    catch (error) { activityMessage = `Could not stop playback: ${String(error)}`; return; }
    // The native output stream is deliberately released on Stop. Treat the
    // track as reloadable so pressing Play opens it again from the beginning.
    playerEnded = true;
    if (currentTrack) activityMessage = `Stopped ${currentTrack.name}`;
  }

  async function nextPlayerTrack() {
    if (playerQueueIndex + 1 < playerQueue.length) await loadPlayerTrack(playerQueueIndex + 1);
    else stopPlayer();
  }

  async function previousPlayerTrack() {
    if (playerCurrentTime > 3 || playerQueueIndex <= 0) {
      try { applyPlaybackStatus(await invoke<PlaybackStatus>('seek_audio', { seconds: 0 })); }
      catch (error) { activityMessage = `Could not rewind playback: ${String(error)}`; }
      return;
    }
    await loadPlayerTrack(playerQueueIndex - 1);
  }

  async function playerTrackEnded() {
    playerPlaying = false;
    playerEnded = true;
    if (playerMode !== 'single' && playerQueueIndex + 1 < playerQueue.length) {
      await loadPlayerTrack(playerQueueIndex + 1);
    }
  }

  function changePlayerMode() {
    window.localStorage.setItem('napstr-player-mode', playerMode);
    if (!currentTrack) return;
    playerQueue = queueForTrack(currentTrack, playerMode, playerOrigin);
    playerQueueIndex = Math.max(0, playerQueue.findIndex((item) => item.fileId === currentTrack?.fileId));
  }

  async function seekPlayer(event: Event) {
    try {
      applyPlaybackStatus(await invoke<PlaybackStatus>('seek_audio', { seconds: Number((event.currentTarget as HTMLInputElement).value) }));
      playerEnded = false;
    } catch (error) { activityMessage = `Could not seek in this track: ${String(error)}`; }
  }

  function changePlayerVolume(event: Event) {
    playerVolume = Number((event.currentTarget as HTMLInputElement).value);
    if (currentTrack) invoke<PlaybackStatus>('set_audio_volume', { volume: playerVolume }).catch(() => {});
    window.localStorage.setItem('napstr-player-volume', String(playerVolume));
  }

  function applyPlaybackStatus(status: PlaybackStatus) {
    if (!currentTrack || status.fileId !== currentTrack.fileId) return;
    if (status.error) {
      playerPlaying = false;
      playerEnded = true;
      if (status.error !== lastPlayerError) activityMessage = `Playback failed: ${status.error}`;
      lastPlayerError = status.error;
      return;
    }
    lastPlayerError = '';
    playerCurrentTime = status.currentTime;
    playerDuration = status.duration;
    playerPlaying = status.playing;
  }

  function syncResultLocality() {
    if (resultUser?.npub === identityNpub) {
      results = mergeSearchResults([], sharedFiles);
    } else if (resultsAreNetwork) {
      results = results.map((result) => {
        if (result.audiobook) {
          const localBook = localAudiobooks.find((book) => book.audiobookId === result.audiobook?.audiobookId);
          const audiobook = localBook ? { ...result.audiobook, local: true, localFolder: localBook.localFolder } : result.audiobook;
          return { ...result, audiobook, remote: !audiobook.local, speed: audiobook.local ? 'Local' : 'Tor' };
        }
        const local = isLocalFile(result.fileId);
        return { ...result, remote: !local, speed: local ? 'Local' : 'Tor' };
      });
    } else if (!query.trim() || searchedQuery === 'local catalogue' || searchedQuery === 'All audio') {
      results = mergeAudiobooks(mapFiles(sharedFiles), [], query.trim());
    } else {
      results = results.filter((result) => isLocalFile(result.fileId));
    }
    resultPage = Math.min(resultPage, resultPageCount(results) - 1);
    reconcileResultSelection();
  }

  function applySnapshot(snapshot: Snapshot) {
    nativeReady = snapshot.native;
    indexedBytes = snapshot.indexedBytes;
    napstrFolder = snapshot.settings.napstrFolder;
    nostrRelays = snapshot.settings.nostrRelays;
    displayName = snapshot.settings.displayName;
    profileAbout = snapshot.settings.profileAbout;
    profilePicture = snapshot.settings.profilePicture;
    localAudiobooks = snapshot.audiobooks;
    sharedFiles = snapshot.files.map((file) => ({ ...file, name: file.filename, readableSize: readableSize(file.size), peers: 0 }));
    localFileIds = new Set(snapshot.files.map((file) => file.fileId));
    downloadLibraryPage = Math.min(downloadLibraryPage, localPageCount(snapshot.files) - 1);
    sharedLibraryPage = Math.min(sharedLibraryPage, localPageCount(visibleSharedFiles()) - 1);
    if (selectedShared) selectedShared = snapshot.files.find((file) => file.fileId === selectedShared?.fileId) ?? null;
    resultUser = null;
    searchUser = null;
    matchingUsers = [];
    results = mergeAudiobooks(mapFiles(snapshot.files), [], '');
    resultPage = 0;
    resultsAreNetwork = false;
    selectResult(results[0] ?? null);
    searchedQuery = 'local catalogue';
    transfers = mapTransfers(snapshot.transfers);
    activityMessage = snapshot.files.length ? `${snapshot.files.length} local file(s) indexed and ready` : 'Choose a Napstr folder to begin';
  }

  async function refreshSnapshot() {
    try { applySnapshot(await invoke<Snapshot>('get_snapshot')); } catch { nativeReady = false; }
  }

  function parseSemver(value: string) {
    const match = value.trim().match(/^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/);
    if (!match) return null;
    return { numbers: match.slice(1, 4).map(Number), prerelease: match[4]?.split('.') ?? [] };
  }

  function compareSemver(leftValue: string, rightValue: string) {
    const left = parseSemver(leftValue);
    const right = parseSemver(rightValue);
    if (!left || !right) return 0;
    for (let index = 0; index < 3; index += 1) {
      if (left.numbers[index] !== right.numbers[index]) return left.numbers[index] > right.numbers[index] ? 1 : -1;
    }
    if (!left.prerelease.length || !right.prerelease.length) {
      return left.prerelease.length === right.prerelease.length ? 0 : left.prerelease.length ? -1 : 1;
    }
    const length = Math.max(left.prerelease.length, right.prerelease.length);
    for (let index = 0; index < length; index += 1) {
      const leftPart = left.prerelease[index];
      const rightPart = right.prerelease[index];
      if (leftPart === undefined || rightPart === undefined) return leftPart === undefined ? -1 : 1;
      if (leftPart === rightPart) continue;
      const leftNumber = /^\d+$/.test(leftPart) ? Number(leftPart) : null;
      const rightNumber = /^\d+$/.test(rightPart) ? Number(rightPart) : null;
      if (leftNumber !== null && rightNumber !== null) return leftNumber > rightNumber ? 1 : -1;
      if (leftNumber !== null || rightNumber !== null) return leftNumber !== null ? -1 : 1;
      return leftPart > rightPart ? 1 : -1;
    }
    return 0;
  }

  function validNapstrReleaseUrl(value: unknown): value is string {
    return typeof value === 'string' && /^https:\/\/github\.com\/lnbits\/napstr\/releases\/tag\/[0-9A-Za-z._+-]{1,100}$/.test(value);
  }

  async function checkForNewRelease() {
    const cacheKey = 'napstr-latest-release';
    let cachedRelease: GitHubRelease | null = null;
    let release: GitHubRelease | null = null;
    try {
      const cached = JSON.parse(window.localStorage.getItem(cacheKey) ?? 'null') as { checkedAt?: number; release?: GitHubRelease | null } | null;
      cachedRelease = cached?.release ?? null;
    } catch { /* ignore invalid old cache data */ }
    try {
      const response = await fetch('https://api.github.com/repos/lnbits/napstr/releases/latest', {
        headers: { Accept: 'application/vnd.github+json' }
      });
      if (!response.ok) throw new Error(`GitHub returned ${response.status}`);
      release = await response.json() as GitHubRelease;
      window.localStorage.setItem(cacheKey, JSON.stringify({ checkedAt: Date.now(), release }));
    } catch { release = cachedRelease; }
    if (typeof release?.tag_name !== 'string' || !validNapstrReleaseUrl(release.html_url)) return;
    if (compareSemver(release.tag_name, appVersion) > 0) {
      newRelease = { version: release.tag_name.replace(/^v/, ''), url: release.html_url };
    }
  }

  async function openNewRelease() {
    if (!newRelease) return;
    try {
      await invoke('open_release_url', { url: newRelease.url });
    } catch (error) {
      activityMessage = `Could not open the release page: ${String(error)}`;
    }
  }

  async function openNapstrfyWebsite(event: MouseEvent) {
    event.preventDefault();
    try {
      await invoke('open_napstrfy_website');
    } catch (error) {
      activityMessage = `Could not open the Napstrfy website: ${String(error)}`;
    }
  }

  function chatNameColor(npub: string) {
    const colours = ['#0000b8', '#006400', '#8b008b', '#a00020', '#005f73', '#7a3e00', '#4b0082', '#006b3c', '#9b1c00', '#0047ab', '#7030a0', '#007070'];
    let hash = 2166136261;
    for (let index = 0; index < npub.length; index += 1) {
      hash ^= npub.charCodeAt(index);
      hash = Math.imul(hash, 16777619);
    }
    return colours[(hash >>> 0) % colours.length];
  }

  async function refreshTrollbox() {
    if (!nativeReady || !networkConnected) return;
    if (trollboxPollPending) {
      trollboxRefreshAgain = true;
      return;
    }
    trollboxPollPending = true;
    trollboxLoading = trollboxMessages.length === 0;
    const stayAtBottom = !trollboxLog || trollboxLog.scrollHeight - trollboxLog.scrollTop - trollboxLog.clientHeight < 45;
    try {
      const messages = await invoke<TrollboxMessage[]>('get_trollbox_messages');
      const changed = messages.length !== trollboxMessages.length || messages.at(-1)?.eventId !== trollboxMessages.at(-1)?.eventId;
      trollboxMessages = messages;
      trollboxError = '';
      if (changed && stayAtBottom) {
        await tick();
        trollboxLog?.scrollTo({ top: trollboxLog.scrollHeight });
      }
    } catch (error) {
      trollboxError = String(error);
    } finally {
      trollboxLoading = false;
      trollboxPollPending = false;
      if (trollboxRefreshAgain) {
        trollboxRefreshAgain = false;
        void refreshTrollbox();
      }
    }
  }

  // ---- Album covers (kind 30427) -------------------------------------------
  //
  // Reading the covers other people published needs no opt-in: it is an
  // ordinary relay query, and it happens as results appear. The two switches
  // here are the parts that leave Napstr — asking a central service, and
  // signing with the user's own key. Both are stored by the backend, and the
  // backend's worker acts on them by itself: there is no button to press for a
  // pass to happen, and leaving one on means it resumes after a restart.
  type CoverCandidate = { key: string; artist: string; album: string; trackCount: number; source: string };
  type CoverStatus = {
    lookupExternal: boolean;
    publishClaims: boolean;
    running: boolean;
    current: string;
    pending: number;
    remaining: number;
    published: number;
    resolved: number;
    alreadyCovered: number;
    noArt: number;
    failed: number;
    backedOff: number;
    stopped: boolean;
    message: string;
  };
  /** How many albums the Covers tab previews. The worker itself is unbounded. */
  const COVER_QUEUE_PREVIEW = 25;
  // Deliberately plain `let`s, like every other variable in this file. A single
  // rune anywhere in the component compiles the whole thing in runes mode, which
  // silently makes every plain `let` here non-reactive - including the
  // `desktopRuntime` gate that draws the window. The result is a blank white
  // screen with no error in the console, which is exactly what a stray `$state`
  // in this file cost once already.
  let coverQueue: CoverCandidate[] = [];
  let coverStatus: CoverStatus | null = null;
  let coverLoading = false;
  let coverError = '';
  // Bumped when the worker produces art, which is what makes the tiles ask
  // again. A long pass would otherwise leave every square blank until it ended.
  let coverRevision = 0;
  let lastCoverRevisionAt = 0;

  // ---- Results presentation ------------------------------------------------
  //
  // A view preference rather than a privacy one, so it lives beside the player
  // mode in the webview's own storage rather than in the database.
  const RESULTS_VIEW_KEY = 'napstr-results-view';
  let resultsView: 'list' | 'thumb' = 'list';

  function setResultsView(view: 'list' | 'thumb') {
    resultsView = view;
    try {
      window.localStorage.setItem(RESULTS_VIEW_KEY, view);
    } catch {
      // A preference that cannot be stored is still honoured for this session.
    }
  }

  /** Are either of the two cover switches on? Passed the status it reads for the
   *  same reason `paginatedResults` is passed the results: a helper that reads
   *  component state itself is invisible to the template's dependency tracking. */
  function coverOptIn(status: CoverStatus | null) {
    return Boolean(status?.lookupExternal || status?.publishClaims);
  }

  /** The worker changed what art exists, so nothing cached may survive it.
   *  Throttled: a pass resolving hundreds of albums must not make the window
   *  re-ask for a page of tiles that many times. */
  function refreshCoverArtwork() {
    const now = Date.now();
    if (now - lastCoverRevisionAt < 5000) return;
    lastCoverRevisionAt = now;
    clearCoverCache();
    coverRevision += 1;
  }

  async function refreshCoverStatus() {
    try {
      coverStatus = await invoke<CoverStatus>('cover_status');
    } catch {
      // The Covers tab reports its own errors; this is only for the switches.
    }
  }

  async function refreshCovers() {
    coverLoading = true;
    coverError = '';
    try {
      coverStatus = await invoke<CoverStatus>('cover_status');
      coverQueue = await invoke<CoverCandidate[]>('cover_candidates', { limit: COVER_QUEUE_PREVIEW });
    } catch (error) {
      coverError = String(error);
    } finally {
      coverLoading = false;
    }
  }

  async function setCoverPreferences(lookupExternal: boolean, publishClaims: boolean) {
    coverError = '';
    try {
      coverStatus = await invoke<CoverStatus>('set_cover_preferences', { lookupExternal, publishClaims });
    } catch (error) {
      coverError = String(error);
    }
  }

  /** Skip the wait: ask the worker to look at whatever is new right now. */
  async function lookForCoversNow() {
    coverError = '';
    try {
      await invoke('nudge_cover_worker');
    } catch (error) {
      coverError = String(error);
    }
  }

  async function stopCoverPass() {
    try {
      await invoke('cancel_cover_scan');
    } catch (error) {
      coverError = String(error);
    }
  }

  /** Report the albums the results pane is drawing.
   *
   * Silent and debounced — this is not a button. It is what makes "albums seen
   * in search results" reach the cover worker without scanning the catalogue
   * cache, which on a used install holds thousands of albums nobody looked at.
   * Pages are accumulated so flicking quickly through results still reports
   * every page that was actually drawn. */
  let noteVisibleHandle: number | null = null;
  const notedAlbums = new Map<string, CoverAlbum>();

  function noteVisibleAlbums(currentResults: Result[], page: number) {
    if (!desktopRuntime || activeView !== 'Search') return;
    for (const album of albumsFor(paginatedResults(currentResults, page))) {
      notedAlbums.set(`${album.artist}\u0000${album.album}`, album);
    }
    if (noteVisibleHandle !== null) window.clearTimeout(noteVisibleHandle);
    noteVisibleHandle = window.setTimeout(flushNotedAlbums, 800);
  }

  function flushNotedAlbums() {
    noteVisibleHandle = null;
    if (!notedAlbums.size) return;
    const albums = [...notedAlbums.values()];
    notedAlbums.clear();
    // One write per page of results, whether or not a switch is on: reporting
    // is harmless, and it means switching lookups on covers what is on screen.
    void invoke('note_visible_albums', { albums }).catch(() => {
      // A window that cannot report says nothing about the results themselves.
    });
  }

  // Re-runs whenever the page of results the pane is showing changes.
  $: noteVisibleAlbums(results, resultPage);

  /** The album whose art is being chosen by hand, or null when the tool is shut. */
  let coverPicker: { artist: string; album: string; currentArt: string } | null = null;

  /** Open the manual art tool for one album, showing the art in use now so the
   *  person can compare it against what MusicBrainz offers. */
  async function openCoverPicker(artist: string, album: string) {
    const trimmedArtist = artist.trim();
    const trimmedAlbum = album.trim();
    if (!trimmedArtist || !trimmedAlbum) return;
    coverPicker = { artist: trimmedArtist, album: trimmedAlbum, currentArt: '' };
    try {
      const cover = await coverFor(trimmedArtist, trimmedAlbum);
      if (coverPicker && coverPicker.artist === trimmedArtist && coverPicker.album === trimmedAlbum) {
        coverPicker = { ...coverPicker, currentArt: artUrl(cover, false) };
      }
    } catch {
      // The art is only here for comparison; the tool works without it.
    }
  }

  /** A pick was filed, so every cached answer for it is now wrong. */
  function coverPickApplied() {
    clearCoverCache();
    coverRevision += 1;
    void refreshCoverStatus();
  }

  /** The cover being reported, or null. Reporting is about the claim that is
   *  actually winning for this album, which is why only the key travels. */
  let coverReport: { key: string; label: string } | null = null;

  function openCoverReport(artist: string, album: string) {
    const key = coverKey(artist, album);
    if (!key) {
      activityMessage = 'This album names no artist and album to report.';
      return;
    }
    coverReport = {
      key,
      label: `${album.trim() || 'Untitled'} · ${artist.trim() || 'Unknown artist'}`
    };
  }

  function activateView(view: View) {
    activeView = view;
    if (view === 'Trollbox') void refreshTrollbox();
    if (view === 'Mobile') void openMobileConnect();
    if (view === 'Covers') void refreshCovers();
  }

  async function openMobileConnect() {
    mobileStreamOnly = false;
    await refreshMobileStatus();
    if (!mobilePairing) await createMobilePairing();
    if (!mobileStreamPairing) await createMobilePairing(true);
  }

  function navigatePairingTabs(event: KeyboardEvent) {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    mobileStreamOnly = event.key === 'Home' ? false : event.key === 'End' ? true : !mobileStreamOnly;
    document.getElementById(`pairing-tab-${mobileStreamOnly ? 'stream' : 'full'}`)?.focus();
  }

  async function refreshMobileStatus() {
    if (!nativeReady || mobileStatusPending) return;
    mobileStatusPending = true;
    try {
      mobileStatusValue = await invoke<MobileStatus>('mobile_status');
      mobileError = mobileStatusValue.error;
    } catch (error) {
      mobileError = String(error);
    } finally {
      mobileStatusPending = false;
    }
  }

  async function createMobilePairing(streamOnly = false) {
    if (!nativeReady || mobileLoading) return;
    mobileLoading = true;
    mobileError = '';
    try {
      const offer = await invoke<MobilePairingOffer>('create_mobile_pairing', { streamOnly });
      if (streamOnly) mobileStreamPairing = offer;
      else mobilePairing = offer;
      await refreshMobileStatus();
    } catch (error) {
      mobileError = String(error);
    } finally {
      mobileLoading = false;
    }
  }

  async function revokeMobileDevice(device: MobileDevice) {
    if (!window.confirm(`Remove ${device.name}? It will need a new QR code before it can connect again.`)) return;
    try {
      await invoke('revoke_mobile_device', { endpointId: device.endpointId });
      await refreshMobileStatus();
    } catch (error) {
      mobileError = String(error);
    }
  }

  function mobileLastSeen(value: string) {
    const time = Date.parse(value);
    if (!Number.isFinite(time)) return 'Never';
    const elapsed = Math.max(0, Date.now() - time);
    if (elapsed < 90_000) return 'Just now';
    if (elapsed < 3_600_000) return `${Math.floor(elapsed / 60_000)} min ago`;
    if (elapsed < 86_400_000) return `${Math.floor(elapsed / 3_600_000)} hr ago`;
    return new Date(time).toLocaleDateString();
  }

  async function sendTrollboxMessage() {
    const content = trollboxDraft.trim();
    if (!content || trollboxSending || !networkConnected) return;
    trollboxSending = true;
    trollboxError = '';
    try {
      await invoke('send_trollbox_message', { content });
      trollboxDraft = '';
      await refreshTrollbox();
    } catch (error) {
      trollboxError = String(error);
    } finally {
      trollboxSending = false;
    }
  }

  function blockTrollboxUser(message: TrollboxMessage) {
    if (!nativeReady || message.npub === identityNpub) return;
    blockConfirmation = { kind: 'user', pubkey: message.pubkey, label: message.displayName };
  }

  function selectResult(item: Result | null, forceSubscribe = false, preserveSelection = false) {
    if (!preserveSelection) {
      selectedResultIds = new Set(item ? [item.fileId] : []);
      resultSelectionAnchor = item?.fileId ?? null;
    }
    const changed = selected?.fileId !== item?.fileId;
    selected = item;
    if (changed || !preserveSelection) selectedSource = 0;
    if (!item || item.audiobook) {
      trackDiscussionFileId = '';
      trackDiscussionMessages = [];
      trackDiscussionDraft = '';
      trackDiscussionError = '';
    } else if (changed || forceSubscribe) {
      void refreshTrackDiscussion(item.fileId, true);
    }
  }

  function selectResultRange(item: Result, event: MouseEvent | KeyboardEvent) {
    const anchor = results.findIndex((result) => result.fileId === resultSelectionAnchor);
    const end = results.findIndex((result) => result.fileId === item.fileId);
    if (event.shiftKey && anchor >= 0 && end >= 0) {
      selectedResultIds = new Set(results.slice(Math.min(anchor, end), Math.max(anchor, end) + 1).map((result) => result.fileId));
      selectResult(item, false, true);
    } else {
      selectResult(item);
    }
  }

  function reconcileResultSelection(forceSubscribe = false) {
    const remaining = results.filter((item) => selectedResultIds.has(item.fileId));
    const next = remaining.find((item) => item.fileId === selected?.fileId) ?? remaining[0] ?? paginatedResults(results, resultPage)[0] ?? null;
    selectedResultIds = new Set(remaining.length ? remaining.map((item) => item.fileId) : next ? [next.fileId] : []);
    if (!results.some((item) => item.fileId === resultSelectionAnchor)) resultSelectionAnchor = next?.fileId ?? null;
    selectResult(next, forceSubscribe, true);
  }

  function selectedResults() {
    return results.filter((item) => selectedResultIds.has(item.fileId));
  }

  function canDownloadResult(item: Result) {
    if (item.audiobook) {
      return !item.audiobook.chapters.every((chapter) => isLocalFile(chapter.fileId))
        && item.audiobook.sources.length > 0
        && !audiobookDownloads.some((download) => download.audiobookId === item.audiobook?.audiobookId);
    }
    return !isLocalFile(item.fileId) && Boolean(item.sourceDetails?.length)
      && !startingDownloads.has(item.fileId)
      && !transfers.some((transfer) => transfer.fileId === item.fileId && isActiveTransfer(transfer));
  }

  async function downloadSelectedResults() {
    if (!nativeReady || downloadingSelection || clearingTransfers) return;
    const generation = downloadGeneration;
    const targets = selectedResults();
    downloadingSelection = true;
    let requested = 0;
    let skipped = 0;
    let failed = 0;
    try {
      for (const target of targets) {
        if (generation !== downloadGeneration) break;
        if (!canDownloadResult(target)) { skipped += 1; continue; }
        try {
          if (await startDownload(target)) requested += 1;
          else failed += 1;
        } catch { failed += 1; }
      }
      if (generation === downloadGeneration) activityMessage = `Requested ${requested} download${requested === 1 ? '' : 's'}`
        + (skipped ? ` · ${skipped} already local, queued, or unavailable` : '')
        + (failed ? ` · ${failed} failed` : '');
    } finally {
      downloadingSelection = false;
    }
  }

  async function refreshTrackDiscussion(fileId = selected?.fileId ?? '', subscribe = false) {
    if (!fileId || !nativeReady || !networkConnected) return;
    if (trackDiscussionFileId !== fileId) {
      trackDiscussionFileId = fileId;
      trackDiscussionMessages = [];
      trackDiscussionDraft = '';
      trackDiscussionError = '';
      subscribe = true;
    }
    if (trackDiscussionPollPending && !subscribe) {
      trackDiscussionRefreshAgain = true;
      return;
    }
    trackDiscussionPollPending = true;
    trackDiscussionLoading = trackDiscussionMessages.length === 0;
    const stayAtBottom = !trackDiscussionLog || trackDiscussionLog.scrollHeight - trackDiscussionLog.scrollTop - trackDiscussionLog.clientHeight < 35;
    try {
      const messages = await invoke<TrollboxMessage[]>('get_track_discussion_messages', { fileId, subscribe });
      if (selected?.fileId !== fileId || trackDiscussionFileId !== fileId) return;
      const changed = messages.length !== trackDiscussionMessages.length || messages.at(-1)?.eventId !== trackDiscussionMessages.at(-1)?.eventId;
      trackDiscussionMessages = messages;
      trackDiscussionError = '';
      if (changed && stayAtBottom) {
        await tick();
        trackDiscussionLog?.scrollTo({ top: trackDiscussionLog.scrollHeight });
      }
    } catch (error) {
      if (selected?.fileId === fileId) trackDiscussionError = String(error);
    } finally {
      if (selected?.fileId === fileId) {
        trackDiscussionLoading = false;
        trackDiscussionPollPending = false;
        if (trackDiscussionRefreshAgain) {
          trackDiscussionRefreshAgain = false;
          void refreshTrackDiscussion(fileId);
        }
      }
    }
  }

  async function sendTrackDiscussionMessage() {
    const fileId = selected?.fileId;
    const content = trackDiscussionDraft.trim();
    if (!fileId || !content || trackDiscussionSending || !networkConnected) return;
    trackDiscussionSending = true;
    trackDiscussionError = '';
    try {
      await invoke('send_track_discussion_message', { fileId, content });
      if (selected?.fileId !== fileId) return;
      trackDiscussionDraft = '';
      await refreshTrackDiscussion(fileId);
    } catch (error) {
      if (selected?.fileId === fileId) trackDiscussionError = String(error);
    } finally {
      trackDiscussionSending = false;
    }
  }

  async function refreshLocalLibrary() {
    try {
      const snapshot = await invoke<Snapshot>('get_snapshot');
      indexedBytes = snapshot.indexedBytes;
      localAudiobooks = snapshot.audiobooks;
      const nextFiles = snapshot.files.map((file) => ({ ...file, name: file.filename, readableSize: readableSize(file.size), peers: 0 }));
      const removedCurrentTrack = currentTrack && !nextFiles.some((file) => file.fileId === currentTrack?.fileId);
      sharedFiles = nextFiles;
      localFileIds = new Set(nextFiles.map((file) => file.fileId));
      downloadLibraryPage = Math.min(downloadLibraryPage, localPageCount(nextFiles) - 1);
      sharedLibraryPage = Math.min(sharedLibraryPage, localPageCount(visibleSharedFiles()) - 1);
      if (selectedShared) selectedShared = nextFiles.find((file) => file.fileId === selectedShared?.fileId) ?? null;
      if (selectedTagFile && !nextFiles.some((file) => file.fileId === selectedTagFile?.fileId)) {
        selectedTagFile = null;
        tagDraft = '';
      }
      if (removedCurrentTrack) {
        invoke<PlaybackStatus>('stop_audio').catch(() => {});
        currentTrack = null;
        playerQueue = [];
        playerQueueIndex = -1;
        playerPlaying = false;
        playerLoading = false;
        playerCurrentTime = 0;
        playerDuration = 0;
        activityMessage = 'Stopped playback because the file was removed from the Napstr folder';
      } else if (currentTrack) {
        playerQueue = queueForTrack(currentTrack, playerMode);
        playerQueueIndex = playerQueue.findIndex((item) => item.fileId === currentTrack?.fileId);
      }
      syncResultLocality();
    } catch { /* the next folder-watch or transfer poll will retry */ }
  }

  function mergeIndexBatch(batch: IndexBatch) {
    const merged = new Map(sharedFiles.map((file) => [file.fileId, file]));
    for (const file of batch.files) {
      merged.set(file.fileId, {
        ...file,
        name: file.filename,
        readableSize: readableSize(file.size),
        peers: merged.get(file.fileId)?.peers ?? 0
      });
    }
    sharedFiles = [...merged.values()].sort((left, right) => left.filename.localeCompare(right.filename));
    localFileIds = new Set(sharedFiles.map((file) => file.fileId));
    indexedBytes = sharedFiles.reduce((total, file) => total + file.size, 0);
    if (selectedShared) selectedShared = merged.get(selectedShared.fileId) ?? selectedShared;
    if (selectedTagFile) selectedTagFile = merged.get(selectedTagFile.fileId) ?? selectedTagFile;
    if (currentTrack) {
      playerQueue = queueForTrack(currentTrack, playerMode);
      playerQueueIndex = playerQueue.findIndex((item) => item.fileId === currentTrack?.fileId);
    }
    syncResultLocality();
  }

  async function connectNetwork() {
    if (!nativeReady || networkConnectPending) return;
    networkConnectPending = true;
    activityMessage = 'Connecting to Nostr relays and opening encrypted inbox…';
    try {
      const status = await invoke<NetworkStatus>('start_network');
      applyNetworkStatus(status);
      activityMessage = `Nostr connected · loading the most available audio from ${status.relayCount} relay(s)…`;
      await search();
      if (status.torError) activityMessage = `Tor failed: ${status.torError} · click the connection panel to retry`;
    } catch (error) {
      networkConnected = false;
      networkError = String(error);
      activityMessage = `Network unavailable: ${String(error)}`;
    } finally {
      networkConnectPending = false;
    }
  }

  function applyNetworkStatus(status: NetworkStatus) {
    networkConnected = status.connected;
    torRunning = status.torRunning;
    torStarting = status.torStarting;
    torProgress = status.torProgress;
    torError = status.torError;
    identityNpub = status.npub;
    networkError = status.error;
  }

  async function recoverAfterSleep() {
    if (!nativeReady) return;
    networkConnected = false;
    torRunning = false;
    torStarting = true;
    torProgress = 0;
    playerPlaying = false;
    playerLoading = false;
    playerCurrentTime = 0;
    playerEnded = currentTrack !== null;
    lastPlayerError = '';
    activityMessage = 'Computer resumed · reconnecting Nostr, Tor, and audio…';
    try {
      await invoke('recover_after_sleep');
    } catch (error) {
      activityMessage = `Resume recovery failed: ${String(error)} · click the connection panel to retry`;
    }
  }

  function torStatusLabel() {
    if (torRunning) return 'Tor connected';
    if (torError) return 'Tor failed';
    if (torStarting && torProgress > 0) return `Tor connecting ${torProgress}%`;
    return nativeReady ? 'Tor connecting' : 'Tor unavailable';
  }

  async function browseUser(user: CatalogueUser) {
    activeView = 'Search';
    sourceProfile = null;
    query = user.displayName;
    searchUser = user;
    format = 'Audio only';
    minimumSources = 1;
    maximumSize = '';
    await search(true);
  }

  function ownCatalogueUser(): CatalogueUser {
    return { pubkey: identityNpub, npub: identityNpub, displayName };
  }

  function knownUsersNamed(value: string): CatalogueUser[] {
    const name = value.trim().toLocaleLowerCase();
    const users: CatalogueUser[] = [
      ...results.flatMap((item) => item.sourceDetails ?? []),
      ...trollboxMessages, ...trackDiscussionMessages, ownCatalogueUser()
    ];
    return users.filter((user) => user.npub && user.displayName.trim().toLocaleLowerCase() === name);
  }

  async function search(replacePending = false) {
    if (searchAction && !replacePending) return;
    const generation = ++browseGeneration;
    browseCursor = null;
    browseLoading = false;
    loadedNetworkMatches = [];
    loadedNetworkAudiobooks = [];
    browseTotalAvailable = 0;
    searchAction = 'search';
    resultUser = null;
    matchingUsers = [];
    try {
      const trimmedQuery = query.trim();
      let user = searchUser && [searchUser.displayName, searchUser.npub, searchUser.pubkey].some((name) => name.trim() === trimmedQuery) ? searchUser : null;
      searchUser = user;
      if (!user && trimmedQuery) {
        const known = knownUsersNamed(trimmedQuery);
        try {
          const users = networkConnected ? await invoke<CatalogueUser[]>('resolve_catalogue_user', { query: trimmedQuery }) : [];
          if (generation !== browseGeneration) return;
          matchingUsers = [...new Map([...known, ...users].map((user) => [user.npub, user])).values()];
        } catch { if (generation === browseGeneration) matchingUsers = [...new Map(known.map((user) => [user.npub, user])).values()]; }
        if (generation !== browseGeneration) return;
        if (matchingUsers.length === 1) user = matchingUsers[0];
      }
      if (user) {
        searchUser = user;
        resultUser = user;
        matchingUsers = [];
        searchedQuery = user.displayName;
        format = 'Audio only';
        results = [];
        resultsAreNetwork = true;
        resultPage = 0;
        selectResult(null);
        try {
          if (user.npub === identityNpub) {
            results = mergeSearchResults([], sharedFiles);
          } else {
            const page = await invoke<CatalogueBrowsePage>('network_browse_user', { pubkey: user.pubkey, cursor: null });
            if (generation !== browseGeneration) return;
            loadedNetworkMatches = page.results;
            browseCursor = page.cursor;
            browseTotalAvailable = page.totalAvailable;
            results = mergeSearchResults(loadedNetworkMatches, []);
          }
          activityMessage = `${results.length} tracks loaded from ${user.displayName}`;
          selectResult(results[0] ?? null, true);
          if (browseCursor) void loadNextBrowsePage();
        } catch (error) {
          if (generation === browseGeneration) activityMessage = `Could not browse ${user.displayName}: ${String(error)}`;
        }
        return;
      }
      searchedQuery = trimmedQuery || (format === 'Audiobooks' ? 'All audiobooks' : 'All audio');
      if (networkConnected) {
        if (format === 'Audiobooks') {
          try {
            loadedNetworkAudiobooks = await invoke<Audiobook[]>('network_search_audiobooks', { query: trimmedQuery });
            if (generation !== browseGeneration) return;
            results = mergeAudiobooks([], loadedNetworkAudiobooks, trimmedQuery);
            resultsAreNetwork = true;
            activityMessage = `${results.length} audiobook collection(s) found`;
          } catch (error) {
            if (generation !== browseGeneration) return;
            loadedNetworkAudiobooks = [];
            results = mergeAudiobooks([], [], trimmedQuery);
            resultsAreNetwork = false;
            activityMessage = `Global audiobook search failed: ${String(error)} · showing ${results.length} local collection(s)`;
          }
        } else {
        const includeAudiobooks = /^audiobooks?$/i.test(trimmedQuery);
        const audiobookRequest = includeAudiobooks
          ? invoke<Audiobook[]>('network_search_audiobooks', { query: trimmedQuery })
              .then((books) => books, () => [] as Audiobook[])
          : Promise.resolve([] as Audiobook[]);
        const [networkOutcome, localOutcome] = await Promise.allSettled([
          trimmedQuery
            ? invoke<NetworkResult[]>('network_search', { query: trimmedQuery })
            : invoke<CatalogueBrowsePage>('network_browse', { cursor: null, limit: 500, cacheLimit: 10000 }),
          invoke<NativeFile[]>('search_catalog', { query: trimmedQuery })
        ]);
        if (generation !== browseGeneration) return;
        const networkMatches = networkOutcome.status === 'fulfilled'
          ? trimmedQuery
            ? networkOutcome.value as NetworkResult[]
            : (networkOutcome.value as CatalogueBrowsePage).results
          : [];
        const localMatches = localOutcome.status === 'fulfilled' ? localOutcome.value : [];
        loadedNetworkMatches = networkMatches;
        loadedNetworkAudiobooks = [];
        browseCursor = format !== 'Audiobooks' && networkOutcome.status === 'fulfilled' && !trimmedQuery
          ? (networkOutcome.value as CatalogueBrowsePage).cursor
          : null;
        browseTotalAvailable = format !== 'Audiobooks' && networkOutcome.status === 'fulfilled' && !trimmedQuery
          ? (networkOutcome.value as CatalogueBrowsePage).totalAvailable
          : 0;
        results = mergeAudiobooks(mergeSearchResults(networkMatches, localMatches), loadedNetworkAudiobooks, trimmedQuery);
        resultsAreNetwork = networkOutcome.status === 'fulfilled';
        if (networkOutcome.status === 'rejected') {
          activityMessage = localOutcome.status === 'fulfilled'
            ? `Global search failed: ${String(networkOutcome.reason)} · showing ${results.length} local match(es)`
            : `Search failed: ${String(networkOutcome.reason)}`;
        } else {
          activityMessage = format === 'Audiobooks'
            ? `${results.length} audiobook collection(s) found`
            : !trimmedQuery
            ? `${results.length} loaded of ${availableResultTotal(results, browseTotalAvailable)} currently available file ID(s), ranked by active seeders`
            : `${results.length} available file ID(s), ranked by active seeders`;
        }
        // Audiobook manifests are additive. Let ordinary track results render
        // as soon as they are ready instead of making every search wait for a
        // second relay query and manifest validation pass.
        if (includeAudiobooks) void audiobookRequest.then((books) => {
          if (generation !== browseGeneration) return;
          loadedNetworkAudiobooks = books;
          results = mergeAudiobooks(
            mergeSearchResults(loadedNetworkMatches, localMatches),
            loadedNetworkAudiobooks,
            trimmedQuery
          );
          resultPage = 0;
          reconcileResultSelection(true);
          activityMessage = format === 'Audiobooks'
            ? `${results.length} audiobook collection(s) found`
            : !trimmedQuery
            ? `${results.length} loaded of ${availableResultTotal(results, browseTotalAvailable)} currently available file ID(s), ranked by active seeders`
            : `${results.length} available file ID(s), ranked by active seeders`;
        });
        }
      } else if (nativeReady) {
        try {
          const matches = await invoke<NativeFile[]>('search_catalog', { query: query.trim() });
          if (generation !== browseGeneration) return;
          results = mergeAudiobooks(mapFiles(matches.filter((item) => minimumSources <= 1 && item.size <= maximumBytes() && matchesType(item.mime, item.format))), [], query.trim());
          resultsAreNetwork = false;
          activityMessage = `${results.length} local match(es) found`;
        } catch (error) { if (generation === browseGeneration) activityMessage = `Search failed: ${String(error)}`; }
      }
      resultPage = 0;
      selectResult(results[0] ?? null, true);
      if (generation === browseGeneration && !query.trim() && browseCursor) {
        void loadNextBrowsePage();
      }
    } finally {
      if (generation === browseGeneration) searchAction = null;
    }
  }

  async function loadNextBrowsePage() {
    const cursor = browseCursor;
    const user = resultUser;
    if (!cursor || browseLoading || (!user && query.trim())) return;
    const generation = browseGeneration;
    browseLoading = true;
    activityMessage = `${results.length} available file ID(s) loaded · fetching the next relay page…`;
    try {
      const page = user
        ? await invoke<CatalogueBrowsePage>('network_browse_user', { pubkey: user.pubkey, cursor })
        : await invoke<CatalogueBrowsePage>('network_browse', { cursor, limit: 500, cacheLimit: 10000 });
      if (generation !== browseGeneration || (!user && query.trim())) return;
      loadedNetworkMatches = mergeNetworkPages(loadedNetworkMatches, page.results);
      browseCursor = page.cursor;
      browseTotalAvailable = page.totalAvailable;
      results = user ? mergeSearchResults(loadedNetworkMatches, []) : mergeAudiobooks(mergeSearchResults(loadedNetworkMatches, sharedFiles), loadedNetworkAudiobooks, '');
      resultsAreNetwork = true;
      reconcileResultSelection();
      activityMessage = user
        ? `${results.length} loaded of ${availableResultTotal(results, browseTotalAvailable)} tracks shared by ${user.displayName}`
        : `${results.length} loaded of ${availableResultTotal(results, browseTotalAvailable)} currently available file ID(s), ranked by active seeders`;
    } catch (error) {
      if (generation === browseGeneration) activityMessage = `Could not load the next catalogue page: ${String(error)}`;
    } finally {
      if (generation === browseGeneration) browseLoading = false;
    }
  }

  async function surpriseMe() {
    if (searchAction) return;
    if (!networkConnected) {
      activityMessage = 'Connect to Nostr before asking for a surprise';
      return;
    }
    const generation = ++browseGeneration;
    browseCursor = null;
    browseLoading = false;
    loadedNetworkMatches = [];
    loadedNetworkAudiobooks = [];
    browseTotalAvailable = 0;
    searchAction = 'surprise';
    searchUser = null;
    resultUser = null;
    matchingUsers = [];
    searchedQuery = 'Surprise me';
    activityMessage = 'Finding 50 random downloadable tracks…';
    try {
      const page = await invoke<CatalogueBrowsePage>('network_browse', { cursor: null, limit: 50, cacheLimit: 50 });
      if (generation !== browseGeneration) return;
      let matches = page.results;
      if (matches.length < 50 && page.cursor) {
        const missing = await invoke<CatalogueBrowsePage>('network_browse', { cursor: page.cursor, limit: 50, cacheLimit: 50 });
        if (generation !== browseGeneration) return;
        matches = mergeNetworkPages(matches, missing.results);
      }
      const downloadable = eligibleNetworkMatches(matches)
        .filter((item) => !isLocalFile(item.fileId) && item.sources.length > 0);
      results = mapNetworkFiles(shuffled(downloadable).slice(0, 50));
      resultsAreNetwork = true;
      resultPage = 0;
      selectResult(results[0] ?? null, true);
      activityMessage = results.length
        ? `${results.length} random downloadable track${results.length === 1 ? '' : 's'} found`
        : 'No downloadable tracks are currently available';
    } catch (error) {
      if (generation === browseGeneration) activityMessage = `Surprise search failed: ${String(error)}`;
    } finally {
      if (generation === browseGeneration) searchAction = null;
    }
  }

  async function requestNetworkDownload(args: Record<string, unknown>) {
    if (clearingTransfers || cancellingFiles.has(String(args.fileId))) throw new Error('Download is being cancelled');
    const request = invoke('request_network_download', args);
    pendingDownloadRequests.set(request, String(args.fileId));
    try { return await request; }
    finally { pendingDownloadRequests.delete(request); }
  }

  async function startDownload(target: Result | null = selected): Promise<boolean> {
    if (!target || clearingTransfers || cancellingFiles.has(target.fileId)) return false;
    const generation = downloadGeneration;
    if (target.audiobook) {
      await startAudiobookDownload(target.audiobook);
      return true;
    }
    if (nativeReady && isLocalFile(target.fileId)) {
      await playAudio(target.fileId, target.name, playerMode, 'search');
      return false;
    }
    const activeTransfer = transfers.find((item) => item.fileId === target.fileId && isActiveTransfer(item));
    if (activeTransfer || startingDownloads.has(target.fileId)) {
      activityMessage = `${target.name} is already downloading`;
      return false;
    }
    if (nativeReady) {
      const sources = target.sourceDetails ?? [];
      if (!sources.length) { activityMessage = 'No seeder is available for this file'; return false; }
      startingDownloads = new Set(startingDownloads).add(target.fileId);
      const attempt = { cancelled: false };
      downloadAttempts.set(target.fileId, attempt);
      transfers = [{
        id: Date.now(), fileId: target.fileId, name: target.name, size: target.size,
        speed: 'Contacting seeders…', progress: 0, status: 'Sending encrypted NIP-17 request', destination: ''
      }, ...transfers];
      const candidateCount = Math.min(sources.length, 3);
      activityMessage = `Racing ${candidateCount} seeder${candidateCount === 1 ? '' : 's'} for the fastest Tor connection…`;
      try {
        await requestNetworkDownload({ fileId: target.fileId, sourcePubkeys: sources.map((source) => source.pubkey) });
        if (generation !== downloadGeneration || attempt.cancelled) return false;
        const updated = await invoke<NativeTransfer[]>('get_transfers');
        if (generation !== downloadGeneration || attempt.cancelled) return false;
        transfers = mapTransfers(updated);
        activityMessage = 'Seeder race started · the fastest responsive source will stream the file';
        return true;
      } catch (error) {
        if (generation !== downloadGeneration || attempt.cancelled) return false;
        try {
          const updated = await invoke<NativeTransfer[]>('get_transfers');
          if (generation !== downloadGeneration || attempt.cancelled) return false;
          transfers = mapTransfers(updated);
        } catch {
          if (generation !== downloadGeneration || attempt.cancelled) return false;
          transfers = transfers.filter((item) => item.fileId !== target.fileId);
        }
        activityMessage = `Request failed: ${String(error)}`;
      } finally {
        downloadAttempts.delete(target.fileId);
        const nextStarting = new Set(startingDownloads);
        nextStarting.delete(target.fileId);
        startingDownloads = nextStarting;
      }
    }
    return false;
  }

  async function playSelectedAudio() {
    if (!selected) return;
    if (selected.audiobook) {
      await playAudiobook(selected.audiobook);
      return;
    }
    if (!isLocalFile(selected.fileId)) return;
    await playAudio(selected.fileId, selected.name, playerMode, 'search');
  }

  function currentFolderAudiobook() {
    if (libraryFolderView === '*') return null;
    return localAudiobooks.find((book) => book.localFolder === libraryFolderView) ?? null;
  }

  function openAudiobookEditor() {
    if (libraryFolderView === '*' || audiobookFolderFiles().length < 1) return;
    const existing = currentFolderAudiobook();
    const files = audiobookFolderFiles();
    const folderTitle = folderName(libraryFolderView).split('/').at(-1) ?? 'Audiobook';
    audiobookTitle = existing?.title || files.find((file) => file.album)?.album || folderTitle.replace(/[_-]+/g, ' ');
    audiobookAuthor = existing?.author || files.find((file) => file.artist)?.artist || '';
    audiobookNarrator = existing?.narrator || '';
    audiobookEditorOpen = true;
  }

  async function saveAudiobookGroup() {
    if (audiobookSaving || libraryFolderView === '*') return;
    audiobookSaving = true;
    try {
      localAudiobooks = await invoke<Audiobook[]>('save_audiobook', {
        folder: libraryFolderView,
        title: audiobookTitle,
        author: audiobookAuthor,
        narrator: audiobookNarrator
      });
      audiobookEditorOpen = false;
      activityMessage = `${audiobookTitle.trim()} grouped and queued for Nostr publication`;
      syncResultLocality();
    } catch (error) {
      activityMessage = `Could not group audiobook: ${String(error)}`;
    } finally {
      audiobookSaving = false;
    }
  }

  async function ungroupAudiobook() {
    const existing = currentFolderAudiobook();
    if (!existing || audiobookSaving) return;
    audiobookSaving = true;
    try {
      localAudiobooks = await invoke<Audiobook[]>('remove_audiobook', { folder: existing.localFolder });
      audiobookEditorOpen = false;
      activityMessage = `${existing.title} is now published as individual tracks only`;
      syncResultLocality();
    } catch (error) {
      activityMessage = `Could not remove audiobook grouping: ${String(error)}`;
    } finally {
      audiobookSaving = false;
    }
  }

  async function playAudiobook(book: Audiobook) {
    const firstReadyChapter = book.chapters.find((chapter) => isLocalFile(chapter.fileId));
    if (!firstReadyChapter) {
      activityMessage = 'Download at least the first chapter before playing this audiobook';
      return;
    }
    await playAudiobookChapter(book, firstReadyChapter.fileId);
  }

  async function playAudiobookChapter(book: Audiobook, fileId: string) {
    const queue = book.chapters.flatMap((chapter) => {
      const file = sharedFiles.find((candidate) => candidate.fileId === chapter.fileId);
      return file ? [toPlayerTrack(file)] : [];
    });
    const index = queue.findIndex((chapter) => chapter.fileId === fileId);
    if (index < 0) {
      activityMessage = 'That chapter has not finished downloading yet';
      return;
    }
    activePlayerAudiobook = book;
    playerOrigin = 'audiobook';
    playerMode = 'all';
    playerQueue = queue;
    await loadPlayerTrack(index);
  }

  function audiobookChapterStatus(book: Audiobook, chapter: AudiobookChapter) {
    if (isLocalFile(chapter.fileId)) return 'Ready';
    const download = audiobookDownloads.find((item) => item.audiobookId === book.audiobookId);
    return download?.activeFileId === chapter.fileId ? 'Downloading' : 'Waiting';
  }

  async function requestNextAudiobookChapter(audiobookId: string) {
    if (clearingTransfers) return;
    const queue = audiobookDownloads.find((item) => item.audiobookId === audiobookId);
    if (!queue || queue.activeFileId) return;
    while (queue.nextIndex < queue.chapters.length && isLocalFile(queue.chapters[queue.nextIndex].fileId)) queue.nextIndex += 1;
    if (queue.nextIndex >= queue.chapters.length) {
      const missing = queue.chapters.filter((chapter) => !isLocalFile(chapter.fileId)).length;
      if (missing) {
        audiobookDownloads = audiobookDownloads.filter((item) => item.audiobookId !== audiobookId);
        activityMessage = `${queue.title} finished with ${missing} missing chapter${missing === 1 ? '' : 's'} · select the book to retry`;
        return;
      }
      if (queue.destinationFolder) {
        try {
          localAudiobooks = await invoke<Audiobook[]>('save_audiobook', {
            folder: `Audiobooks/${queue.destinationFolder}`,
            title: queue.title,
            author: queue.author,
            narrator: queue.narrator
          });
          syncResultLocality();
        } catch { /* downloaded chapters remain valid and can be grouped manually */ }
      }
      audiobookDownloads = audiobookDownloads.filter((item) => item.audiobookId !== audiobookId);
      activityMessage = `${queue.title} downloaded and ready to play`;
      return;
    }
    const chapter = queue.chapters[queue.nextIndex];
    queue.activeFileId = chapter.fileId;
    audiobookDownloads = [...audiobookDownloads];
    startingDownloads = new Set(startingDownloads).add(chapter.fileId);
    try {
      await requestNetworkDownload({
        fileId: chapter.fileId,
        sourcePubkeys: queue.sources.map((source) => source.pubkey),
        destinationFolder: queue.destinationFolder
      });
      if (!audiobookDownloads.includes(queue)) return;
      transfers = mapTransfers(await invoke<NativeTransfer[]>('get_transfers'));
      activityMessage = `Downloading ${queue.title} · chapter ${queue.nextIndex + 1} of ${queue.chapters.length}`;
    } catch (error) {
      if (!audiobookDownloads.includes(queue)) return;
      queue.failed += 1;
      queue.nextIndex += 1;
      queue.activeFileId = '';
      audiobookDownloads = [...audiobookDownloads];
      activityMessage = `Chapter ${queue.nextIndex} could not start: ${String(error)} · continuing with the book`;
      void requestNextAudiobookChapter(audiobookId);
    } finally {
      const nextStarting = new Set(startingDownloads);
      nextStarting.delete(chapter.fileId);
      startingDownloads = nextStarting;
    }
  }

  async function advanceAudiobookDownloads() {
    for (const queue of [...audiobookDownloads]) {
      if (queue.activeFileId && isLocalFile(queue.activeFileId)) {
        queue.nextIndex += 1;
        queue.activeFileId = '';
      } else if (queue.activeFileId) {
        const transfer = transfers.find((item) => item.fileId === queue.activeFileId);
        if (transfer && isFinishedTransfer(transfer) && !isCompleteTransfer(transfer)) {
          queue.failed += 1;
          queue.nextIndex += 1;
          queue.activeFileId = '';
        }
      }
      audiobookDownloads = [...audiobookDownloads];
      if (!queue.activeFileId) await requestNextAudiobookChapter(queue.audiobookId);
    }
  }

  async function startAudiobookDownload(book: Audiobook) {
    if (clearingTransfers) return;
    if (book.chapters.every((chapter) => isLocalFile(chapter.fileId))) {
      await playAudiobook(book);
      return;
    }
    if (!book.sources.length) {
      activityMessage = 'No complete audiobook seeder is currently available';
      return;
    }
    if (audiobookDownloads.some((item) => item.audiobookId === book.audiobookId)) {
      activityMessage = `${book.title} is already in the download queue`;
      return;
    }
    audiobookDownloads = [...audiobookDownloads, {
      audiobookId: book.audiobookId,
      title: book.title,
      author: book.author,
      narrator: book.narrator,
      destinationFolder: `${book.title.replace(/[\x00-\x1f/\\:*?"<>|]/g, '_').replace(/^[.\s]+|[.\s]+$/g, '').slice(0, 86) || 'Audiobook'} [${book.audiobookId.slice(0, 8)}]`,
      chapters: book.chapters,
      sources: book.sources,
      nextIndex: 0,
      activeFileId: '',
      failed: 0
    }];
    await requestNextAudiobookChapter(book.audiobookId);
  }

  function selectedAudiobookComplete() {
    return selected?.audiobook?.chapters.every((chapter) => isLocalFile(chapter.fileId)) ?? false;
  }

  function selectedAudiobookDownloading() {
    const audiobookId = selected?.audiobook?.audiobookId;
    return Boolean(audiobookId && audiobookDownloads.some((item) => item.audiobookId === audiobookId));
  }

  async function playSelectedAudiobook() {
    const book = selected?.audiobook;
    if (book) await playAudiobook(book);
  }

  async function downloadSelectedAudiobook() {
    const book = selected?.audiobook;
    if (book) await startAudiobookDownload(book);
  }

  async function playSelectedSharedAudio() {
    if (!selectedShared) return;
    await playAudio(selectedShared.fileId, selectedShared.filename, playerMode, 'shared');
  }

  async function playSelectedFolder() {
    if (!selectedShared) return;
    await playAudio(selectedShared.fileId, selectedShared.filename, 'folder', 'shared');
  }

  async function playAllSongs() {
    const first = selectedShared ?? visibleSharedFiles()[0] ?? sharedFiles[0];
    if (!first) return;
    await playAudio(first.fileId, first.filename, 'all', 'shared');
  }

  async function activateSelected() {
    if (nativeReady && selected?.audiobook?.local) await playSelectedAudio();
    else if (nativeReady && selected && isLocalFile(selected.fileId)) await playSelectedAudio();
    else await startDownload();
  }

  function blockSelectedFile() {
    if (!nativeReady || !selected?.remote) return;
    blockConfirmation = { kind: 'file', fileId: selected.fileId, label: selected.name };
  }

  function blockSelectedUser() {
    const source = selected?.sourceDetails?.[selectedSource];
    if (!nativeReady || !source) return;
    blockConfirmation = { kind: 'user', pubkey: source.pubkey, label: source.displayName };
  }

  async function confirmBlock() {
    if (!blockConfirmation || blockInProgress) return;
    const target = blockConfirmation;
    blockInProgress = true;
    try {
      if (target.kind === 'file') {
        await invoke('block_file', { fileId: target.fileId });
        activityMessage = 'File hash blocked locally';
      } else {
        await invoke('block_user', { pubkey: target.pubkey });
        activityMessage = 'Nostr publisher blocked locally';
      }
      blockConfirmation = null;
      if (activeView === 'Trollbox') {
        trollboxMessages = trollboxMessages.filter((message) => message.pubkey !== ('pubkey' in target ? target.pubkey : ''));
        await refreshTrollbox();
      } else {
        await search();
      }
    } catch (error) {
      activityMessage = `Could not block ${target.kind}: ${String(error)}`;
    } finally {
      blockInProgress = false;
    }
  }

  async function removeTransfer(id: number) {
    if (clearingTransfers || removingTransfers.has(id)) return;
    const target = transfers.find((transfer) => transfer.id === id);
    const fileId = target && isActiveTransfer(target) ? target.fileId : undefined;
    audiobookDownloads = audiobookDownloads.filter((book) => book.activeFileId !== fileId);
    removingTransfers = new Set(removingTransfers).add(id);
    if (fileId) {
      cancellingFiles.add(fileId);
      const attempt = downloadAttempts.get(fileId);
      if (attempt) attempt.cancelled = true;
    }
    const pending = [...pendingDownloadRequests].filter(([, file]) => file === fileId).map(([request]) => request);
    activityMessage = 'Stopping download and cleaning partial files…';
    try {
      if (nativeReady) {
        // Optimistic UI IDs are not database IDs. Find the actual row, and
        // drain any request still entering the backend before the final sweep.
        const removeRows = async () => {
          const rows = await invoke<NativeTransfer[]>('get_transfers');
          for (const row of rows) {
            if (row.id === id || (pending.length && fileId && row.id < 0 && row.fileId === fileId && isActiveTransfer(row))) {
              await invoke('remove_transfer', { id: row.id });
            }
          }
        };
        await removeRows();
        await Promise.allSettled(pending);
        if (pending.length) await removeRows();
      }
      transfers = transfers.filter((transfer) => transfer.id !== id && !(pending.length && fileId && transfer.fileId === fileId && isActiveTransfer(transfer)));
      activityMessage = 'Transfer removed; completed audio kept';
    } catch (error) {
      activityMessage = `Could not remove transfer: ${String(error)}`;
    } finally {
      if (fileId) cancellingFiles.delete(fileId);
      removingTransfers = new Set([...removingTransfers].filter((value) => value !== id));
    }
  }

  async function clearAllTransfers() {
    if (clearingTransfers || removingTransfers.size) return;
    clearingTransfers = true;
    downloadGeneration += 1;
    audiobookDownloads = [];
    const pending = [...pendingDownloadRequests.keys()];
    activityMessage = 'Stopping downloads and cleaning partial files…';
    try {
      if (nativeReady) {
        await invoke('clear_all_transfers');
        // A request already sent by the UI may still be entering the backend.
        await Promise.allSettled(pending);
        if (pending.length) await invoke('clear_all_transfers');
        transfers = mapTransfers(await invoke<NativeTransfer[]>('get_transfers'));
      } else transfers = [];
      activityMessage = 'All transfers cleared; partial downloads removed and completed audio kept';
    } catch (error) {
      activityMessage = `Could not clear all transfers: ${String(error)}`;
    } finally {
      clearingTransfers = false;
    }
  }

  async function clearFinishedTransfers() {
    if (clearingTransfers || removingTransfers.size) return;
    const finished = transfers.filter(isFinishedTransfer);
    if (!finished.length) return;
    const removed = new Set<number>();
    for (const transfer of finished) {
      try {
        if (nativeReady) await invoke('remove_transfer', { id: transfer.id });
        removed.add(transfer.id);
      } catch (error) {
        activityMessage = `Could not clear every finished transfer: ${String(error)}`;
        break;
      }
    }
    transfers = transfers.filter((transfer) => !removed.has(transfer.id));
    if (removed.size === finished.length) activityMessage = `Cleared ${removed.size} finished transfer${removed.size === 1 ? '' : 's'}`;
    if (nativeReady) await refreshLocalLibrary();
  }

  async function togglePause() {
    paused = !paused;
    if (nativeReady) {
      try { await invoke('set_downloads_paused', { paused }); activityMessage = paused ? 'All active downloads paused' : 'Downloads resumed'; }
      catch (error) { activityMessage = `Could not change download state: ${String(error)}`; }
    }
  }

  async function chooseNapstrFolder() {
    if (!nativeReady) { activityMessage = 'Folder selection is available in the packaged desktop app'; return; }
    try {
      const selectedPath = await open({ directory: true, multiple: false, title: 'Choose the folder Napstr uses for downloads and sharing', defaultPath: napstrFolder || undefined });
      if (!selectedPath || Array.isArray(selectedPath)) return;
      activityMessage = 'Indexing files and calculating SHA-256 hashes…';
      const report = await invoke<{ fileCount: number; totalBytes: number; errors: string[]; errorCount: number; changedFiles: number }>('set_napstr_folder', { path: selectedPath });
      activityMessage = `Indexed ${report.fileCount} file(s), ${readableSize(report.totalBytes)}${report.errorCount ? ` · ${report.errorCount} skipped` : ''}`;
    } catch (error) { activityMessage = `Folder selection failed: ${String(error)}`; }
  }

  async function openNapstrFolder() {
    if (!nativeReady) return;
    try { await invoke('open_napstr_folder'); }
    catch (error) { activityMessage = `Could not open Napstr folder: ${String(error)}`; }
  }

  async function rescanSharedFolder() {
    if (!nativeReady || rescanPending) return;
    rescanPending = true;
    activityMessage = 'Rescanning Napstr folder…';
    try {
      const report = await invoke<{ fileCount: number; totalBytes: number; changedFiles: number }>('rescan_napstr_folder');
      activityMessage = `Indexed ${report.fileCount} file(s), ${readableSize(report.totalBytes)}`;
    } catch (error) {
      activityMessage = `Rescan failed: ${String(error)}`;
    } finally {
      rescanPending = false;
    }
  }

  async function cancelLibraryScan() {
    if (!nativeReady || !indexing) return;
    try {
      await invoke('cancel_library_scan');
      activityMessage = 'Cancelling the library scan…';
    } catch (error) {
      activityMessage = `Could not cancel indexing: ${String(error)}`;
    }
  }

  async function persistSettings() {
    if (!nativeReady) return;
    try {
      applySnapshot(await invoke<Snapshot>('save_settings', { settings: { napstrFolder, nostrRelays, displayName, profileAbout, profilePicture } }));
      if (networkConnected) await invoke('publish_profile');
      activityMessage = networkConnected ? 'Settings saved and profile published' : 'Settings saved';
    } catch (error) { activityMessage = `Could not save settings: ${String(error)}`; }
  }

  const windowCommand = async (command: 'minimise_window' | 'toggle_maximise' | 'close_window') => {
    if (nativeReady) await invoke(command);
  };

  function beginWindowResize(event: PointerEvent, direction: WindowResizeDirection) {
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    getCurrentWindow().startResizeDragging(direction).catch(() => {});
  }

  function transferPaneMaximum() {
    return typeof window === 'undefined' ? 300 : Math.max(80, window.innerHeight - 395);
  }

  function setTransferPaneHeight(height: number, remember = false) {
    transferPaneHeight = Math.round(Math.min(transferPaneMaximum(), Math.max(48, height)));
    if (remember) window.localStorage.setItem('napstr-transfer-pane-height', String(transferPaneHeight));
  }

  function beginTransferResize(event: PointerEvent) {
    if (event.button !== 0) return;
    event.preventDefault();
    stopTransferResize();
    const startY = event.clientY;
    const startHeight = transferPaneHeight;
    const move = (next: PointerEvent) => setTransferPaneHeight(startHeight + startY - next.clientY);
    const stop = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', stop);
      window.removeEventListener('pointercancel', stop);
      document.body.classList.remove('resizing-transfer-pane');
      window.localStorage.setItem('napstr-transfer-pane-height', String(transferPaneHeight));
      stopTransferResize = () => {};
    };
    stopTransferResize = stop;
    document.body.classList.add('resizing-transfer-pane');
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', stop);
    window.addEventListener('pointercancel', stop);
  }

  function resizeTransferWithKeyboard(event: KeyboardEvent) {
    if (event.key === 'ArrowUp') setTransferPaneHeight(transferPaneHeight + 20, true);
    else if (event.key === 'ArrowDown') setTransferPaneHeight(transferPaneHeight - 20, true);
    else if (event.key === 'Home') setTransferPaneHeight(48, true);
    else if (event.key === 'End') setTransferPaneHeight(transferPaneMaximum(), true);
    else return;
    event.preventDefault();
  }

  onMount(() => {
    desktopRuntime = '__TAURI_INTERNALS__' in window;
    if (!desktopRuntime) return;
    const savedPlayerMode = window.localStorage.getItem('napstr-player-mode');
    if (savedPlayerMode === 'single' || savedPlayerMode === 'folder' || savedPlayerMode === 'all') playerMode = savedPlayerMode;
    const savedPlayerVolume = Number(window.localStorage.getItem('napstr-player-volume'));
    if (Number.isFinite(savedPlayerVolume) && savedPlayerVolume >= 0 && savedPlayerVolume <= 1) playerVolume = savedPlayerVolume;
    const savedTransferHeight = Number(window.localStorage.getItem('napstr-transfer-pane-height'));
    setTransferPaneHeight(Number.isFinite(savedTransferHeight) && savedTransferHeight > 0 ? savedTransferHeight : window.innerHeight < 700 ? 94 : 119);
    const clampTransferPane = () => setTransferPaneHeight(transferPaneHeight);
    window.addEventListener('resize', clampTransferPane);
    if (window.localStorage.getItem(RESULTS_VIEW_KEY) === 'thumb') resultsView = 'thumb';
    refreshSnapshot().then(connectNetwork);
    // The two cover switches live in the database, so the window can show their
    // real state straight away instead of only after a visit to the Covers tab.
    void refreshCoverStatus();
    void getVersion()
      .then((version) => {
        appVersion = version;
        return checkForNewRelease();
      })
      .catch(() => {
        appVersion = 'unknown';
      });
    let destroyed = false;
    const eventUnlisteners: UnlistenFn[] = [];
    void listen<string>('napstr-public-chat', ({ payload: topic }) => {
      if (topic === 'napstr-trollbox') void refreshTrollbox();
      const fileId = selected?.fileId?.toLowerCase();
      if (fileId && topic === `napstr-${fileId}`) void refreshTrackDiscussion(fileId);
    }).then((unlisten) => {
      if (destroyed) unlisten();
      else eventUnlisteners.push(unlisten);
    });
    void listen('napstr-library-changed', () => {
      void refreshLocalLibrary();
    }).then((unlisten) => {
      if (destroyed) unlisten();
      else eventUnlisteners.push(unlisten);
    });
    void listen('napstr-transfers-changed', () => {
      void invoke<NativeTransfer[]>('get_transfers')
        .then((items) => { transfers = mapTransfers(items); })
        .catch(() => {});
    }).then((unlisten) => {
      if (destroyed) unlisten();
      else eventUnlisteners.push(unlisten);
    });
    void listen<CoverStatus>('napstr-cover-status', ({ payload }) => {
      const grew =
        payload.published > (coverStatus?.published ?? 0) ||
        payload.resolved > (coverStatus?.resolved ?? 0);
      const finished = Boolean(coverStatus?.running) && !payload.running;
      coverStatus = payload;
      // New art exists, so the tiles are stale. On a long pass this also lets
      // covers appear as they are found instead of all at the end.
      if (grew || finished) refreshCoverArtwork();
      if (finished && activeView === 'Covers') void refreshCovers();
    }).then((unlisten) => {
      if (destroyed) unlisten();
      else eventUnlisteners.push(unlisten);
    });
    void listen<IndexBatch>('napstr-index-batch', ({ payload }) => {
      mergeIndexBatch(payload);
    }).then((unlisten) => {
      if (destroyed) unlisten();
      else eventUnlisteners.push(unlisten);
    });
    void listen<IndexProgress>('napstr-index-progress', ({ payload }) => {
      indexing = payload.scanning;
      if (payload.message) activityMessage = payload.message;
      if (!payload.scanning) rescanPending = false;
    }).then((unlisten) => {
      if (destroyed) unlisten();
      else eventUnlisteners.push(unlisten);
    });
    const updateClock = () => {
      clock = new Intl.DateTimeFormat('en-GB', { hour: '2-digit', minute: '2-digit' }).format(new Date());
    };
    updateClock();
    const clockTimer = window.setInterval(updateClock, 30000);
    let lastWakeTick = Date.now();
    let lastWakeRecovery = 0;
    const detectWake = () => {
      const now = Date.now();
      const elapsed = now - lastWakeTick;
      lastWakeTick = now;
      if (elapsed > 20000 && now - lastWakeRecovery > 15000) {
        lastWakeRecovery = now;
        window.setTimeout(() => void recoverAfterSleep(), 1200);
      }
    };
    const foregrounded = () => {
      if (!document.hidden) detectWake();
    };
    window.addEventListener('focus', foregrounded);
    document.addEventListener('visibilitychange', foregrounded);
    const wakeTimer = window.setInterval(detectWake, 5000);
    let networkPollPending = false;
    const networkTimer = window.setInterval(async () => {
      if (!nativeReady || networkPollPending) return;
      networkPollPending = true;
      try {
        const status = await invoke<NetworkStatus>('network_status');
        const previousTorError = torError;
        const wasConnected = networkConnected;
        applyNetworkStatus(status);
        if (status.torError && status.torError !== previousTorError) {
          activityMessage = `Tor failed: ${status.torError} · click the connection panel to retry`;
        } else if (!wasConnected && status.connected) {
          activityMessage = 'Nostr reconnected · refreshing the catalogue';
          void search();
          if (activeView === 'Trollbox') void refreshTrollbox();
        } else if (!status.connected && !networkConnectPending) {
          void connectNetwork();
        }
      } catch { /* the next health poll retries */ }
      finally { networkPollPending = false; }
    }, 5000);
    let transferPollPending = false;
    const transferTimer = window.setInterval(async () => {
      const transferWorkPending =
        startingDownloads.size > 0 ||
        audiobookDownloads.length > 0 ||
        transfers.some(isActiveTransfer);
      if (!nativeReady || transferPollPending || !transferWorkPending || clearingTransfers || removingTransfers.size) return;
      transferPollPending = true;
      try {
        const items = await invoke<NativeTransfer[]>('get_transfers');
        if (clearingTransfers || removingTransfers.size) return;
        const previouslyComplete = new Set(transfers.filter(isCompleteTransfer).map((transfer) => transfer.fileId));
        const updated = mapTransfers(items);
        const newlyComplete = updated.filter((transfer) => isCompleteTransfer(transfer) && !previouslyComplete.has(transfer.fileId));
        const vanishedActive = transfers.filter((transfer) => !startingDownloads.has(transfer.fileId) && isActiveTransfer(transfer) && !updated.some((item) => item.id === transfer.id));
        const optimistic = transfers.filter((transfer) => startingDownloads.has(transfer.fileId) && !updated.some((item) => item.fileId === transfer.fileId));
        transfers = [...optimistic, ...updated];
        if (newlyComplete.length || vanishedActive.length) {
          await refreshLocalLibrary();
          const latest = newlyComplete[0] ?? vanishedActive.find((transfer) => isLocalFile(transfer.fileId));
          if (latest) activityMessage = `${latest.name} downloaded, verified, and ready to play`;
        }
        if (audiobookDownloads.length) await advanceAudiobookDownloads();
      } catch { /* the next transfer poll retries */ }
      finally { transferPollPending = false; }
    }, 1000);
    const playerTimer = window.setInterval(() => {
      if (!nativeReady || !currentTrack || playerLoading) return;
      invoke<PlaybackStatus>('audio_status').then((status) => {
        const naturallyEnded = status.fileId === currentTrack?.fileId && status.ended && !playerEnded;
        applyPlaybackStatus(status);
        if (naturallyEnded) void playerTrackEnded();
      }).catch(() => {});
    }, 250);
    const mobileTimer = window.setInterval(() => {
      if (mobilePairing && mobilePairing.expiresAt <= Math.floor(Date.now() / 1000)) mobilePairing = null;
      if (mobileStreamPairing && mobileStreamPairing.expiresAt <= Math.floor(Date.now() / 1000)) mobileStreamPairing = null;
      if (activeView === 'Mobile') void refreshMobileStatus();
    }, 3000);
    return () => {
      destroyed = true;
      eventUnlisteners.forEach((unlisten) => unlisten());
      clearInterval(clockTimer);
      clearInterval(wakeTimer);
      clearInterval(networkTimer);
      clearInterval(transferTimer);
      clearInterval(playerTimer);
      clearInterval(mobileTimer);
      window.removeEventListener('resize', clampTransferPane);
      window.removeEventListener('focus', foregrounded);
      document.removeEventListener('visibilitychange', foregrounded);
      stopTransferResize();
      if (nativeReady && currentTrack) invoke<PlaybackStatus>('stop_audio').catch(() => {});
    };
  });
</script>

<svelte:head><title>Napstr - own your music again</title></svelte:head>

{#if desktopRuntime}
<main class="desktop">
  <section class="app-window" style={`--transfer-height: ${transferPaneHeight}px`} aria-label="Napstr application window">
    <button class="window-resize-handle resize-n" aria-label="Resize window from top" onpointerdown={(event) => beginWindowResize(event, 'North')}></button>
    <button class="window-resize-handle resize-e" aria-label="Resize window from right" onpointerdown={(event) => beginWindowResize(event, 'East')}></button>
    <button class="window-resize-handle resize-s" aria-label="Resize window from bottom" onpointerdown={(event) => beginWindowResize(event, 'South')}></button>
    <button class="window-resize-handle resize-w" aria-label="Resize window from left" onpointerdown={(event) => beginWindowResize(event, 'West')}></button>
    <button class="window-resize-handle resize-ne" aria-label="Resize window from top right" onpointerdown={(event) => beginWindowResize(event, 'NorthEast')}></button>
    <button class="window-resize-handle resize-se" aria-label="Resize window from bottom right" onpointerdown={(event) => beginWindowResize(event, 'SouthEast')}></button>
    <button class="window-resize-handle resize-sw" aria-label="Resize window from bottom left" onpointerdown={(event) => beginWindowResize(event, 'SouthWest')}></button>
    <button class="window-resize-handle resize-nw" aria-label="Resize window from top left" onpointerdown={(event) => beginWindowResize(event, 'NorthWest')}></button>

    <header class="titlebar" data-tauri-drag-region>
      <div class="title-left"><span class="app-icon"><img src="/napstr-logo.png" alt="" /></span><span>Napstr - own your music again</span></div>
      <div class="window-controls" aria-hidden="true">
        <button tabindex="-1" onclick={() => windowCommand('minimise_window')}>_</button><button tabindex="-1" onclick={() => windowCommand('toggle_maximise')}>□</button><button tabindex="-1" onclick={() => windowCommand('close_window')}>×</button>
      </div>
    </header>

    <div class="toolbar">
      <div class="toolbar-brand" title="Napstr home">
        <img src="/napstr-logo.png" alt="Napstr" />
      </div>
      <div class="toolbar-separator"></div>
      {#each views as view}
        <button class:active={activeView === view.label} class="tool-button" onclick={() => activateView(view.label)}>
          <span class="tool-icon icon-{view.label.toLowerCase()}">{view.icon}</span>
          <span>{view.label}</span>
        </button>
      {/each}
      <div class="toolbar-spacer"></div>
      {#if newRelease}
        <button class="release-button" onclick={openNewRelease} title={`Open Napstr ${newRelease.version} on GitHub`}>
          <span class="release-arrow">⇧</span>
          <span><strong>New release</strong><small>{newRelease.version} available</small></span>
        </button>
      {/if}
      <button class="connection-box" onclick={connectNetwork} title={torError || networkError || 'Reconnect Nostr and Tor'}>
        <span class="connection-status"><i class:amber={!networkConnected} class="led"></i><strong>{networkConnected ? 'Nostr connected' : 'Connect Nostr'}</strong></span>
        <span class="connection-status"><i class:amber={!torRunning} class:error={Boolean(torError)} class="led"></i><strong>{torStatusLabel()}</strong></span>
      </button>
      <button class="tool-button help-button" onclick={() => (aboutOpen = true)}><span class="tool-icon">?</span><span>About</span></button>
    </div>

    <div class="network-strip">
      <span class="network-pulse">▥</span>
      <span>{activityMessage}</span>
      <span class="strip-right"><button class="user-name" disabled={!identityNpub} onclick={() => browseUser(ownCatalogueUser())}>{displayName}</button> <i class:amber={!nativeReady} class="led"></i></span>
    </div>

    <section class="player-bar" aria-label="Napstr audio player">
      <div class="player-display">
        <span class:playing={playerPlaying} class="player-led">{playerLoading ? '···' : playerPlaying ? '▶' : '■'}</span>
        <div><strong>{currentTrack?.name ?? 'No track selected'}</strong><small>{currentTrack ? `${currentTrack.artist || 'Unknown artist'} · ${folderName(currentTrack.folder)}` : 'Choose a local song to begin'}</small></div>
      </div>
      <div class="player-controls">
        <button onclick={previousPlayerTrack} disabled={!currentTrack || playerLoading} title="Previous track">|◀</button>
        <button class="player-primary" onclick={togglePlayer} disabled={playerLoading} title={playerPlaying ? 'Pause' : 'Play'}>{playerLoading ? '…' : playerPlaying ? 'Ⅱ' : '▶'}</button>
        <button onclick={stopPlayer} disabled={!currentTrack || playerLoading} title="Stop">■</button>
        <button onclick={nextPlayerTrack} disabled={playerLoading || playerQueueIndex < 0 || playerQueueIndex + 1 >= playerQueue.length} title="Next track">▶|</button>
      </div>
      <div class="player-seek">
        <input aria-label="Track position" type="range" min="0" max={Math.max(0, playerDuration || 0)} step="0.1" value={playerCurrentTime} oninput={seekPlayer} disabled={!currentTrack} />
        <span>{formatPlayerTime(playerCurrentTime)} / {formatPlayerTime(playerDuration)}</span>
      </div>
      <label class="player-mode">After track
        <select bind:value={playerMode} onchange={changePlayerMode}>
          <option value="single">Stop</option>
          <option value="folder">Play folder</option>
          <option value="all">Play all</option>
        </select>
      </label>
      <label class="player-volume">Vol <input aria-label="Volume" type="range" min="0" max="1" step="0.05" value={playerVolume} oninput={changePlayerVolume} /></label>
    </section>

    {#if coverPicker}
      <CoverPicker
        artist={coverPicker.artist}
        album={coverPicker.album}
        currentArt={coverPicker.currentArt}
        onClose={() => (coverPicker = null)}
        onApplied={coverPickApplied}
      />
    {/if}

    {#if coverReport}
      <CoverReport
        albumKey={coverReport.key}
        label={coverReport.label}
        onClose={() => (coverReport = null)}
        onReported={(message: string) => (activityMessage = message)}
      />
    {/if}

    <div class="workspace">
      {#if activeView === 'Search'}
        <section class="panel search-panel">
          <div class="panel-title"><span></span><b>Search the Napstr network</b><span></span></div>
          <form class="search-form" onsubmit={(e) => { e.preventDefault(); search(); }}>
            <label for="search-query">Search:</label>
            <input id="search-query" bind:value={query} placeholder="punk, rock, jazz, audiobook" />
            <label for="format">File type:</label>
            <select id="format" bind:value={format} disabled={searchAction !== null} onchange={() => void search()}><option>Audio only</option><option>Audiobooks</option></select>
            <button class="classic-button primary search-button" type="submit" disabled={searchAction !== null} aria-busy={searchAction === 'search'}>
              {#if searchAction === 'search'}<span class="search-spinner" aria-hidden="true"></span>{/if}
              {searchAction === 'search' ? 'Searching' : 'Search'}
            </button>
            <button class="classic-button surprise-button" type="button" onclick={surpriseMe} disabled={searchAction !== null || !networkConnected} aria-busy={searchAction === 'surprise'}>
              {#if searchAction === 'surprise'}<span class="search-spinner" aria-hidden="true"></span>{/if}
              {searchAction === 'surprise' ? 'Choosing…' : 'Surprise me'}
            </button>
          </form>
          {#if resultUser}
            <div class="user-search-status"><span>Shared by <b>{resultUser.displayName}</b> <code title={resultUser.npub}>{resultUser.npub.slice(0, 18)}…</code></span><button class="classic-button" disabled={searchAction !== null} onclick={() => { query = ''; searchUser = null; void search(); }}>Clear user filter</button></div>
          {:else if matchingUsers.length > 1}
            <div class="user-search-status"><span>Several users have this name. Choose whose songs to browse:</span>{#each matchingUsers as user}<button class="classic-button" title={user.npub} onclick={() => browseUser(user)}>{user.displayName} · {user.npub.slice(0, 18)}…</button>{/each}</div>
          {/if}
          <button class="advanced-toggle" onclick={() => (advanced = !advanced)}><span>{advanced ? '▼' : '▶'}</span> {advanced ? 'Hide' : 'Show'} advanced search options</button>
          {#if advanced}
            <div class="advanced-row"><label>Minimum seeders: <input type="number" bind:value={minimumSources} min="1" /></label><label>Maximum size: <input bind:value={maximumSize} placeholder="e.g. 2 GB" /></label><label><input type="checkbox" checked disabled /> Online seeders only</label></div>
          {/if}
        </section>

        <div class="split-content">
          <section class="results-pane" aria-label="Search results">
            <div class="section-caption">
              <span>Search results for “{searchedQuery}”</span>
              <div class="results-caption-tools">
                <small>{format === 'Audiobooks' ? `${results.length} audiobook${results.length === 1 ? '' : 's'} found` : browseTotalAvailable ? `${results.length} loaded of ${resultAvailableTotal} available` : `${results.length} file IDs found`}</small>
                <button type="button" class="view-toggle" class:active={resultsView === 'list'} title="Show these results as a list" aria-pressed={resultsView === 'list'} onclick={() => setResultsView('list')}>▤</button>
                <button type="button" class="view-toggle" class:active={resultsView === 'thumb'} title="Show these results as thumbnails, with album art" aria-pressed={resultsView === 'thumb'} onclick={() => setResultsView('thumb')}>▦</button>
              </div>
            </div>
            {#if resultsView === 'thumb'}
              <div class="result-grid" aria-label="Search results as thumbnails">
                {#each resultPageItems as item}
                  <button
                    type="button"
                    class="result-card"
                    class:selected={selectedResultIds.has(item.fileId)}
                    aria-pressed={selectedResultIds.has(item.fileId)}
                    onclick={(event) => selectResultRange(item, event)}
                    ondblclick={(event) => { if (!event.shiftKey) void activateSelected(); }}
                  >
                    {#if item.audiobook}
                      <span class="cover-art medium empty" aria-hidden="true">▥</span>
                    {:else}
                      <CoverArt artist={item.artist ?? ''} album={item.album ?? ''} preferThumb size="medium" revision={coverRevision} />
                    {/if}
                    <b title={item.name}>{item.name}</b>
                    <small title={item.artist || undefined}>{item.artist || 'Unknown artist'}</small>
                    <small class="result-card-album" title={item.album || undefined}>{item.album || '—'}</small>
                    <span class="result-card-meta"><i class="source-dot"></i>{item.sources} {item.sources === 1 ? 'seeder' : 'seeders'} · {item.size}</span>
                  </button>
                {/each}
                {#if results.length === 0}<p class="empty-state">Nothing to show yet.</p>{/if}
              </div>
            {:else}
            <div class="table-wrap">
              <table class="file-table search-results-table">
                <thead><tr><th class="name-col">Name</th><th>Type</th><th class="number">Size</th><th class="number">Seeders</th><th>Line speed</th><th>Length</th></tr></thead>
                <tbody>
                  {#each resultPageItems as item}
                    <tr class:selected={selectedResultIds.has(item.fileId)} aria-selected={selectedResultIds.has(item.fileId)} tabindex="0"
                      onclick={(event) => selectResultRange(item, event)}
                      onkeydown={(event) => { if (event.key === ' ' || event.key === 'Enter') { event.preventDefault(); selectResultRange(item, event); } }}
                      ondblclick={(event) => { if (!event.shiftKey) void activateSelected(); }}>
                      <td><span class:audiobook-icon={Boolean(item.audiobook)} class="file-icon">{item.audiobook ? '▥' : '▶'}</span>{item.name}</td><td>{item.format}</td><td class="number">{item.size}</td><td class="number"><span class="source-dot"></span>{item.sources}</td><td>{item.speed}</td><td>{item.length}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
            {/if}
            <div class="results-pager">
              <button onclick={() => void changeResultPage(resultPage - 1)} disabled={resultPage === 0}>◀ Previous</button>
              <span>{resultRangeLabel} of {results.length} loaded{browseTotalAvailable ? ` · ${resultAvailableTotal} available` : ''} · Page {resultPage + 1} of {resultPageTotal}{browseCursor ? '+' : ''}</span>
              <button onclick={() => void changeResultPage(resultPage + 1)} disabled={browseLoading || (resultPage + 1 >= resultPageTotal && !browseCursor)}>{browseLoading ? 'Loading…' : 'Next ▶'}</button>
            </div>
          </section>

          <aside class="details-pane">
            <div class="section-caption"><span>File details</span></div>
            {#if selected}
              {#if selectedResultIds.size > 1}
                <div class="selected-file">
                  <div class="large-file-icon">♫</div>
                  <div><strong>{selectedResultIds.size} tracks selected</strong><span>Click a track, then Shift-click another to select a range.</span></div>
                </div>
                <div class="detail-actions"><button class="classic-button primary" onclick={downloadSelectedResults} disabled={!nativeReady || downloadingSelection || !selectedResults().some(canDownloadResult)} aria-busy={downloadingSelection}>{downloadingSelection ? '… Requesting' : '⇩ Download All'}</button></div>
                <p class="privacy-note"><span>♜</span> Downloads use Tor. Tracks already on this computer, queued, or without seeders are skipped.</p>
              {:else if selected.audiobook}
                <div class="selected-file audiobook-selected">
                  <div class="large-file-icon">▥</div>
                  <div><strong>{selected.audiobook.title}</strong><span>Audiobook · {selected.audiobook.chapters.length} chapters · {selected.size}</span><small>Edition ID: {selected.audiobook.audiobookId}</small></div>
                </div>
                <div class="file-metadata"><p><b>{selected.audiobook.author || 'Unknown author'}</b>{selected.audiobook.narrator ? ` · Narrated by ${selected.audiobook.narrator}` : ''}</p><small>Chapters are ordered and each file is independently SHA-256 verified.</small></div>
                <div class="audiobook-chapters" aria-label="Audiobook chapters">
                  {#each selected.audiobook.chapters as chapter}
                    <button
                      type="button"
                      class:chapter-local={isLocalFile(chapter.fileId)}
                      class:chapter-playing={currentTrack?.fileId === chapter.fileId}
                      disabled={!isLocalFile(chapter.fileId)}
                      title={isLocalFile(chapter.fileId) ? `Play ${chapter.title}` : `${chapter.title} has not downloaded yet`}
                      onclick={() => playAudiobookChapter(selected!.audiobook!, chapter.fileId)}
                    ><span>{String(chapter.position).padStart(2, '0')}</span><b>{chapter.title}</b><small>{readableSize(chapter.size)}</small><i>{audiobookChapterStatus(selected.audiobook!, chapter)}</i></button>
                  {/each}
                </div>
                <div class="detail-actions">{#if selectedAudiobookComplete()}<button class="classic-button primary" onclick={playSelectedAudiobook}>▶ Play book</button><button class="classic-button" onclick={openNapstrFolder}>Open folder</button>{:else}<button class="classic-button primary" disabled={selectedAudiobookDownloading()} onclick={downloadSelectedAudiobook}>⇩ {selectedAudiobookDownloading() ? 'Downloading…' : 'Download book'}</button>{/if}</div>
                {#if !selected.audiobook.local}<p class="privacy-note"><span>♜</span> Chapters download first-to-last through private Tor onion services. Play each chapter as soon as it shows Ready.</p>{:else}<p class="privacy-note"><span>♬</span> This complete audiobook is ready to play.</p>{/if}
              {:else}
              <div class="detail-cover">
                <CoverArt artist={selected.artist ?? ''} album={selected.album ?? ''} size="large" revision={coverRevision} />
              </div>
              {#if selected.artist && selected.album}
                <div class="detail-actions"><button class="classic-button" onclick={() => void openCoverPicker(selected?.artist ?? '', selected?.album ?? '')}>Fix album art…</button><button class="classic-button" title="Report the cover published for this album as a NIP-56 report" onclick={() => openCoverReport(selected?.artist ?? '', selected?.album ?? '')}>Report cover…</button></div>
              {/if}
              <div class="selected-file">
                <div class="large-file-icon">▶</div>
                <div><strong>{selected.name}</strong><span>{selected.format} · {selected.size} · {selected.length}</span><small>File ID: {selected.fileId}</small></div>
              </div>
              {#if selected.artist || selected.album}<div class="file-metadata"><small>{selected.artist ? `Artist: ${selected.artist}` : ''}{selected.artist && selected.album ? ' · ' : ''}{selected.album ? `Album: ${selected.album}` : ''}</small></div>{/if}
              {#if selected.tags}<div class="file-metadata"><small>Tags: {selected.tags}</small></div>{/if}
              <fieldset><legend>Seeders</legend>
                <div class="sources-list">
                  {#if !isLocalFile(selected.fileId)}
                    {#each (selected.sourceDetails ?? []).slice(0, VISIBLE_SEEDER_LIMIT) as source, index}
                      <div class:selected-source={selectedSource === index} class="source-row"><button class="user-icon source-select" title={`Select ${source.displayName} for profile and moderation actions`} onclick={() => (selectedSource = index)}>☺</button><button class="user-name" title={`Browse songs shared by ${source.displayName}`} onclick={() => browseUser(source)}>{source.displayName}</button><small>{source.npub.slice(0, 12)}…</small><span class="online"><i></i> Seeding</span></div>
                    {/each}
                  {:else}
                    <div><span class="user-icon">☺</span><b>This computer</b><small>Local</small><span class="online"><i></i> Ready</span></div>
                  {/if}
                </div>
              </fieldset>
              <div class="detail-actions">{#if !isLocalFile(selected.fileId)}<button class="classic-button primary" disabled={startingDownloads.has(selected.fileId)} onclick={() => startDownload()}>{startingDownloads.has(selected.fileId) ? '… Requesting' : '⇩ Download'}</button><button class="classic-button" onclick={() => (sourceProfile = selected?.sourceDetails?.[selectedSource] ?? null)}>View profile</button>{:else}<button class="classic-button primary" onclick={playSelectedAudio}>▶ Play</button><button class="classic-button" onclick={openNapstrFolder}>Open folder</button>{/if}</div>
              {#if !isLocalFile(selected.fileId)}<div class="detail-actions moderation-actions"><button class="classic-button" onclick={blockSelectedFile}>Block file</button><button class="classic-button" onclick={blockSelectedUser}>Block user</button></div>{/if}
              {#if !isLocalFile(selected.fileId)}<p class="privacy-note"><span>♜</span> Transfer will use the seeder’s private, app-session Tor onion service.</p>{:else}<p class="privacy-note"><span>♬</span> Downloaded and verified · ready to play from your Napstr folder.</p>{/if}
              <section class="track-discussion" aria-label={`Discussion for ${selected.name}`}>
                <div class="track-discussion-title"><b>Track discussion</b><small>Public · Nostr</small></div>
                <div class="track-discussion-log" bind:this={trackDiscussionLog} aria-live="polite">
                  {#if trackDiscussionLoading}<p class="trollbox-notice">Loading comments…</p>{/if}
                  {#if !trackDiscussionLoading && trackDiscussionMessages.length === 0 && !trackDiscussionError}<p class="trollbox-notice">No comments yet.</p>{/if}
                  {#each trackDiscussionMessages as message (message.eventId)}
                    <div class="trollbox-message"><button class="trollbox-name" style:color={chatNameColor(message.npub)} title={`Browse songs shared by ${message.displayName} · ${message.npub}`} onclick={() => browseUser(message)}>{message.displayName}:</button><span>{message.content}</span>{#if message.npub !== identityNpub}<button class="chat-block" aria-label={`Block ${message.displayName}`} onclick={() => blockTrollboxUser(message)}>Block</button>{/if}</div>
                  {/each}
                </div>
                {#if trackDiscussionError}<div class="track-discussion-error">{trackDiscussionError}</div>{/if}
                <div class="track-discussion-compose">
                  <input bind:value={trackDiscussionDraft} maxlength="500" autocomplete="off" placeholder={networkConnected ? 'Comment on this track…' : 'Connect to Nostr to comment'} disabled={!networkConnected || trackDiscussionSending} aria-label="Track discussion comment" onkeydown={(event) => { if (event.key === 'Enter') { event.preventDefault(); void sendTrackDiscussionMessage(); } }} />
                  <button class="classic-button primary" type="button" disabled={!networkConnected || trackDiscussionSending || !trackDiscussionDraft.trim()} onclick={() => void sendTrackDiscussionMessage()}>{trackDiscussionSending ? '…' : 'Send'}</button>
                </div>
              </section>
              {/if}
            {:else}<p class="empty-state">Select a result to see active seeders.</p>{/if}
          </aside>
        </div>
      {:else if activeView === 'Downloads'}
        <section class="full-panel downloads-view">
          <div class="panel-title"><span></span><b>Download Manager</b><span></span></div>
          <div class="actionbar"><button class="classic-button" onclick={togglePause}>{paused ? '▶ Resume all' : 'Ⅱ Pause all'}</button><button class="classic-button" onclick={openNapstrFolder}>Open Napstr folder</button><button class="classic-button" onclick={clearFinishedTransfers} disabled={clearingTransfers || !transfers.some(isFinishedTransfer)}>Clear finished</button><button class="classic-button" onclick={clearAllTransfers} disabled={clearingTransfers || removingTransfers.size > 0 || (!transfers.length && !audiobookDownloads.length && !startingDownloads.size)}>{clearingTransfers ? 'Clearing…' : 'Clear all'}</button><div class="spacer"></div><span>{transfers.filter(isActiveTransfer).length} active · {transfers.filter(isCompleteTransfer).length} ready to play</span></div>
          <div class="download-queue">
            {#each audiobookDownloads as book}
              <div class="audiobook-download-row"><span class="audiobook-glyph">▥</span><b>{book.title}</b><div class="progress"><span style={`width:${book.chapters.length ? (book.nextIndex / book.chapters.length) * 100 : 0}%`}></span><b>{book.nextIndex}/{book.chapters.length}</b></div><span>{book.activeFileId ? `Downloading chapter ${book.nextIndex + 1}` : 'Preparing next chapter'}</span></div>
            {/each}
            <table class="file-table download-table"><thead><tr><th>Download order</th><th>Progress</th><th>Size</th><th>Speed</th><th>Status</th><th></th></tr></thead><tbody>
              {#each transfers as transfer}
                <tr class:transfer-complete={isCompleteTransfer(transfer)} ondblclick={() => { if (isCompleteTransfer(transfer)) playAudio(transfer.fileId, transfer.name, playerMode, 'downloads'); }}><td><span class="download-arrow">{isCompleteTransfer(transfer) ? '▶' : '⇩'}</span>{transfer.name}</td><td><div class="progress"><span style={`width:${transfer.progress}%`}></span><b>{Math.round(transfer.progress)}%</b></div></td><td>{transfer.size}</td><td>{isCompleteTransfer(transfer) ? 'Local' : transfer.speed}</td><td>{isCompleteTransfer(transfer) ? 'Ready to play' : transfer.status}</td><td class="transfer-actions">{#if isCompleteTransfer(transfer)}<button class="classic-button transfer-play" onclick={(event) => { event.stopPropagation(); playAudio(transfer.fileId, transfer.name, playerMode, 'downloads'); }} title="Play verified audio">▶ Play</button>{/if}<button class="tiny-button" disabled={clearingTransfers || removingTransfers.has(transfer.id)} onclick={(event) => { event.stopPropagation(); removeTransfer(transfer.id); }} aria-label={`Remove transfer: ${transfer.name}`} title="Cancel and clear entry; keep completed audio">×</button></td></tr>
              {/each}
            </tbody></table>
            {#if transfers.length === 0}<p class="empty-state compact">There are no downloads in the queue.</p>{/if}
          </div>
          <div class="panel-title"><span></span><b>Track Tags</b><span></span></div>
          <div class="tag-editor">
            <b>{selectedTagFile?.filename ?? 'Select a local track below'}</b>
            <input bind:value={tagDraft} disabled={!selectedTagFile || tagSaving} maxlength="256" placeholder="punk, live, audiobook" onkeydown={(event) => { if (event.key === 'Enter') saveTags(); }} />
            <button class="classic-button primary" onclick={saveTags} disabled={!selectedTagFile || tagSaving}>{tagSaving ? 'Saving…' : 'Save tags'}</button>
            <small>Comma-separated · published with your signed catalogue</small>
          </div>
          <div class="tag-library">
            <table class="file-table tags-table"><thead><tr><th>Name</th><th>Folder</th><th>Tags</th></tr></thead><tbody>
              {#each paginatedTagFiles() as file}
                <tr class:selected={selectedTagFile?.fileId === file.fileId} onclick={() => selectTagFile(file)} ondblclick={() => playAudio(file.fileId, file.filename, playerMode, 'downloads')}><td><button type="button" class="file-icon file-play-button" title={`Play ${file.filename}`} aria-label={`Play ${file.filename}`} onclick={(event) => { event.stopPropagation(); selectTagFile(file); playAudio(file.fileId, file.filename, playerMode, 'downloads'); }}>▶</button>{file.filename}</td><td>{folderName(file.folder)}</td><td>{file.tags || '—'}</td></tr>
              {/each}
            </tbody></table>
            {#if sharedFiles.length === 0}<p class="empty-state compact">Downloaded and shared tracks will appear here.</p>{/if}
          </div>
          {#if sharedFiles.length > LOCAL_PAGE_SIZE}<div class="results-pager"><button disabled={downloadLibraryPage === 0} onclick={() => changeDownloadLibraryPage(downloadLibraryPage - 1)}>◀ Previous</button><span>{localPageRange(downloadLibraryPage, sharedFiles.length)} of {sharedFiles.length} · Page {downloadLibraryPage + 1} of {localPageCount(sharedFiles)}</span><button disabled={downloadLibraryPage + 1 >= localPageCount(sharedFiles)} onclick={() => changeDownloadLibraryPage(downloadLibraryPage + 1)}>Next ▶</button></div>{/if}
        </section>
      {:else if activeView === 'Shared'}
        <section class="full-panel">
          <div class="panel-title"><span></span><b>My Shared Files</b><span></span></div>
          <div class="actionbar"><button class="classic-button" onclick={indexing ? cancelLibraryScan : rescanSharedFolder}>{indexing ? '× Cancel scan' : rescanPending ? '… Rescanning' : '↻ Rescan'}</button><button class="classic-button" onclick={openNapstrFolder}>Open folder</button><button class="classic-button" onclick={playSelectedSharedAudio} disabled={!selectedShared}>▶ Play</button><button class="classic-button" onclick={playSelectedFolder} disabled={!selectedShared}>▶ Play folder</button><button class="classic-button primary" onclick={playAllSongs} disabled={!sharedFiles.length}>▶ Play all</button><button class="classic-button audiobook-button" onclick={openAudiobookEditor} disabled={libraryFolderView === '*' || audiobookFolderFiles().length < 1}>▥ {currentFolderAudiobook() ? 'Edit audiobook' : 'Group as audiobook…'}</button><div class="spacer"></div><span>Sharing {sharedFiles.length} files · {readableSize(indexedBytes)}</span></div>
          <div class="folder-path"><b>Napstr folder:</b><input value={napstrFolder || 'No folder selected'} readonly /><button class="classic-button" onclick={chooseNapstrFolder}>Browse…</button></div>
          <div class="library-filter">
            <span class="library-filter-label">View folder:</span>
            <div class="folder-picker" use:containLibraryFolderMenu>
              <button type="button" class="folder-picker-toggle" aria-haspopup="listbox" aria-expanded={libraryFolderMenuOpen} onclick={() => (libraryFolderMenuOpen = !libraryFolderMenuOpen)} title={libraryFolderView === '*' ? 'All folders' : folderName(libraryFolderView)}>
                <span>{libraryFolderView === '*' ? 'All folders' : folderName(libraryFolderView)}</span><i aria-hidden="true">▼</i>
              </button>
              {#if libraryFolderMenuOpen}
                <div class="folder-picker-menu" role="listbox" aria-label="View folder">
                  <button type="button" role="option" aria-selected={libraryFolderView === '*'} class:selected={libraryFolderView === '*'} onclick={() => selectLibraryFolder('*')}>All folders</button>
                  {#each libraryFolders() as folder}
                    <button type="button" role="option" aria-selected={libraryFolderView === folder} class:selected={libraryFolderView === folder} onclick={() => selectLibraryFolder(folder)} title={folderName(folder)}>{folderName(folder)}</button>
                  {/each}
                </div>
              {/if}
            </div>
            <span class="library-song-count">{visibleSharedFiles().length} song{visibleSharedFiles().length === 1 ? '' : 's'} shown</span>
          </div>
          {#if !currentFolderAudiobook() && libraryFolderView.toLowerCase().includes('audiobook') && audiobookFolderFiles().length >= 1}<div class="audiobook-folder-banner"><span class="audiobook-glyph">▥</span><div><b>Possible audiobook detected</b><small>Review the natural chapter order before making the collection public.</small></div><button class="classic-button primary" onclick={openAudiobookEditor}>Group as audiobook…</button></div>{/if}
          {#if currentFolderAudiobook()}<div class="audiobook-folder-banner"><span class="audiobook-glyph">▥</span><div><b>{currentFolderAudiobook()?.title}</b><small>{currentFolderAudiobook()?.author || 'Unknown author'} · {currentFolderAudiobook()?.chapters.length} ordered chapters · published as one audiobook</small></div><button class="classic-button primary" onclick={() => playAudiobook(currentFolderAudiobook()!)}>▶ Play book</button></div>{/if}
          <table class="file-table shared-table"><thead><tr><th>Name</th><th>Folder</th><th>Size</th><th>Catalogue</th><th>Active peers</th></tr></thead><tbody>{#each paginatedSharedFiles() as file}<tr class:selected={selectedShared?.fileId === file.fileId} onclick={() => (selectedShared = { ...file })} ondblclick={() => playAudio(file.fileId, file.name, playerMode, 'shared')}><td><span class="file-icon">▶</span>{file.name}</td><td>{folderName(file.folder)}</td><td>{file.readableSize}</td><td><span class:amber={!networkConnected} class="led"></span>{networkConnected ? 'Published' : 'Indexed'}</td><td>{file.peers}</td></tr>{/each}</tbody></table>
          {#if visibleSharedFiles().length > LOCAL_PAGE_SIZE}<div class="results-pager"><button disabled={sharedLibraryPage === 0} onclick={() => changeSharedLibraryPage(sharedLibraryPage - 1)}>◀ Previous</button><span>{localPageRange(sharedLibraryPage, visibleSharedFiles().length)} of {visibleSharedFiles().length} · Page {sharedLibraryPage + 1} of {localPageCount(visibleSharedFiles())}</span><button disabled={sharedLibraryPage + 1 >= localPageCount(visibleSharedFiles())} onclick={() => changeSharedLibraryPage(sharedLibraryPage + 1)}>Next ▶</button></div>{/if}
          <p class="privacy-note wide"><span>♜</span> Only validated MP3, FLAC, WAV, Ogg Vorbis, and Opus audio is indexed recursively. Put book folders or complete one-file books inside Audiobooks for automatic grouping. Existing contents are never replaced. Folder names remain local and embedded cover artwork is allowed.</p>
        </section>
      {:else if activeView === 'Trollbox'}
        <section class="full-panel trollbox-view">
          <div class="panel-title"><span></span><b>Napstr Trollbox</b><span></span></div>
          <div class="trollbox-status"><span><i class:amber={!networkConnected} class="led"></i> Public Nostr chat: <b>#napstr-trollbox</b></span><small>NIP-C7 messages are public and signed by your Napstr Nostr identity.</small></div>
          <div class="trollbox-log" bind:this={trollboxLog} aria-live="polite" aria-label="Napstr public chat messages">
            {#if trollboxLoading}<p class="trollbox-notice">Connecting to the trollbox…</p>{/if}
            {#if !trollboxLoading && trollboxMessages.length === 0 && !trollboxError}<p class="trollbox-notice">No messages yet. Say hello.</p>{/if}
            {#each trollboxMessages as message (message.eventId)}
              <div class="trollbox-message"><button class="trollbox-name" style:color={chatNameColor(message.npub)} title={`Browse songs shared by ${message.displayName} · ${message.npub}`} onclick={() => browseUser(message)}>{message.displayName}:</button><span>{message.content}</span>{#if message.npub !== identityNpub}<button class="chat-block" aria-label={`Block ${message.displayName}`} onclick={() => blockTrollboxUser(message)}>Block</button>{/if}</div>
            {/each}
          </div>
          {#if trollboxError}<div class="trollbox-error">{trollboxError}</div>{/if}
          <div class="trollbox-compose">
            <input bind:value={trollboxDraft} maxlength="500" autocomplete="off" placeholder={networkConnected ? 'Type a public message…' : 'Connect to Nostr to chat'} disabled={!networkConnected || trollboxSending} aria-label="Trollbox message" onkeydown={(event) => { if (event.key === 'Enter') { event.preventDefault(); void sendTrollboxMessage(); } }} />
            <button class="classic-button primary" type="button" disabled={!networkConnected || trollboxSending || !trollboxDraft.trim()} onclick={() => void sendTrollboxMessage()}>{trollboxSending ? 'Sending…' : 'Send'}</button>
          </div>
        </section>
      {:else if activeView === 'Covers'}
        <section class="full-panel cover-scan-view">
          <div class="panel-title"><span></span><b>Album covers</b><span></span></div>
          <p class="privacy-note wide"><span>i</span> Napstr finds album art in two steps. Reading the kind <code>30427</code> claims other people published needs nothing switched on: it is an ordinary relay query, and it happens by itself as results appear. The two switches below are the steps that leave Napstr. Napstr acts on them on its own — there is nothing else to press — and they are stored, so leaving one on means it carries on after a restart.</p>

          <div class="cover-scan-controls">
            <label class="cover-scan-toggle">
              <input type="checkbox" checked={coverStatus?.lookupExternal ?? false} onchange={(event) => void setCoverPreferences(event.currentTarget.checked, coverStatus?.publishClaims ?? false)} />
              <span>Look up art automatically<small>Asks MusicBrainz and the Cover Art Archive about every album without a cover: the ones on this computer, and the albums the results pane has shown you</small></span>
            </label>
            <label class="cover-scan-toggle">
              <input type="checkbox" checked={coverStatus?.publishClaims ?? false} onchange={(event) => void setCoverPreferences(coverStatus?.lookupExternal ?? false, event.currentTarget.checked)} />
              <span>Sign and publish the covers I resolve<small>A kind <code>30427</code> claim signed with your own identity and sent to your relays, as fast as they can be signed</small></span>
            </label>
            {#if coverStatus?.running}
              <button class="classic-button" onclick={() => void stopCoverPass()}>Stop</button>
            {:else}
              <button class="classic-button" onclick={() => void lookForCoversNow()} disabled={!coverOptIn(coverStatus)}>Look now</button>
            {/if}
            <button class="classic-button" onclick={() => void refreshCovers()} disabled={coverLoading}>Refresh</button>
          </div>

          {#if coverError}<div class="trollbox-error">{coverError}</div>{/if}

          {#if coverStatus}
            <div class="cover-scan-status">
              <span><b>{coverStatus.running ? coverStatus.remaining : coverStatus.pending}</b> {coverStatus.running ? 'left in this pass' : 'in the last pass'}</span>
              <span><b>{coverStatus.published}</b> published</span>
              <span><b>{coverStatus.resolved}</b> resolved, not signed</span>
              <span><b>{coverStatus.alreadyCovered}</b> covered by others</span>
              <span><b>{coverStatus.noArt}</b> no art anywhere</span>
              {#if coverStatus.failed}<span><b>{coverStatus.failed}</b> failed</span>{/if}
              {#if coverStatus.backedOff}<span><b>{coverStatus.backedOff}</b> throttled waits</span>{/if}
            </div>
            {#if coverStatus.current}<p class="cover-scan-current">Working on {coverStatus.current}</p>{/if}
            {#if coverStatus.message}<p class="cover-scan-message">{coverStatus.message}</p>{/if}
          {/if}

          <div class="cover-candidate-list">
            {#each coverQueue as candidate (candidate.key)}
              <div class="cover-candidate">
                <div><b>{candidate.album}</b><small>{candidate.artist}</small></div>
                <span>{candidate.trackCount} {candidate.trackCount === 1 ? 'track' : 'tracks'} · {candidate.source}</span>
                <code title={candidate.key}>{candidate.key}</code>
                <button class="classic-button" onclick={() => void openCoverPicker(candidate.artist, candidate.album)}>Find art…</button>
                <button class="classic-button" title="Report the cover published for this album" onclick={() => openCoverReport(candidate.artist, candidate.album)}>Report</button>
              </div>
            {/each}
            {#if !coverLoading && coverQueue.length === 0}
              <p class="empty-state compact">{coverOptIn(coverStatus) ? 'Nothing is waiting. New music and new browsing wake Napstr by themselves.' : 'Switch one of these on and Napstr starts on its own.'}</p>
            {/if}
          </div>
        </section>
      {:else if activeView === 'Mobile'}
        <section class="full-panel mobile-connect-view">
          <div class="panel-title"><span></span><b>Mobile connect</b><span></span></div>
          <div class="mobile-connect-status">
            <span><i class:amber={!mobileStatusValue?.online} class:error={Boolean(mobileStatusValue?.error)} class="led"></i><b>{mobileStatusValue?.online ? 'Iroh ready' : mobileStatusValue?.running ? 'Iroh connecting…' : 'Iroh unavailable'}</b></span>
            <small>Napstr stays in control of discovery and Tor downloads.</small>
          </div>
          {#if mobileError}<div class="trollbox-error">{mobileError}</div>{/if}
          <div class="mobile-connect-grid">
            <section class="pair-phone-card">
              <h2>Pair Napstrfy</h2>
              <div class="pairing-tabs" role="tablist" aria-label="Pairing access">
                {#each [false, true] as streamOnly}
                  <button type="button" role="tab" id={`pairing-tab-${streamOnly ? 'stream' : 'full'}`} aria-controls={`pairing-panel-${streamOnly ? 'stream' : 'full'}`} aria-selected={mobileStreamOnly === streamOnly} tabindex={mobileStreamOnly === streamOnly ? 0 : -1} onclick={() => (mobileStreamOnly = streamOnly)} onkeydown={navigatePairingTabs}>{streamOnly ? 'uncle jim' : 'Full access'}</button>
                {/each}
              </div>
            {#each [false, true] as streamOnly}
              {@const offer = streamOnly ? mobileStreamPairing : mobilePairing}
              <div role="tabpanel" id={`pairing-panel-${streamOnly ? 'stream' : 'full'}`} aria-labelledby={`pairing-tab-${streamOnly ? 'stream' : 'full'}`} hidden={mobileStreamOnly !== streamOnly} tabindex="0">
                <p>{streamOnly ? 'Read-only, listen to and cache your local music and audiobooks. The connection cannot ask Napstr to download new songs.' : 'Browse, listen, save songs for offline listening, and ask Napstr to download tracks over Tor.'}</p>
                <p>Scan in <a href="https://napstr.net/napstrfy.html" onclick={openNapstrfyWebsite}>Napstrfy</a>. Keep Napstr open while streaming.</p>
                {#if offer}
                  <div class="pairing-qr" aria-label={streamOnly ? 'Read-only Napstrfy pairing QR code' : 'Full-access Napstrfy pairing QR code'}>{@html offer.qrSvg}</div>
                  <p class="pairing-expiry">One use · expires {new Date(offer.expiresAt * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</p>
                  <details><summary>Pair without a camera</summary><textarea readonly value={offer.ticket} aria-label={streamOnly ? 'Manual read-only pairing code' : 'Manual full-access pairing code'}></textarea></details>
                  <button class="classic-button" onclick={() => createMobilePairing(streamOnly)} disabled={mobileLoading}>{mobileLoading ? 'Preparing…' : 'Create a new code'}</button>
                {:else}
                  <button class="classic-button primary" onclick={() => createMobilePairing(streamOnly)} disabled={mobileLoading}>{mobileLoading ? 'Preparing Iroh…' : 'Create pairing code'}</button>
                  <div class="pairing-placeholder"><span>▦</span><b>Your one-use QR code will appear here</b></div>
                {/if}
              </div>
            {/each}
            </section>
            <section class="paired-devices-card">
              <p>Napstrfy creates a private, encrypted tunnel from your phone to Napstr, letting you listen to your catalogue by connecting directly to your Napstr instance. Only for your own use and for people you trust.</p>
              <h2>Paired phones</h2>
              <p>Each phone keeps the access granted by its pairing code. Scan a new code to change its access.</p>
              <div class="paired-device-list">
                {#each mobileStatusValue?.devices ?? [] as device (device.endpointId)}
                  <div class="paired-device">
                    <span class="phone-glyph">▯</span>
                    <div><b>{device.name}</b><small>{device.streamOnly ? 'Read only' : 'Full access'}</small><small>Last connected {mobileLastSeen(device.lastSeen)}</small><code title={device.endpointId}>{device.endpointId}</code></div>
                    <button class="classic-button" onclick={() => revokeMobileDevice(device)}>Remove</button>
                  </div>
                {/each}
                {#if (mobileStatusValue?.devices.length ?? 0) === 0}<p class="empty-state compact">No phones are paired yet.</p>{/if}
              </div>
              <p class="privacy-note wide"><span>i</span> The QR secret is random, expires after five minutes, and is invalidated by the first successful pairing. Removing a phone immediately revokes future connections.</p>
            </section>
          </div>
        </section>
      {:else if activeView === 'Profile'}
        <section class="full-panel profile-view">
          <div class="panel-title"><span></span><b>Napstr Profile</b><span></span></div>
          <div class="profile-card"><div class="avatar"><img src="/napstr-logo.png" alt="Napstr mascot" /></div><div><h2><button class="user-name" disabled={!identityNpub} onclick={() => browseUser(ownCatalogueUser())}>{displayName}</button></h2><p>Your dedicated Napstr Nostr identity.</p><code>{identityNpub || 'Connect to create identity'}</code><div class="profile-stats"><span><b>{sharedFiles.length}</b> shared files</span><span><b>{transfers.length}</b> transfers</span><span><b>{networkConnected ? 'Nostr online' : 'Offline'}</b></span></div></div></div>
          <fieldset class="edit-profile"><legend>Profile</legend><label>Display name <input bind:value={displayName} /></label><label>About <input bind:value={profileAbout} /></label><label>Picture URL <input bind:value={profilePicture} placeholder="https://…" /></label><button class="classic-button primary" onclick={persistSettings}>Save profile</button></fieldset>
          <p class="privacy-note wide"><span>i</span> Your profile and shared catalogue are public on Nostr. Transfer addresses and credentials are never published.</p>
        </section>
      {:else}
        <section class="full-panel settings-view">
          <div class="panel-title"><span></span><b>Napstr Settings</b><span></span></div>
          <fieldset><legend>Network</legend><label><input type="checkbox" checked disabled /> Connect automatically at startup</label><label>Nostr relays <input bind:value={nostrRelays} /></label><label>Tor <input value="Bundled, managed automatically" readonly /></label></fieldset>
          <fieldset><legend>Files</legend><label>Downloads and shared audio <input value={napstrFolder} readonly /><button class="classic-button" onclick={chooseNapstrFolder}>Browse…</button></label><label>Transfer mode <select disabled><option>Whole file</option></select></label><label><input type="checkbox" checked disabled /> Downloaded audio is automatically shared</label><label><input type="checkbox" checked disabled /> Verify the complete file with SHA-256</label></fieldset>
          <div class="settings-actions"><button class="classic-button primary" onclick={persistSettings}>OK</button><button class="classic-button" onclick={refreshSnapshot}>Cancel</button><button class="classic-button" onclick={persistSettings}>Apply</button></div>
        </section>
      {/if}
    </div>

    <section class="transfer-dock">
      <button
        type="button"
        class="dock-resizer"
        aria-label="Resize Transfer Manager"
        title="Drag to resize Transfer Manager · double-click to reset"
        onpointerdown={beginTransferResize}
        onkeydown={resizeTransferWithKeyboard}
        ondblclick={() => setTransferPaneHeight(window.innerHeight < 700 ? 94 : 119, true)}
      ></button>
      <div class="dock-title"><span></span><b>Transfer Manager</b><span></span><button class="dock-clear" onclick={clearFinishedTransfers} disabled={clearingTransfers || !transfers.some(isFinishedTransfer)}>Clear finished</button><button class="dock-clear" onclick={clearAllTransfers} disabled={clearingTransfers || removingTransfers.size > 0 || (!transfers.length && !audiobookDownloads.length && !startingDownloads.size)}>{clearingTransfers ? 'Clearing…' : 'Clear all'}</button><button onclick={() => (activeView = 'Downloads')} title="Open Download Manager">□</button></div>
      <div class="mini-transfers">
        {#each audiobookDownloads as book}
          <div class="mini-row audiobook-mini-row"><span class="audiobook-glyph">▥</span><span class="mini-name">{book.title} · chapter {Math.min(book.nextIndex + 1, book.chapters.length)} of {book.chapters.length}</span><div class="progress"><span style={`width:${book.chapters.length ? (book.nextIndex / book.chapters.length) * 100 : 0}%`}></span></div><span>{readableSize(book.chapters.reduce((sum, chapter) => sum + chapter.size, 0))}</span><span>Book</span></div>
        {/each}
        {#each transfers as transfer}
          <div class:transfer-complete={isCompleteTransfer(transfer)} class="mini-row">{#if isCompleteTransfer(transfer)}<button class="mini-play" onclick={() => playAudio(transfer.fileId, transfer.name, playerMode, 'downloads')} title="Play verified audio">▶</button>{:else}<span class="download-arrow">⇩</span>{/if}<span class="mini-name">{transfer.name}</span><div class="progress"><span style={`width:${transfer.progress}%`}></span></div><span>{transfer.size}</span><span>{isCompleteTransfer(transfer) ? 'Ready' : transfer.speed}</span><button class="tiny-button mini-cancel" onclick={() => removeTransfer(transfer.id)} disabled={clearingTransfers || removingTransfers.has(transfer.id)} aria-label={`${isActiveTransfer(transfer) ? 'Cancel download' : 'Clear entry'}: ${transfer.name}`} title={isActiveTransfer(transfer) ? 'Cancel download and remove partial file' : 'Clear entry; keep completed audio'}>×</button></div>
        {/each}
      </div>
    </section>

    <footer class="statusbar"><span>{activityMessage}</span><span><i class:amber={!networkConnected} class="led"></i> Nostr {networkConnected ? 'online' : 'offline'}</span><span title={torError}>♜ Tor: {torRunning ? 'ready' : torError ? 'failed' : torStarting && torProgress > 0 ? `${torProgress}%` : 'starting'}</span><span class="status-clock">{clock}</span></footer>
  </section>

  {#if aboutOpen}
    <div class="modal-backdrop" role="presentation" onclick={() => (aboutOpen = false)}>
      <dialog class="dialog" open aria-label="About Napstr" onclick={(e) => e.stopPropagation()} onkeydown={(e) => { if (e.key === 'Escape') aboutOpen = false; }}>
        <header class="titlebar"><div class="title-left"><span class="app-icon"><img src="/napstr-logo.png" alt="" /></span><span>About Napstr</span></div><div class="window-controls"><button onclick={() => (aboutOpen = false)}>×</button></div></header>
        <div class="dialog-body about-dialog-body">
          <div class="about-summary"><div class="about-logo"><img src="/napstr-logo.png" alt="" /></div><div><h2>Napstr</h2><p>Version {appVersion}</p><p>Public discovery over Nostr.<br />Private verified transfers over Tor.</p></div></div>
          <p class="about-donation">donations welcome!<br /><code>bc1qwgms685z3j69qtgalyjtrfuqg5f6pt302z0k60</code></p>
        </div>
        <div class="dialog-actions"><button class="classic-button primary" onclick={() => (aboutOpen = false)}>OK</button></div>
      </dialog>
    </div>
  {/if}

  {#if sourceProfile}
    <div class="modal-backdrop" role="presentation" onclick={() => (sourceProfile = null)}>
      <dialog class="dialog" open aria-label="Napstr public profile" onclick={(e) => e.stopPropagation()}>
        <header class="titlebar"><div class="title-left"><span class="app-icon"><img src="/napstr-logo.png" alt="" /></span><span>Public Napstr Profile</span></div><div class="window-controls"><button onclick={() => (sourceProfile = null)}>×</button></div></header>
        <div class="dialog-body"><div class="about-logo">☺</div><div><h2><button class="user-name" onclick={() => browseUser(sourceProfile!)}>{sourceProfile.displayName}</button></h2><p>{sourceProfile.about || 'No profile description published.'}</p><code>{sourceProfile.npub}</code></div></div>
        <div class="dialog-actions"><button class="classic-button primary" onclick={() => (sourceProfile = null)}>OK</button></div>
      </dialog>
    </div>
  {/if}

  {#if audiobookEditorOpen}
    <div class="modal-backdrop" role="presentation" onclick={() => { if (!audiobookSaving) audiobookEditorOpen = false; }}>
      <dialog class="dialog audiobook-dialog" open aria-label="Group folder as audiobook" onclick={(event) => event.stopPropagation()} onkeydown={(event) => { if (event.key === 'Escape' && !audiobookSaving) audiobookEditorOpen = false; }}>
        <header class="titlebar"><div class="title-left"><span class="app-icon">▥</span><span>Publish Audiobook</span></div><div class="window-controls"><button disabled={audiobookSaving} onclick={() => (audiobookEditorOpen = false)}>×</button></div></header>
        <div class="audiobook-dialog-body">
          <p>Napstr will publish this folder as one ordered audiobook while retaining its normal chapter file events.</p>
          <label>Title <input bind:value={audiobookTitle} maxlength="256" /></label>
          <label>Author <input bind:value={audiobookAuthor} maxlength="256" /></label>
          <label>Narrator <input bind:value={audiobookNarrator} maxlength="256" /></label>
          <fieldset><legend>Chapter order</legend><div class="audiobook-preview">{#each audiobookFolderFiles() as file, index}<div><span>{String(index + 1).padStart(2, '0')}</span><b>{file.title || file.filename}</b><small>{file.readableSize}</small></div>{/each}</div></fieldset>
          <p class="privacy-note"><span>i</span> The title, author, narrator, chapter names, and ordered file hashes will be public. Your folder name and filesystem path remain private.</p>
        </div>
        <div class="dialog-actions audiobook-dialog-actions">{#if currentFolderAudiobook()}<button class="classic-button" disabled={audiobookSaving} onclick={ungroupAudiobook}>Publish separately</button>{/if}<span></span><button class="classic-button primary" disabled={audiobookSaving || !audiobookTitle.trim()} onclick={saveAudiobookGroup}>{audiobookSaving ? 'Publishing…' : 'Save & publish'}</button><button class="classic-button" disabled={audiobookSaving} onclick={() => (audiobookEditorOpen = false)}>Cancel</button></div>
      </dialog>
    </div>
  {/if}

  {#if blockConfirmation}
    <div class="modal-backdrop" role="presentation" onclick={() => { if (!blockInProgress) blockConfirmation = null; }}>
      <dialog class="dialog confirm-dialog" open aria-label="Confirm block" onclick={(e) => e.stopPropagation()} onkeydown={(e) => { if (e.key === 'Escape' && !blockInProgress) blockConfirmation = null; }}>
        <header class="titlebar"><div class="title-left"><span class="app-icon">!</span><span>Confirm block</span></div><div class="window-controls"><button disabled={blockInProgress} onclick={() => (blockConfirmation = null)}>×</button></div></header>
        <div class="dialog-body"><div class="confirm-icon">!</div><div><h3>Are you sure?</h3>{#if blockConfirmation.kind === 'file'}<p>Block <strong>{blockConfirmation.label}</strong>?</p><p>Every seeder offering these exact file bytes will be hidden.</p>{:else}<p>Block <strong>{blockConfirmation.label}</strong>?</p><p>Their catalogue entries, public chat messages, and download requests will be ignored.</p>{/if}</div></div>
        <div class="dialog-actions"><button class="classic-button primary" disabled={blockInProgress} onclick={confirmBlock}>{blockInProgress ? 'Blocking…' : 'Block'}</button><button class="classic-button" disabled={blockInProgress} onclick={() => (blockConfirmation = null)}>Cancel</button></div>
      </dialog>
    </div>
  {/if}
</main>
{/if}
