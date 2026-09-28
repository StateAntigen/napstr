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

const full = { browse: true, fetch: true, control: true, privileged: true };
const readable = { browse: true, fetch: true, control: false, privileged: false };

const myComputer = { endpointId: 'endpoint', desktopName: 'My Napstr', rights: full, primary: true };
const friend = { endpointId: id('f'), desktopName: "Ada's Napstr", rights: readable, primary: false };

async function openApp(page, { known = [myComputer, friend], library = [mine], libraries = {} } = {}) {
  await mockNative(page, { platform: 'android' });
  await page.route('**/fixture.wav', serveAudio);
  await page.addInitScript(({ known, library, libraries }) => {
    window.remoteHosts = known;
    window.remoteLibrary = library;
    window.remoteLibraryByHost = libraries;
    // The hardware back button, the way Android drives it: the page is handed a
    // press only while it advertises a destination, and a press it is not given
    // leaves the app.
    window.backAvailable = null;
    window.appExits = 0;
    window.NapstrfyBack = {
      setBackAvailable: (available) => { window.backAvailable = available; },
      setDrawerOpen: (open) => { window.backAvailable = open; }
    };
  }, { known, library, libraries });
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

test('Napstrfy browses the library of the computer it is told to', async ({ page }) => {
  await openApp(page, { libraries: { [friend.endpointId]: [theirs] } });

  const picker = page.locator('.source-picker select');
  // The choice is offered because this phone holds two computers.
  await expect(picker).toBeVisible();
  await expect(picker).toHaveAttribute('aria-label', "Which computer's library");
  // It begins on the computer this phone acts through, which is what the app has
  // always shown.
  await expect(picker).toHaveValue('');
  await expect(page.locator('.track-row strong')).toHaveText(['My Song']);

  await picker.selectOption({ label: "Ada's Napstr" });

  await expect(page.locator('.track-row strong')).toHaveText(['Ada Ripped This']);
  // The asking went to their computer rather than to the one this phone acts
  // through, which is the whole of the difference being made here.
  const asked = await lastLibraryCall(page);
  expect(asked.source).toBe(id('f'));

  // Choosing the phone's own computer again asks the same way it did before.
  await picker.selectOption({ label: 'My Napstr' });
  await expect(page.locator('.track-row strong')).toHaveText(['My Song']);
  expect((await lastLibraryCall(page)).source).toBe(null);
});

test('Napstrfy offers no choice when it holds a single computer', async ({ page }) => {
  await openApp(page, { known: [myComputer] });

  await expect(page.locator('.track-row strong')).toHaveText(['My Song']);
  // Nothing to pick between, so nothing is offered and the screen is untouched.
  await expect(page.locator('.source-picker')).toHaveCount(0);
  // And the asking still names no computer, which is what every phone that has
  // ever been paired does.
  expect((await lastLibraryCall(page)).source).toBe(null);
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
  await expect(rows.first()).toContainText('This phone acts through this one');
  await expect(rows.last()).toContainText("Ada's Napstr");

  await page.getByRole('button', { name: "Forget Ada's Napstr" }).click();

  // The forgetting is the computer's to do, so the call is the assertion - and
  // the person is asked before anything happens.
  await expect
    .poll(() => page.evaluate(() => window.calls.filter((call) => call.cmd === 'forget_mobile_host').map((call) => call.args.endpointId)))
    .toEqual([id('f')]);
  expect(asked.some((message) => message.includes("Forget Ada's Napstr"))).toBe(true);
  // One computer is not a choice, so the list gives way to the row that adds one.
  await expect(rows).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Add a computer' })).toBeVisible();
  // And the library on screen is still this phone's own computer's.
  await expect(page.locator('.track-row strong')).toHaveText(['My Song']);
});

test('Napstrfy adds a computer from settings, and the choice appears', async ({ page }) => {
  await openApp(page, { known: [myComputer] });
  // One computer is not a choice, which is why the picker is not there yet.
  await expect(page.locator('.source-picker')).toHaveCount(0);

  await openSettings(page);
  await expect(page.locator('.computer-row')).toHaveCount(0);
  await page.getByRole('button', { name: 'Add a computer' }).click();

  // The pairing screen, said the way a phone that already holds a computer
  // should hear it. It covers the app rather than sitting inside it.
  await expect(page.getByText('Another computer.')).toBeVisible();
  await expect(page.locator('.app-shell')).toHaveCount(0);
  await expect(page.locator('.pair-copy')).toContainText("A code from someone else's Napstr is read-only");

  // The same way in as the first pairing: a code, pasted.
  await page.locator('.manual-pair summary').click();
  await page.locator('.manual-pair textarea').fill('napstrfy://pair/example');
  await page.locator('.manual-pair button').click();

  // Back in the app, holding two computers, so the choice is offered now.
  await expect(page.locator('.app-shell')).toBeVisible();
  await expect(page.locator('.source-picker select')).toBeVisible();
  await expect(page.locator('.source-picker option')).toHaveCount(2);
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
