export async function mockNative(page, { app = 'napstrfy', nativeLocale = 'en-GB', saved, paired = true, blockedStorage = false, platform = 'linux', remote = false } = {}) {
  await page.route('https://**', (route) => route.fulfill({ contentType: 'application/json', body: '{"results":[]}' }));
  await page.addInitScript(({ app, nativeLocale, saved, paired, blockedStorage, platform, remote }) => {
    if (saved) localStorage.setItem('napstr-language', saved);
    if (blockedStorage) {
      Storage.prototype.getItem = () => { throw new DOMException('Disabled', 'SecurityError'); };
      Storage.prototype.setItem = () => { throw new DOMException('Disabled', 'SecurityError'); };
    }
    window.calls = [];
    window.nativeLocale = nativeLocale;
    const track = { fileId: 'a'.repeat(64), filename: 'Search.wav', title: 'Search', artist: 'Settings', album: 'User album', folder: '', path: '/music/Search.wav', format: 'WAV', mime: 'audio/wav', size: 1234567, tags: 'Rock', local: !remote, sources: [], license: '', description: '', status: 'Shared' };
    const episode = { id: 'episode', title: 'Original episode', feedTitle: 'Original podcast', audioUrl: 'https://example.com/episode.mp3', datePublished: 1700000000, duration: 100, description: '', image: '', mime: 'audio/mpeg' };
    const status = () => ({ paired, connected: paired, desktopName: 'Music computer', streamOnly: false, endpointId: 'endpoint', libraryRevision: 1, error: '' });
    const transfers = [{ id: 1, fileId: track.fileId, filename: track.filename, size: track.size, progress: 100, status: 'Verified · Complete', speed: '', destination: '/music/Search.wav' }, { id: 2, fileId: 'b'.repeat(64), filename: 'Downloading.wav', size: 100, progress: 12, status: 'Downloading', speed: '', destination: '' }];
    // The computer's player, idle until a test says otherwise.
    const idleRemote = () => ({
      active: false, playing: false, fileId: '', title: '', artist: '', album: '',
      positionMs: 0, durationMs: 0, volume: 0.85, queueLen: 0, queueIndex: -1,
      track: null, queue: [], repeat: 'off', shuffle: false, remoteControl: false,
      error: '', updatedAt: 0
    });
    const remoteNow = () => window.remotePlaying ?? idleRemote();
    const startRemote = (fileId, queue) => {
      const known = window.remoteLibrary ?? [track];
      const found = known.find((item) => item.fileId === fileId) ?? { ...track, fileId };
      window.remotePlaying = {
        ...remoteNow(),
        active: true, playing: true, fileId,
        title: found.title, artist: found.artist, album: found.album,
        track: found, queue,
        queueLen: queue.length,
        queueIndex: Math.max(0, queue.indexOf(fileId))
      };
      return window.remotePlaying;
    };
    // The desktop's playlists, as the host stores them: rows a test seeds, which
    // the page then lists, edits and deletes through the same commands the real
    // host implements.
    const playlistRows = () => window.playlistStore ?? [];
    const ownPlaylistAuthor = () => window.playlistAuthor ?? 'c'.repeat(64);
    const findPlaylist = (author, playlistId) =>
      playlistRows().find((row) => row.playlistId === playlistId && (!author || row.author === author)) ?? null;
    const savePlaylistRow = (row) => {
      window.playlistStore = [
        ...playlistRows().filter((existing) => !(existing.playlistId === row.playlistId && existing.author === row.author)),
        row
      ];
      return row;
    };
    const dropPlaylistRow = (author, playlistId) => {
      window.playlistStore = playlistRows().filter(
        (row) => !(row.playlistId === playlistId && (!author || row.author === author))
      );
    };
    const playlistSummaries = () =>
      playlistRows().map((row) => ({
        playlistId: row.playlistId,
        title: row.title,
        author: row.author,
        displayName: row.displayName ?? '',
        image: row.image ?? '',
        // The row's artwork comes from the member the playlist opens with.
        firstFileId: (row.tracks ?? [])[0]?.fileId ?? '',
        trackCount: row.tracks?.length ?? 0,
        private: Boolean(row.private),
        published: Boolean(row.published),
        updatedAt: row.updatedAt ?? 0
      }));
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
      transformCallback: () => 1,
      unregisterCallback: () => {},
      invoke: async (cmd, args = {}) => {
        window.calls.push({ cmd, args });
        switch (cmd) {
          case 'plugin:os|locale': return window.nativeLocale;
          case 'plugin:app|version': return '0.2.2';
          case 'plugin:event|listen': return 1;
          case 'plugin:event|unlisten': return;
          case 'get_snapshot':
          case 'save_settings': return { native: true, indexedBytes: track.size, files: [track], audiobooks: [], transfers, settings: { napstrFolder: '/music', nostrRelays: '', displayName: 'Original user', profileAbout: 'Original profile', profilePicture: '' } };
          case 'get_transfers': return transfers;
          case 'start_network':
          case 'network_status': return { connected: true, pubkey: 'c'.repeat(64), npub: 'npub1test', relayCount: 1, torRunning: true, torStarting: false, torProgress: 100, torError: '', error: '' };
          case 'network_browse': return { results: [track], cursor: null, totalAvailable: 1 };
          // Both searches answer with the same one local track unless a test
          // seeds its own rows, which is how a spec tells "here" from "the
          // network" without inventing a second fixture track everywhere.
          case 'network_search': return window.networkSearchResults ?? [track];
          case 'search_catalog': return window.localSearchResults ?? [track];
          case 'network_search_audiobooks': return [];
          case 'get_track_discussion_messages':
          case 'get_trollbox_messages': return [{ eventId: 'event', content: 'Search', displayName: 'Settings', npub: 'npubother', pubkey: 'd'.repeat(64), createdAt: 1700000000 }];
          case 'mobile_status': return { running: true, online: true, endpointId: 'endpoint', error: '', devices: [] };
          case 'play_audio': return { fileId: track.fileId, currentTime: 0, duration: 60, playing: true, ended: false, error: '' };
          case 'client_platform': return platform;
          case 'companion_status': return status();
          case 'cached_library': return { ...status(), tracks: paired ? [track] : [], total: paired ? 1 : 0 };
          case 'remote_library': return { tracks: window.remoteLibrary ?? [track], total: (window.remoteLibrary ?? [track]).length };
          case 'remote_search': if (window.searchError) throw window.searchError; return window.networkSearchResults ?? [track];
          case 'remote_transfers': return [{ ...transfers[1], fileId: track.fileId }];
          case 'reconcile_audio_cache': return true;
          // The native side draws the track code, so answer the way it does.
          case 'track_code': return '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 4 4"><rect width="4" height="4" fill="#ffffff"/><path fill="#000000" d="M0 0h1v1H0z"/></svg>';
          case 'cache_remote_audio': return { url: location.origin + (window.nextMediaSource || '/fixture.wav'), track: args.track };
          case 'podcast_playback_url': return { url: location.origin + '/fixture.wav', downloaded: false };
          case 'prefetch_remote_audio': return;
          case 'remote_audiobook_library': return { audiobooks: [], total: 0 };
          // The computer's own player. A test seeds `window.remotePlaying` to
          // describe what it is doing; a handoff stops it, and a play command
          // starts it on the track it was asked for, so a test can watch both
          // sides of a handover rather than only the request that was sent.
          case 'remote_playback_state': return remoteNow();
          case 'remote_playback': {
            const command = args.command ?? {};
            if (command.type === 'handoff') {
              const state = remoteNow();
              // It stops as it answers: asking again must not find it still
              // playing, or a phone could take the same track over twice.
              window.remotePlaying = { ...state, playing: false };
              return state;
            }
            if (command.type === 'playTrack') return startRemote(command.fileId, command.queue ?? []);
            if (command.type === 'volume') return (window.remotePlaying = { ...remoteNow(), volume: command.percent / 100 });
            if (command.type === 'stop' || command.type === 'pause') return (window.remotePlaying = { ...remoteNow(), playing: false });
            if (command.type === 'play' || command.type === 'toggle') return (window.remotePlaying = { ...remoteNow(), playing: true });
            return remoteNow();
          }
          case 'remote_library_by_ids': {
            const known = window.remoteLibrary ?? [track];
            return args.fileIds
              .map((fileId) => known.find((item) => item.fileId === fileId))
              .filter(Boolean);
          }
          case 'remote_download': return 'request-1';
          case 'own_playlist_author': return ownPlaylistAuthor();
          case 'new_playlist_id': return window.nextPlaylistId ?? '11111111-1111-4111-8111-111111111111';
          // The phone reads and writes the same store the desktop page does, one
          // command at a time; only the shapes differ.
          case 'remote_new_playlist_id': return window.nextPlaylistId ?? '11111111-1111-4111-8111-111111111111';
          case 'remote_playlists': {
            const rows = playlistSummaries();
            return { playlists: rows, total: rows.length };
          }
          case 'remote_playlist': {
            const row = window.remotePlaylist ?? findPlaylist(args.author, args.playlistId);
            if (!row) throw 'That playlist is not on this computer';
            const tracks = row.tracks ?? [];
            const offset = Number(args.offset ?? 0);
            const limit = Number(args.limit ?? 100);
            return { ...row, tracks: tracks.slice(offset, offset + limit), total: tracks.length };
          }
          // Which playlists name a file, answered from the same rows: one lookup
          // rather than a page of members per playlist, the way the host does it.
          case 'remote_playlists_containing':
            return playlistRows()
              .filter((row) => (row.tracks ?? []).some((member) => member.fileId === args.fileId))
              .map((row) => ({ author: row.author ?? ownPlaylistAuthor(), playlistId: row.playlistId }));
          case 'remote_save_playlist':
            return savePlaylistRow({
              ...args.playlist,
              author: ownPlaylistAuthor(),
              updatedAt: 1787000000
            });
          case 'remote_publish_playlist':
            return savePlaylistRow({
              ...args.playlist,
              author: ownPlaylistAuthor(),
              published: true,
              updatedAt: 1787000000
            });
          case 'remote_delete_playlist': dropPlaylistRow(args.author, args.playlistId); return null;
          case 'remote_withdraw_playlist': dropPlaylistRow('', args.playlistId); return null;
          case 'playlists': return playlistSummaries();
          case 'playlist': return findPlaylist(args.author, args.playlistId);
          // Both desktop writes stamp the author the way the host does, so a page
          // under test sees the coordinate it will really get back.
          case 'save_playlist':
            return savePlaylistRow({ ...args.playlist, author: ownPlaylistAuthor(), updatedAt: 1787000000 });
          case 'publish_playlist':
            return savePlaylistRow({
              ...args.playlist,
              author: ownPlaylistAuthor(),
              published: true,
              updatedAt: 1787000000
            });
          case 'withdraw_playlist': dropPlaylistRow('', args.playlistId); return null;
          case 'delete_playlist': dropPlaylistRow(args.author, args.playlistId); return null;
          case 'podcast_downloads': return [{ episode, ready: false, status: 'Downloading', progress: 20 }];
          case 'podcast_parse_search': return [{ id: 1, title: 'Original podcast', author: 'Original author', feedUrl: 'https://example.com/feed', image: '', description: '', language: 'en', episodeCount: 1, genres: ['Music'] }];
          case 'podcast_episodes': return [episode];
          case 'pair_desktop': paired = true; return 'Music computer';
          case 'forget_desktop': paired = false; return;
          default: return null;
        }
      }
    };
  }, { app, nativeLocale, saved, paired, blockedStorage, platform, remote });
}

