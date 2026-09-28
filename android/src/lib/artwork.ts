import { invoke } from '@tauri-apps/api/core';
import { writable } from 'svelte/store';
import { recordCoverEvent } from './coverDebug';
import type { RemoteTrack } from './types';

/**
 * Album artwork, which this phone draws from its own disk.
 *
 * The paired host resolves the claims - it owns the relay pool, the catalogue,
 * and the availability heartbeats that decide which one wins - and it is also
 * where the pixels come from. Before this, the host answered with an address and
 * this phone fetched the picture from a publisher, which told that publisher the
 * phone's address and the album it was looking at, on a connection its owner
 * paid for. Now the picture travels over the pairing, and the address it is drawn
 * from is this phone's own.
 *
 * What that needs is a name for each picture, and the name is a hash. The host
 * says which hash answers an album; this phone asks for the bytes and keeps them
 * under that name; the address it draws from is minted from the hash. A picture
 * already here therefore costs nothing, and a picture that changes is a
 * different name rather than a stale file.
 *
 * A host built before any of this names no hashes, and the answer here is then no
 * art rather than a fetch from the address it does send. That is the point of the
 * change rather than a gap in it: falling back to the address would restore
 * exactly the behaviour this exists to end, quietly, on the machines least likely
 * to be looked at.
 */
export type AlbumCover = {
  key: string;
  /** Local address of the full picture, empty while this phone holds none. */
  art: string;
  /** Local address of the smaller rendition, empty while this phone holds none. */
  thumb: string;
  /** Hash of the full picture the host can serve, empty when it has none. */
  artHash: string;
  /** Hash of the smaller rendition the host can serve, empty when it has none. */
  thumbHash: string;
  mbid: string;
  year: string;
  genre: string;
  collection: string;
  /** Provenance hint from the publisher, such as `itunes`, `musicbrainz`, or `embedded`. */
  source: string;
  coverFileId: string;
  mime: string;
  author: string;
  seeder: boolean;
};

/**
 * What the host said about one album, as far as this phone needs it.
 *
 * `artHash` and `thumbHash` are what it can serve now. A host that knows of a
 * picture it has not fetched yet answers with an address and no hash, which is
 * not the same as having no art: that difference is why this phone keeps the copy
 * it already holds instead of throwing it away and drawing a placeholder.
 */
type HostCover = Omit<AlbumCover, 'art' | 'thumb'>;

/** What this phone has confirmed it holds, by album: the hash of each picture. */
type Held = { full: string; thumb: string };

/** What is remembered about an album between launches. */
type StoredCover = { at: number; cover: HostCover; held: Held };

/**
 * What an answer looks like coming off the wire: the host's own addresses beside
 * the hashes.
 *
 * They are read once, to tell an album the host knows of a picture for from one
 * it has nothing for, and then dropped. Nothing here is ever drawn from, and
 * nothing here is written to disk — an address that outlives the answer it came
 * in is exactly what this phone stopped fetching from.
 */
type WireCover = HostCover & { art: string; thumb: string };

/** The answer to one ask for one rendition. */
type AlbumArtwork = { url: string; hash: string };

/**
 * The namespace covers are remembered in.
 *
 * Bumped when what is remembered changes shape. The version that held publisher
 * addresses had to go rather than be migrated: an address stored here is exactly
 * the thing this phone must never draw from again, and a build that read one back
 * would go on fetching from a publisher quietly.
 */
const CACHE_PREFIX = 'napstrfy-cover:v3:';
/** Namespaces from before this one, swept so they cannot linger forever. */
const LEGACY_CACHE_PREFIXES = ['napstrfy-cover:v2:', 'napstrfy-artwork:'];
/** Keys per companion call. Mirrors `MAX_COVER_KEYS * 4` in the phone crate. */
const INVOKE_KEY_LIMIT = 160;
/**
 * How long stored artwork is trusted before it is asked for again.
 *
 * Publishers replace their cover claim, and image URLs rot, so a cover that
 * answered once is not permanent. Asking is a local call to the host.
 */
