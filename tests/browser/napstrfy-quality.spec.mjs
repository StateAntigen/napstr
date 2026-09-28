import { test, expect } from '@playwright/test';
import { mockNative, serveAudio } from './helpers/native.mjs';

/**
 * Music quality is the phone's setting, because the phone is what pays.
 *
 * These tests pin down the two halves of that: what this phone asks for, and
 * what it says when it does not ask. The tracks carry the facts Napstr reads out
 * of each file - the format, and the bitrate the container declares - so a test
 * can tell a policy that judged the file from one that only looked at its name.
 */
const id = (letter) => letter.repeat(64);

const track = (letter, { title, format = 'MP3', lossless = false, bitrateKbps = 128, size = 4_000_000 } = {}) => ({
  fileId: id(letter),
  filename: `${title}.${format.toLowerCase()}`,
  title,
  artist: 'Artist',
  album: 'Album',
  format,
  mime: `audio/${format.toLowerCase()}`,
  size,
  tags: '',
  local: true,
  sources: [],
  bitrateKbps,
  sampleRateHz: 44_100,
  channels: 2,
  lossless,
  durationMs: 200_000
});

const losslessSong = track('a', { title: 'Loud and big', format: 'FLAC', lossless: true, bitrateKbps: 900, size: 42_000_000 });
const otherLosslessSong = track('c', { title: 'Also loud', format: 'FLAC', lossless: true, bitrateKbps: 900, size: 42_000_000 });
const plainSong = track('b', { title: 'Small and fine', format: 'MP3', bitrateKbps: 128 });
const heavySong = track('d', { title: 'Heavy but small', format: 'MP3', bitrateKbps: 320, size: 9_000_000 });

/** The two profiles, as a test needs to seed one of them. */
const profiles = (metered = {}) => ({
  unmetered: { formats: ['MP3', 'FLAC', 'WAV', 'OGG', 'OPUS'], maxBitrateKbps: 0 },
  metered: { formats: ['MP3', 'OGG', 'OPUS'], maxBitrateKbps: 0, ...metered }
});

async function openApp(page, { library = [losslessSong, plainSong], metered = true, cached = [], quality = null } = {}) {
  await mockNative(page, { platform: 'android' });
  await page.route('**/fixture.wav', serveAudio);
  await page.addInitScript(({ library, metered, cached, quality }) => {
    // The bridge the Android webview adds. `metered` is read on every ask, so a
    // test can move the phone onto another connection mid-run if it wants to.
    window.NapstrfyNetwork = {
      metered: () => metered,
      kind: () => (metered ? 'metered' : 'unmetered')
    };
    window.cachedLibrary = cached;
    if (quality) window.localStorage.setItem('napstrfy-quality', JSON.stringify(quality));
    const invoke = window.__TAURI_INTERNALS__.invoke;
    window.__TAURI_INTERNALS__.invoke = async (cmd, args = {}) => {
      if (cmd === 'remote_library' && !args.query) return { tracks: library, total: library.length };
      return invoke(cmd, args);
    };
  }, { library, metered, cached, quality });
  await page.goto('http://127.0.0.1:15174');
  await expect(page.locator('.track-row strong')).toHaveText(library.map((item) => item.title));
}

const play = (page, title) => page.locator('.track-row').filter({ hasText: title }).locator('.track-open').click();
const callsTo = (page, cmd) => page.evaluate((name) => window.calls.filter((call) => call.cmd === name).length, cmd);

/**
 * Waits until the phone is ready for another track.
 *
 * One track is loaded at a time (`caching` in the page), and a tap that arrives
 * while the last one is still being fetched and started is dropped rather than
 * queued - which is the app's own behaviour, and not what any of these are about.
 */
async function settle(page) {
  await expect
    .poll(() => page.evaluate(() => Boolean(document.querySelector('audio')) && !document.querySelector('audio').paused))
    .toBe(true);
  await page.waitForTimeout(400);
}

/** Records every dialog the page opens and answers it the way the test asks. */
function answerDialogs(page, answer = 'accept') {
  const messages = [];
  page.on('dialog', (dialog) => {
    messages.push(dialog.message());
    void (answer === 'accept' ? dialog.accept() : dialog.dismiss());
  });
  return messages;
}

test('Napstrfy asks once before spending mobile data on a lossless track', async ({ page }) => {
  await openApp(page, { library: [losslessSong, otherLosslessSong, plainSong], metered: true });
  const dialogs = answerDialogs(page);

  await play(page, 'Loud and big');
  await expect.poll(() => dialogs.length).toBe(1);
  expect(dialogs[0]).toContain('Lossless files are held back on mobile data');
  expect(dialogs[0]).toContain('Fetch it anyway?');
  await expect.poll(() => callsTo(page, 'cache_remote_audio')).toBe(1);
  await settle(page);

  // The same answer covers the rest of the session, because a queue is many
  // tracks and a question per track is an argument rather than a choice.
  await play(page, 'Also loud');
  await expect.poll(() => callsTo(page, 'cache_remote_audio')).toBe(2);
  expect(dialogs.length).toBe(1);
});

