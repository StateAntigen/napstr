/**
 * The shape of a track, as the bars a player draws.
 *
 * A waveform is not the audio and does not need to be: what a person reads from
 * it is where the quiet parts and the loud parts are, and how far into the track
 * they are. So it is reduced twice on the way here - once by decoding at a low
 * rate, and again by keeping a couple of numbers per bar - and what is left is a
 * few hundred numbers per track rather than a few million.
 */

/**
 * One bar: the quietest and the loudest edge of its slice, in -1..1.
 *
 * Held as a pair rather than a single height so the drawer has one shape to draw
 * whatever the measure produces - and so a measure with something to say about the
 * quiet edge as well as the loud one stays possible.
 */
export type WaveBar = { low: number; high: number };

/** How the busy-ness of a bar becomes a height. */
export type ShapeOptions = {
  /** Bars either side to average with, which is what makes the outline undulate. */
  span?: number;
  /** The percentiles of the track that become the bottom and the top of the shape. */
  lowPercentile?: number;
  highPercentile?: number;
  /** How far below the top still counts, in decibels, or null for no curve at all. */
  floorDb?: number | null;
  /** A power curve on what is left: above 1 it pulls the middle and the quiet down. */
  gamma?: number;
};

/**
 * Every bar of the shape: how busy each slice is, put on the chosen curve.
 *
 * The measure is *busy-ness* rather than level - the mean of a slice's absolute
 * samples over that same slice's loudest one - and the reason is mastering. A
 * brickwalled record is loud everywhere, so its loudest sample is the ceiling in
 * every bar and its energy barely moves either: measured either way, a modern
 * master draws as one slab whatever curve is put on it. How much of a bar is doing
 * something is the one thing a limiter cannot hold still, because a sparse verse
 * and a packed chorus sit at exactly the same level and are nothing like each
 * other in that.
 *
 * That measure then goes through three knobs, which are the whole difference
 * between a shape that reads and one that does not:
 *
 * - percentiles rather than the extremes decide the scale, because one loud moment
 *   should not be what the rest of the track is measured against. Everything above
 *   the upper percentile is full height, everything below the lower one is flat,
 *   and the music in between is spread across the bar.
 * - the decibel curve keeps a quiet passage visible beside a loud one.
 * - the power curve above 1 pulls the middle and the quiet down, which is the knob
 *   for how far apart the loud and the quiet parts are drawn.
 *
 * Finally each bar is averaged with `span` either side. This is a shape rather than
 * a reading: eleven bars is about a second and a half of a four-minute track, which
 * is shorter than any section a person would name, so the averaging softens the
 * texture without moving a chorus.
 */
export function waveformShape(
  samples: Float32Array,
  bars: number,
  options: ShapeOptions = {}
): WaveBar[] {
  const span = options.span ?? 5;
  const lowPercentile = options.lowPercentile ?? 20;
  const highPercentile = options.highPercentile ?? 90;
  const floorDb = options.floorDb === undefined ? 45 : options.floorDb;
  const gamma = options.gamma ?? 1.4;
  const perBar = Math.max(1, Math.floor(samples.length / bars));
  const busy: number[] = [];
  for (let bar = 0; bar < bars; bar += 1) {
    const from = bar * perBar;
    const to = Math.min(samples.length, from + perBar);
    let sum = 0;
    let peak = 0;
    for (let index = from; index < to; index += 1) {
      const value = Math.abs(samples[index]);
      sum += value;
      if (value > peak) peak = value;
    }
    busy.push(peak > 1e-6 ? sum / Math.max(1, to - from) / peak : 0);
  }
  const sorted = [...busy].sort((left, right) => left - right);
  const at = (percent: number) =>
    sorted[
      Math.min(sorted.length - 1, Math.max(0, Math.round((percent / 100) * (sorted.length - 1))))
    ] ?? 0;
  const low = at(lowPercentile);
  const high = at(highPercentile);
  const spread = high - low;
  const shaped = busy.map((value) => {
    // A track with no variation at all has no shape to draw, and a spread of zero is
    // that track rather than a division problem: every bar lands on the bottom.
    const unit = spread > 1e-6 ? Math.min(1, Math.max(0, (value - low) / spread)) : 0;
    const mapped =
      floorDb === null
        ? unit
        : Math.max(0, Math.min(1, (20 * Math.log10(Math.max(1e-6, unit)) + floorDb) / floorDb));
    return gamma === 1 ? mapped : Math.pow(mapped, gamma);
  });
  const smoothed =
    span <= 0 ? shaped : shaped.map((_value, index) => averageAround(shaped, index, span));
  return smoothed.map((value) => ({ low: -value, high: value }));
}

/** The mean of `span` values either side, clamped at the ends rather than shortened. */
function averageAround(values: number[], index: number, span: number): number {
  let sum = 0;
  for (let at = index - span; at <= index + span; at += 1) {
    sum += values[Math.min(values.length - 1, Math.max(0, at))];
  }
  return sum / (span * 2 + 1);
}

/**
 * How many of `bars` are behind the play-head at `fraction` of the track.
 *
 * Rounding rather than flooring so the bar the play-head is on is filled the
 * moment it starts rather than the moment it ends - a progress bar that lags its
 * own sound by a bar reads as a stutter.
 */
export function playedBars(fraction: number, bars: number): number {
  const clamped = Math.min(1, Math.max(0, fraction));
  return Math.round(clamped * bars);
}