const STORED_COVER_TTL_MS = 7 * 24 * 60 * 60 * 1000;
/**
 * How long "the host has no cover for this" is trusted on its own.
 *
 * The host reports a revision whenever its art changes, which is the accurate
 * signal; this is what still works against a host too old to report one.
 */
const NEGATIVE_TTL_MS = 60 * 1000;
/**
 * How long a tile waits before asking again after this album had nothing to
 * draw, or the image the host named would not load.
 */
export const ARTWORK_RETRY_MS = 60 * 1000;
/** A page composes in stages: gather its keys briefly before asking. */
const BATCH_DELAY_MS = 40;

/**
 * Bumped when there is a cached "no cover" worth asking about again, so
 * artwork already on screen asks instead of waiting for a re-render. Read as
 * `$coverRevision` by the components that draw art.
 */
export const coverRevision = writable(0);
/** The host revision this phone has already acted on. */
let appliedCoverRevision = 0;

type Waiter = (cover: HostCover | null) => void;

/**
 * Covers resolved while the app is open, including albums the host has no cover
 * for. Absence is deliberately not persisted: the host re-checks relays on its
 * own schedule and answering again costs one batched call, so carrying a "no"
 * across launches only ever delays artwork that has since been published.
 */
const sessionCovers = new Map<string, HostCover | null>();
/**
 * When the host last gave an answer that is worth revising: that it has no art
 * for an album, or that it has one it cannot serve yet. Neither is permanent —
 * the host is looking albums up as they are shown here, and fetching the
 * pictures it finds — so an answer that has stood for `NEGATIVE_TTL_MS` is asked
 * about again rather than kept.
 */
const negativeAt = new Map<string, number>();
/**
 * What this phone holds, by album. A hash here is a promise that the bytes are
 * on disk under that name, and it is what keeps a picture this phone has already
 * fetched when the host's own copy has since gone.
 */
const heldArt = new Map<string, Held>();
const pending = new Map<string, Waiter[]>();
let flushHandle: number | null = null;

/**
 * Whether the host can serve this album's picture yet. An answer that can serve
 * nothing is asked about again shortly, which is how art that arrives on the
 * host a moment later reaches a screen that asked too early.
 */
function servable(cover: HostCover): boolean {
  return Boolean(cover.thumbHash || cover.artHash);
}

function heldFor(key: string): Held {
  return heldArt.get(key) ?? { full: '', thumb: '' };
}

/** What this phone can draw for an album, by hash, best rendition first. */
function heldHashes(key: string): string[] {
  const held = heldFor(key);
  return [held.thumb, held.full].filter(Boolean);
}

/**
 * `undefined` means nothing is known yet; `null` means the host already answered
 * and has no art for this album. The two must stay distinct: treating
 * "unknown" as "no art" resolves the answer without ever asking for it.
 */
function cachedCover(key: string): HostCover | null | undefined {
  const known = sessionCovers.get(key);
  if (known !== undefined) {
    if ((known !== null && servable(known)) || !expiredNegative(key)) return known;
    // Falling through asks again, which is the point of an expiry.
  }
  const stored = readStoredCover(key);
  if (stored === undefined) return undefined;
  sessionCovers.set(key, stored);
  return stored;
}

/** Forget an answer that has been trusted for long enough. */
function expiredNegative(key: string): boolean {
  const asked = negativeAt.get(key) ?? 0;
  if (Date.now() - asked <= NEGATIVE_TTL_MS) return false;
  sessionCovers.delete(key);
  negativeAt.delete(key);
  return true;
}

function readStoredCover(key: string): HostCover | undefined {
  try {
    const raw = window.localStorage.getItem(CACHE_PREFIX + key);
    if (!raw) return undefined;
    const stored = JSON.parse(raw) as StoredCover;
    if (!stored.cover) {
      // An earlier build could store a "no cover" verdict. Drop it rather than
      // let a stale no keep suppressing the question.
      dropStoredCover(key);
      return undefined;
    }
    if (Date.now() - stored.at > STORED_COVER_TTL_MS) return undefined;
    // What is held outlives what the host last said: a picture fetched here is
    // still here when the host's own copy has gone, so the two are remembered
    // together and either one is enough to be worth keeping.
    if (stored.held) heldArt.set(key, stored.held);
    if (!servable(stored.cover) && !heldHashes(key).length) return undefined;
    return stored.cover;
  } catch {
    return undefined;
  }
}

