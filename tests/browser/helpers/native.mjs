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
    // The computer's own key, which is what tells its playlists from public
    // ones somebody else published. A stranger's playlist is one seeded with
    // any other author.
    const desktopPubkey = () => window.playlistAuthor ?? 'c'.repeat(64);
    // The computers this phone holds, and whether each of them answered.
    //
    // The list is what a ping produced, so a computer that is asleep is one the
    // list says nothing about: `window.desktopReachable = false` is the older way
    // a spec says the acting computer is away, and `window.hostsOffline` names
    // individual ones - which is the whole point of a phone that holds two.
    const hostRows = () => window.remoteHosts ?? [];
    const homeRow = () => hostRows().find((host) => host.home) ?? hostRows()[0];
    const hostOnline = (host) =>
      window.desktopReachable !== false &&
      host.online !== false &&
      !(window.hostsOffline ?? []).includes(host.endpointId);
    const status = () => ({
      paired,
      // A phone can be paired and still not reach the computer, which is the
      // state a playlist has to survive: `window.desktopReachable = false` is
      // how a spec asks for it, and every `remote_*` call answers the way the
      // real channel would - with a failure.
      connected: paired && window.desktopReachable !== false && (!homeRow() || hostOnline(homeRow())),
      connecting: false,
      desktopName: homeRow()?.desktopName ?? 'Music computer',
      streamOnly: Boolean(window.streamOnly),
      // The rights beside the older flag. A spec that lends only some of them
      // names them: `window.mayDownload = true` with `window.streamOnly = true`
      // is the pairing that may fill itself with music and sign nothing.
      mayDownload: Boolean(window.mayDownload ?? !window.streamOnly),
      mayControl: Boolean(window.mayControl ?? !window.streamOnly),
      endpointId: 'endpoint',
      libraryRevision: 1,
      coverRevision: 0,
      pubkey: desktopPubkey(),
      error: ''
    });
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
    // The key a phone's own things are filed under is the phone's, not the
    // computer's, so the stand-in reads it from the identity it hands out.
    const phonePubkey = () => (window.nostrIdentity ?? { pubkey: 'c'.repeat(64) }).pubkey;
    // What a computer holds for this phone's key: file ids, kept the way the host
    // keeps them - 64 hex characters, once each, in the order given.
    const cleanedIds = (fileIds) => {
      const wanted = [];
      for (const fileId of fileIds ?? []) {
        const clean = String(fileId ?? '').trim().toLowerCase();
        if (clean.length === 64 && /^[0-9a-f]+$/.test(clean) && !wanted.includes(clean)) wanted.push(clean);
      }
      return wanted;
    };
    const hostLikes = () => window.hostLikes ?? [];
    const storeHostLikes = (fileIds) => {
      window.hostLikes = cleanedIds(fileIds);
      return window.hostLikes;
    };
    // The never-play list is a list of its own on the host, so it is one on the
    // stand-in too: a track can be liked and turned off at once, and neither list
    // may move the other.
    const hostDislikes = () => window.hostDislikes ?? [];
    const storeHostDislikes = (fileIds) => {
      window.hostDislikes = cleanedIds(fileIds);
      return window.hostDislikes;
    };
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
    // What the relays answer about this identity's own playlists, as the reader
    // reports it. Empty is "they were read, and have none of ours"; a test that
    // seeds rows models coordinates that exist there and not here, which is what
    // the Restore button is for. `window.playlistRelayReadFails` models a look
    // that never happened, which has to stay different from an empty answer.
    const playlistRelayOwn = () => window.playlistRelayOwn ?? [];
    // A withdrawal is the end of the coordinate everywhere, the relay answer
    // included: a playlist this computer just took back is not one to restore.
    const forgetRelayPlaylist = (playlistId) => {
      window.playlistRelayOwn = playlistRelayOwn().filter((row) => row.playlistId !== playlistId);
    };
    // A copy is a new playlist of this identity's, so the host mints the id: the
    // mock does the same rather than filing the copy under somebody else's.
    const filePlaylistRevision = (playlist, extra = {}) => {
      const foreign = Boolean(playlist.author) && playlist.author !== ownPlaylistAuthor();
      return savePlaylistRow({
        ...playlist,
        playlistId: foreign ? window.nextCopyPlaylistId ?? '33333333-3333-4333-8333-333333333333' : playlist.playlistId,
        author: ownPlaylistAuthor(),
        published: foreign ? false : (extra.published ?? playlist.published ?? false),
        updatedAt: 1787000000,
        ...extra
      });
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
      // The asset protocol, which is how a window draws a picture this computer
      // already holds. The shape is the one the real side produces on Windows.
      convertFileSrc: (path, protocol = 'asset') =>
        `http://${protocol}.localhost/${encodeURIComponent(path)}`,
      invoke: async (cmd, args = {}) => {
        window.calls.push({ cmd, args });
        // An unreachable computer fails every request to it, exactly as the
        // phone's own channel does - and the phone's local commands keep working,
        // which is the whole point of the state.
        if (window.desktopReachable === false && cmd.startsWith('remote_')) {
          throw 'Could not reach Napstr';
        }
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
          // What the host counts about the rows on screen: people, not messages,
          // and only for files a test has said anything about. A file it has no row
          // for is simply not in the answer, which is how a quiet file looks.
          case 'track_discussion_activity': {
            const known = window.discussionActivity ?? {};
            return (args.fileIds ?? []).map((fileId) => known[fileId]).filter(Boolean);
          }
          case 'mobile_status': return { running: true, online: true, endpointId: 'endpoint', error: '', devices: window.mobileDevices ?? [] };
          case 'remote_hosts': return hostRows().map((host) => ({ ...host, online: hostOnline(host) }));
          case 'remote_file_hosts': return window.fileHosts ?? {};
          case 'set_mobile_home_host': {
            // Exactly one computer is home, and the list the window reads back is
            // the answer rather than its own optimism about the write.
            const hosts = hostRows();
            if (!hosts.some((host) => host.endpointId === args.endpointId)) {
              throw new Error('That computer is not paired with this phone');
            }
            for (const host of hosts) host.home = host.endpointId === args.endpointId;
            return null;
          }
          case 'set_mobile_host_included': {
            // The phone decides nothing here: the flag is written down where it
            // is read from, and the list the window reads back is the answer.
            const row = (window.remoteHosts ?? []).find((host) => host.endpointId === args.endpointId);
            if (!row) throw new Error('That computer is not paired with this phone');
            row.included = args.included;
            return null;
          }
          case 'set_mobile_device_rights': {
            // The host decides, so the mock applies the write and the list the
            // window reads back is the answer rather than its own optimism.
            const row = (window.mobileDevices ?? []).find((entry) => entry.endpointId === args.endpointId);
            if (!row) throw new Error('That device is not paired with Napstr');
            row.rights = args.rights;
            return null;
          }
          case 'play_audio': return { fileId: track.fileId, currentTime: 0, duration: 60, playing: true, ended: false, error: '' };
          case 'client_platform': return platform;
          case 'companion_status': return status();
          // This phone's own key. The companion makes one on first launch and it
          // is always there, so the host every other spec talks to has one too -
          // and a spec that wants a particular key sets `window.nostrIdentity`.
          case 'nostr_identity':
            return window.nostrIdentity ?? { pubkey: 'c'.repeat(64), npub: `npub1${'c'.repeat(58)}` };
          case 'export_nostr_identity':
            return window.nostrSecret ?? `nsec1${'c'.repeat(58)}`;
          case 'import_nostr_identity': {
            // The companion refuses anything that is not a key rather than
            // adopting it, so the stand-in does too: what a spec sees is the
            // answer, not its own optimism.
            const secret = String(args.secret ?? '').trim();
            if (!/^(nsec1[0-9a-z]+|[0-9a-f]{64})$/.test(secret)) {
              throw 'That is not a Nostr secret key: paste an nsec1… value or 64 hex characters';
            }
            window.nostrSecret = secret;
            window.nostrIdentity = { pubkey: 'd'.repeat(64), npub: `npub1${'d'.repeat(58)}` };
            return window.nostrIdentity;
          }
          case 'cached_library': {
            // What this phone holds itself, which is what a playlist played
            // offline is queued from.
            const rows = window.cachedLibrary ?? (paired ? [track] : []);
            return { ...status(), tracks: rows, total: rows.length };
          }
          case 'remote_library': {
            // The whole library unless a page is asked for, which is what the
            // add sheet's list does as it is scrolled. A source is one computer's
            // library; no source is every computer's, as one list, each file once
            // - which is what the phone now asks for and what the host unions.
            //
            // Only the computers that answered are in it, the acting one
            // included: a question asked of every computer is answered by the
            // ones that are here, and the rows of one that is asleep are not
            // invented for it. `window.remoteLibrary` is the acting computer's
            // own library, which is what the fixture convention means by it.
            const hosts = hostRows().filter((host) => host.included !== false);
            const home = homeRow();
            const named = args.source ? window.remoteLibraryByHost?.[args.source] : null;
            const actedFor = !home || hostOnline(home) ? (window.remoteLibrary ?? [track]) : [];
            const union = named ?? [
              ...actedFor,
              ...hosts.filter(hostOnline).flatMap((host) => window.remoteLibraryByHost?.[host.endpointId] ?? [])
            ];
            const seen = new Set();
            const rows = union.filter((row) => !seen.has(row.fileId) && seen.add(row.fileId));
            const offset = Number(args.offset ?? 0);
            const limit = Number(args.limit ?? rows.length);
            return { tracks: rows.slice(offset, offset + limit), total: rows.length };
          }
          case 'remote_search': if (window.searchError) throw window.searchError; return (args.source && window.remoteLibraryByHost?.[args.source]) ?? window.networkSearchResults ?? [track];
          // What the computer's own mirror says is live on the network. Empty
          // unless a spec seeds it, because it is a suggestion and not a page.
          //
          // The two modes are the two ends of how many people are keeping each
          // file alive, and the computer is the side that ranks them - so the
          // stand-in ranks by the same field the host does. Rows a spec gives no
          // seeder count keep the order it gave them.
          case 'remote_discover': {
            const found = window.discoverTracks ?? [];
            const ranked =
              args.mode === 'leastSeeded'
                ? [...found].sort((left, right) => (left.seeders ?? 0) - (right.seeders ?? 0))
                : [...found];
            const offset = Number(args.offset ?? 0);
            const limit = Number(args.limit ?? ranked.length);
            return { tracks: ranked.slice(offset, offset + limit), total: ranked.length };
          }
          // The conversation around a track: a test seeds what has been said, and
          // a send appends to the same list, so the page asks again and finds it.
          case 'remote_track_discussion': return args.before ? [] : window.discussionMessages ?? [];
          case 'remote_send_track_discussion': {
            const said = window.discussionMessages ?? [];
            const parent = said.find((message) => message.eventId === args.replyTo);
            const sent = {
              eventId: `sent-${said.length}`,
              pubkey: 'c'.repeat(64),
              npub: 'npub1me',
              displayName: 'Me',
              content: args.content,
              createdAt: 1_800_000_100,
              // A reply answers one message and carries its opening line, which is
              // what the host resolves from its own cache.
              ...(args.replyTo ? { replyTo: args.replyTo } : {}),
              ...(parent ? { reply: { author: parent.displayName, excerpt: parent.content } } : {})
            };
            window.discussionMessages = [...said, sent];
            return sent.eventId;
          }
          case 'remote_track_discussion_activity': {
            const known = window.discussionActivity ?? {};
            return args.fileIds.map((fileId) => known[fileId]).filter(Boolean);
          }
          // What a phone lent the network but not the signature says: the same
          // comment, signed by the phone's own key instead of by this computer's.
          // The event is the phone's, so it is authored by the phone.
          case 'remote_send_device_discussion': {
            const said = window.discussionMessages ?? [];
            const parent = said.find((message) => message.eventId === args.replyTo);
            const sent = {
              eventId: `device-${said.length}`,
              pubkey: phonePubkey(),
              npub: 'npub1device',
              displayName: 'This phone',
              content: args.content,
              createdAt: 1_800_000_200,
              ...(args.replyTo ? { replyTo: args.replyTo } : {}),
              ...(parent ? { reply: { author: parent.displayName, excerpt: parent.content } } : {})
            };
            window.discussionMessages = [...said, sent];
            return sent.eventId;
          }
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
          // The relays, as the desktop's reader reports them. Both of the two
          // desktop calls answer from the same seeded state, so a test can tell
          // "the relays said none of yours" apart from "nobody looked".
          case 'read_playlists': {
            if (window.playlistRelayReadFails) throw 'playlist discovery failed: no relay answered';
            return { stored: 0, withdrawn: 0, own: playlistRelayOwn() };
          }
          case 'playlist_reconciliation':
            return window.playlistRelayReadFails ? null : playlistRelayOwn();
          case 'restore_playlist': {
            const found = playlistRelayOwn().find((known) => known.playlistId === args.playlistId);
            if (!found) throw 'The relays do not have that playlist';
            return savePlaylistRow({ ...found, author: ownPlaylistAuthor(), published: true });
          }
          case 'playlist_member_availability': {
            const counts = window.playlistMemberAvailability ?? [];
            return counts.filter((entry) => args.fileIds.includes(entry.fileId));
          }
          // The phone reads and writes the same store the desktop page does, one
          // command at a time; only the shapes differ.
          case 'remote_new_playlist_id': return window.nextPlaylistId ?? '11111111-1111-4111-8111-111111111111';
          case 'remote_playlists': {
            // `ownOnly` is the picker's question: the playlists filed under this
            // phone's own key, because neither a public playlist somebody else
            // published nor the computer's own is a list this phone may add a
            // track to - editing one of those makes a copy.
            const rows = args.ownOnly
              ? playlistSummaries().filter((row) => !row.author || row.author === phonePubkey())
              : playlistSummaries();
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
            // A phone's save is filed under the phone's own key, which is what
            // makes two phones' playlists two separate sets.
            return savePlaylistRow({ ...args.playlist, author: phonePubkey(), updatedAt: 1787000000 });
          case 'remote_likes': return hostLikes();
          case 'remote_set_likes': return storeHostLikes(args.fileIds);
          case 'remote_dislikes': return hostDislikes();
          case 'remote_set_dislikes': return storeHostDislikes(args.fileIds);
          case 'carry_own_data': {
            // The computer being left is the one thing that can make this fail,
            // and the failure has to be visible rather than silent: nothing is
            // lost, but a page that said "moved" would be lying.
            if (window.carryFails) throw 'the computer being left is not answering';
            const moved = storeHostLikes([...hostLikes(), ...(args.likes ?? [])]);
            const turnedOff = storeHostDislikes([...hostDislikes(), ...(args.dislikes ?? [])]);
            const mine = playlistSummaries().filter((row) => !row.author || row.author === phonePubkey());
            window.carriedTo = args.to;
            window.carriedFrom = args.from;
            return { likes: moved.length, dislikes: turnedOff.length, playlists: mine.length, skipped: 0, failed: [] };
          }
          case 'remote_publish_playlist':
            return filePlaylistRevision(args.playlist, { published: true });
          // Publishing without the owner's signature: the phone signed the event
          // itself, so what is filed is the list it signed.
          case 'remote_publish_device_playlist':
            return filePlaylistRevision(args.playlist, { published: true });
          case 'remote_delete_playlist': dropPlaylistRow(args.author, args.playlistId); return null;
          case 'remote_withdraw_playlist': dropPlaylistRow('', args.playlistId); forgetRelayPlaylist(args.playlistId); return null;
          case 'playlists': return playlistSummaries();
          case 'playlist': return findPlaylist(args.author, args.playlistId);
          // Both desktop writes stamp the author the way the host does, so a page
          // under test sees the coordinate it will really get back - including a
          // copy filed under an id of its own when the revision is not this
          // identity's playlist.
          case 'save_playlist':
            return filePlaylistRevision(args.playlist);
          case 'publish_playlist':
            return filePlaylistRevision(args.playlist, { published: true });
          case 'withdraw_playlist': dropPlaylistRow('', args.playlistId); forgetRelayPlaylist(args.playlistId); return null;
          case 'delete_playlist': dropPlaylistRow(args.author, args.playlistId); return null;
          case 'podcast_downloads': return [{ episode, ready: false, status: 'Downloading', progress: 20 }];
          case 'podcast_parse_search': return [{ id: 1, title: 'Original podcast', author: 'Original author', feedUrl: 'https://example.com/feed', image: '', description: '', language: 'en', episodeCount: 1, genres: ['Music'] }];
          case 'podcast_episodes': return [episode];
          case 'pair_desktop': {
            // Pairing a second computer adds it rather than replacing the first,
            // which is what the phone's Settings list reads back. A test can name
            // the computer the next code belongs to.
            paired = true;
            const name = window.pairedComputerName ?? 'Music computer';
            const id = window.pairedComputerId ?? 'paired-computer';
            const held = (window.remoteHosts ?? []).filter((host) => host.endpointId !== id);
            held.push({
              endpointId: id,
              desktopName: name,
              rights: { browse: true, fetch: true, control: true, download: true, privileged: true },
              home: held.length === 0,
              included: true,
              online: true,
              mayDownload: true
            });
            window.remoteHosts = held;
            return name;
          }
          case 'forget_desktop': paired = false; window.remoteHosts = []; return;
          case 'forget_mobile_host': {
            const held = window.remoteHosts ?? [];
            if (!held.some((host) => host.endpointId === args.endpointId)) {
              throw new Error('That computer is not paired with this phone');
            }
            window.remoteHosts = held.filter((host) => host.endpointId !== args.endpointId);
            // A phone left holding nothing is unpaired, exactly as the computer
            // it forgot would leave it.
            if (!window.remoteHosts.length) paired = false;
            return null;
          }
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
