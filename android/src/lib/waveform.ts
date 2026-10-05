/**
 * The shape of a track, as the bars a player draws.
 *
 * A waveform is not the audio and does not need to be: what a person reads from
 * it is where the quiet parts and the loud parts are, and how far into the track
 * they are. So it is reduced twice on the way here - once by decoding at a low
 * rate, and again by keeping one number per bar - and what is left is a few
 * hundred numbers per track rather than a few million.
 */

/**
 * One bar per slice, each the loudest sample in it.
 *
 * Scaled so that the loudest slice of the track is full height. Without that a
 * quietly mastered record would be a row of stubs, which tells a person nothing
 * about the record and says something misleading about the file: the shape is
 * what is being drawn, and a shape needs its own top to be read against. A track
 * with nothing in it stays at zero rather than being scaled up into noise.
 */
export function waveformPeaks(samples: Float32Array, bars: number): number[] {
  const peaks: number[] = [];
  const perBar = Math.max(1, Math.floor(samples.length / bars));
  for (let bar = 0; bar < bars; bar += 1) {
    const from = bar * perBar;
    const to = Math.min(samples.length, from + perBar);
    let peak = 0;
    for (let index = from; index < to; index += 1) {
      const value = Math.abs(samples[index]);
      if (value > peak) peak = value;
    }
    peaks.push(peak);
  }
  const loudest = Math.max(...peaks);
  return loudest > 0 ? peaks.map((peak) => peak / loudest) : peaks;
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