/**
 * Remember what a request learned. Only what is worth keeping goes to disk: the
 * hashes the host can serve and the hashes this phone holds, never an address,
 * because an address is the thing this phone must not fetch from again.
 */
function rememberCover(key: string, cover: HostCover | null) {
  sessionCovers.set(key, cover);
  if (!cover) {
    negativeAt.set(key, Date.now());
    // A claim can be withdrawn, so what is remembered of it goes when the host
    // says there is no art rather than lingering until its own expiry. What is
    // *held* stays: it is this phone's copy of a picture, not the host's claim.
    dropStoredCover(key);
    return;
  }
  if (!servable(cover)) {
    // A picture the host knows of and has not fetched yet. Worth asking about
    // again shortly, and worth writing down only when this phone holds a copy of
    // its own, because that copy is what outlives the host's answer: dropping it
    // here would forget a picture this phone has already fetched.
    negativeAt.set(key, Date.now());
    if (heldHashes(key).length) writeStoredCover(key, cover);
    else dropStoredCover(key);
    return;
  }
  negativeAt.delete(key);
  writeStoredCover(key, cover);
}

/** What this phone holds for an album, with the host's own answer beside it. */
function writeStoredCover(key: string, cover: HostCover) {
  try {
    const record: StoredCover = { at: Date.now(), cover, held: heldFor(key) };
    window.localStorage.setItem(CACHE_PREFIX + key, JSON.stringify(record));
  } catch {
    // Artwork is cosmetic; a full or unavailable cache must not affect playback.
  }
}

function dropStoredCover(key: string) {
  try {
    window.localStorage.removeItem(CACHE_PREFIX + key);
  } catch {
    // Nothing to reclaim; the entry is only wasted space.
  }
}

function dropLegacyCache() {
  try {
    const stale: string[] = [];
    for (let index = 0; index < window.localStorage.length; index += 1) {
      const name = window.localStorage.key(index);
      if (name && LEGACY_CACHE_PREFIXES.some((prefix) => name.startsWith(prefix))) {
        stale.push(name);
      }
    }
    for (const name of stale) window.localStorage.removeItem(name);
  } catch {
    // A cache that cannot be swept is only wasted space.
  }
}

dropLegacyCache();

/**
 * The cover key from the cover NIP: `trim(artist)|trim(album)`, lowercased,
 * preserved verbatim otherwise so it matches the catalogue display strings.
 * Exported because album grouping in the UI must use the same identity.
 */
export function coverKey(artist: string, album: string): string {
  const artistHalf = artist.trim().toLowerCase();
  const albumHalf = album.trim().toLowerCase();
  if (!artistHalf || !albumHalf) return '';
  if (artistHalf.includes('|') || albumHalf.includes('|')) return '';
  const key = `${artistHalf}|${albumHalf}`;
  return key.length <= 300 ? key : '';
}

export function coverFor(track: RemoteTrack): Promise<AlbumCover | null> {
  const key = coverKey(track.artist ?? '', track.album ?? '');
  if (!key) {
    recordCoverEvent('skip', `${track.title || track.filename}: no artist or album tag`);
    return Promise.resolve(null);
  }
  return requestCover(key).then((cover) => (cover ? withArtwork(cover) : null));
}

/**
 * One rendition this phone has confirmed is on disk, by ask.
 *
 * Keyed by album, rendition and hash rather than by address, because the answer
 * is not an address until it has been asked for: a screen drawn twice must not ask
 * twice, the first ask is what tells this phone whether a picture is already here,
 * and a hash is part of the key because a picture that changed is a different ask
 * rather than the same one answered from a memo.
 */
const artAsks = new Map<string, Promise<string>>();

/**
 * The address to draw one rendition from, fetching it from the host if this
 * phone does not hold it.
 *
 * The order of the questions is the point of remembering hashes at all:
 *
 * 1. What the host says it can serve now, which is what a screen should show.
 * 2. What this phone already holds, which is what it has even when the host's
 *    copy has gone or the host cannot be reached.
 * 3. For a thumbnail, the full picture drawn smaller — the fallback every screen
 *    here already used, and cheaper than a placeholder.
 */
