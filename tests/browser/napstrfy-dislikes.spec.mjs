import { test, expect } from '@playwright/test';
import { mockNative, serveAudio } from './helpers/native.mjs';

/**
 * The songs this phone will not play again.
 *
 * A list of its own rather than a sign on the liked one, because a track can be
 * liked and turned off at once and neither answer may move the other. It is kept
 * the same way the likes are - this phone's own copy under this phone's key,
 * mirrored to the computer it acts through - and it is deliberately reachable
 * from a pairing that may not sign or publish anything, since a phone lent only
 * a library is the one with most reason to want a track gone.
 *
 * What it does is narrow on purpose: the player stops *choosing* the track. The
 * person still can, by tapping it, which is why these tests check both halves.
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
const other = track(id('b'), 'Another Song');

const full = { browse: true, fetch: true, control: true, download: true, privileged: true };

const myComputer = { endpointId: 'endpoint', desktopName: 'My Napstr', rights: full, home: true, included: true, online: true, mayDownload: true };

async function openApp(page, { library = [mine, other], known = [myComputer], hostDislikes = [], dislikes = [], orphans = [], synced = [], hostLikes = [], rights = full } = {}) {
  await mockNative(page, { platform: 'android' });
  await page.route('**/fixture.wav', serveAudio);
  await page.addInitScript(({ library, known, hostDislikes, dislikes, orphans, synced, hostLikes, rights }) => {
    window.remoteHosts = [{ endpointId: 'endpoint', desktopName: 'My Napstr', rights, home: true, included: true, online: true, mayDownload: true }];
    window.remoteLibrary = library;
    window.hostDislikes = hostDislikes;
    window.hostLikes = hostLikes;
    // The hardware back button, the way Android drives it: the page is handed a
    // press only while it advertises a destination.
    window.backAvailable = null;
    window.appExits = 0;
    window.NapstrfyBack = {
      setBackAvailable: (available) => { window.backAvailable = available; },
      setDrawerOpen: (open) => { window.backAvailable = open; }
    };
    if (dislikes.length > 0) window.localStorage.setItem('napstrfy-disliked-music', JSON.stringify(dislikes));
    if (orphans.length > 0) window.localStorage.setItem('napstrfy-disliked-orphans', JSON.stringify(orphans));
    if (synced.length > 0) window.localStorage.setItem('napstrfy-dislikes-synced', JSON.stringify(synced));
  }, { library, known, hostDislikes, dislikes, orphans, synced, hostLikes, rights });
  await page.goto('http://127.0.0.1:15174');
}

/** The calls the phone made, by command name. */
const calls = (page, cmd) => page.evaluate((name) => window.calls.filter((call) => call.cmd === name).map((call) => call.args), cmd);

/** The ids this phone has written down, which is what its own list is. */
const stored = (page) =>
  page.evaluate(() => JSON.parse(window.localStorage.getItem('napstrfy-disliked-music') ?? '[]').map((row) => row.fileId));

async function openDisliked(page) {
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.locator('.settings-row', { hasText: 'Songs not played again' }).click();
  await expect(page.locator('.disliked-page')).toBeVisible();
}

/** Presses back the way Android does, and says whether the page took it. */
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

test('a dislike is written down on this phone and on the computer it acts through', async ({ page }) => {
  await openApp(page, { library: [mine] });
  await page.locator('button[data-tab="music"]').click();
  await expect(page.locator('.track-row')).toHaveCount(1);

  await page.locator('.track-row .track-more').first().click();
  await page.getByRole('button', { name: 'Never play this again' }).click();

  // This phone's copy first, because that is the one that is drawn.
  await expect.poll(() => stored(page)).toEqual([mine.fileId]);
  // And the computer's copy, as ids under this phone's key.
  await expect.poll(() => calls(page, 'remote_set_dislikes')).toContainEqual({ fileIds: [mine.fileId] });
});

test('the toast says what happened and carries the way back', async ({ page }) => {
  // The only toast in the app with an action on it, and it exists because a
  // dislike is a guess: made with a thumb, on a dashboard, about a track that
  // may have started badly. It has to be one press to take back.
  await openApp(page, { library: [mine] });
  await page.locator('button[data-tab="music"]').click();
  await page.locator('.track-row .track-more').first().click();
  await page.getByRole('button', { name: 'Never play this again' }).click();

  const toast = page.locator('.toast.toast-action');
  await expect(toast).toContainText('My Song');
  await expect(toast).toContainText('will not be played again');

  await toast.getByRole('button', { name: 'Undo' }).click();

  await expect.poll(() => stored(page)).toEqual([]);
  // The computer is told about the change back too, or the next merge would
  // return the track to this list.
  await expect.poll(() => calls(page, 'remote_set_dislikes')).toContainEqual({ fileIds: [] });
  await expect(page.locator('.toast.toast-action')).toHaveCount(0);
  await expect(page.locator('.toast')).toContainText('can be played again');
});

test('a list the computer holds is merged in, and one this phone dropped is not resurrected', async ({ page }) => {
  // Two different reasons the two copies disagree, and they have to be told
  // apart: an id this phone never had is somebody turning something off
  // elsewhere, and an id it had and dropped was dropped on purpose.
  await openApp(page, {
    library: [mine, other],
    hostDislikes: [other.fileId, mine.fileId],
    dislikes: [],
    synced: [mine.fileId]
  });

  await openDisliked(page);

  // Only the computer's own addition arrives: the id this phone had already
  // agreed on and dropped stays dropped.
  await expect(page.locator('.disliked-page .track-row strong')).toHaveText(['Another Song']);
  await expect.poll(() => stored(page)).toEqual([other.fileId]);
});

test('the list is in Settings, says how long it is, and each row carries the way back', async ({ page }) => {
  await openApp(page, { library: [mine], dislikes: [mine], hostDislikes: [mine.fileId] });

  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(page.locator('.settings-row', { hasText: 'Songs not played again' })).toContainText('1 turned off');

  await page.locator('.settings-row', { hasText: 'Songs not played again' }).click();
  await expect(page.locator('.disliked-page')).toBeVisible();
  await expect(page.locator('.disliked-page .track-row strong')).toHaveText(['My Song']);

  // Named rather than just "Allow again": the button's own label says which
  // track it lets back in, so a person reading it aloud hears the answer.
  await page.getByRole('button', { name: 'Allow My Song again' }).click();

  await expect(page.locator('.disliked-page .track-row')).toHaveCount(0);
  await expect(page.locator('.disliked-page')).toContainText('Nothing is turned off');
  await expect.poll(() => stored(page)).toEqual([]);
  await expect.poll(() => calls(page, 'remote_set_dislikes')).toContainEqual({ fileIds: [] });
});

test('back leaves the list rather than the app', async ({ page }) => {
  // The page is opened from Settings, which is already gone by the time it is
  // drawn, so back has to look for it on its own or the press leaves the app.
  await openApp(page, { library: [mine], dislikes: [mine] });
  await openDisliked(page);
  await expect.poll(() => page.evaluate(() => window.backAvailable)).toBe(true);

  expect(await pressBack(page)).toBe('handled');
  await expect(page.locator('.disliked-page')).toHaveCount(0);
  expect(await page.evaluate(() => window.appExits)).toBe(0);
});
