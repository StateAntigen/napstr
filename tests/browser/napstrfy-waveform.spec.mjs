import { test, expect } from '@playwright/test';
import { mockNative, serveAudio } from './helpers/native.mjs';

// The track's own shape, on the player's timeline.
//
// What a person reads from a waveform is where the quiet parts are and how far in
// they are, so it is worked out on this phone, from the file this phone is playing,
// and reduced twice on the way: decoded at a low rate, then kept as one number per
// bar. The control underneath is still the slider it always was - a drag, a
// keyboard and a screen reader all go on working - so these tests are about what is
// drawn, and about that control still working while it is drawn over.
//
// The fixture this plays is a silent WAV, so the shape that comes out of it is a
// flat line of bars. That is enough to prove the bars are there, that there are the
// number the drawer claims, and that the filled part of them follows the play-head.
// The shape itself - the slicing and the scaling - is a pure function of the
// samples and is tested as one in tests/waveform.test.mjs.

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

const song = track('a', 'Doubleback');

async function openApp(page) {
  await mockNative(page, { platform: 'android' });
  await page.route('**/fixture.wav', serveAudio);
  await page.addInitScript(({ library }) => {
    window.remoteLibrary = library;
  }, { library: [song] });
  await page.goto('http://127.0.0.1:15174');
}

const audioPaused = (page) => page.evaluate(() => document.querySelector('audio')?.paused ?? true);
const audioPosition = (page) => page.evaluate(() => document.querySelector('audio')?.currentTime ?? -1);

/** Play the one track there is and open the drawer over it, paused. */
async function playAndOpen(page) {
  await page.locator('.track-open').first().click();
  await expect.poll(() => audioPaused(page)).toBe(false);
  await page.locator('.now-open').click();
  await expect(page.locator('.now-sheet')).toBeVisible();
  // Paused before anything is measured: a track that is still playing moves under
  // the assertions, and "the filled part follows the play-head" would then be a
  // race rather than a fact.
  await page.locator('.play-main').click();
  await expect.poll(() => audioPaused(page)).toBe(true);
}

test('the timeline becomes the track’s own shape once the file has been read', async ({ page }) => {
  await openApp(page);
  await playAndOpen(page);

  // The bars are the drawer's own count, so the drawing and the arithmetic behind
  // the played part of it cannot drift apart.
  await expect(page.locator('.now-sheet-timeline.waves')).toBeVisible();
  await expect(page.locator('.now-sheet-wave rect')).toHaveCount(140);
  // Back to the beginning: at the start of the track, no bar is behind the
  // play-head. (Not asserted from wherever playback happened to be paused, which is
  // however long the file took to start.)
  await page.getByRole('slider', { name: 'Seek' }).fill('0');
  await expect(page.locator('.now-sheet-wave rect.played')).toHaveCount(0);
});

test('the filled part of the shape is the part behind the play-head', async ({ page }) => {
  await openApp(page);
  await playAndOpen(page);

  const timeline = page.getByRole('slider', { name: 'Seek' });
  const played = page.locator('.now-sheet-wave rect.played');
  const filled = () => played.count();

  // A minute of audio: a third of the way in is a third of the bars, give or take
  // the bar the play-head is standing on.
  await timeline.fill('20');
  await expect.poll(() => audioPosition(page)).toBe(20);
  await expect.poll(filled).toBeGreaterThan(44);
  expect(await filled()).toBeLessThan(50);

  await timeline.fill('50');
  await expect.poll(() => audioPosition(page)).toBe(50);
  await expect.poll(filled).toBeGreaterThan(114);
  expect(await filled()).toBeLessThan(120);
});

test('the shape is drawn over the control rather than instead of it', async ({ page }) => {
  // The one thing that must not break: the slider under the drawing is what seeks,
  // and what a screen reader finds. It is invisible, not absent.
  await openApp(page);
  await playAndOpen(page);
  await expect(page.locator('.now-sheet-timeline.waves')).toBeVisible();

  const timeline = page.getByRole('slider', { name: 'Seek' });
  await expect(timeline).toHaveAttribute('max', '60');
  await expect(timeline).toBeEnabled();

  await timeline.fill('45');
  await expect.poll(() => audioPosition(page)).toBe(45);
  await expect.poll(() => page.locator('.now-sheet-wave rect.played').count()).toBeGreaterThan(100);
});
