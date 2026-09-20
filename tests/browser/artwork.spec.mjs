import { test, expect } from '@playwright/test';
import { mockNative, serveAudio } from './helpers/native.mjs';

// Our phone has no scraper: the paired host resolves kind-30427 cover events and
// answers `remote_covers`. These tests play the host, and watch which albums the
// phone asks about and when it asks again.
const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=', 'base64');

/**
 * `count` tracks, each its own album, so every tile has its own cover key. The
 * host answers every key, with one of two URLs that says which album it was.
 */
async function seedLibrary(page, count = 40) {
  await page.addInitScript((count) => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    const makeCover = (key, art) => ({ key, art, thumb: art, mbid: '', year: '', genre: '', collection: '',
      source: 'itunes', coverFileId: '', mime: 'image/png', author: '', seeder: false });
    const artFor = (key) => `/cover-${Number(key.split('|')[1].slice('album '.length)) % 2 ? 'b' : 'a'}.png`;
    const tracks = Array.from({ length: count }, (_, index) => ({
      fileId: index.toString(16).padStart(64, '0'),
      filename: `Song ${index}.wav`, title: `Song ${index}`, artist: 'Rancid', album: `Album ${index}`,
      format: 'WAV', mime: 'audio/wav', size: 1234567, tags: '', local: true, sources: []
    }));
    window.coverAsks = [];
    window.coverFails = false;
    window.__TAURI_INTERNALS__.invoke = async (cmd, args = {}) => {
      if (cmd === 'cached_library') return { paired: true, connected: true, tracks, total: tracks.length };
      if (cmd === 'remote_library') return { tracks, total: tracks.length };
      if (cmd === 'remote_covers') {
        window.coverAsks.push([...args.keys]);
        if (window.coverFails) throw new Error('Host is not reachable');
        return args.keys.map((key) => makeCover(key, artFor(key)));
      }
      return invoke(cmd, args);
    };
  }, count);
}

const asks = (page) => page.evaluate(() => window.coverAsks);
const askCount = (page) => page.evaluate(() => window.coverAsks.length);

test('Napstrfy asks the host about the artwork it can show, in batches by album, and paints the player the same way', async ({ page }) => {
  await mockNative(page);
  await seedLibrary(page);
  await page.route('**/cover-*.png', (route) => route.fulfill({ contentType: 'image/png', body: PNG }));
  await page.route('**/fixture.wav', serveAudio);
  await page.goto('http://127.0.0.1:15174');
  const rows = page.locator('.track-row');
  await expect(rows).toHaveCount(40);
  await expect(rows.first().locator('.artwork img')).toHaveAttribute('src', '/cover-a.png');
  // One batched ask for the whole screen, not one per tile, and the last row's
  // album is nowhere in it: it is far below the fold, so it was never asked for.
  const [first] = await asks(page);
  expect(await askCount(page)).toBe(1);
  expect(first.length).toBeGreaterThan(4);
  expect(first.length).toBeLessThan(40);
  expect(new Set(first).size).toBe(first.length);
  expect(first).not.toContain('rancid|album 39');
  // Scrolling it into view is what asks for it.
  await rows.nth(39).scrollIntoViewIfNeeded();
  await expect(rows.nth(39).locator('.artwork img')).toHaveAttribute('src', '/cover-b.png');
  await expect.poll(() => askCount(page)).toBe(2);
  const [, second] = await asks(page);
  expect(second).toContain('rancid|album 39');
  // Playing it draws the same answer in the player, without leaving the screen
  // and without asking again.
  await rows.nth(39).locator('.track-open').click();
  await expect(page.locator('.now-sheet-art img')).toHaveAttribute('src', '/cover-b.png');
  await expect(rows).toHaveCount(40);
  expect(await askCount(page)).toBe(2);
});

test('Napstrfy artwork recovers from a transient host failure while the screen stays open', async ({ page }) => {
  await mockNative(page);
  await seedLibrary(page, 1);
  await page.addInitScript(() => { window.coverFails = true; });
  await page.clock.install();
  await page.route('**/cover-*.png', (route) => route.fulfill({ contentType: 'image/png', body: PNG }));
  await page.goto('http://127.0.0.1:15174');
  await page.clock.runFor(500);
  await expect(page.locator('.track-row .artwork img')).toHaveClass('fallback');
  expect(await askCount(page)).toBe(1);
  // A request that failed is not an answer, so the tile asks again on its own
  // bounded retry rather than accepting that the album has no cover.
  await page.evaluate(() => { window.coverFails = false; });
  await page.clock.runFor(61_000);
  await page.clock.runFor(500);
  await expect(page.locator('.track-row .artwork img')).toHaveAttribute('src', '/cover-a.png');
  expect(await askCount(page)).toBe(2);
  expect(await page.locator('.track-row').count()).toBe(1);
});

test('Napstrfy artwork gives up after three tries, so a dead host is not polled forever', async ({ page }) => {
  await mockNative(page);
  await seedLibrary(page, 1);
  await page.addInitScript(() => { window.coverFails = true; });
  await page.clock.install();
  await page.goto('http://127.0.0.1:15174');
  for (let round = 0; round < 5; round += 1) {
    await page.clock.runFor(61_000);
    await page.clock.runFor(500);
  }
  await expect(page.locator('.track-row .artwork img')).toHaveClass('fallback');
  expect(await askCount(page)).toBe(3);
});
