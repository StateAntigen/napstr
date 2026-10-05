import { test, expect } from '@playwright/test';
import { mockNative, serveAudio } from './helpers/native.mjs';

/**
 * Which computer's library the phone is looking at.
 *
 * A phone may be paired with more than one computer now. Exactly one of them lets
 * it act as its owner; the others are libraries to read from, so browsing a
 * friend's music means asking *their* computer and nobody else's.
 *
 * The choice is offered only where there is one to make: a phone holding a single
 * computer asks the same computer it always did and sees the same screen.
 */
const id = (letter) => letter.repeat(64);

const track = (fileId, title) => ({
  fileId,
  filename: `${title}.mp3`,
  title,
  artist: 'Artist',
  album: 'Album',
  format: 'MP3',
  mime: 'audio/mpeg',
  size: 4_000_000,
  tags: '',
  local: true,
  sources: [],
  bitrateKbps: 128,
  sampleRateHz: 44_100,
  channels: 2,
  lossless: false,
  durationMs: 200_000
});

const mine = track(id('a'), 'My Song');
const theirs = track(id('b'), 'Ada Ripped This');

const full = { browse: true, fetch: true, control: true, download: true, privileged: true };
const readable = { browse: true, fetch: true, control: false, download: false, privileged: false };

const myComputer = { endpointId: 'endpoint', desktopName: 'My Napstr', rights: full, home: true, included: true, online: true, mayDownload: true };
const friend = { endpointId: id('f'), desktopName: "Ada's Napstr", rights: readable, home: false, included: true, online: true, mayDownload: false };

async function openApp(page, { known = [myComputer, friend], library = [mine], libraries = {}, fileHosts = {}, offline = [] } = {}) {
  await mockNative(page, { platform: 'android' });
  await page.route('**/fixture.wav', serveAudio);
  await page.addInitScript(({ known, library, libraries, fileHosts, offline }) => {
    window.remoteHosts = known;
    window.remoteLibrary = library;
    window.remoteLibraryByHost = libraries;
    window.fileHosts = fileHosts;
    // Which of them answered just now, which is a different question from what
    // they hold: a computer can be paired, hold a library and be asleep.
    window.hostsOffline = offline;
    // The hardware back button, the way Android drives it: the page is handed a
    // press only while it advertises a destination, and a press it is not given
    // leaves the app.
    window.backAvailable = null;
    window.appExits = 0;
    window.NapstrfyBack = {
      setBackAvailable: (available) => { window.backAvailable = available; },
      setDrawerOpen: (open) => { window.backAvailable = open; }
    };
  }, { known, library, libraries, fileHosts, offline });
  await page.goto('http://127.0.0.1:15174');
}

/**
 * Presses the hardware back button the way Android does, and reports which of the
 * two things happened: the page took the press, or it was never offered it.
 */
async function pressBack(page) {
  return page.evaluate(() => {
    if (!window.backAvailable) {
      window.appExits += 1;
      return 'exit';
    }
    window.backAvailable = false;
    window.dispatchEvent(new CustomEvent('napstrfy-back'));
    return 'handled';
  });
}

const lastLibraryCall = (page) =>
  page.evaluate(() => {
    const calls = window.calls.filter((call) => call.cmd === 'remote_library');
    return { count: calls.length, source: calls.at(-1)?.args?.source ?? null };
  });

test('Napstrfy shows every computer\u2019s music as one list', async ({ page }) => {
  await openApp(page, { libraries: { [friend.endpointId]: [theirs] } });

  // The phone's own music first, then what the friend holds, and no control for
  // choosing between them: there is nothing to choose any more.
  await expect(page.locator('.track-row strong')).toHaveText(['My Song', 'Ada Ripped This']);
  await expect(page.locator('.source-picker')).toHaveCount(0);
  const asked = await lastLibraryCall(page);
  expect(asked.source).toBe(null);
});

test('Napstrfy marks a track that came from another computer', async ({ page }) => {
  await openApp(page, {
    libraries: { [friend.endpointId]: [theirs] },
    fileHosts: { [theirs.fileId]: id('f') }
  });

  const rows = page.locator('.track-row');
  await expect(rows).toHaveCount(2);
  // The phone's own row is unmarked; the friend's carries a mark in that
  // computer's colour and names it.
  await expect(rows.first().locator('.track-badge.elsewhere')).toHaveCount(0);
  await expect(rows.last().locator('.track-badge.elsewhere')).toHaveCount(1);
  await expect(rows.last().locator('.track-badge')).toHaveAttribute('aria-label', "Stored on Ada's Napstr");
});

