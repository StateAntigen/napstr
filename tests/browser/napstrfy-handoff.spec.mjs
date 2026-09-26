import { test, expect } from '@playwright/test';
import { mockNative, serveAudio } from './helpers/native.mjs';

// Handing playback between the phone and the computer. Both directions are one
// switch of the source picker, and both have to leave exactly one device making
// a sound: the one the user chose.

const id = (letter) => letter.repeat(64);
const track = (letter, title, artist) => ({
  fileId: id(letter), filename: `${title}.wav`, title, artist, album: '',
  format: 'WAV', mime: 'audio/wav', size: 1234567, tags: '', local: true, sources: []
});

const first = track('a', 'Doubleback', 'ZZ Top');
const second = track('b', 'Gimme All Your Lovin', 'ZZ Top');

async function openApp(page, { seed = null } = {}) {
  await mockNative(page, { platform: 'android' });
  await page.route('**/fixture.wav', serveAudio);
  await page.addInitScript(({ seed }) => {
    window.remoteLibrary = [seed?.library ?? []].flat();
    window.remotePlaying = seed?.playing ?? null;
  }, { seed });
  await page.goto('http://127.0.0.1:15174');
}

/** The source picker, from the now-playing drawer's own button. */
async function openSourcePicker(page) {
  await page.locator('.now-open').click();
  await expect(page.locator('.now-sheet')).toBeVisible();
  await page.getByRole('button', { name: /^Play on:/ }).click();
  await expect(page.locator('.actions-row').first()).toBeVisible();
}

const computerRow = (page) => page.locator('.actions-row').nth(1);
const phoneRow = (page) => page.locator('.actions-row').first();
const commands = (page, type) =>
  page.evaluate((type) => window.calls.filter((call) => call.cmd === 'remote_playback' && call.args.command?.type === type).map((call) => call.args.command), type);
const audioPaused = (page) => page.evaluate(() => document.querySelector('audio')?.paused ?? true);
const remotePlaying = (page) => page.evaluate(() => window.remotePlaying?.playing ?? false);

test('Napstrfy hands the track, the queue and the second it had reached to the computer', async ({ page }) => {
  await openApp(page, { seed: { library: [first, second] } });
  // Play here first, then give it to the computer.
  await page.locator('.track-open').first().click();
  await expect.poll(() => audioPaused(page)).toBe(false);
  await openSourcePicker(page);
  await computerRow(page).click();

  const [handed] = await expect
    .poll(() => commands(page, 'playTrack'))
    .toHaveLength(1)
    .then(() => commands(page, 'playTrack'));
  // The list this phone was showing is the computer's queue, in its order.
  expect(handed.queue).toEqual([first.fileId, second.fileId]);
  expect(handed.fileId).toBe(first.fileId);
  // Where the phone had got to, so the computer resumes rather than restarts.
  expect(handed.positionMs).toBeGreaterThanOrEqual(0);
  // And the phone really did stop: one player at a time.
  await expect.poll(() => audioPaused(page)).toBe(true);
  await expect.poll(() => remotePlaying(page)).toBe(true);
});

test('Napstrfy takes playback over from the computer and stops it', async ({ page }) => {
  await openApp(page, { seed: { library: [first, second] } });
  // Play here first: the bar needs something loaded to open, and handing over to
  // the computer is how the computer comes to be the one playing.
  await page.locator('.track-open').nth(1).click();
  await expect.poll(() => audioPaused(page)).toBe(false);
  await openSourcePicker(page);
  await computerRow(page).click();
  await expect.poll(() => remotePlaying(page)).toBe(true);
  await expect.poll(() => audioPaused(page)).toBe(true);
  const sheet = page.locator('.now-sheet');
  await expect(sheet).toContainText('2 tracks there');
  await expect(sheet).toContainText(second.title);
  // The computer has been playing for a while by the time the user switches back.
  await page.evaluate(() => {
    window.remotePlaying = { ...window.remotePlaying, playing: true, positionMs: 31_000 };
  });

  await page.getByRole('button', { name: /^Play on:/ }).click();
  await phoneRow(page).click();

  // One request carries both halves of the handover: stop, and say what you were
  // doing. Two would let the computer move on in between.
  await expect.poll(() => commands(page, 'handoff')).toHaveLength(1);
  await expect.poll(() => remotePlaying(page)).toBe(false);
  // The track it was playing is playing here now, at the second it had got to.
  await expect.poll(() => audioPaused(page)).toBe(false);
  await expect
    .poll(() => page.evaluate(() => document.querySelector('audio')?.currentTime ?? 0))
    .toBeGreaterThan(30);
  await expect.poll(() => page.evaluate(() => window.remotePlaying.fileId)).toBe(second.fileId);
});
