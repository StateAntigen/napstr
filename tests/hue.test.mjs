import test from 'node:test';
import assert from 'node:assert/strict';
import { hueOfPixels } from '../android/src/lib/hue.ts';

// The colour in a cover, as far as the app should act on it. Three answers matter:
// a colour, no colour, and nothing to look at at all.

/** RGBA bytes from a list of colours, which is all this needs of a picture. */
const pixels = (colours) => {
  const data = new Uint8ClampedArray(colours.length * 4);
  colours.forEach(([red, green, blue, alpha = 255], index) => {
    data[index * 4] = red;
    data[index * 4 + 1] = green;
    data[index * 4 + 2] = blue;
    data[index * 4 + 3] = alpha;
  });
  return data;
};
const repeated = (colour, count) => Array.from({ length: count }, () => colour);

test('a picture of one strong colour is named after it', () => {
  // The middle of the ten-degree band each colour falls in, red at the top of the wheel.
  assert.equal(hueOfPixels(pixels(repeated([200, 20, 20], 64))), 5);
  assert.equal(hueOfPixels(pixels(repeated([20, 200, 20], 64))), 125);
  assert.equal(hueOfPixels(pixels(repeated([20, 20, 200], 64))), 245);
});

test('a black and white picture names no colour rather than inventing one', () => {
  // The case this exists for: a monochrome sleeve. Black, white and greys in between
  // are not hues, and the answer is null so a caller can use the app's own colour.
  const greys = [...repeated([0, 0, 0], 32), ...repeated([255, 255, 255], 32), ...repeated([128, 128, 128], 32)];

  assert.equal(hueOfPixels(pixels(greys)), null);
});

test('a picture that is mostly grey but has a colour in it names that colour', () => {
  const mostlyGrey = [...repeated([120, 120, 120], 40), ...repeated([200, 30, 30], 20)];

  assert.equal(hueOfPixels(pixels(mostlyGrey)), 5);
});

test('a picture with nothing in it names nothing', () => {
  assert.equal(hueOfPixels(pixels([])), null);
  assert.equal(hueOfPixels(pixels(repeated([200, 20, 20, 0], 64))), null);
});

test('the hue is the one most of the picture agrees on, not the loudest pixel', () => {
  // One bright pixel of a different colour does not decide an album's colour.
  const mixed = [...repeated([20, 20, 200], 60), [255, 255, 0]];

  assert.equal(hueOfPixels(pixels(mixed)), 245);
});