/** Opens the settings sheet. */
async function openSettings(page) {
  await page.getByLabel('Settings').click();
  await expect(page.getByRole('button', { name: 'Add a computer' })).toBeVisible();
}

test('Napstrfy lists the computers it may read, and forgets one without the rest', async ({ page }) => {
  const asked = [];
  page.on('dialog', (dialog) => {
    asked.push(dialog.message());
    void dialog.accept();
  });
  await openApp(page);
  await openSettings(page);

  const rows = page.locator('.computer-row');
  await expect(rows).toHaveCount(2);
  // The computer this phone acts through is named as such, so the two are not
  // just two names.
  await expect(rows.first()).toContainText('My Napstr');
  await expect(rows.first()).toContainText("This phone's home computer");
  await expect(rows.last()).toContainText("Ada's Napstr");

  await page.getByRole('button', { name: "Forget Ada's Napstr" }).click();

  // The forgetting is the computer's to do, so the call is the assertion - and
  // the person is asked before anything happens.
  await expect
    .poll(() => page.evaluate(() => window.calls.filter((call) => call.cmd === 'forget_mobile_host').map((call) => call.args.endpointId)))
    .toEqual([id('f')]);
  expect(asked.some((message) => message.includes("Forget Ada's Napstr"))).toBe(true);
  // The computer went, and the row that adds one is still there.
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText('My Napstr');
  await expect(page.getByRole('button', { name: 'Add a computer' })).toBeVisible();
  // And the library on screen is the phone's own computer's again.
  await expect(page.locator('.track-row strong')).toHaveText(['My Song']);
});

test('Napstrfy adds a computer from settings, and the choice appears', async ({ page }) => {
  await openApp(page, { known: [myComputer] });
  // One computer is all this phone may read for now.
  await expect(page.locator('.track-row strong')).toHaveText(['My Song']);

  await openSettings(page);
  await expect(page.locator('.computer-row')).toHaveCount(1);
  await page.getByRole('button', { name: 'Add a computer' }).click();

  // The pairing screen, said the way a phone that already holds a computer
  // should hear it. It covers the app rather than sitting inside it.
  await expect(page.getByText('Another computer.')).toBeVisible();
  await expect(page.locator('.app-shell')).toHaveCount(0);
  await expect(page.locator('.pair-copy')).toContainText('one home computer that signs and downloads for it');

  // The same way in as the first pairing: a code, pasted.
  await page.locator('.manual-pair summary').click();
  await page.locator('.manual-pair textarea').fill('napstrfy://pair/example');
  await page.locator('.manual-pair button').click();

  // Back in the app, holding two computers, so both are listed and both are read.
  await expect(page.locator('.app-shell')).toBeVisible();
  await expect(page.locator('.track-row strong')).toHaveText(['My Song']);
  await openSettings(page);
  await expect(page.locator('.computer-row')).toHaveCount(2);
  await expect(page.locator('.computer-row').last()).toContainText('Music computer');
});

test('Napstrfy leaves the adding screen on a back press rather than the app', async ({ page }) => {
  await openApp(page, { known: [myComputer] });
  await openSettings(page);
  await page.getByRole('button', { name: 'Add a computer' }).click();
  await expect(page.getByText('Another computer.')).toBeVisible();

  // Android only gives the press because the page said it could take it.
  expect(await page.evaluate(() => window.backAvailable)).toBe(true);
  expect(await pressBack(page)).toBe('handled');
  await expect(page.locator('.app-shell')).toBeVisible();
  await expect(page.getByText('Another computer.')).toHaveCount(0);
  expect(await page.evaluate(() => window.appExits)).toBe(0);
});

test('Napstrfy leaves a computer out without unpairing it', async ({ page }) => {
  await openApp(page, { libraries: { [friend.endpointId]: [theirs] } });
  await expect(page.locator('.track-row strong')).toHaveText(['My Song', 'Ada Ripped This']);

  await openSettings(page);
  const friendRow = page.locator('.computer-row').last();
  await expect(friendRow).toContainText('Connected');
  await friendRow.locator('.computer-include input').uncheck();

  // The switch is the computer's to keep, so the call is the assertion - and the
  // library on screen follows it, with the computer still paired.
  await expect
    .poll(() => page.evaluate(() => window.calls.filter((call) => call.cmd === 'set_mobile_host_included').map((call) => String(call.args.included))))
    .toEqual(['false']);
  await expect(friendRow).toContainText('Left out');
  await expect(page.locator('.track-row strong')).toHaveText(['My Song']);
  await expect(page.locator('.computer-row')).toHaveCount(2);
});

