import { test, expect } from '@playwright/test';
import { mockNative, serveAudio } from './helpers/native.mjs';
import { artHashes, heldArtUrl, refusePublisherArtwork, serveHeldArtwork } from './helpers/artwork.mjs';

// Our phone has no scraper, and it no longer has any reason to reach a publisher
// either. The paired host resolves kind-30427 cover events, answers
// `remote_covers` with the hash of every picture it can serve, and hands the
// bytes over when the phone asks with `remote_art`. These tests play the host,
// and check both halves of that bargain: that a screen draws what the host
// supplied, from this phone's own address, and that nothing here reaches for a
// publisher.
/** The album one track belongs to, which is also the cover key. */
const key = (index) => `rancid|album ${index}`;

/**
 * `count` tracks, each its own album, so every tile has its own cover key.
 *
 * The host answers each key with a hash for both renditions - which is what it
 * can serve - and with the publisher address the claim came with, which this
 * phone must never draw from. `pending` names albums the host knows of a picture
 * for and has not fetched: it answers those with an address and no hash, which is
 * a third case, and the one that must not read as "no art".
 */
async function seedLibrary(page, count = 40, { pending = [] } = {}) {
  const hashes = Array.from({ length: count }, (_, index) => artHashes(key(index)));
  await page.addInitScript(({ count, pending, hashes }) => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    const makeCover = (coverKey, index) => ({ key: coverKey,
      art: `/publisher-${index}.png`, thumb: `/publisher-${index}-thumb.png`,
      artHash: pending.includes(index) ? '' : hashes[index].full,
      thumbHash: pending.includes(index) ? '' : hashes[index].thumb,
      mbid: '', year: '', genre: '', collection: '', source: 'itunes', coverFileId: '', mime: 'image/png', author: '', seeder: false });
    const indexFor = (coverKey) => Number(coverKey.split('|')[1].slice('album '.length));
    const tracks = Array.from({ length: count }, (_, index) => ({
      fileId: index.toString(16).padStart(64, '0'),
      filename: `Song ${index}.wav`, title: `Song ${index}`, artist: 'Rancid', album: `Album ${index}`,
      format: 'WAV', mime: 'audio/wav', size: 1234567, tags: '', local: true, sources: []
    }));
    window.coverAsks = [];
    window.artAsks = [];
    window.coverFails = false;
    window.__TAURI_INTERNALS__.invoke = async (cmd, args = {}) => {
      if (cmd === 'cached_library') return { paired: true, connected: true, tracks, total: tracks.length };
      if (cmd === 'remote_library') return { tracks, total: tracks.length };
      if (cmd === 'remote_covers') {
        window.coverAsks.push([...args.keys]);
        if (window.coverFails) throw new Error('Host is not reachable');
        return args.keys.map((coverKey) => makeCover(coverKey, indexFor(coverKey)));
      }
      if (cmd === 'remote_art') {
        window.artAsks.push({ key: args.key, rendition: args.rendition, hash: args.hash });
        // Everything the host serves is served from this phone's own address.
        return { url: `${location.origin}/test-token/art/${args.hash}`, hash: args.hash };
      }
      return invoke(cmd, args);
    };
  }, { count, pending, hashes });
  return hashes;
}

const asks = (page) => page.evaluate(() => window.coverAsks);
const askCount = (page) => page.evaluate(() => window.coverAsks.length);
const artAsks = (page) => page.evaluate(() => window.artAsks);