async function ensureRendition(cover: HostCover, rendition: 'thumb' | 'full'): Promise<string> {
  const held = heldFor(cover.key);
  const preferred = rendition === 'thumb' ? cover.thumbHash : cover.artHash;
  const ours = rendition === 'thumb' ? held.thumb : held.full;
  for (const candidate of new Set([preferred, ours].filter(Boolean))) {
    const url = await askForRendition(cover.key, rendition, candidate);
    if (url) return url;
  }
  if (rendition === 'full') return '';
  // No thumbnail to be had: the full picture is what a row draws instead.
  return ensureRendition(cover, 'full');
}

/** Ask the host for one rendition, or answer from what is already here. */
function askForRendition(
  key: string,
  rendition: 'thumb' | 'full',
  hash: string
): Promise<string> {
  const token = `${key}|${rendition}|${hash}`;
  const asked = artAsks.get(token);
  if (asked) return asked;
  const promise = invoke<AlbumArtwork>('remote_art', { key, rendition, hash })
    .then((artwork) => {
      if (!artwork.url || !artwork.hash) {
        // The host has nothing to serve yet, which is an answer and not an error -
        // but not one worth remembering. The host fetches what it is asked for,
        // and what moves when it lands is the cover revision, which brings this
        // ask back here. Remembering the empty answer would mean the phone held a
        // placeholder for a picture that had arrived, and the host waiting for a
        // question it had already answered "no" to.
        artAsks.delete(token);
        return '';
      }
      rememberHeld(key, rendition, artwork.hash);
      return artwork.url;
    })
    .catch((error) => {
      // A failed ask is not an answer, so it is forgotten and tried again rather
      // than remembered as "this album has no art".
      artAsks.delete(token);
      recordCoverEvent('error', `${key} ${rendition}: ${String(error)}`);
      return '';
    });
  artAsks.set(token, promise);
  return promise;
}

/**
 * Note what this phone holds for an album, and write it down.
 *
 * The host's answer is written beside it because the two travel together: what
 * this phone holds is what it can draw when the host has nothing to say.
 */
function rememberHeld(key: string, rendition: 'thumb' | 'full', hash: string) {
  const held = heldFor(key);
  if (rendition === 'thumb') held.thumb = hash;
  else held.full = hash;
  heldArt.set(key, held);
  const known = sessionCovers.get(key);
  if (known) writeStoredCover(key, known);
  recordCoverEvent('cached', `${key} ${rendition} → ${hash.slice(0, 12)}`);
}

/**
 * The host's answer about an album, with the addresses this phone can draw from.
 *
 * The thumbnail is what a list needs, so a list is what fetches it. The full
 * picture is only addressed here when this phone already holds it: asking for it
 * on the way past would fetch a second picture for every album that scrolls by,
 * and the screens that show one large ask for it themselves.
 */
async function withArtwork(cover: HostCover): Promise<AlbumCover> {
  const thumb = await ensureRendition(cover, 'thumb');
  const held = heldFor(cover.key);
  const art = held.full ? await askForRendition(cover.key, 'full', held.full) : '';
  return { ...cover, thumb, art };
}

/** Renditions already asked for, by URL, with the load that was started. */
const renditionLoads = new Map<string, Promise<string>>();

/**
 * Fetch a rendition now and hand back what landed: its URL, or '' when the image
 * would not load.
 *
 * One fetch per URL however many callers ask, so a view that draws the same
 * cover, and a preload that has already fetched it, cost one download between
 * them. A failure is remembered for the session, which is what keeps an album
 * whose image 404s from being retried on every track.
 */
function loadRendition(url: string): Promise<string> {
  if (!url) return Promise.resolve('');
  const asked = renditionLoads.get(url);
  if (asked) return asked;
  const landed = new Promise<string>((resolve) => {
    // Nothing holds this element: the fetch it starts is the point, and whether
    // the cover is still wanted when it lands is decided by the screen itself.
    const image = new Image();
    image.decoding = 'async';
    image.onload = () => resolve(url);
    image.onerror = () => resolve('');
    image.src = url;
  });
  renditionLoads.set(url, landed);
  return landed;
}

