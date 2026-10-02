/**
 * How many tracks ahead this phone keeps warm.
 *
 * Warming a track means the computer fetches its audio, so this is how far ahead
 * the queue is given a head start on the network: at three, the track after this
 * one, the one after that, and the one after that are already here by the time
 * they are reached, and a tap does not wait on a download.
 *
 * Zero is a real choice rather than a disabled state: it means fetch exactly what
 * is played and nothing more, which is what somebody on a metered connection may
 * want. The quality profile still decides whether a track would be held back at
 * all — this only says how many are asked for.
 */
export const PRELOAD_KEY = 'napstrfy-preload-depth';

/**
 * The most a person may ask for.
 *
 * Past this, a page's worth of listening is being fetched ahead of the thing being
 * listened to, and the download starts to compete with it for the same Iroh slots
 * the library and the artwork are also using.
 */
export const MAX_PRELOAD_DEPTH = 5;

/** Three: the track after this one, and the two behind it. */
export const DEFAULT_PRELOAD_DEPTH = 3;

/** Every depth the setting offers, which is every whole number up to the cap. */
export const PRELOAD_DEPTHS = Array.from({ length: MAX_PRELOAD_DEPTH + 1 }, (_, index) => index);

export function readPreloadDepth(): number {
  try {
    const stored = window.localStorage.getItem(PRELOAD_KEY);
    // No stored depth is not a depth of zero: `Number(null)` is 0, which would read
    // as "pre-load nothing" for everyone who has never opened this setting.
    if (stored !== null) {
      const depth = Number(stored);
      if (Number.isInteger(depth) && depth >= 0 && depth <= MAX_PRELOAD_DEPTH) return depth;
    }
  } catch {
    // A phone that will not read its own storage gets the default, which is not a
    // failure: nothing here is load-bearing.
  }
  return DEFAULT_PRELOAD_DEPTH;
}

export function storePreloadDepth(depth: number) {
  try {
    const clamped = Math.min(Math.max(Math.round(depth), 0), MAX_PRELOAD_DEPTH);
    window.localStorage.setItem(PRELOAD_KEY, String(clamped));
  } catch {
    // As above: a preference nobody can read costs a refetch, not a session.
  }
}