test('Napstrfy asks the host about the artwork it can show, in batches by album, and paints it from this phone', async ({ page }) => {
  await mockNative(page);
  const hashes = await seedLibrary(page);
  const far = await refusePublisherArtwork(page, ['**/publisher-*']);
  const served = await serveHeldArtwork(page);
  await page.route('**/fixture.wav', serveAudio);
  await page.goto('http://127.0.0.1:15174');
  const rows = page.locator('.track-row');
  await expect(rows).toHaveCount(40);
  // A row draws the host's thumbnail from this phone's own address, and that
  // address names the hash the host said would answer for the album.
  await expect(rows.first().locator('.artwork img')).toHaveAttribute('src', heldArtUrl(hashes[0].thumb));
  // One batched ask for the albums the screen shows, rather than one call per
  // tile. The batch is whatever registered inside the flush window, so how many
  // albums it holds depends on how much of the list had laid out by then: the
  // count is bounded, and no row index is pinned, because "album 39 is not in
  // it" was never something the batching promises. Duplicates are.
  const [first] = await asks(page);
  expect(await askCount(page)).toBe(1);
  expect(first.length).toBeGreaterThan(4);
  expect(first.length).toBeLessThan(40);
  expect(new Set(first).size).toBe(first.length);
  // Every screenshot of a list costs the host nothing new: a picture is asked for
  // once per album and rendition, however many times the tile that draws it is
  // rebuilt.
  const thumbsAskedFor = (asks) => asks.filter((ask) => ask.key === 'rancid|album 0' && ask.rendition === 'thumb');
  await expect.poll(async () => thumbsAskedFor(await artAsks(page)).length).toBe(1);
  // Scrolling to the far end is what asks for whatever was never asked for. How
  // many batches that takes is timing, not behaviour, so only the fact that new
  // work happened is asserted.
  await rows.nth(39).scrollIntoViewIfNeeded();
  await expect(rows.nth(39).locator('.artwork img')).toHaveAttribute('src', heldArtUrl(hashes[39].thumb));
  await expect.poll(() => askCount(page)).toBeGreaterThan(1);
  // Playing it draws the same answer in the player, without leaving the screen
  // and - the guarantee worth pinning - without asking the host about the album
  // again. The thumbnail is what the list already fetched; the full picture is
  // what playing it makes worth fetching, and the drawer fades that in over the
  // thumbnail it starts from.
  const asksBeforePlay = await askCount(page);
  await rows.nth(39).locator('.track-open').click();
  await expect(page.locator('.now-sheet-art img.now-sheet-art-thumb')).toHaveAttribute('src', heldArtUrl(hashes[39].thumb));
  await expect(page.locator('.now-sheet-art img.now-sheet-art-full')).toHaveAttribute('src', heldArtUrl(hashes[39].full));
  await expect(rows).toHaveCount(40);
  expect(await askCount(page)).toBe(asksBeforePlay);
  // Nothing reached a publisher for any of it, and every picture that was served
  // was one this phone asked for by hash: the two halves of the bargain.
  expect(far).toEqual([]);
  expect(served.length).toBeGreaterThan(0);
  expect(served.every((hash) => /^[0-9a-f]{64}$/.test(hash))).toBe(true);
});

test('Napstrfy fetches the artwork of the track next in the queue while the one before it plays', async ({ page }) => {
  await mockNative(page);
  // A list long enough that the album this test plays next cannot have been on
  // screen: "nothing has asked about it yet" has to be a fact about the list
  // rather than about how much of it happened to be laid out by the time the
  // assertion ran, which is what made this test disagree with itself under load.
  const target = 100;
  await seedLibrary(page, 120);
  const far = await refusePublisherArtwork(page, ['**/publisher-*']);
  await serveHeldArtwork(page);
  await page.route('**/fixture.wav', serveAudio);
  await page.goto('http://127.0.0.1:15174');
  const rows = page.locator('.track-row');
  await expect(rows).toHaveCount(120);
  await rows.first().locator('.track-open').click();
  // Every row of the playlist looks its own album up as it comes into view, as
  // the library's rows do, so what a row shows is a fact about whether it has
  // been on screen rather than about where it sits in the list.
  await page.getByRole('button', { name: 'Open the playlist' }).click();
  const asked = async () => (await asks(page)).flat().join('\n');
  const fetched = async () =>
    (await artAsks(page)).map((ask) => `${ask.key} ${ask.rendition}`).join('\n');
  await expect.poll(asked).toContain('album 5');
  // The rows on screen have asked for their own albums by now and drawn them; the
  // ones below the fold have not been looked at by anything, so this is a real
  // absence rather than a race not yet lost. The row this test plays is far
  // enough down the list that no viewport reaches it, which is what makes that a
  // fact about the list rather than about how much of it had been laid out.
  const queueRows = page.locator('.queue-row');
  await expect(queueRows.nth(11).locator('.artwork img:not(.fallback)')).toHaveCount(1);
  await expect(queueRows.nth(target).locator('.artwork img:not(.fallback)')).toHaveCount(0);
  // The full rendition is what the preload fetches and nothing else does: a tile
  // only ever draws the thumbnail, so the album a hundred tracks along has never
  // been asked for one.
  expect(await fetched()).not.toContain(`rancid|album ${target + 1} full`);
  expect(far).toEqual([]);
  await queueRows.nth(target).locator('.queue-open').click();
  // Playing it is what asks on behalf of the track that will follow it, and what
  // fetches both of its renditions: the thumbnail so the player has a cover at
  // once, and the full one so the drawer and the lock screen have nothing left to
  // wait for. The row's own tile would have asked about the album anyway once it
  // scrolled into view, so the full rendition is what says the preload did this.
  await expect.poll(asked).toContain(`album ${target + 1}`);
  await expect.poll(fetched).toContain(`rancid|album ${target + 1} thumb`);
  await expect.poll(fetched).toContain(`rancid|album ${target + 1} full`);
  // The row the click scrolled to has drawn its album for itself, which is the
  // thing the cap used to stop happening past the twelfth row.
  await expect(queueRows.nth(target).locator('.artwork img:not(.fallback)')).toHaveCount(1);
});

