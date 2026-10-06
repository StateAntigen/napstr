import test from 'node:test';
import assert from 'node:assert/strict';
import { playedBars, waveformShape } from '../android/src/lib/waveform.ts';

// The shape a player draws of a track. It is not the audio: each bar is how busy its
// slice is, put on a curve and averaged with its neighbours, then drawn as a height.
// These tests ask for the measure on its own - no smoothing, no percentiles, no curve -
// so the numbers they assert are the measure's own, and the options that are the app's
// own choice are asserted where they are the subject.

const plain = { span: 0, lowPercentile: 0, highPercentile: 100, floorDb: null, gamma: 1 };
const height = (bar) => bar.high - bar.low;

/**
 * Samples whose bars have the busy-ness asked for: a slice of `busy` hits at full
 * scale and the rest silence has a loudest sample of 1 and a mean of `busy`.
 */
function samplesOfBars(busy, perBar = 100) {
  const samples = new Float32Array(busy.length * perBar);
  busy.forEach((value, bar) => {
    const hits = Math.round(value * perBar);
    for (let index = 0; index < hits; index += 1) samples[bar * perBar + index] = index % 2 === 0 ? 1 : -1;
  });
  return samples;
}

test('a quiet half and a loud half are drawn as exactly that', () => {
  const samples = samplesOfBars([0, 0, 0, 0, 0, 1, 1, 1, 1, 1]);

  const bars = waveformShape(samples, 10, plain);

  assert.equal(bars.length, 10);
  for (const bar of bars.slice(0, 5)) assert.equal(height(bar), 0);
  for (const bar of bars.slice(5)) assert.equal(bar.high, 1);
  // Symmetric about the centre line, which is what the bar of a played track is.
  assert.equal(bars[5].low, -1);
});

test('a brickwalled master still shows where it is busy and where it is sparse', () => {
  // The case this measure exists for. Both halves reach the ceiling - the peaks are
  // identical - and they differ only in how much of each bar is doing something: a
  // few hits with silence between them, then a bar packed with sound. A peak per bar
  // draws the same block for both; this draws one at nothing and one at full height.
  const samples = samplesOfBars([0.05, 0.05, 0.05, 0.05, 0.05, 0.95, 0.95, 0.95, 0.95, 0.95]);

  const bars = waveformShape(samples, 10, plain);

  assert.equal(bars[1].high, 0);
  assert.equal(bars[8].high, 1);
});

test('a track with nothing to say is drawn flat rather than as a full block', () => {
  // A drone, a test tone, a file of one level throughout. There is no shape in it to
  // find, so every bar lands on the same height instead of the scale inventing one.
  const samples = new Float32Array(100).fill(0.01);

  const bars = waveformShape(samples, 10, plain);

  assert.equal(bars.length, 10);
  for (const bar of bars) assert.equal(height(bar), 0);
});

test('silence is drawn as silence rather than scaled up into a full block', () => {
  // No height at all, which is the whole of it: the sign of a nought is not a thing
  // to assert on, and a bar of silence draws as a line either way.
  for (const bar of waveformShape(new Float32Array(100), 10, plain)) {
    assert.equal(height(bar), 0);
  }
  assert.equal(waveformShape(new Float32Array(0), 4, plain).length, 4);
});

test('a file shorter than the bar count gives one bar per sample it has', () => {
  // Never a division by zero and never a missing bar: a very short file is a very
  // short file, and the drawer still draws the number of bars it says it does. A bar
  // with nothing in it stays flat on the centre line.
  const bars = waveformShape(new Float32Array(3).fill(1), 10, plain);

  assert.equal(bars.length, 10);
  for (const bar of bars.slice(0, 3)) assert.equal(bar.high, 1);
  for (const bar of bars.slice(3)) assert.equal(height(bar), 0);
});

test('the percentiles are the scale, so one loud moment cannot flatten the rest', () => {
  // A body of bars that differ slightly, and one bar that is as busy as a bar can be.
  const busy = [];
  for (let index = 0; index < 98; index += 1) busy.push(0.28 + (index % 5) * 0.01);
  busy.push(1);
  const samples = samplesOfBars(busy);

  // Measured against the extremes, the one loud bar is the whole scale and the body
  // is a hairline; against the 20th to 90th percentile it is the body that fills the
  // bar, and the loud moment simply runs off the top.
  const extremes = waveformShape(samples, busy.length, plain);
  const percentiles = waveformShape(samples, busy.length, {
    span: 0,
    lowPercentile: 20,
    highPercentile: 90,
    floorDb: null,
    gamma: 1
  });

  // The body, not the whole track: the loud bar is meant to be at the top either way.
  const spreadOfBody = (bars) => {
    const body = bars.slice(0, -1).map((bar) => bar.high);
    return Math.max(...body) - Math.min(...body);
  };
  assert.ok(spreadOfBody(extremes) < 0.1, String(spreadOfBody(extremes)));
  assert.ok(spreadOfBody(percentiles) > 0.5, String(spreadOfBody(percentiles)));
});

test('the shape is averaged with its neighbours, so a step arrives as a shoulder', () => {
  // Eleven bars either side, which is the whole difference between a shape and a bar
  // chart: a plateau that began on an instant would draw square shoulders, and one
  // averaged into its neighbours draws a line.
  const samples = samplesOfBars([...Array(30).fill(0), ...Array(30).fill(1)]);

  const smoothed = waveformShape(samples, 60, { span: 5 });

  assert.equal(smoothed[20].high, 0);
  assert.equal(smoothed[59].high, 1);
  assert.ok(smoothed[29].high > 0 && smoothed[29].high < 1, String(smoothed[29].high));
});

test('the play-head fills the bar it is on rather than the one behind it', () => {
  // Rounding, not flooring: a progress bar that lags its own sound by a bar reads
  // as a stutter, and at the very end of a track the last bar is filled.
  assert.equal(playedBars(0, 140), 0);
  assert.equal(playedBars(0.5, 140), 70);
  assert.equal(playedBars(0.504, 140), 71);
  assert.equal(playedBars(1, 140), 140);
  // Clamped, because a position past the end is a broken reading and not a longer
  // track - and a negative one is the same kind of reading.
  assert.equal(playedBars(1.4, 140), 140);
  assert.equal(playedBars(-0.2, 140), 0);
});