/**
 * Warm the artwork of the track coming next, so the player is not waiting for a
 * cover when it starts.
 *
 * The thumbnail is always fetched: every row, tile and bar draws one, and it is
 * what the player puts on screen in its first frame. The full-size picture is
 * fetched only when something that draws one is open, which is what `full` says.
 *
 * Why the second half is conditional: a tile never draws the full picture, so
 * with nothing open the only thing that could draw it is the player - and the
 * player is not playing this track yet. Asking for it anyway downloads a picture
 * for a screen nobody is looking at, and on a metered connection that is somebody
 * else's money. With a sheet open it is about to be drawn, which is what makes it
 * worth fetching early at all.
 *
 * What the gate costs: with nothing open, the lock screen's upgrade to the full
 * cover waits until the track actually starts rather than being ready before it.
 * It still arrives - the track becoming the one that plays is its own trigger -
 * so the cost is a frame or two of the thumbnail, on a surface that is usually
 * in somebody's pocket at the time.
 */
export function preloadArtwork(track: RemoteTrack, { full = false }: { full?: boolean } = {}) {
  void coverFor(track).then((cover) => {
    if (!cover) return;
    void loadRendition(cover.thumb);
    if (full) void loadFullCover(cover);
  });
}

/**
 * The full picture of an album, fetched from the host if it is not here yet and
 * resolved when it has landed. The address is handed back rather than a flag so a
 * caller can publish the very image it waited for, and '' means there is nothing
 * to draw.
 */
export function loadFullCover(cover: AlbumCover | null): Promise<string> {
  if (!cover || !cover.artHash) return Promise.resolve('');
  return ensureRendition(cover, 'full').then(loadRendition);
}

/**
 * Forget one album's cover because the image itself would not load, so the
 * next ask resolves a fresh URL instead of handing back the dead one. The
 * stored copy goes too: it names the same URL.
 */
export function dropCover(track: RemoteTrack) {
  const key = coverKey(track.artist ?? '', track.album ?? '');
  if (!key) return;
  sessionCovers.delete(key);
  negativeAt.delete(key);
  dropStoredCover(key);
}

function requestCover(key: string): Promise<HostCover | null> {
  const cached = cachedCover(key);
  if (cached !== undefined) {
    recordCoverEvent('cached', `${key} → ${cached ? 'cover' : 'host has none'}`);
    return Promise.resolve(cached);
  }
  return new Promise((resolve) => {
    const waiters = pending.get(key);
    if (waiters) waiters.push(resolve);
    else pending.set(key, [resolve]);
    if (flushHandle === null) flushHandle = window.setTimeout(flush, BATCH_DELAY_MS);
  });
}

async function flush() {
  flushHandle = null;
  const batch = [...pending.keys()];
  const { covers, unanswered } = await resolveCovers(batch);
  for (const key of batch) {
    const waiters = pending.get(key) ?? [];
    pending.delete(key);
    const cover = covers.get(key) ?? null;
    // A request that failed is not an answer, so it must not be remembered as
    // "this album has no cover". Only the host saying so is remembered.
    if (!unanswered.has(key)) rememberCover(key, cover);
    for (const resolve of waiters) resolve(cover);
  }
  // Keys requested while this batch was in flight wait for the next one.
  if (pending.size) flushHandle = window.setTimeout(flush, BATCH_DELAY_MS);
}

type CoverResolution = {
  /** Covers the host answered with, keyed by cover key. */
  covers: Map<string, HostCover>;
  /** Keys whose request failed, so nothing was learned about them. */
  unanswered: Set<string>;
};

