import { test, expect } from '@playwright/test';
import { mockNative, serveAudio } from './helpers/native.mjs';

/**
 * The conversation around a track, on the phone.
 *
 * The messages are public and the computer fetches them, so a phone may read a
 * discussion whoever is holding it. Posting is the one act here that speaks in the
 * user's own name on a public relay, and it is signed by the computer: a lent
 * phone may not, and is not offered the box.
 */
const id = (letter) => letter.repeat(64);

const song = {
  fileId: id('a'),
  filename: 'Song.mp3',
  title: 'Song',
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
};

const said = (index, name, pubkey, content) => ({
  eventId: `event-${index}`,
  pubkey,
  npub: `npub1${name.toLowerCase()}`,
  displayName: name,
  content,
  createdAt: 1_800_000_000 + index
});

const themFirst = said(0, 'Ada', id('b'), 'This rip is clean.');
const mineLast = said(1, 'Me', id('c'), 'Agreed.');

async function openApp(page, { messages = [], streamOnly = false } = {}) {
  await mockNative(page, { platform: 'android' });
  await page.route('**/fixture.wav', serveAudio);
  await page.addInitScript(({ song, messages, streamOnly }) => {
    window.remoteLibrary = [song];
    window.discussionMessages = messages;
    window.streamOnly = streamOnly;
  }, { song, messages, streamOnly });
  await page.goto('http://127.0.0.1:15174');
  await expect(page.locator('.track-row strong')).toHaveText(['Song']);
}

/** The track menu, then the row that opens the conversation. */
async function openDiscussion(page) {
  await page.locator('.track-more').first().click();
  await page.getByRole('button', { name: 'Track discussion' }).click();
  await expect(page.locator('.discussion-view')).toBeVisible();
}

const callsTo = (page, cmd) => page.evaluate((name) => window.calls.filter((call) => call.cmd === name).length, cmd);

test('Napstrfy shows a track conversation and marks what this identity said', async ({ page }) => {
  await openApp(page, { messages: [themFirst, mineLast] });
  await openDiscussion(page);

  const rows = page.locator('.discussion-message');
  await expect(rows).toHaveCount(2);
  await expect(rows.first()).toContainText('This rip is clean.');
  await expect(rows.last()).toContainText('Agreed.');
  // The computer's own key is what tells the phone which of these are its user's,
  // so their own comments are drawn differently without any local history.
  await expect(rows.first()).not.toHaveClass(/mine/);
  await expect(rows.last()).toHaveClass(/mine/);
});

test('Napstrfy posts a comment by asking the computer to sign it', async ({ page }) => {
  await openApp(page, { messages: [themFirst] });
  await openDiscussion(page);

  await page.locator('.discussion-compose input').fill('Where did you find this?');
  await page.locator('.discussion-compose button').click();

  await expect.poll(() => callsTo(page, 'remote_send_track_discussion')).toBe(1);
  const sent = await page.evaluate(() => window.calls.find((call) => call.cmd === 'remote_send_track_discussion').args);
  expect(sent.fileId).toBe(song.fileId);
  expect(sent.content).toBe('Where did you find this?');
  // The box empties and the comment is read back rather than assumed.
  await expect(page.locator('.discussion-compose input')).toHaveValue('');
  await expect(page.locator('.discussion-message')).toHaveCount(2);
  await expect(page.locator('.discussion-message').last()).toContainText('Where did you find this?');
});

test('Napstrfy does not offer a lent phone the box it may not use', async ({ page }) => {
  await openApp(page, { messages: [themFirst], streamOnly: true });
  await openDiscussion(page);

  // It may read, because the messages are public and the computer fetches them...
  await expect(page.locator('.discussion-message')).toHaveCount(1);
  // ...and it may not post, so there is no composer and no way to try.
  await expect(page.locator('.discussion-compose')).toHaveCount(0);
  await expect(page.locator('.discussion-view .settings-note')).toContainText('read only');
  expect(await callsTo(page, 'remote_send_track_discussion')).toBe(0);
});
