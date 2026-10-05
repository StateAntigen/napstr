import { test, expect } from '@playwright/test';
import { mockNative, serveAudio } from './helpers/native.mjs';

// Playback speed, on the phone's own player.
//
// Speed and not pitch: the rate lives on the element, so the voices and the
// instruments stay where the artist put them and only the tempo moves. A queue
// carries it from one file to the next because it is a property of the player
// rather than of the track, which is why nothing here has to save it per song.
//
// The computer playing is a different machine, and its own player: the chip that
// opens this panel is only drawn when this phone is the one playing.

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

async function openApp(page, { stored = null } = {}) {
  await mockNative(page, { platform: 'android' });
  await page.route('**/fixture.wav', serveAudio);
  await page.addInitScript(({ library, stored }) => {
    window.remoteLibrary = library;
    window.backAvailable = null;
    window.appExits = 0;
    window.NapstrfyBack = {
      setBackAvailable: (available) => { window.backAvailable = available; },
      setDrawerOpen: (open) => { window.backAvailable = open; }
    };
    if (stored !== null) window.localStorage.setItem('napstrfy-playback-speed', String(stored));
  }, { library: [song], stored });
  await page.goto('http://127.0.0.1:15174');
}

const audioPaused = (page) => page.evaluate(() => document.querySelector('audio')?.paused ?? true);
const rate = (page) => page.evaluate(() => document.querySelector('audio')?.playbackRate ?? -1);
const pitchKept = (page) => page.evaluate(() => document.querySelector('audio')?.preservesPitch ?? false);
const storedRate = (page) => page.evaluate(() => window.localStorage.getItem('napstrfy-playback-speed'));

/** Play the one track there is, open the drawer over it, and open the panel. */
async function openSpeed(page) {
  await page.locator('.track-open').first().click();
  await expect.poll(() => audioPaused(page)).toBe(false);
  await page.locator('.now-open').click();
  await expect(page.locator('.now-sheet')).toBeVisible();
  await page.getByRole('button', { name: 'Playback speed' }).click();
  await expect(page.locator('.speed-panel')).toBeVisible();
}

/** Presses back the way Android does, and says whether the page took the press. */
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

test('the chip reads the rate and the panel is where it is changed', async ({ page }) => {
  await openApp(page);
  await openSpeed(page);

  await expect(page.locator('.speed-panel h2')).toContainText('Playback speed');
  await expect(page.locator('.speed-panel .speed-read')).toContainText('Speed: 1×');
  // The two ends are on screen, so the range is legible before it is dragged.
  await expect(page.locator('.speed-panel .speed-ends')).toContainText('0.5×');
  await expect(page.locator('.speed-panel .speed-ends')).toContainText('2.5×');

  await page.getByRole('slider', { name: 'Playback speed' }).fill('1.5');

  await expect(page.locator('.speed-panel .speed-read')).toContainText('Speed: 1.5×');
  await expect.poll(() => rate(page)).toBe(1.5);
  // The promise the panel's own words make: tempo changes, pitch does not.
  expect(await pitchKept(page)).toBe(true);
});

test('the rate is the phone’s, so it is written down and comes back with it', async ({ page }) => {
  await openApp(page);
  await openSpeed(page);

  await page.getByRole('slider', { name: 'Playback speed' }).fill('1.75');

  await expect.poll(() => storedRate(page)).toBe('1.75');
  // The round trip, and the harder half of it: a phone left at 1.75 starts at
  // 1.75, and the rate survives the file being loaded - which is the moment the
  // element forgets a rate it was not told to keep.
  await page.reload();
  await page.locator('.track-open').first().click();
  await expect.poll(() => audioPaused(page)).toBe(false);
  await expect.poll(() => rate(page)).toBe(1.75);
});

test('the slider’s ends are the ends of the rate', async ({ page }) => {
  // A rate written down by a build whose ends were wider than this one's is clamped
  // rather than trusted: the control could not put it back, and a slider that cannot
  // reach the rate in force is worse than one the rate was cut down to.
  await openApp(page, { stored: 9 });
  await openSpeed(page);

  const slider = page.getByRole('slider', { name: 'Playback speed' });
  await expect(slider).toHaveAttribute('min', '0.5');
  await expect(slider).toHaveAttribute('max', '2.5');
  await expect(page.locator('.speed-panel .speed-read')).toContainText('Speed: 2.5×');
  await expect.poll(() => rate(page)).toBe(2.5);
});

test('the rate reads as a rate: no trailing zeros, no long decimals', async ({ page }) => {
  await openApp(page);
  await openSpeed(page);

  const read = page.locator('.speed-panel .speed-read');
  await page.getByRole('slider', { name: 'Playback speed' }).fill('2');
  await expect(read).toContainText('Speed: 2×');
  await page.getByRole('slider', { name: 'Playback speed' }).fill('0.5');
  await expect(read).toContainText('Speed: 0.5×');
});

test('back closes the panel rather than the drawer it was opened on', async ({ page }) => {
  await openApp(page);
  await openSpeed(page);
  await expect.poll(() => page.evaluate(() => window.backAvailable)).toBe(true);

  expect(await pressBack(page)).toBe('handled');

  await expect(page.locator('.speed-panel')).toHaveCount(0);
  await expect(page.locator('.now-sheet')).toBeVisible();
  // And the chip is still there, showing the rate that is still in force.
  await expect(page.getByRole('button', { name: 'Playback speed' })).toContainText('Speed: 1×');
  expect(await page.evaluate(() => window.appExits)).toBe(0);
});
