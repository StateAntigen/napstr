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

/**
 * Files this phone does not have anywhere: the network has them, and getting one
 * means the computer fetching it over Tor before this phone can play it at all.
 */
const elsewhere = (letter, title) => ({
  ...track(letter, title),
  format: 'MP3',
  mime: 'audio/mpeg',
  local: false,
  sources: [{ pubkey: letter === 'c' ? id('1') : id('2'), displayName: '' }]
});
const away = elsewhere('c', 'Sharp Dressed Man');
const furtherAway = elsewhere('d', 'Legs');

async function openApp(page, { library = [first, second], results = [found], fetching = null, holds = null, refuse = null } = {}) {
  await mockNative(page, { platform: 'android' });
  await page.route('**/fixture.wav', serveAudio);
  await page.addInitScript(({ library, results, fetching, holds, refuse }) => {
    window.remoteLibrary = library;
    // The answers a search gives, which a spec rewrites while the app is running
    // when it wants a file to change hands - the mock reads them at every call.
    window.searchResults = results;
    // The one download the computer is running, in the same mutable shape: the
    // fetch a queued track is waiting for is moved along by the test that made it.
    window.fetching = fetching;
    // A verdict for one file rather than for all of them, which is what a computer
    // that has given up on a single download looks like from here.
    window.statuses = {};
    // A file the computer already holds, as its own record of it: asking to
    // download one is refused, exactly as the real computer refuses it.
    window.holds = holds;
    // A refusal that is not about holding the file: a lent pairing, or a host that
    // will not fetch this one at all.
    window.refuse = refuse;
    const invoke = window.__TAURI_INTERNALS__.invoke;
    window.__TAURI_INTERNALS__.invoke = async (cmd, args = {}) => {
      if (cmd === 'remote_download' && window.holds) {
        // Asked of the mock first, so the call is recorded as every other one is,
        // and then refused the way the computer refuses it.
        await invoke(cmd, args).catch(() => {});
        throw 'this audio is already on this computer; play it locally';
      }
      if (cmd === 'remote_library_by_ids' && window.holds) return window.holds;
      // A search answers at once: what is being looked at is the menu a result
      // offers, not the search itself.
      if (cmd === 'remote_search') return window.searchResults;
      if (cmd === 'remote_library' && args.query) {
        return { tracks: window.searchResults, total: window.searchResults.length };
      }
      if (cmd === 'remote_transfers') {
        return (window.asked ?? [])
          .filter((fileId) => !(window.arrived ?? []).includes(fileId))
          .map((fileId) => ({
            fileId,
            filename: 'Sharp Dressed Man.wav',
            size: 1234567,
            progress: window.fetching?.progress ?? 12,
            status: window.statuses?.[fileId] ?? window.fetching?.status ?? 'Downloading',
            speed: '',
            destination: ''
          }));
      }
      if (cmd === 'remote_download') {
        // Recorded here as well as in `calls`, because the transfer list below is
        // derived from it: a real host lists what it is fetching, and a mock that
        // answered with somebody else's row would make the phone believe a file had
        // already arrived.
        window.asked = [...(window.asked ?? []), args.fileId];
        if (window.refuse) {
          // A host that will not fetch this one says so instead of starting a
          // transfer, which is what a lent pairing and a refused file look like.
          await invoke(cmd, args).catch(() => {});
          throw window.refuse;
        }
        return invoke(cmd, args);
      }
      return invoke(cmd, args);
    };
  }, { library, results, fetching, holds, refuse });
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
/**
 * The phone, with the network's own list as the only thing worth playing.
 *
 * That list is other people's catalogue entries, so one file can be reachable
 * through two of them.
 */
async function openDiscoverApp(page, discovered) {
  // One track of this phone's own, so the run has a runway to place - and one the
  // network's list does not name, so it stays that.
  const library = [first];
  await mockNative(page, { platform: 'android' });
  await page.route('**/fixture.wav', serveAudio);
  await page.addInitScript(({ library, discovered }) => {
    window.remoteLibrary = library;
    window.discoverTracks = discovered;
  }, { library, discovered });
  await page.goto('http://127.0.0.1:15174');
}

test('a list that names one file twice still opens, as one row', async ({ page }) => {
  // The whole list used to refuse to draw for a list like this - the network's
  // rows, and the playlist built from them - since both are drawn keyed by file
  // and a keyed list that names one file twice is a list Svelte will not draw at
  // all. Nothing said so: the section was simply not there, and the playlist was
  // not there either.
  const twice = [away, { ...away }, furtherAway];
  await openDiscoverApp(page, twice);
  await page.locator('.bottom-nav button[data-tab="search"]').click();
  await page.locator('section[aria-label="Discover"] .track-open').first().click();

  await openQueue(page);
  // A file is one thing however many entries name it, so the row is there once -
  // and the count is the count of files rather than of entries.
  await expect(page.locator('.queue-row', { hasText: 'Sharp Dressed Man' })).toHaveCount(1);
  await expect(page.locator('.queue-row', { hasText: 'Legs' })).toHaveCount(1);
  await expect(page.locator('.queue-view')).toContainText('3 tracks');
});

async function addFoundTrack(page, result = found) {
  await page.locator('.bottom-nav button[data-tab="search"]').click();
  const input = page.getByRole('textbox', { name: 'Search tracks', exact: true });
  await input.fill('zz top');
  await input.press('Enter');
  const row = page.locator('.track-row', { hasText: result.title });
  await expect(row).toBeVisible();
  await row.locator('.track-more').click();
  await page.getByRole('button', { name: 'Add to queue' }).click();
}

/** The files the phone has asked the computer to fetch, in the order it asked. */
const downloads = (page) =>
  page.evaluate(() =>
    window.calls
      .filter((call) => call.cmd === 'remote_download')
      .map((call) => call.args.fileId)
  );

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

test('a queued track the computer already holds plays instead of asking again', async ({ page }) => {
  // The row was made when nobody here had the file and the computer does now, so
  // asking to download it is refused with "already on this computer". Waiting for
  // the download that refusal describes is a player stuck for ever - which is what
  // a turn of a queued network track did instead of playing it.
  const held = { ...away, local: true, sources: [] };
  await openApp(page, { library: [held], results: [{ ...away }], holds: [held] });
  await addFoundTrack(page, away);

  await expect.poll(() => audioPaused(page)).toBe(false);
  // The refusal is not something to report: the file is taken over Iroh and plays.
  await expect(page.locator('.error-banner')).toHaveCount(0);
  const asked = await page.evaluate(() =>
    window.calls.filter((call) => call.cmd === 'remote_download').length
  );
  expect(asked).toBe(1);
});

test('a fetch the computer refuses does not leave the player waiting for ever', async ({ page }) => {
  // Nobody here holds this one, so playing it asks the computer to fetch it - and
  // the computer refuses, which is what a lent pairing or a file the host will not
  // take looks like. The wait for those bytes has to end when the refusal does:
  // left standing it disables the player for the rest of the session, and the
  // track that was asked for can never be asked for again.
  await openApp(page, {
    library: [],
    results: [{ ...away }],
    refuse: 'This pairing is read only. It cannot ask Napstr to download songs.'
  });
  // Queued while nothing is playing, which is the one route into this that goes
  // through `playTrack` - the same path a queued network track's turn takes.
  await addFoundTrack(page, away);

  // The refusal is what the person is told, in the host's own words.
  await expect(page.locator('.error-banner')).toContainText('read only');
  // And the bar is not left showing that it is fetching something that will never
  // arrive: its button is a button again. `caching` is the flag this reads, and it
  // is the flag the next tap is measured against.
  await expect(page.locator('.now-play')).toBeEnabled();

  // The decisive part: the same track can be asked for a second time, from the
  // queue it is in. `playTrack` refuses to start at all while it believes a fetch
  // is running, so a tap that lands is a tap that found the flag clear.
  await openQueue(page);
  await page.locator('.queue-row').first().locator('.queue-open').click();
  await expect.poll(async () => (await downloads(page)).length).toBe(2);
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

test('a queued track nobody here holds is fetched while it is still songs away', async ({ page }) => {
  await openApp(page, { results: [away, furtherAway] });
  await playFirst(page);
  await addFoundTrack(page, away);
  await addFoundTrack(page, furtherAway);

  // The playlist is the library that was playing and then the two network files,
  // so the first is two songs away and the second three. A phone cannot reach the
  // network itself: the computer fetches what nobody here holds, two at a time and
  // in the order it is asked, so being asked now is what gets these here before
  // their turn rather than after it has arrived.
  await expect.poll(() => downloads(page)).toEqual([away.fileId, furtherAway.fileId]);
  // Who holds each one travels with the ask, because the computer has never seen
  // either file and that list is the whole of what it knows about getting it.
  const asked = await page.evaluate(() =>
    window.calls.filter((call) => call.cmd === 'remote_download').map((call) => call.args.sourcePubkeys)
  );
  expect(asked).toEqual([[id('1')], [id('2')]]);
});

test('a download the computer gives up on leaves the queue, so the next one is asked for', async ({ page }) => {
  // Two of these three files are never going to arrive, and this phone asks for two
  // at a time. While it waits on a pair the computer has silently given up on, the
  // third is never asked for at all: both ends show a queue that is not moving, and
  // the only thing that changes it is being told the two are not coming.
  const stuck = elsewhere('e', 'Rough Boy');
  const alsoStuck = elsewhere('f', 'Velcro Fly');
  const later = elsewhere('g', 'Sleeping Bag');
  await openApp(page, { results: [stuck, alsoStuck, later] });
  await playFirst(page);
  await addFoundTrack(page, stuck);
  await addFoundTrack(page, alsoStuck);
  await addFoundTrack(page, later);
  await expect.poll(() => downloads(page)).toEqual([stuck.fileId, alsoStuck.fileId]);
  // The drawer is the only place the queue is drawn, and it is opened before the
  // verdicts arrive: a screen opened after the stall is a screen that cannot be
  // reached, because the bar is showing a fetch that never ends.
  await openQueue(page);
  await expect(queueRows(page)).toHaveCount(5);

  // The computer gives up on both: the rows it was waiting on end as failures, and
  // that is the whole of what reaches this phone - there is no callback for it, and
  // a row that simply stopped appearing would read as "not started yet".
  await page.evaluate((fileIds) => {
    window.statuses = Object.fromEntries(
      fileIds.map((fileId) => [fileId, 'Failed: no seeder accepted the request'])
    );
  }, [stuck.fileId, alsoStuck.fileId]);

  // The dead entries are gone rather than sitting in the playlist as turns nobody can
  // take: the player would stop on each of them every time round.
  await expect
    .poll(async () => (await queueRows(page).allTextContents()).join('\n'), { timeout: 15_000 })
    .not.toContain('Rough Boy');
  await expect(queueRows(page)).toHaveCount(3);
  const titles = (await queueRows(page).allTextContents()).join('\n');
  expect(titles).not.toContain('Velcro Fly');

  // The slots those two were holding are free, so the file behind them - which
  // nothing had asked for while the pair was stalled - is asked for now. This is
  // the part a person notices: with the dead entries left in place, no later track
  // is ever asked for at all and the queue simply stops.
  await expect.poll(() => downloads(page), { timeout: 15_000 }).toContain(later.fileId);
});

test('a turn that has not arrived says how far along it is, then plays', async ({ page }) => {
  // One file, in the library and in the search, with nothing of the phone's own
  // anywhere: adding it to an empty playlist starts it, so this is a turn arriving
  // with nothing to play.
  await openApp(page, {
    library: [{ ...away }],
    results: [{ ...away }],
    fetching: { fileId: away.fileId, progress: 12, status: 'Downloading' }
  });
  await addFoundTrack(page, away);

  // Not an error and not silence: the bar names what is being fetched and says how
  // far along it is, which is the one thing the player can honestly report while
  // the computer is getting it.
  await expect(page.locator('.now-copy small')).toContainText('Fetching');
  await expect(page.locator('.now-copy small')).toContainText('12%');
  expect(await audioPaused(page)).toBe(true);

  // It lands: the computer stops listing it, and the file is this phone's now, so
  // the phone plays it - the wait was the download, not the decision.
  await page.evaluate(() => {
    window.arrived = [window.asked[0]];
    window.searchResults = window.searchResults.map((item) => ({ ...item, local: true }));
    window.remoteLibrary = window.remoteLibrary.map((item) => ({ ...item, local: true }));
  });
  await expect.poll(() => audioPaused(page)).toBe(false);
  await expect(page.locator('.now-copy small')).toContainText('ZZ Top');
});
