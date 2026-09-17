import { invoke } from '@tauri-apps/api/core';
import { recordCoverEvent } from './coverDebug';
import type { RemoteTrack } from './types';

/**
 * Album artwork asserted by Napstr kind `30427` cover events.
 *
 * The paired host resolves these, because it owns the relay pool, the
 * catalogue, and the availability heartbeats that decide which claim wins.
 * Lookups are batched: a list page asks for every album it shows in one call
 * instead of querying per row. An album the host has no cover event for simply
 * has no artwork here.
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
  /** Provenance hint from the publisher, such as `itunes`, `musicbrainz`, or `embedded`. */
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
/**
 * How long stored artwork is trusted before it is asked for again.
 *
 * Publishers replace their cover claim, and image URLs rot, so a cover that
 * answered once is not permanent. Asking is a local call to the host.
 */
const STORED_COVER_TTL_MS = 7 * 24 * 60 * 60 * 1000;
/** A page composes in stages: gather its keys briefly before asking. */
const BATCH_DELAY_MS = 40;

type Waiter = (cover: AlbumCover | null) => void;

/**
 * Covers resolved while the app is open, including albums the host has no cover
 * for. Absence is deliberately not persisted: the host re-checks relays on its
 * own schedule and answering again costs one batched call, so carrying a "no"
 * across launches only ever delays artwork that has since been published.
 */
const sessionCovers = new Map<string, AlbumCover | null>();
const pending = new Map<string, Waiter[]>();
let flushHandle: number | null = null;

/**
 * `undefined` means nothing is known yet; `null` means the host already answered
 * and has no cover for this album. The two must stay distinct: treating
 * "unknown" as "no cover" resolves the answer without ever asking for it.
 */
function cachedCover(key: string): AlbumCover | null | undefined {
  const known = sessionCovers.get(key);
  if (known !== undefined) return known;
  const stored = readStoredCover(key);
  if (stored === undefined) return undefined;
  sessionCovers.set(key, stored);
  return stored;
}

function readStoredCover(key: string): AlbumCover | undefined {
  try {
    const raw = window.localStorage.getItem(CACHE_PREFIX + key);
    if (!raw) return undefined;
    const stored = JSON.parse(raw) as { at: number; cover: AlbumCover | null };
    if (!stored.cover) {
      // An earlier build could store a "no cover" verdict. Drop it rather than
      // let a stale no keep suppressing the question.
      dropStoredCover(key);
      return undefined;
    }
    if (Date.now() - stored.at > STORED_COVER_TTL_MS) return undefined;
    return stored.cover;
  } catch {
    return undefined;
  }
}

/** Remember what a request learned. Only artwork is worth writing to disk. */
function rememberCover(key: string, cover: AlbumCover | null) {
  sessionCovers.set(key, cover);
  if (!cover) {
    // A claim can be withdrawn, so a stored cover must go when the host says
    // there is none rather than linger until its own expiry.
    dropStoredCover(key);
    return;
  }
  try {
    window.localStorage.setItem(CACHE_PREFIX + key, JSON.stringify({ at: Date.now(), cover }));
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
      if (name?.startsWith(LEGACY_CACHE_PREFIX)) stale.push(name);
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
 */
function coverKey(artist: string, album: string): string {
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
  return requestCover(key);
}

function requestCover(key: string): Promise<AlbumCover | null> {
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
  covers: Map<string, AlbumCover>;
  /** Keys whose request failed, so nothing was learned about them. */
  unanswered: Set<string>;
};

async function resolveCovers(keys: string[]): Promise<CoverResolution> {
  const covers = new Map<string, AlbumCover>();
  const unanswered = new Set<string>();
  for (let index = 0; index < keys.length; index += INVOKE_KEY_LIMIT) {
    const slice = keys.slice(index, index + INVOKE_KEY_LIMIT);
    const started = Date.now();
    recordCoverEvent('request', `${slice.length} keys: ${previewKeys(slice)}`);
    try {
      const found = await invoke<AlbumCover[]>('remote_covers', { keys: slice });
      recordCoverEvent(
        'answer',
        `${slice.length} keys → ${found.length} covers in ${Date.now() - started} ms`
      );
      for (const cover of found) {
        if (cover.art || cover.thumb || cover.coverFileId) covers.set(cover.key, cover);
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
    return {
      fileId: track.fileId,
      title: track.title || track.filename,
      key,
      state: !key
        ? 'no key'
        : known === null
          ? 'host: none'
          : cover
            ? 'resolved'
            : 'unknown',
      url: cover ? cover.thumb || cover.art : ''
    };
  });
}

/** Temporary: drop every cached cover so the next render asks again. */
export function clearCoverCache() {
  sessionCovers.clear();
  pending.clear();
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
