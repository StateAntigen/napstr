/**
 * The colour in a picture.
 *
 * Here rather than in the artwork module because this is arithmetic over pixels and
 * nothing else: no store, no host, no cover cache, so it can be tested on its own
 * with a handful of pixels rather than a phone.
 */

/** How much colour a pixel has to have before its hue is worth listening to. */
const VOICE_FLOOR = 0.02;

/** How much colour a picture has to have, per pixel, before a hue is worth naming. */
const PICTURE_FLOOR = 0.08;

/**
 * The hue most of a set of RGBA pixels agree on, or null when they are grey.
 *
 * Every pixel votes for a hue, weighted by how colourful it is and by how far it is
 * from black and from white: a pixel that is nearly grey abstains rather than voting
 * for a hue it does not have, and a nearly black or nearly white pixel has little
 * colour to give whatever its arithmetic says. A monochrome sleeve therefore returns
 * null, which is the answer a caller should be able to act on: there is no colour in
 * this picture to name, so nothing should be invented.
 *
 * The winner is the busiest ten-degree band, reported at the middle of that band -
 * near enough for a tint, and stable against a single stray pixel.
 */
export function hueOfPixels(pixels: Uint8ClampedArray): number | null {
  const bins = new Array<number>(36).fill(0);
  let opaque = 0;
  let colourfulness = 0;
  for (let index = 0; index < pixels.length; index += 4) {
    if (pixels[index + 3] < 128) continue;
    opaque += 1;
    const red = pixels[index] / 255;
    const green = pixels[index + 1] / 255;
    const blue = pixels[index + 2] / 255;
    const high = Math.max(red, green, blue);
    const low = Math.min(red, green, blue);
    const range = high - low;
    if (range <= 0.02) continue;
    const lightness = (high + low) / 2;
    const saturation = range / (1 - Math.abs(2 * lightness - 1));
    const voice = saturation * Math.min(1, 4 * Math.min(lightness, 1 - lightness));
    if (voice <= VOICE_FLOOR) continue;
    let hue;
    if (high === red) hue = ((green - blue) / range + 6) % 6;
    else if (high === green) hue = (blue - red) / range + 2;
    else hue = (red - green) / range + 4;
    bins[Math.min(35, Math.floor((hue * 60) / 10))] += voice;
    colourfulness += voice;
  }
  if (opaque === 0 || colourfulness < opaque * PICTURE_FLOOR) return null;
  let best = 0;
  for (let index = 1; index < bins.length; index += 1) if (bins[index] > bins[best]) best = index;
  return (best * 10 + 5) % 360;
}

/**
 * The hue of a picture that has already been decoded: a number, null when the
 * picture has no colour to name, or undefined when its pixels could not be read.
 *
 * Drawing it small is the whole of the work: 48 x 48 of a cover is a fair sample of
 * its colours and costs nothing. What goes wrong is the read: a picture drawn from
 * another origin taints the canvas unless it was fetched as a CORS request, which is
 * what `crossorigin` on the image element asks for, and every read off a tainted
 * canvas throws. That is reported as its own answer rather than as null, because a
 * picture nobody could look at and a picture of grey are not the same thing - and
 * calling the first one grey is how a whole app ends up tinted one colour.
 */
export function hueOfImage(image: HTMLImageElement, size = 48): number | null | undefined {
  try {
    const canvas = document.createElement('canvas');
    canvas.width = size;
    canvas.height = size;
    const context = canvas.getContext('2d', { willReadFrequently: true });
    if (!context) return undefined;
    context.drawImage(image, 0, 0, size, size);
    return hueOfPixels(context.getImageData(0, 0, size, size).data);
  } catch {
    return undefined;
  }
}