test('Napstrfy leaves a held-back track alone when the answer is no', async ({ page }) => {
  await openApp(page, { metered: true });
  answerDialogs(page, 'dismiss');

  await play(page, 'Loud and big');
  await expect(page.locator('.toast')).toContainText('Lossless files are held back on mobile data');
  expect(await callsTo(page, 'cache_remote_audio')).toBe(0);
});

test('Napstrfy plays a held-back track that is already on this phone', async ({ page }) => {
  // Nothing is being spent on a file this phone holds, and a setting about data
  // must not stop music that is already here.
  await openApp(page, { metered: true, cached: [losslessSong] });
  const dialogs = answerDialogs(page);

  await play(page, 'Loud and big');
  await expect.poll(() => callsTo(page, 'cache_remote_audio')).toBe(1);
  expect(dialogs).toEqual([]);
});

test('Napstrfy holds nothing back over Wi-Fi unless it is asked to', async ({ page }) => {
  await openApp(page, { metered: false });
  const dialogs = answerDialogs(page);

  await play(page, 'Loud and big');
  await expect.poll(() => callsTo(page, 'cache_remote_audio')).toBe(1);
  expect(dialogs).toEqual([]);
});

test('Napstrfy prefetches the next track when it fits', async ({ page }) => {
  // The control for the test below: the warm-ahead does happen, so an absence
  // there is the setting and not a path that never runs.
  await openApp(page, { library: [plainSong, track('e', { title: 'Also small', format: 'MP3' })], metered: true });
  await play(page, 'Small and fine');
  await expect.poll(() => callsTo(page, 'prefetch_remote_audio')).toBe(1);
});

test('Napstrfy does not warm ahead into a track it would hold back', async ({ page }) => {
  await openApp(page, { library: [plainSong, losslessSong], metered: true });
  await play(page, 'Small and fine');
  await expect.poll(() => callsTo(page, 'cache_remote_audio')).toBe(1);
  // The warm-ahead is asked for once the file being played has landed, so give
  // that moment room to pass before saying it never came.
  await page.waitForTimeout(500);
  expect(await callsTo(page, 'prefetch_remote_audio')).toBe(0);
});

test('Napstrfy holds back a file the host reported as over the ceiling', async ({ page }) => {
  await openApp(page, {
    library: [heavySong],
    metered: true,
    quality: profiles({ maxBitrateKbps: 128 })
  });
  const dialogs = answerDialogs(page, 'dismiss');

  await play(page, 'Heavy but small');
  await expect.poll(() => dialogs.length).toBe(1);
  expect(dialogs[0]).toContain('320 kb/s is above the 128 kb/s');
  expect(await callsTo(page, 'cache_remote_audio')).toBe(0);
});

test('Napstrfy keeps the quality setting on the phone', async ({ page }) => {
  await openApp(page, { metered: true });
  await page.locator('.header-icon[aria-label="Settings"]').click();

  const row = page.getByRole('button', { name: /On mobile data/ });
  await expect(row).toContainText('MP3, OGG, OPUS');
  await expect(row).toContainText('No limit');
  // The Wi-Fi profile is the permissive one, which is what makes the two rows
  // worth drawing apart.
  await expect(page.getByRole('button', { name: /On Wi-Fi/ })).toContainText('All formats');

  await row.click();
  await page.getByRole('button', { name: 'OGG', exact: true }).click();
  await page.getByRole('button', { name: '192 kb/s', exact: true }).click();
  await expect(row).toContainText('MP3, OPUS');
  await expect(row).toContainText('Up to 192 kb/s');

  // It is this phone's setting rather than the computer's, so it is stored here
  // and it is still there after a reload.
  const stored = await page.evaluate(() => window.localStorage.getItem('napstrfy-quality'));
  expect(JSON.parse(stored).metered).toEqual({ formats: ['MP3', 'OPUS'], maxBitrateKbps: 192 });
  await page.reload();
  await page.locator('.header-icon[aria-label="Settings"]').click();
  await expect(page.getByRole('button', { name: /On mobile data/ })).toContainText('Up to 192 kb/s');
});

test('Napstrfy shows what each file is on the row it is listed on', async ({ page }) => {
  await openApp(page, { library: [losslessSong, plainSong], metered: false });
  const meta = page.locator('.track-row').filter({ hasText: 'Loud and big' }).locator('.track-meta');
  await expect(meta).toContainText('FLAC · lossless · 900 kb/s');
  await expect(page.locator('.track-row').filter({ hasText: 'Small and fine' }).locator('.track-meta')).toContainText('MP3 · 128 kb/s');
});

test("Napstrfy's now playing drawer repeats the file's bitrate rather than working it out", async ({ page }) => {
  // 4 MB over 200 seconds is 160 kb/s by arithmetic; the file says 128. The
  // drawer has to say what the file says, since that is the audio and not the
  // whole file.
  await openApp(page, { library: [plainSong], metered: false });
  await play(page, 'Small and fine');
  await settle(page);
  await page.locator('.now-playing .now-open').click();
  await expect(page.locator('.now-sheet-copy em')).toHaveText('MP3 · 128 kb/s');
});
