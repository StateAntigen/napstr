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
  }, { known, library, libraries });
  await page.goto('http://127.0.0.1:15174');
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