export function silentAudio() {
  const wav = Buffer.alloc(44 + 8000 * 2 * 60);
  wav.write('RIFF'); wav.writeUInt32LE(wav.length - 8, 4); wav.write('WAVEfmt ', 8);
  wav.writeUInt32LE(16, 16); wav.writeUInt16LE(1, 20); wav.writeUInt16LE(1, 22);
  wav.writeUInt32LE(8000, 24); wav.writeUInt32LE(16000, 28); wav.writeUInt16LE(2, 32); wav.writeUInt16LE(16, 34);
  wav.write('data', 36); wav.writeUInt32LE(wav.length - 44, 40);
  return wav;
}

export async function serveAudio(route) {
  const wav = silentAudio();
  const range = route.request().headers().range?.match(/^bytes=(\d+)-(\d*)$/);
  const headers = { 'Accept-Ranges': 'bytes' };
  if (range) {
    const start = Number(range[1]);
    const end = range[2] ? Math.min(Number(range[2]), wav.length - 1) : wav.length - 1;
    headers['Content-Range'] = `bytes ${start}-${end}/${wav.length}`;
    await route.fulfill({ status: 206, contentType: 'audio/wav', headers, body: wav.subarray(start, end + 1) });
  } else await route.fulfill({ contentType: 'audio/wav', headers, body: wav });
}
