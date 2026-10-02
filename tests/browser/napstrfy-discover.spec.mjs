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
async function openApp(page, { library = [inLibrary], discovered = live, arrives = null, preload = null } = {}) {
  await mockNative(page, { platform: 'android' });
  await page.route('**/fixture.wav', serveAudio);
  await page.addInitScript(({ library, discovered, arrives, preload }) => {
    window.remoteLibrary = library;
    window.discoverTracks = discovered;
    if (preload !== null) window.localStorage.setItem('napstrfy-preload-depth', String(preload));
    const invoke = window.__TAURI_INTERNALS__.invoke;
    window.__TAURI_INTERNALS__.invoke = async (cmd, args = {}) => {
      if (cmd === 'remote_transfers' && arrives) {
        return [{ fileId: arrives, filename: 'Doubleback.wav', size: 1234567, progress: 100, status: 'Verified · Complete', speed: '', destination: '' }];
      }
      return invoke(cmd, args);
    };
  }, { library, discovered, arrives, preload });
  await page.goto('http://127.0.0.1:15174');
}

/** The second nav tab, which is where a search starts. */
async function openSearchTab(page) {
  await page.locator('.bottom-nav button[data-tab="search"]').click();
}

const calls = (page, cmd) =>
  page.evaluate((cmd) => window.calls.filter((call) => call.cmd === cmd).map((call) => call.args), cmd);
const audioPaused = (page) => page.evaluate(() => document.querySelector('audio')?.paused ?? true);
/** The files one kind of ask named, in the order the phone made the asks. */
const files = (page, cmd) =>
  page.evaluate(
    (cmd) => window.calls.filter((call) => call.cmd === cmd).map((call) => call.args.fileId ?? call.args.track?.fileId),
    cmd
  );

test('the search tab opened on nothing shows what the network has, and nothing else', async ({ page }) => {
  await openApp(page);
  await openSearchTab(page);

  const section = page.locator('section[aria-label="Discover"]');
  await expect(section).toBeVisible();
  const rows = section.locator('.track-row');
  await expect(rows).toHaveCount(2);
  await expect(rows.first()).toContainText('Gimme All Your Lovin');
  // The label says how many the whole list holds, which is more than this page.
  await expect(page.locator('.section-label', { hasText: 'Discover' })).toContainText('2 live on the network');
  // Nothing of the library: an empty search tab is where the network's list goes,
  // and a list of what is already here is an answer to a search rather than a page
  // standing there before one has been made.
  await expect(page.locator('.track-list')).toHaveCount(1);
  await expect(page.locator('.track-row')).toHaveCount(2);

  // Searching is what brings it in.
  const input = page.getByRole('textbox', { name: 'Search tracks', exact: true });
  await input.fill('doubleback');
  await input.press('Enter');
  await expect(page.locator('section[aria-label="Discover"]')).toHaveCount(0);
  await expect(page.locator('.track-row')).toHaveCount(1);
});

test('playing a row nobody here holds asks the computer to fetch it, from the row’s own seeders', async ({ page }) => {
  // Nothing is warmed ahead here, so the row's own ask is the whole of it.
  await openApp(page, { preload: 0 });
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
  // Nothing is warmed ahead here, so the ask the tap makes is the only one.
  const wanted = track('a', 'Doubleback', false);
  await openApp(page, { library: [inLibrary], discovered: [wanted, live[1]], arrives: wanted.fileId, preload: 0 });
  await openSearchTab(page);
  await page.locator('.discover-play').click();

  const downloads = await expect.poll(() => calls(page, 'remote_download')).toHaveLength(1).then(() => calls(page, 'remote_download'));
  expect(downloads[0].fileId).toBe(wanted.fileId);
  // A tap on something nobody here holds means play, and the download is only how:
  // so the row starts when its bytes arrive rather than leaving the tap one short.
  await expect.poll(() => audioPaused(page)).toBe(false);
});