test('Napstrfy counts the computers that answered in the status line', async ({ page }) => {
  await openApp(page, { known: [{ ...friend, online: false }, myComputer] });

  const chip = page.locator('.status-chip');
  // One dot per computer, and the one that did not answer is set apart from the
  // one that did.
  await expect(chip.locator('.status-dots i')).toHaveCount(2);
  await expect(chip.locator('.status-dots i.on')).toHaveCount(1);
  await expect(chip.locator('.status-dots i.away')).toHaveCount(1);
  await expect(chip).toContainText('1/2 Online');
});

/**
 * The case the label used to get wrong.
 *
 * The status line was drawn from the computer the phone acts through, and only
 * counted the others once that one was connected - so a phone whose own computer
 * was asleep said "Offline" while a friend's computer sat there answering, and
 * "1/2 Online" could not be reached at all. It is also the reason the library
 * looked empty: the app was told there was nothing to draw.
 */
test('a phone whose own computer is asleep still reads the one that answered', async ({ page }) => {
  await openApp(page, {
    known: [{ ...myComputer, online: false }, friend],
    libraries: { [friend.endpointId]: [theirs] },
    offline: [myComputer.endpointId]
  });

  const chip = page.locator('.status-chip');
  await expect(chip).toContainText('1/2 Online');
  await expect(chip).not.toHaveClass(/offline/);
  await expect(chip.locator('.status-dots i.on')).toHaveCount(1);

  // And the music that computer holds is on screen: the question is asked of
  // every computer either way, and one that is not there is a library with
  // nothing in it rather than a reason to draw none of them.
  await expect(page.locator('.track-row strong')).toHaveText(['Ada Ripped This']);
});

test('Napstrfy lets the home computer be chosen, and keeps the choice', async ({ page }) => {
  await openApp(page);
  await openSettings(page);

  const rows = page.locator('.computer-row');
  await expect(rows.first()).toContainText("This phone's home computer");
  await expect(rows.last()).toContainText('A library this phone reads');
  // Only one row offers the choice, because the home one already is.
  const choose = page.locator('.computer-row button:has-text("Make home")');
  await expect(choose).toHaveCount(1);

  await rows.last().getByRole('button', { name: "Make Ada's Napstr the home computer" }).click();

  // The choice is the phone's to keep, so the call is the assertion - and the
  // list on screen follows it rather than guessing.
  await expect
    .poll(() => page.evaluate(() => window.calls.filter((call) => call.cmd === 'set_mobile_home_host').map((call) => call.args.endpointId)))
    .toEqual([id('f')]);
  await expect(rows.last()).toContainText("This phone's home computer");
  await expect(rows.first()).toContainText('A library this phone reads');
  await expect(choose).toHaveCount(1);
});

test('a cold start says it is connecting rather than offline, then connects once', async ({ page }) => {
  await mockNative(page, { platform: 'android' });
  await page.addInitScript((row) => {
    window.remoteLibrary = [row];
    const invoke = window.__TAURI_INTERNALS__.invoke;
    let asked = 0;
    window.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
      // The first question finds no tunnel, which is where every launch begins.
      // The phone's own channel reports an attempt in flight rather than a
      // failure, and the next question is what finds the tunnel - so the app has
      // to be told the truth about the wait instead of being told it is offline.
      if (cmd === 'companion_status' && asked++ === 0) {
        return { ...(await invoke(cmd, args)), connected: false, connecting: true };
      }
      // The second one takes long enough to be seen, so the first assertion is
      // about the word rather than about a race.
      if (cmd === 'companion_status') await new Promise((done) => window.setTimeout(done, 1000));
      return invoke(cmd, args);
    };
  }, mine);
  await page.goto('http://127.0.0.1:15174');

  const chip = page.locator('.status-chip');
  // The word a person used to see for the first seconds of every cold start.
  await expect(chip).toContainText('Connecting');
  await expect(chip).toContainText('Music computer');
  await expect(page.locator('.track-row strong')).toHaveText(['My Song']);
});

test('a computer that cannot be reached still says offline rather than connecting', async ({ page }) => {
  await openApp(page, { known: [myComputer] });
  // The chip is the reconnect button, and this is the state it is there for.
  await page.evaluate(() => { window.desktopReachable = false; });
  await page.locator('.status-chip').click();
  await expect(page.locator('.status-chip')).toContainText('Offline');
});
