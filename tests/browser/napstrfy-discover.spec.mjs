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

  const section = page.locator('section[aria-label="Network Library"]');
  await expect(section).toBeVisible();
  const rows = section.locator('.track-row');
  await expect(rows).toHaveCount(2);
  await expect(rows.first()).toContainText('Gimme All Your Lovin');
  // The heading says what this is not: not a library of this computer's own, but
  // what the network is holding - and it says the one thing a row here does not,
  // which is that nothing plays the moment it is picked.
  await expect(page.locator('.library-heading')).toContainText('Network Library');
  await expect(page.locator('.library-heading')).toContainText('Tracks hosted across the peer-to-peer network');
  await expect(page.locator('.library-heading')).toContainText('Each track buffers before it plays.');
  // And the button that starts the list is there with it, which is where the count
  // of the whole list used to be.
  await expect(page.locator('.discover-start')).toBeVisible();
  // Nothing of the library: an empty search tab is where the network's list goes,
  // and a list of what is already here is an answer to a search rather than a page
  // standing there before one has been made.
  await expect(page.locator('.track-list')).toHaveCount(1);
  await expect(page.locator('.track-row')).toHaveCount(2);

  // Searching is what brings it in.
  const input = page.getByRole('textbox', { name: 'Search tracks', exact: true });
  await input.fill('doubleback');
  await input.press('Enter');
  await expect(page.locator('section[aria-label="Network Library"]')).toHaveCount(0);
  await expect(page.locator('.track-row')).toHaveCount(1);
});

test('playing a row nobody here holds asks the computer to fetch it, from the row’s own seeders', async ({ page }) => {
  // Nothing is warmed ahead here, so the row's own ask is the whole of it.
  await openApp(page, { preload: 0 });
  await openSearchTab(page);
  await page.locator('section[aria-label="Network Library"] .track-open').first().click();

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
  const section = page.locator('section[aria-label="Network Library"]');
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

  await expect(page.locator('section[aria-label="Network Library"]')).toHaveCount(0);
  await expect.poll(() => calls(page, 'remote_download')).toHaveLength(0);
});

test('crate digging chooses the rarest end of the list, and starts only when asked', async ({ page }) => {
  // The library holds one track that this run does not name, so that is the lead -
  // and there is nothing of ours left to warm behind the first row, so one deep is
  // the whole of what is asked for.
  //
  // The two rows are deliberately in the *other* order in the fixture: the common
  // file first. Digging is a question about the far end of the computer's ranking,
  // so a phone that read the list as given would play the common one - which is
  // exactly what this rules out.
  const mine = {
    ...inLibrary,
    fileId: id('e'),
    filename: 'Cheap Sunglasses.mp3',
    title: 'Cheap Sunglasses'
  };
  const common = { ...track('b', 'Doubleback', false), seeders: 20 };
  const rare = { ...track('c', 'Sharp Dressed Man', false), seeders: 1 };
  await openApp(page, { library: [mine], discovered: [common, rare], preload: 1, arrives: rare.fileId });
  await expect(page.locator('.track-row')).toHaveCount(1);
  await openSearchTab(page);
  await page.getByRole('button', { name: 'Crate Digging' }).click();

  // The chip is a way of looking at the network rather than a decision to spend the
  // evening on it: the list changes, and nothing starts.
  await expect
    .poll(() => calls(page, 'remote_discover'))
    .toContainEqual(expect.objectContaining({ mode: 'leastSeeded' }));
  expect(await audioPaused(page)).toBe(true);

  // The button beside the title is what starts it, and the row the computer ranked
  // rarest is the one it starts on - the order was the computer's to choose, so the
  // other end of its ranking is what was asked for rather than a sort done here.
  // Nothing is heard until that row has been fetched, which is what the line under
  // the title says: the sound follows the file.
  await page.locator('.discover-start').click();
  // The row it starts on, and then the one after it - one deep, which is what the
  // preload depth in the fixture asks for. Both are network rows now, so both are
  // asks: the queue has nothing of ours in it to be warmed instead. That the sound
  // follows the file once it lands is asserted on its own, below.
  await expect.poll(() => files(page, 'remote_download')).toEqual([rare.fileId, common.fileId]);

  // And the queue is the list in its own order, with nothing of ours dealt between
  // the rows: a run of the network's music is a run of the network's music.
  await page.locator('.now-open').click();
  await expect(page.locator('.now-sheet')).toBeVisible();
  await page.locator('[aria-label="Open the playlist"]').click();
  await expect(page.locator('.queue-row')).toHaveCount(2);
  await expect(page.locator('.queue-row').nth(0)).toContainText('Sharp Dressed Man');
  await expect(page.locator('.queue-row').nth(1)).toContainText('Doubleback');
});