test('a run of the network list leaves a track of ours between each pair', async ({ page }) => {
  // Four files nobody here holds and one track of this phone's own, five deep. A
  // track of ours between each pair is not a mixing whim: fetching one of the
  // network files takes minutes, and the track already here is what is playing
  // while the next one is fetched. Its id is nobody else's and it is not the file
  // the fixture phone already holds, or there would be nothing to take over.
  const list = [
    track('b', 'Gimme All Your Lovin', false),
    track('c', 'Sharp Dressed Man', false),
    track('d', 'Legs', false),
    track('e', 'Tush', false)
  ];
  const mine = { ...inLibrary, fileId: id('f'), filename: 'Cheap Sunglasses.mp3', title: 'Cheap Sunglasses', format: 'MP3', mime: 'audio/mpeg' };
  await openApp(page, { library: [mine], discovered: list, preload: 5 });
  // The library has to be here before the run starts, and the music tab is where it
  // is listed: the track of ours that goes between the new ones is one this phone
  // holds, so a run started before the library arrived would have nothing to put
  // there.
  await expect(page.locator('.track-row')).toHaveCount(1);
  await openSearchTab(page);
  await page.locator('.discover-play').click();

  // Asked for in play order, and no further ahead than the computer can run: it
  // fetches two at a time, so a third ask would queue behind those two and put the
  // track about to play behind the ones after it.
  await expect.poll(() => files(page, 'remote_download')).toEqual([list[0].fileId, list[1].fileId]);
  // And the track of ours is genuinely in the run - it is the one being taken over
  // Iroh before its turn, which is what says the queue holds it between the new
  // ones rather than after them.
  await expect.poll(() => files(page, 'prefetch_remote_audio')).toContain(mine.fileId);
});

test('a row of the list starts the list as the queue from that row', async ({ page }) => {
  const list = [
    track('b', 'Gimme All Your Lovin', false),
    track('c', 'Sharp Dressed Man', false),
    track('d', 'Legs', false),
    track('e', 'Tush', false)
  ];
  const mine = { ...inLibrary, fileId: id('f'), filename: 'Cheap Sunglasses.mp3', title: 'Cheap Sunglasses', format: 'MP3', mime: 'audio/mpeg' };
  await openApp(page, { library: [mine], discovered: list, preload: 2 });
  await expect(page.locator('.track-row')).toHaveCount(1);
  await openSearchTab(page);
  await page.locator('section[aria-label="Discover"] .track-row').nth(2).locator('.track-open').click();

  // The run is network, ours, network... so the third network row is the fifth entry
  // and the two asks are its own file and the one after it: the list from that row,
  // rather than the whole thing from the top.
  await expect.poll(() => files(page, 'remote_download')).toEqual([list[2].fileId, list[3].fileId]);
  // The track of ours after it is the one being taken over Iroh while the fetch
  // runs, which is what says the run arrived at the row that was tapped.
  await expect.poll(() => files(page, 'prefetch_remote_audio')).toContain(mine.fileId);
});

test('the list fills to a hundred rows out of pages of thirty', async ({ page }) => {
  // One control frame carries thirty rows, so a hundred is four pages - the last
  // one short. The list is filled to what it says it shows, rather than stopping
  // at the first page, and the button under it offers more than the computer has.
  const hex = (index) => index.toString(16).padStart(64, '0');
  const many = Array.from({ length: 140 }, (_, index) => ({
    ...track('b', 'Gimme All Your Lovin', false),
    fileId: hex(index),
    filename: `Track ${index + 1}.mp3`,
    title: `Track ${index + 1}`
  }));
  await openApp(page, { discovered: many });
  await openSearchTab(page);

  const rows = page.locator('section[aria-label="Discover"] .track-row');
  await expect(rows).toHaveCount(120);
  // Four pages of thirty - a hundred is passed by the fourth, so the list is a
  // whole page longer than the length it was filled to.
  const offsets = await page.evaluate(() =>
    window.calls.filter((call) => call.cmd === 'remote_discover').map((call) => call.args.offset)
  );
  expect(offsets).toEqual([0, 30, 60, 90]);

  // Asking for more gets the rest of what the computer has said it holds, and then
  // stops: an empty page is not a reason to ask again.
  await page.locator('section[aria-label="Discover"] .load-more').click();
  await expect(rows).toHaveCount(140);
  await expect(page.locator('section[aria-label="Discover"] .load-more')).toHaveCount(0);
});
