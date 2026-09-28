import type { RemoteTrack } from './types';

/**
 * What this phone is willing to spend data on.
 *
 * A phone's connection decides how expensive a file is to fetch, and the file's
 * own container decides what it costs: a lossless FLAC is tens of megabytes for
 * a song that a 128 kb/s MP3 gives away for three. Napstr knows both of those
 * numbers because it read them out of the file when it scanned its library, so
 * the choice between them belongs on the phone, where the data is being spent.
 *
 * The setting is a preference about what this phone *asks for*, never a promise
 * about what arrives. A track's bytes come from whoever holds them, and the
 * copy of a file this phone already has is what it plays.
 */

/** The formats Napstr shares, in the order the chips are drawn. */
export const AUDIO_FORMATS = ['MP3', 'FLAC', 'WAV', 'OGG', 'OPUS'];

/** The two that keep the samples themselves, which is why they are the big ones. */
export const LOSSLESS_FORMATS = ['FLAC', 'WAV'];

/** Ceilings a phone may ask for. 0 means no ceiling at all. */
export const BITRATE_CHOICES = [64, 128, 192, 256, 320, 0];

export type QualityProfile = {
  /** The formats this connection may fetch. Everything else waits. */
  formats: string[];
  /** Kilo-bits per second, or 0 for no ceiling. */
  maxBitrateKbps: number;
};

export type QualitySettings = {
  /** Wi-Fi, ethernet, or anything else the phone calls unmetered. */
  unmetered: QualityProfile;
  /** Mobile data, and a hotspot that says it is metered. */
  metered: QualityProfile;
};

/** Why a track is being held back, for a message that can say which. */
export type HoldVerdict =
  | { held: false }
  | {
      held: true;
      reason: 'format';
      format: string;
      lossless: boolean;
    }
  | {
      held: true;
      reason: 'bitrate';
      bitrateKbps: number;
      ceilingKbps: number;
    };

const STORAGE_KEY = 'napstrfy-quality';

/**
 * The phone's own defaults, which are meant to be unsurprising.
 *
 * Wi-Fi is treated as free: nothing is held back, which is what this app did
 * before it had a setting at all. Mobile data keeps only what a lossless copy
 * could never be worth - losing the samples saves most of the bandwidth, and the
 * lossy formats are left alone because a ceiling on them is a matter of taste
 * rather than of data, so it is the user's to set.
 */
export function defaultQuality(): QualitySettings {
  return {
    unmetered: { formats: [...AUDIO_FORMATS], maxBitrateKbps: 0 },
    metered: { formats: AUDIO_FORMATS.filter((format) => !LOSSLESS_FORMATS.includes(format)), maxBitrateKbps: 0 },
  };
}

/** What this phone last chose, or the defaults. A damaged setting is not fatal. */
export function readQuality(): QualitySettings {
  const fallback = defaultQuality();
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return fallback;
    const stored = JSON.parse(raw) as Partial<QualitySettings>;
    return {
      unmetered: readProfile(stored.unmetered, fallback.unmetered),
      metered: readProfile(stored.metered, fallback.metered)
    };
  } catch {
    return fallback;
  }
}

export function writeQuality(settings: QualitySettings) {
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
  } catch {
    // A setting that cannot be written is a setting for this session only.
  }
}

/**
 * A stored profile, kept only as far as it makes sense.
 *
 * A format this build does not know and a ceiling it does not offer are both
 * dropped rather than trusted: the first could hold back everything, and the
 * second could hold back nothing while looking like it does.
 */
function readProfile(value: unknown, fallback: QualityProfile): QualityProfile {
  if (!value || typeof value !== 'object') return fallback;
  const stored = value as Partial<QualityProfile>;
  const formats = Array.isArray(stored.formats)
    ? stored.formats.filter((format): format is string => AUDIO_FORMATS.includes(format))
    : fallback.formats;
  const ceiling = typeof stored.maxBitrateKbps === 'number' && BITRATE_CHOICES.includes(stored.maxBitrateKbps)
    ? stored.maxBitrateKbps
    : fallback.maxBitrateKbps;
  return { formats, maxBitrateKbps: ceiling };
}

/** The profile that applies to the connection this phone is on. */
export function activeProfile(settings: QualitySettings, metered: boolean): QualityProfile {
  return metered ? settings.metered : settings.unmetered;
}

/**
 * Whether this track fits what the connection may spend, and if not, why.
 *
 * Only what the file itself states is judged. A track whose bitrate Napstr did
 * not report is never held back for its bitrate, because a number this phone
 * does not have cannot be over a ceiling - the format still decides, and a
 * format is always known, since it is in the filename and in the tags. A format
 * that is not one of Napstr's own is left alone for the same reason: this
 * setting is about music files it can identify, and guessing about the rest
 * would be inventing a policy.
 */
export function fitsProfile(track: RemoteTrack, profile: QualityProfile): HoldVerdict {
  const format = (track.format ?? '').toUpperCase();
  if (!AUDIO_FORMATS.includes(format)) return { held: false };
  const lossless = track.lossless || LOSSLESS_FORMATS.includes(format);
  if (!profile.formats.includes(format)) {
    return { held: true, reason: 'format', format, lossless };
  }
  // A lossless copy of a song is always above any ceiling a phone can choose,
  // but the container saying nothing is not evidence, so it is not treated as
  // though it were.
  if (profile.maxBitrateKbps > 0 && track.bitrateKbps > profile.maxBitrateKbps) {
    return {
      held: true,
      reason: 'bitrate',
      bitrateKbps: track.bitrateKbps,
      ceilingKbps: profile.maxBitrateKbps
    };
  }
  return { held: false };
}

/** Whether this track is a music file the setting has an opinion about at all. */
export function judgedFormat(track: RemoteTrack): boolean {
  return AUDIO_FORMATS.includes((track.format ?? '').toUpperCase());
}