test('Napstrfy warms the next track’s thumbnail only, while nothing that draws a full cover is open', async ({ page }) => {
  await mockNative(page);
  await seedLibrary(page, 5);
  const far = await refusePublisherArtwork(page, ['**/publisher-*']);
  await serveHeldArtwork(page);
  await page.route('**/fixture.wav', serveAudio);
  await page.goto('http://127.0.0.1:15174');
  const rows = page.locator('.track-row');
  await expect(rows).toHaveCount(5);
  // Playing a row from the library leaves nothing open, so the track that follows
  // has its thumbnail warmed and nothing more: its full-size picture would be
  // downloaded for a screen that does not exist yet. A tile never draws one.
  await rows.first().locator('.track-open').click();
  const fetched = async () =>
    (await artAsks(page)).map((ask) => `${ask.key} ${ask.rendition}`).join('\n');
  await expect.poll(fetched).toContain('rancid|album 1 thumb');
  expect(await fetched()).not.toContain('rancid|album 1 full');
  // What is playing is a different question: the lock screen draws its full-size
  // picture whether or not anything in this app is open, so that one is fetched.
  await expect.poll(fetched).toContain('rancid|album 0 full');
  expect(far).toEqual([]);
});

test('Napstrfy keeps the art it already holds while the host is still fetching its own copy', async ({ page }) => {
  await mockNative(page);
  // The host knows of a picture for this album and has not fetched it, so it
  // answers with the publisher's address and no hash. That is not "no art", and a
  // phone that read it that way would throw away the copy it holds and draw a
  // placeholder for an album whose art is on its way.
  const heldHash = 'c'.repeat(60) + '0000';
  await seedLibrary(page, 1, { pending: [0] });
  await page.addInitScript((hash) => {
    localStorage.setItem('napstrfy-cover:v3:rancid|album 0', JSON.stringify({
      at: Date.now(),
      cover: { key: 'rancid|album 0', artHash: '', thumbHash: '', mbid: '', year: '', genre: '',
        collection: '', source: 'itunes', coverFileId: '', mime: 'image/png', author: '', seeder: false },
      held: { full: '', thumb: hash }
    }));
  }, heldHash);
  const far = await refusePublisherArtwork(page, ['**/publisher-*']);
  await serveHeldArtwork(page);
  await page.goto('http://127.0.0.1:15174');
  // Drawn from the copy this phone holds, asked for by the hash it already has,
  // and without asking the host about the album at all.
  await expect(page.locator('.track-row .artwork img')).toHaveAttribute('src', heldArtUrl(heldHash));
  expect(await askCount(page)).toBe(0);
  expect((await artAsks(page)).some((ask) => ask.hash === heldHash)).toBe(true);
  expect(far).toEqual([]);
});

test('Napstrfy artwork recovers from a transient host failure while the screen stays open', async ({ page }) => {
  await mockNative(page);
  const hashes = await seedLibrary(page, 1);
  await page.addInitScript(() => { window.coverFails = true; });
  await page.clock.install();
  await refusePublisherArtwork(page, ['**/publisher-*']);
  await serveHeldArtwork(page);
  await page.goto('http://127.0.0.1:15174');
  await page.clock.runFor(500);
  await expect(page.locator('.track-row .artwork img')).toHaveClass('fallback');
  expect(await askCount(page)).toBe(1);
  // A request that failed is not an answer, so the tile asks again on its own
  // bounded retry rather than accepting that the album has no cover.
  await page.evaluate(() => { window.coverFails = false; });
  await page.clock.runFor(61_000);
  await page.clock.runFor(500);
  await expect(page.locator('.track-row .artwork img')).toHaveAttribute('src', heldArtUrl(hashes[0].thumb));
  expect(await askCount(page)).toBe(2);
  expect(await page.locator('.track-row').count()).toBe(1);
});

test('Napstrfy artwork gives up after three tries, so a dead host is not polled forever', async ({ page }) => {
  await mockNative(page);
  await seedLibrary(page, 1);
  await page.addInitScript(() => { window.coverFails = true; });
  await page.clock.install();
  await refusePublisherArtwork(page, ['**/publisher-*']);
  await page.goto('http://127.0.0.1:15174');
  for (let round = 0; round < 5; round += 1) {
    await page.clock.runFor(61_000);
    await page.clock.runFor(500);
  }
  await expect(page.locator('.track-row .artwork img')).toHaveClass('fallback');
  expect(await askCount(page)).toBe(3);
});