test('random deals every row of the list once, in an order nobody chose', async ({ page }) => {
  // A shuffle rather than a draw with repeats: over a long session a person should
  // still hear each row, and a row that came round twice would mean one that never
  // came round at all. The order itself cannot be asserted - it is dealt from a
  // random seed - so what is asserted is that it is a permutation, and that the
  // run still opens with something already here.
  const list = [
    track('b', 'Gimme All Your Lovin', false),
    track('c', 'Sharp Dressed Man', false),
    track('d', 'Legs', false),
    track('e', 'Tush', false)
  ];
  const mine = ['f', 'g', 'h'].map((letter, index) => ({
    ...inLibrary,
    fileId: id(letter),
    filename: `Mine ${index + 1}.mp3`,
    title: `Mine ${index + 1}`
  }));
  await openApp(page, { library: mine, discovered: list, preload: 0 });
  await expect(page.locator('.track-row')).toHaveCount(3);
  await openSearchTab(page);

  const drawn = await page.locator('section[aria-label="Network Library"] .track-row strong').allTextContents();
  await page.getByRole('button', { name: 'Random' }).click();

  // Dealt again, and not started: the same rows, in an order nobody chose.
  const played = await page.locator('section[aria-label="Network Library"] .track-row strong').allTextContents();
  expect([...played].sort()).toEqual([...drawn].sort());
  expect(played).not.toEqual(drawn);
  expect(await audioPaused(page)).toBe(true);

  // And the button starts that order as the queue: the row the shuffle put first is
  // the one asked for, and the whole list is behind it exactly as drawn.
  const first = list.find((item) => item.title === played[0]);
  await page.locator('.discover-start').click();
  await expect.poll(() => files(page, 'remote_download')).toEqual([first.fileId]);
  await page.locator('.now-open').click();
  await page.locator('[aria-label="Open the playlist"]').click();
  const queued = await page.locator('.queue-row .queue-copy strong').allTextContents();
  expect(queued).toEqual(played);
});

// Two tests stood here about the queue the network's list used to build: a track of
// ours after every one of its own, and the runway that filled the gaps. That order
// is the thing this suite now asserts is gone - the list is the queue, in its own
// order - so what they described cannot be asserted any more. The substitution they
// exercised is unchanged in the app; it now only has something to offer when the
// queue already holds a row that can play, which no fixture here builds.

test('the list draws only the file types it is asked for', async ({ page }) => {
  // One format for now: anything larger is a fetch this phone would rather not pay
  // for. The row is left out of the drawing rather than out of the answer.
  await openApp(page, {
    discovered: [
      track('b', 'Gimme All Your Lovin', false),
      { ...track('d', 'Legs', false), format: 'FLAC', mime: 'audio/flac' }
    ]
  });
  await openSearchTab(page);

  const rows = page.locator('section[aria-label="Network Library"] .track-row');
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText('Gimme All Your Lovin');
});

