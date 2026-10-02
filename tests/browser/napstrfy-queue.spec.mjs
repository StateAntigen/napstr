import { test, expect } from '@playwright/test';
import { mockNative, serveAudio } from './helpers/native.mjs';

// The playlist the player walks, and the one way into it that is not "play this
// now": a track added from the track menu goes on the end of it, wherever it came
// from and wherever its bytes live. On a phone whose music sits on somebody
// else's disk that last part is the point — a queued track is fetched when its
// turn comes, so being on this phone is not a condition of being queued.

const id = (letter) => letter.repeat(64);
const track = (letter, title) => ({
  fileId: id(letter),
  filename: `${title}.wav`,
  title,
  artist: 'ZZ Top',
  album: '',
  format: 'WAV',
  mime: 'audio/wav',
  size: 1234567,
  tags: '',
  local: true,
  sources: []
});

const first = track('a', 'Doubleback');
const second = track('b', 'Gimme All Your Lovin');
/** Nothing plays this: it is the search result the menu is used on. */
const found = track('c', 'Sharp Dressed Man');

async function openApp(page, { library = [first, second], results = [found] } = {}) {
  await mockNative(page, { platform: 'android' });
  await page.route('**/fixture.wav', serveAudio);
  await page.addInitScript(({ library, results }) => {
    window.remoteLibrary = library;
    const invoke = window.__TAURI_INTERNALS__.invoke;
    window.__TAURI_INTERNALS__.invoke = async (cmd, args = {}) => {
      // A search answers at once: what is being looked at is the menu a result
      // offers, not the search itself.
      if (cmd === 'remote_search') return results;
      if (cmd === 'remote_library' && args.query) return { tracks: results, total: results.length };
      return invoke(cmd, args);
    };
  }, { library, results });
  await page.goto('http://127.0.0.1:15174');
}

const audioPaused = (page) => page.evaluate(() => document.querySelector('audio')?.paused ?? true);
const queueRows = (page) => page.locator('.queue-row');

/** Play a library track, which makes the whole library the playlist. */
async function playFirst(page) {
  await page.locator('.track-open').first().click();
  await expect.poll(() => audioPaused(page)).toBe(false);
}

/**
 * The playlist, opened the way a person opens it: the bar's drawer, then its own
 * playlist button. The button is only clickable once the drawer has arrived, so
 * the click is retried rather than aimed at a moving sheet.
 */
async function openQueue(page) {
  await page.locator('.now-open').click();
  await expect(async () => {
    await page.locator('[aria-label="Open the playlist"]').click();
    await expect(page.locator('.queue-view')).toBeVisible({ timeout: 700 });
  }).toPass();
}

/** Add a search result from its own ⋮ menu. */
async function addFoundTrack(page) {
  await page.locator('.bottom-nav button[data-tab="search"]').click();
  const input = page.getByRole('textbox', { name: 'Search tracks', exact: true });
  await input.fill('zz top');
  await input.press('Enter');
  const row = page.locator('.track-row', { hasText: found.title });
  await expect(row).toBeVisible();
  await row.locator('.track-more').click();
  await page.getByRole('button', { name: 'Add to queue' }).click();
}

test('a search result goes on the end of the playlist from its own menu', async ({ page }) => {
  await openApp(page);
  await playFirst(page);
  await addFoundTrack(page);

  await openQueue(page);
  // The library this phone played is the playlist, so the result is the third
  // entry: what was playing keeps its place and the new one goes last.
  await expect(queueRows(page)).toHaveCount(3);
  const titles = await queueRows(page).allTextContents();
  expect(titles[2]).toContain('Sharp Dressed Man');
});

test('adding to a playlist nobody is playing starts it', async ({ page }) => {
  await openApp(page);
  await page.locator('.track-more').nth(1).click();
  await page.getByRole('button', { name: 'Add to queue' }).click();
  // An entry nobody can hear is not what a person meant by "add", so it plays.
  await expect.poll(() => audioPaused(page)).toBe(false);
  await openQueue(page);
  await expect(queueRows(page)).toHaveCount(1);
});

test('a track already in the playlist is not queued twice', async ({ page }) => {
  await openApp(page);
  await playFirst(page);
  // The whole library is the playlist, so this one is already in it. The queue is
  // keyed by file and cannot hold it twice, so the tap says so instead of
  // appending something the playlist would collapse.
  await page.locator('.track-more').nth(1).click();
  await page.getByRole('button', { name: 'Add to queue' }).click();
  await expect(page.locator('.toast')).toContainText('Gimme All Your Lovin');
  await openQueue(page);
  await expect(queueRows(page)).toHaveCount(2);
});

test('the row is not offered while the computer is the one playing', async ({ page }) => {
  await openApp(page);
  await playFirst(page);
  // Hand playback to the computer, whose playlist this phone cannot add to: the
  // drawer and its playlist are showing the computer's list, so adding here would
  // look like it went somewhere it did not.
  await page.locator('.now-open').click();
  await page.getByRole('button', { name: /^Play on:/ }).click();
  await page.locator('.actions-row').nth(1).click();
  await page.getByRole('button', { name: 'Close the now playing screen' }).click();

  await page.locator('.track-more').nth(1).click();
  await expect(page.getByRole('button', { name: /Add to queue/ })).toBeDisabled();
});
