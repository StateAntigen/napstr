import { invoke } from '@tauri-apps/api/core';
import type { RemoteTrack } from './types';

/**
 * Album artwork asserted by Napstr kind `30427` cover events.
 *
 * The paired host resolves these, because it owns the relay pool, the
 * catalogue, and the availability heartbeats that decide which claim wins.
 * Lookups are batched: a list page asks for every album it shows in one call
 * instead of querying per row.
 */
export type AlbumCover = {
  key: string;
  /** Front cover URL. Empty when the publisher only shared an embedded copy. */
  art: string;
  /** Smaller rendition of the same image, when the publisher supplied one. */
  thumb: string;
  mbid: string;
  year: string;
  genre: string;
  collection: string;
  /** Provenance hint such as `itunes`, `embedded`, or `musicbrainz`. */
  source: string;
  coverFileId: string;
  mime: string;
  author: string;
  seeder: boolean;
};

const CACHE_PREFIX = 'napstrfy-cover:v2:';
/** The pre-cover-event namespace, swept once so it cannot linger forever. */
const LEGACY_CACHE_PREFIX = 'napstrfy-artwork:';
/** Keys per companion call. Mirrors `MAX_COVER_KEYS * 4` in the phone crate. */
const INVOKE_KEY_LIMIT = 160;
/** Albums nothing has artwork for are retried after this long. */
const NEGATIVE_TTL_MS = 7 * 24 * 60 * 60 * 1000;
/** A page composes in stages: gather its keys briefly before asking. */
const BATCH_DELAY_MS = 40;
/** MusicBrainz asks clients to stay near one request per second. */
const LOOKUP_SPACING_MS = 1100;

type CachedCover = { at: number; cover: AlbumCover | null };
type Waiter = (cover: AlbumCover | null) => void;

const pending = new Map<string, Waiter[]>();
let flushHandle: number | null = null;
let lookupQueue: Promise<void> = Promise.resolve();
let nextLookup = 0;

function emptyCover(key: string): AlbumCover {
  return {
    key,
    art: '',
    thumb: '',
    mbid: '',
    year: '',
    genre: '',
    collection: '',
    source: '',
    coverFileId: '',
    mime: '',
    author: '',
    seeder: false
  };
}

function readCache(key: string): CachedCover | null {
  try {
    const raw = window.localStorage.getItem(CACHE_PREFIX + key);
    if (!raw) return null;
    const cached = JSON.parse(raw) as CachedCover;
    if (!cached.cover && Date.now() - cached.at > NEGATIVE_TTL_MS) return null;
    return cached;
  } catch {
    return null;
  }
}

function writeCache(key: string, cover: AlbumCover | null) {
  try {
    window.localStorage.setItem(CACHE_PREFIX + key, JSON.stringify({ at: Date.now(), cover }));
  } catch {
    // Artwork is cosmetic; a full or unavailable cache must not affect playback.
  }
}

function dropLegacyCache() {
  try {
    const stale: string[] = [];
    for (let index = 0; index < window.localStorage.length; index += 1) {
      const name = window.localStorage.key(index);
      if (name?.startsWith(LEGACY_CACHE_PREFIX)) stale.push(name);
    }
    for (const name of stale) window.localStorage.removeItem(name);
  } catch {
    // A cache that cannot be swept is only wasted space.
  }
}

dropLegacyCache();

function pause(ms: number) {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

/**
 * The cover key from the cover NIP: `trim(artist)|trim(album)`, lowercased,
 * preserved verbatim otherwise so it matches the catalogue display strings.
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
  return key ? requestCover(key) : Promise.resolve(null);
}

/** The URL for a small artwork tile, preferring the published thumbnail. */
export async function artworkFor(track: RemoteTrack): Promise<string> {
  const cover = await coverFor(track);
  if (!cover) return '';
  return cover.thumb || cover.art;
}

/** Warm the cache for tracks about to be shown, so scrolling never stalls. */
export function preloadCovers(tracks: RemoteTrack[]) {
  for (const track of tracks) void coverFor(track);
}

function requestCover(key: string): Promise<AlbumCover | null> {
  const cached = readCache(key);
  if (cached) return Promise.resolve(cached.cover);
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
  const resolved = await resolveCovers(batch);
  for (const key of batch) {
    const waiters = pending.get(key) ?? [];
    pending.delete(key);
    const cover = resolved.get(key) ?? null;
    writeCache(key, cover);
    for (const resolve of waiters) resolve(cover);
  }
  // Keys requested while this batch was in flight wait for the next one.
  if (pending.size) flushHandle = window.setTimeout(flush, BATCH_DELAY_MS);
}

async function resolveCovers(keys: string[]): Promise<Map<string, AlbumCover>> {
  const resolved = new Map<string, AlbumCover>();
  const misses: string[] = [];
  for (let index = 0; index < keys.length; index += INVOKE_KEY_LIMIT) {
    const slice = keys.slice(index, index + INVOKE_KEY_LIMIT);
    try {
      const covers = await invoke<AlbumCover[]>('remote_covers', { keys: slice });
      for (const cover of covers) {
        if (cover.art || cover.thumb || cover.coverFileId) resolved.set(cover.key, cover);
      }
    } catch {
      // Unpaired, offline, or a host older than the cover NIP: the external
      // lookup below is exactly the fallback that case is meant to use.
    }
    for (const key of slice) if (!resolved.has(key)) misses.push(key);
  }
  for (const key of misses) {
    const url = await externalArtwork(key);
    if (url) {
      // A cover event satisfies its key, so only albums without one get here.
      resolved.set(key, { ...emptyCover(key), art: url, thumb: url, source: 'musicbrainz' });
    }
  }
  return resolved;
}

/**
 * MusicBrainz release search followed by the Cover Art Archive, kept as the
 * client-optional fallback the cover NIP allows for albums no cover event
 * answers. Serialised and spaced out to respect the MusicBrainz rate limit.
 */
function externalArtwork(key: string): Promise<string> {
  const [artist, album] = key.split('|');
  if (!artist || !album) return Promise.resolve('');
  const work = lookupQueue.then(async () => {
    const wait = Math.max(0, nextLookup - Date.now());
    if (wait) await pause(wait);
    nextLookup = Date.now() + LOOKUP_SPACING_MS;
    try {
      const query = `release:${JSON.stringify(album)} AND artist:${JSON.stringify(artist)}`;
      const response = await fetch(
        `https://musicbrainz.org/ws/2/release/?fmt=json&limit=1&query=${encodeURIComponent(query)}`
      );
      if (!response.ok) throw new Error('artwork lookup failed');
      const data = (await response.json()) as { releases?: { id?: string }[] };
      const candidate = data.releases?.[0]?.id ?? '';
      const release = /^[0-9a-f]{8}-[0-9a-f-]{27}$/i.test(candidate) ? candidate : '';
      return release ? `https://coverartarchive.org/release/${release}/front-250` : '';
    } catch {
      return '';
    }
  });
  lookupQueue = work.then(
    () => undefined,
    () => undefined
  );
  return work;
}

export function artworkHue(fileId: string) {
  return Number.parseInt(fileId.slice(0, 6) || '5632aa', 16) % 360;
}
