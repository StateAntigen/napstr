import { test, expect } from '@playwright/test';
import { mockNative, serveAudio } from './helpers/native.mjs';

// The network's own list, on the tab where somebody goes looking. It is chosen by
// the computer out of the catalogue it already mirrors rather than searched for,
// and every row is a file no computer of yours holds — so playing one is a fetch,
// and the seeders the row names are what the fetch is asked for.

const id = (letter) => letter.repeat(64);
const track = (letter, title, local) => ({
  fileId: id(letter),
  filename: `${title}.wav`,
  title,
  artist: 'ZZ Top',
  album: '',
  format: 'WAV',
  mime: 'audio/wav',
  size: 1234567,
  tags: '',
  local,
  sources: local ? [] : [{ pubkey: id('1'), displayName: '' }, { pubkey: id('2'), displayName: '' }]
});

const inLibrary = track('a', 'Doubleback', true);
/** Two files the network has and this phone's computer does not. */
const live = [track('b', 'Gimme All Your Lovin', false), track('c', 'Sharp Dressed Man', false)];

/**
 * The phone, with a library and a network list.
 *
 * `arrives` names a file the computer is to have finished fetching, which is the
 * moment a row that was asked to play stops being a download and becomes sound.
 */
async function openApp(page, { library = [inLibrary], discovered = live, arrives = null } = {}) {
  await mockNative(page, { platform: 'android' });
  await page.route('**/fixture.wav', serveAudio);
  await page.addInitScript(({ library, discovered, arrives }) => {
    window.remoteLibrary = library;
    window.discoverTracks = discovered;
    const invoke = window.__TAURI_INTERNALS__.invoke;
    window.__TAURI_INTERNALS__.invoke = async (cmd, args = {}) => {
      if (cmd === 'remote_transfers' && arrives) {
        return [{ fileId: arrives, filename: 'Doubleback.wav', size: 1234567, progress: 100, status: 'Verified · Complete', speed: '', destination: '' }];
      }
      return invoke(cmd, args);
    };
  }, { library, discovered, arrives });
  await page.goto('http://127.0.0.1:15174');
}

/** The second nav tab, which is where a search starts. */
async function openSearchTab(page) {
  await page.locator('.bottom-nav button[data-tab="search"]').click();
}

const calls = (page, cmd) =>
  page.evaluate((cmd) => window.calls.filter((call) => call.cmd === cmd).map((call) => call.args), cmd);
const audioPaused = (page) => page.evaluate(() => document.querySelector('audio')?.paused ?? true);

test('the search tab opened on nothing shows what the network has, above the library', async ({ page }) => {
  await openApp(page);
  await openSearchTab(page);

  const section = page.locator('section[aria-label="Discover"]');
  await expect(section).toBeVisible();
  const rows = section.locator('.track-row');
  await expect(rows).toHaveCount(2);
  await expect(rows.first()).toContainText('Gimme All Your Lovin');
  // The label says how many the whole list holds, which is more than this page.
  await expect(page.locator('.section-label', { hasText: 'Discover' })).toContainText('2 live on the network');
  // And the library is still below it: the list was added, not swapped in.
  await expect(page.locator('.track-list').last().locator('.track-row')).toHaveCount(1);
});

test('playing a row nobody here holds asks the computer to fetch it, from the row’s own seeders', async ({ page }) => {
  await openApp(page);
  await openSearchTab(page);
  await page.locator('section[aria-label="Discover"] .track-open').first().click();

  const downloads = await expect.poll(() => calls(page, 'remote_download')).toHaveLength(1).then(() => calls(page, 'remote_download'));
  expect(downloads[0].fileId).toBe(live[0].fileId);
  // The seeders travel with the ask: the computer has never seen this file, so who
  // is holding it is the whole of what it knows about getting it.
  expect(downloads[0].sourcePubkeys).toEqual([id('1'), id('2')]);
});

test('play all asks for the first row, and it starts as soon as the computer has it', async ({ page }) => {
  // The row the list leads with is the fixture track's own file id, so the
  // computer's finished transfer is that file's: the fetch lands and it plays.
  const wanted = track('a', 'Doubleback', false);
  await openApp(page, { library: [inLibrary], discovered: [wanted, live[1]], arrives: wanted.fileId });
  await openSearchTab(page);
  await page.locator('.discover-play').click();

  const downloads = await expect.poll(() => calls(page, 'remote_download')).toHaveLength(1).then(() => calls(page, 'remote_download'));
  expect(downloads[0].fileId).toBe(wanted.fileId);
  // A tap on something nobody here holds means play, and the download is only how:
  // so the row starts when its bytes arrive rather than leaving the tap one short.
  await expect.poll(() => audioPaused(page)).toBe(false);
});