async function resolveCovers(keys: string[]): Promise<CoverResolution> {
  const covers = new Map<string, HostCover>();
  const unanswered = new Set<string>();
  for (let index = 0; index < keys.length; index += INVOKE_KEY_LIMIT) {
    const slice = keys.slice(index, index + INVOKE_KEY_LIMIT);
    const started = Date.now();
    recordCoverEvent('request', `${slice.length} keys: ${previewKeys(slice)}`);
    try {
      const found = await invoke<WireCover[]>('remote_covers', { keys: slice });
      recordCoverEvent(
        'answer',
        `${slice.length} keys → ${found.length} covers in ${Date.now() - started} ms`
      );
      for (const cover of found) {
        const { art, thumb, ...host } = cover;
        // A cover the host has *something* for is worth remembering: a hash
        // means it can serve the picture now, and an address with no hash means
        // it knows of one and has not fetched it yet. Only the album with
        // nothing at all is an album with no art.
        if (art || thumb || host.artHash || host.thumbHash || host.coverFileId) {
          covers.set(host.key, host);
        }
      }
    } catch (error) {
      // Unpaired, offline, or a host older than the cover NIP.
      recordCoverEvent(
        'error',
        `${slice.length} keys failed in ${Date.now() - started} ms: ${String(error)}`
      );
      for (const key of slice) unanswered.add(key);
    }
  }
  return { covers, unanswered };
}

function previewKeys(keys: string[]): string {
  const shown = keys.slice(0, 2).join(', ');
  return keys.length > 2 ? `${shown}, +${keys.length - 2}` : shown;
}

/** Temporary: what the cache currently knows, for the debug panel. */
export function debugCoverState(tracks: RemoteTrack[]) {
  return tracks.map((track) => {
    const key = coverKey(track.artist ?? '', track.album ?? '');
    const known = key ? sessionCovers.get(key) : undefined;
    const stored = key ? readStoredCover(key) : undefined;
    const cover = known ?? stored ?? null;
    const held = key ? heldFor(key) : { full: '', thumb: '' };
    return {
      fileId: track.fileId,
      title: track.title || track.filename,
      key,
      state: !key
        ? 'no key'
        : known === null
          ? 'host: none'
          : cover
            ? servable(cover)
              ? 'host: has art'
              : 'host: fetching'
            : 'unknown',
      // What is on this phone rather than on the host, because that is what a
      // blank tile is usually about.
      url: held.thumb || held.full ? `${held.thumb ? 'thumb' : ''}${held.thumb && held.full ? ' + ' : ''}${held.full ? 'full' : ''}` : ''
    };
  });
}

/**
 * Ask again about every album this phone has no picture to draw for, once the
 * host's own art has changed.
 *
 * That is both the album it was told nobody had art for and the album whose art
 * the host had not fetched yet when it was asked: from here they are the same
 * question, and the revision moving is the answer to it arriving. Art already on
 * screen is deliberately left alone: replacing a picture that is drawn costs a
 * flash of the fallback for no gain, and a claim that supersedes another is rare
 * enough to wait for the stored copy to age out.
 */
export function invalidateCoverNegatives(revision: number) {
  if (revision === appliedCoverRevision) return;
  appliedCoverRevision = revision;
  let dropped = 0;
  for (const [key, cover] of sessionCovers) {
    if (cover !== null && servable(cover)) continue;
    sessionCovers.delete(key);
    negativeAt.delete(key);
    dropped += 1;
  }
  // Nothing was waiting on art, so nothing on screen needs to move.
  if (dropped === 0) return;
  recordCoverEvent('refresh', `host art changed at revision ${revision}; re-asking ${dropped}`);
  coverRevision.update((value) => value + 1);
}

/** Temporary: drop every cached cover so the next render asks again. */
export function clearCoverCache() {
  sessionCovers.clear();
  negativeAt.clear();
  heldArt.clear();
  artAsks.clear();
  pending.clear();
  coverRevision.update((value) => value + 1);
  try {
    const stale: string[] = [];
    for (let index = 0; index < window.localStorage.length; index += 1) {
      const name = window.localStorage.key(index);
      if (name?.startsWith(CACHE_PREFIX)) stale.push(name);
    }
    for (const name of stale) window.localStorage.removeItem(name);
  } catch {
    // Nothing to clear.
  }
}

export function artworkHue(fileId: string) {
  return Number.parseInt(fileId.slice(0, 6) || '5632aa', 16) % 360;
}
