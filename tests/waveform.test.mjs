import test from 'node:test';
import assert from 'node:assert/strict';
import { playedBars, waveformPeaks } from '../android/src/lib/waveform.ts';

// The shape a player draws of a track. It is not the audio: it is one number per
// bar, scaled so the loudest bar of the track is full height, because the shape is
// what is being read and a shape needs its own top to be read against.

test('a quiet half and a loud half are drawn as exactly that', () => {
  const samples = new Float32Array(2000);
  for (let index = 1000; index < 2000; index += 1) samples[index] = 0.5;

  const peaks = waveformPeaks(samples, 10);

  assert.equal(peaks.length, 10);
  assert.deepEqual(peaks.slice(0, 5), [0, 0, 0, 0, 0]);
  assert.deepEqual(peaks.slice(5), [1, 1, 1, 1, 1]);
});

test('a quietly mastered track still shows its own shape', () => {
  // Everything at a hundredth of full scale except one moment twice that loud: the
  // loudest bar is what the shape is scaled to, so the rest of the track is visible
  // rather than a row of stubs at the bottom.
  const samples = new Float32Array(100).fill(0.01);
  samples[50] = 0.02;

  const peaks = waveformPeaks(samples, 10);

  assert.equal(Math.max(...peaks), 1);
  assert.equal(peaks[5], 1);
  assert.equal(peaks[0], 0.5);
});

test('silence is drawn as silence rather than scaled up into a full block', () => {
  assert.deepEqual(waveformPeaks(new Float32Array(100), 10), [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
  assert.deepEqual(waveformPeaks(new Float32Array(0), 4), [0, 0, 0, 0]);
});

test('a file shorter than the bar count gives one bar per sample it has', () => {
  // Never a division by zero and never a missing bar: a very short file is a very
  // short file, and the drawer still draws the number of bars it says it does.
  const peaks = waveformPeaks(new Float32Array(3).fill(1), 10);

  assert.equal(peaks.length, 10);
  assert.deepEqual(peaks, [1, 1, 1, 0, 0, 0, 0, 0, 0, 0]);
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
