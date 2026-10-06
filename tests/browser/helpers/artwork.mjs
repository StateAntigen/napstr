/**
 * The art half of the companion contract, for tests.
 *
 * A phone no longer draws a picture from the address a claim came with. The host
 * says which hash answers for an album, the phone asks for the bytes over the
 * pairing and keeps them under that name, and what it draws is served from its
 * own disk. Every spec that shows artwork has to play that, so the shape lives
 * here once.
 */

/** The token the native side mints for a run. */
export const ART_TOKEN = 'test-token';

/** The address this phone serves one held picture from. */
export function heldArtUrl(hash) {
  return `http://127.0.0.1:15174/${ART_TOKEN}/art/${hash}`;
}

/**
 * A 64-character hexadecimal name for a picture, from whatever a test has to
 * identify it by. Deterministic, so a test can name the hash it expects to be
 * drawn rather than keeping a table of them.
 *
 * The real names are SHA-256 of the bytes; nothing here depends on that, only on
 * a name being a name.
 */
export function artHash(seed, rendition = 'thumb') {
  const source = `${rendition}:${seed}`;
  let hex = '';
  let state = 7;
  for (let index = 0; index < source.length; index += 1) {
    state = (state * 31 + source.charCodeAt(index)) >>> 0;
    hex += state.toString(16).padStart(8, '0');
  }
  return hex.padEnd(64, '0').slice(0, 64);
}

/** Both renditions of one album, as the host would name them. */
export function artHashes(seed) {
  return { thumb: artHash(seed, 'thumb'), full: artHash(seed, 'full') };
}

/**
 * A 1x1 PNG, so every artwork in every test is a real image to the browser.
 *
 * Opaque red, and a stream a decoder will actually accept: the one this used to
 * hold failed its zlib checksum, so whether anything was ever drawn from it came
 * down to how forgiving the decoder felt - which is not a thing for a test that
 * shows artwork to rest on. Specs that only read the `src` never noticed.
 */
export const ART_PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC',
  'base64'
);

/**
 * Serve the pictures this phone holds, and record which hash was asked for.
 *
 * This route stands in for the phone's own server: what the page draws has to
 * come from here, so a test can tell a picture it held from one it did not.
 */
export async function serveHeldArtwork(page, asked = []) {
  await page.route('**/art/**', (route) => {
    asked.push(new URL(route.request().url()).pathname.split('/').pop());
    return route.fulfill({ contentType: 'image/png', body: ART_PNG });
  });
  return asked;
}

/**
 * Deny every address a publisher gave, and record anything that reached one
 * anyway.
 *
 * The reason art travels over the pairing is that this app should never tell a
 * third party what it is looking at, so a test is not satisfied by the artwork
 * appearing: nothing may have asked a publisher for it. The addresses are named
 * by the caller, because that is what makes this a real refusal rather than a
 * blanket rule that could hide a regression.
 */
export async function refusePublisherArtwork(page, addresses = []) {
  const far = [];
  page.on('request', (request) => {
    if (request.resourceType() !== 'image') return;
    const url = new URL(request.url());
    if (url.hostname === '127.0.0.1' || url.hostname === 'localhost') return;
    far.push(url.href);
  });
  for (const address of addresses) await page.route(address, (route) => route.abort());
  return far;
}