test('the network’s rows are dealt behind what is queued, not between it', async ({ page }) => {
  // What a chip shows is the network's list, and what the button does is put that
  // list in the queue: behind whatever is already there, in the list's own order,
  // with no track of ours dealt between the rows. Fetching one of the network's
  // files takes minutes, and what covers that is the substitution - a track of ours
  // taken when the moment comes, not promised into the order beforehand.
  const list = [
    track('b', 'Gimme All Your Lovin', false),
    track('c', 'Sharp Dressed Man', false),
    track('d', 'Legs', false),
    track('e', 'Tush', false)
  ];
  const mine = ['f', 'g', 'h'].map((letter, index) => ({
    ...inLibrary,
    fileId: id(letter),
    filename: `Mine ${index + 1}.mp3`,
    title: `Mine ${index + 1}`
  }));
  await openApp(page, { library: mine, discovered: list, preload: 5 });
  // One of this phone's own is playing before the network's list is asked for, so
  // the queue it goes behind is a real one rather than an empty list, which would
  // make the order prove nothing.
  await expect(page.locator('.track-row')).toHaveCount(3);
  await page.locator('.track-open').first().click();
  await expect.poll(() => audioPaused(page)).toBe(false);
  await openSearchTab(page);
  await page.getByRole('button', { name: 'Crate Digging' }).click();
  await page.locator('.discover-start').click();

  // Asked for in play order, and no further ahead than the computer can run: it
  // fetches two at a time, so a third ask would queue behind those two and put the
  // track about to play behind the ones after it.
  await expect.poll(() => files(page, 'remote_download')).toEqual([list[0].fileId, list[1].fileId]);

  // The queue is the library that was playing, and then the whole network list in
  // its own order: every row of it once, and nothing of ours among them.
  await page.locator('.now-open').click();
  await page.locator('[aria-label="Open the playlist"]').click();
  const order = await page.locator('.queue-row .queue-copy strong').allTextContents();
  expect(order).toEqual(['Mine 1', 'Mine 2', 'Mine 3', ...list.map((item) => item.title)]);
});

test('a row of the list plays that row, and leaves the rest of the list behind it', async ({ page }) => {
  const list = [
    track('b', 'Gimme All Your Lovin', false),
    track('c', 'Sharp Dressed Man', false),
    track('d', 'Legs', false),
    track('e', 'Tush', false)
  ];
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
  await page.locator('section[aria-label="Network Library"] .track-row').nth(2).locator('.track-open').click();

  // The tapped row is what plays, and the list carries on after it: the two asks are
  // its own file and the one after it, rather than the whole thing from the top.
  await expect.poll(() => files(page, 'remote_download')).toEqual([list[2].fileId, list[3].fileId]);

  // And the queue is the list in its own order, from its first row through to its
  // last, with the play-head standing on the row that was tapped.
  await page.locator('.now-open').click();
  await page.locator('[aria-label="Open the playlist"]').click();
  const order = await page.locator('.queue-row .queue-copy strong').allTextContents();
  expect(order).toEqual(list.map((item) => item.title));
  await expect(page.locator('.queue-row.playing')).toContainText('Legs');
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
  await page.getByRole('button', { name: 'Crate Digging' }).click();
  await page.locator('.discover-start').click();
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

  const rows = page.locator('section[aria-label="Network Library"] .track-row');
  await expect(rows).toHaveCount(120);
  // Four pages of thirty - a hundred is passed by the fourth, so the list is a
  // whole page longer than the length it was filled to.
  const offsets = await page.evaluate(() =>
    window.calls.filter((call) => call.cmd === 'remote_discover').map((call) => call.args.offset)
  );
  expect(offsets).toEqual([0, 30, 60, 90]);

  // Asking for more gets the rest of what the computer has said it holds, and then
  // stops: an empty page is not a reason to ask again.
  await page.locator('section[aria-label="Network Library"] .load-more').click();
  await expect(rows).toHaveCount(140);
  await expect(page.locator('section[aria-label="Network Library"] .load-more')).toHaveCount(0);
});
