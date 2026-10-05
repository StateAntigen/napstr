import { test, expect } from '@playwright/test';
import { mockNative, serveAudio } from './helpers/native.mjs';

// The network's own list, on the tab where somebody goes looking. It is chosen by
// the computer out of the catalogue it already mirrors rather than searched for,
// and every row is a file no computer of yours holds — so playing one is a fetch,
// and the seeders the row names are what the fetch is asked for.

const id = (letter) => letter.repeat(64);
const track = (letter, title, local) => ({
  fileId: id(letter),
  filename: `${title}.mp3`,
  title,
  artist: 'ZZ Top',
  album: '',
  format: 'MP3',
  mime: 'audio/mpeg',
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
async function openApp(page, { library = [inLibrary], discovered = live, arrives = null, preload = null, streamOnly = false, mayDownload = null } = {}) {
  await mockNative(page, { platform: 'android' });
  await page.route('**/fixture.wav', serveAudio);
  await page.addInitScript(({ library, discovered, arrives, preload, streamOnly, mayDownload }) => {
    window.remoteLibrary = library;
    window.discoverTracks = discovered;
    // What the pairing allows, said the way the app is told it: the older
    // `streamOnly` is the signature, and reaching the network is its own answer
    // beside it. `mayDownload: true` with `streamOnly: true` is the pairing this
    // right exists for - a phone lent the network that may sign nothing.
    window.streamOnly = streamOnly;
    if (mayDownload !== null) window.mayDownload = mayDownload;
    if (preload !== null) window.localStorage.setItem('napstrfy-preload-depth', String(preload));
    const invoke = window.__TAURI_INTERNALS__.invoke;
    window.__TAURI_INTERNALS__.invoke = async (cmd, args = {}) => {
      if (cmd === 'remote_transfers' && arrives) {
        return [{ fileId: arrives, filename: 'Doubleback.wav', size: 1234567, progress: 100, status: 'Verified · Complete', speed: '', destination: '' }];
      }
      return invoke(cmd, args);
    };
  }, { library, discovered, arrives, preload, streamOnly, mayDownload });
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

/**
 * The pairing the new right was separated for.
 *
 * Downloads and signatures used to be one thing, so a phone that could fill
 * itself with music could also publish under its owner's name. This is the
 * combination that is supposed to be possible now: the network, and nothing
 * said in anybody's name.
 */
test('a phone lent the network but not the signature may fetch a row and may publish nothing', async ({ page }) => {
  await openApp(page, { preload: 0, streamOnly: true, mayDownload: true });
  await openSearchTab(page);

  // The list is offered, because reaching the network is what this pairing may do.
  const section = page.locator('section[aria-label="Discover"]');
  await expect(section).toBeVisible();
  await expect(section.locator('.track-row')).toHaveCount(2);

  // And playing a row it does not hold is allowed: the ask is the proof.
  await section.locator('.track-open').first().click();
  const downloads = await expect.poll(() => calls(page, 'remote_download')).toHaveLength(1).then(() => calls(page, 'remote_download'));
  expect(downloads[0].fileId).toBe(live[0].fileId);
});

/** The other half of the same split: no network right is no list at all. */
test('a phone that may not reach the network is shown no list to play from', async ({ page }) => {
  await openApp(page, { preload: 0, streamOnly: true, mayDownload: false });
  await openSearchTab(page);

  await expect(page.locator('section[aria-label="Discover"]')).toHaveCount(0);
  await expect.poll(() => calls(page, 'remote_download')).toHaveLength(0);
});

test('play all leads with a track of this phone’s own, and asks for the first row', async ({ page }) => {
  // The library holds one track that this run does not name, so that is the lead -
  // and there is nothing of ours left to warm behind the first row. One deep, so
  // the ask proved here is the first row of the list and nothing else.
  const mine = {
    ...inLibrary,
    fileId: id('e'),
    filename: 'Cheap Sunglasses.mp3',
    title: 'Cheap Sunglasses'
  };
  const wanted = track('b', 'Doubleback', false);
  // Only one track of this phone's own, and the run does not name it - so the lead
  // is that track and there is nothing else of ours for the runway. The file the
  // computer says this phone already holds is avoided on purpose: a cached file is
  // skipped by the pre-load, so a row with that id would prove nothing here.
  await openApp(page, { library: [mine], discovered: [wanted, live[1]], preload: 1 });
  await expect(page.locator('.track-row')).toHaveCount(1);
  await openSearchTab(page);
  await page.locator('.discover-play').click();

  // It makes a sound at once, rather than after the first row has been fetched.
  await expect.poll(() => audioPaused(page)).toBe(false);
  // One deep, so the first row of the list is the whole of what is asked for.
  await expect.poll(() => files(page, 'remote_download')).toEqual([wanted.fileId]);

  // And the queue is that track, then the row the computer ranked first.
  await page.locator('.now-open').click();
  await expect(page.locator('.now-sheet')).toBeVisible();
  await page.locator('[aria-label="Open the playlist"]').click();
  await expect(page.locator('.queue-row').nth(0)).toContainText('Cheap Sunglasses');
  await expect(page.locator('.queue-row').nth(1)).toContainText('Doubleback');
});

test('a run does not park on a track that has not arrived: the nearest one that can play is pulled in', async ({ page }) => {
  // The run names two files nobody here holds and the library has two of its own,
  // so the queue is an owned track, a fetch, another owned track, a fetch. The
  // entry after what Play all opened with is a fetch that has not landed - and
  // waiting for it is what left the player looking stuck: it stood on a download
  // for minutes, or until that download failed. Advancing skips it instead.
  const mine = ['f', 'g'].map((letter, index) => ({
    ...inLibrary,
    fileId: id(letter),
    filename: `Mine ${index + 1}.mp3`,
    title: `Mine ${index + 1}`
  }));
  // Nothing is warmed, so the only thing that could make a sound is a track that
  // was already here: the fetches are never started.
  await openApp(page, { library: mine, discovered: live, preload: 0 });
  await expect(page.locator('.track-row')).toHaveCount(2);
  await openSearchTab(page);
  await page.locator('.discover-play').click();
  await expect.poll(() => audioPaused(page)).toBe(false);

  const playing = () => page.locator('.now-title').innerText();
  const opened = await playing();
  expect(mine.map((track) => track.title)).toContain(opened);

  // The next entry is the first row of the network's list, which this computer has
  // not fetched, so the run takes the nearest entry that can play instead.
  await page.locator('.now-open').click();
  await page.getByLabel('Next track').click();
  await expect.poll(playing).not.toBe(opened);
  const advanced = await playing();
  expect(mine.map((track) => track.title)).toContain(advanced);

  // And it says what it passed over, rather than appearing to lose a track.
  await expect(page.locator('.toast')).toContainText('Gimme All Your Lovin');
  await expect(page.locator('.toast')).toContainText('has not arrived yet');

  // The queue was reformed rather than merely walked: the track that could play is
  // now in the position the player was about to take, and the fetch it passed over
  // is behind it - still in the run, so its turn comes round if it does land.
  await page.locator('[aria-label="Open the playlist"]').click();
  const order = await page.locator('.queue-row .queue-copy strong').allTextContents();
  expect(order[0]).toBe(opened);
  expect(order[1]).toBe(advanced);
  expect(order[2]).toBe('Gimme All Your Lovin');
  expect(order[3]).toBe('Sharp Dressed Man');
});

test('the list draws only the file types it is asked for', async ({ page }) => {
  // One format for now: anything larger is a fetch this phone would rather not pay
  // for. The row is left out of the drawing rather than out of the answer - the
  // label above still says how many the network has, because that is what it says.
  await openApp(page, {
    discovered: [
      track('b', 'Gimme All Your Lovin', false),
      { ...track('d', 'Legs', false), format: 'FLAC', mime: 'audio/flac' }
    ]
  });
  await openSearchTab(page);

  const rows = page.locator('section[aria-label="Discover"] .track-row');
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText('Gimme All Your Lovin');
  await expect(page.locator('.section-label', { hasText: 'Discover' })).toContainText('2 live on the network');
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
  // Three tracks of this phone's own, because a Play all takes one of them as its
  // lead and the run then uses each of the rest once: with a single track the lead
  // is all there is, and nothing of ours is left for the rows further down.
  const mine = ['f', 'g', 'h'].map((letter, index) => ({
    ...inLibrary,
    fileId: id(letter),
    filename: `Mine ${index + 1}.mp3`,
    title: `Mine ${index + 1}`
  }));
  await openApp(page, { library: mine, discovered: list, preload: 5 });
  // The library has to be here before the run starts, and the music tab is where it
  // is listed: the tracks of ours that go in front of and between the new ones are
  // ones this phone holds, so a run started before they arrived would have nothing
  // to put there.
  await expect(page.locator('.track-row')).toHaveCount(3);
  await openSearchTab(page);
  await page.locator('.discover-play').click();

  // Asked for in play order, and no further ahead than the computer can run: it
  // fetches two at a time, so a third ask would queue behind those two and put the
  // track about to play behind the ones after it.
  await expect.poll(() => files(page, 'remote_download')).toEqual([list[0].fileId, list[1].fileId]);
  // And the track of ours is genuinely in the run - it is the one being taken over
  // Iroh before its turn, which is what says the queue holds it between the new
  // ones rather than after them.
  const prefetched = await expect
    .poll(() => files(page, 'prefetch_remote_audio'))
    .not.toHaveLength(0)
    .then(() => files(page, 'prefetch_remote_audio'));
  expect(prefetched.some((fileId) => mine.some((item) => item.fileId === fileId))).toBe(true);
});

test('a row of the list starts the list as the queue from that row', async ({ page }) => {
  const list = [
    track('b', 'Gimme All Your Lovin', false),
    track('c', 'Sharp Dressed Man', false),
    track('d', 'Legs', false),
    track('e', 'Tush', false)
  ];
  // Three tracks of this phone's own, because the run uses each of them once: with
  // only one, its place is taken by the first row and nothing of ours is left for
  // the rows further down - which is the whole reason the runway is bounded.
  const mine = ['f', 'g', 'h'].map((letter, index) => ({
    ...inLibrary,
    fileId: id(letter),
    filename: `Mine ${index + 1}.mp3`,
    title: `Mine ${index + 1}`,
    format: 'MP3',
    mime: 'audio/mpeg'
  }));
  await openApp(page, { library: mine, discovered: list, preload: 2 });
  await expect(page.locator('.track-row')).toHaveCount(3);
  await openSearchTab(page);
  await page.locator('section[aria-label="Discover"] .track-row').nth(2).locator('.track-open').click();

  // The run is network, ours, network... so the third network row is the fifth entry
  // and the two asks are its own file and the one after it: the list from that row,
  // rather than the whole thing from the top.
  await expect.poll(() => files(page, 'remote_download')).toEqual([list[2].fileId, list[3].fileId]);
  // And one of ours really is in the run, being taken over Iroh after the row that
  // was tapped - which is what says the runway is filled that far down.
  const prefetched = await expect
    .poll(() => files(page, 'prefetch_remote_audio'))
    .not.toHaveLength(0)
    .then(() => files(page, 'prefetch_remote_audio'));
  expect(prefetched.some((fileId) => mine.some((item) => item.fileId === fileId))).toBe(true);
});

test('the list stays the queue when a track fetched for it lands', async ({ page }) => {
  // The row the list leads with is a file this phone *already holds* - which the
  // computer does not know, because its discover list leaves out only what the
  // computer itself has. So the fetch lands on a file that is both in the queue
  // and on this phone, which is the case that used to name one file twice in a
  // keyed list and lose the whole queue, drawer and all.
  await openApp(page, {
    library: [inLibrary],
    discovered: [track('a', 'Doubleback', false), track('c', 'Sharp Dressed Man', false)],
    arrives: inLibrary.fileId,
    preload: 0
  });
  await expect(page.locator('.track-row')).toHaveCount(1);
  await openSearchTab(page);
  await page.locator('.discover-play').click();
  await expect.poll(() => audioPaused(page)).toBe(false);

  // The run is the queue - the two new files, once each - rather than the library
  // this phone happened to be showing.
  await page.locator('.now-open').click();
  await expect(page.locator('.now-sheet')).toBeVisible();
  await page.locator('[aria-label="Open the playlist"]').click();
  const rows = page.locator('.queue-row');
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0)).toContainText('Doubleback');
  await expect(rows.nth(1)).toContainText('Sharp Dressed Man');
  // And the row of the one playing is marked as the one playing.
  await expect(page.locator('.queue-row.playing')).toHaveCount(1);
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
