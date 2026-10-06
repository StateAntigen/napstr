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
    // The hardware back button, the way Android drives it: the page is handed a
    // press only while it advertises a destination, and a press it is not given
    // leaves the app. The bridge is not in the browser, so it is recorded instead.
    window.backAvailable = null;
    window.appExits = 0;
    window.NapstrfyBack = {
      setBackAvailable: (available) => { window.backAvailable = available; },
      setDrawerOpen: (open) => { window.backAvailable = open; }
    };
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

test('Napstrfy posts a comment with its own key when the computer signs nothing', async ({ page }) => {
  await openApp(page, { messages: [themFirst], streamOnly: true });
  // This phone's own key, which is not the computer's: it is what makes the comment
  // the phone's to say, and what the assertion below about "mine" turns on.
  await page.addInitScript(() => {
    window.nostrIdentity = { pubkey: 'd'.repeat(64), npub: `npub1${'d'.repeat(58)}` };
  });
  await openDiscussion(page);

  // It may read, because the messages are public and the computer fetches them...
  await expect(page.locator('.discussion-message')).toHaveCount(1);
  // ...and it may write, because what it writes is its own. The comment is signed
  // with the key this phone made and never handed over, and the computer's part is
  // to put it on the relays.
  await page.locator('.discussion-compose input').fill('Where did you find this?');
  await page.locator('.discussion-compose button').click();

  await expect.poll(() => callsTo(page, 'remote_send_device_discussion')).toBe(1);
  // The path that spends the owner's name is not touched on the way past.
  expect(await callsTo(page, 'remote_send_track_discussion')).toBe(0);
  // The box empties, the comment is read back rather than assumed, and it is drawn
  // as this person's own: the phone's key is theirs too.
  await expect(page.locator('.discussion-compose input')).toHaveValue('');
  await expect(page.locator('.discussion-message')).toHaveCount(2);
  await expect(page.locator('.discussion-message').last()).toHaveClass(/mine/);
  await expect(page.locator('.discussion-message').last()).toContainText('Where did you find this?');
});

/** Plays the one track and opens the player sheet, which is where the card lives. */
async function openPlayer(page) {
  await page.locator('.track-open').first().click();
  await expect(page.locator('.now-playing:not(.empty)')).toBeVisible();
  await page.locator('.now-playing .now-open').click();
  await expect(page.locator('.now-sheet')).toBeVisible();
  // The sheet slides up from the bottom, and anything measured while it is still
  // moving belongs to a different frame than the next thing measured.
  await expect
    .poll(async () => Math.round((await page.locator('.now-sheet').boundingBox()).y))
    .toBe(0);
}

test('Napstrfy keeps a social card in the player sheet, with the newest comment', async ({ page }) => {
  await openApp(page, { messages: [themFirst, mineLast] });
  await openPlayer(page);

  const card = page.locator('.sheet-card');
  await expect(card).toBeVisible();
  await expect(card.locator('.sheet-card-head')).toContainText('Track discussion');
  await expect(card.locator('.sheet-card-line')).toContainText('Agreed.');

  // The card is the whole reason the sheet has this much room to give, so it is
  // checked where it is drawn rather than where it is written: one line of it
  // inside the sheet, without anything having to be scrolled, and the record above
  // it still a record.
  const box = await card.boundingBox();
  const sheet = await page.locator('.now-sheet').boundingBox();
  expect(box.y).toBeGreaterThan(0);
  expect(box.y + box.height).toBeLessThanOrEqual(sheet.y + sheet.height + 1);
  const art = await page.locator('.now-sheet-art').boundingBox();
  expect(art.height).toBeGreaterThan(120);
});

test('Napstrfy invites a comment on a track nobody has spoken about', async ({ page }) => {
  await openApp(page, { messages: [] });
  await openPlayer(page);

  const card = page.locator('.sheet-card');
  await expect(card.locator('.sheet-card-line')).toHaveText('Comment on this track…');
  // An invitation that opens a box, rather than a thread with nothing in it: the
  // card knows there is nothing to read, so it takes the reader straight to writing.
  await card.click();
  await expect(page.locator('.discussion-view')).toBeVisible();
  await expect.poll(() => page.evaluate(() => document.activeElement?.tagName ?? '')).toBe('INPUT');
});

test('Napstrfy asks about a conversation once per track, not once per glance', async ({ page }) => {
  await openApp(page, { messages: [themFirst] });
  await openPlayer(page);
  await expect.poll(() => callsTo(page, 'remote_track_discussion')).toBe(1);

  // Closing and re-opening the sheet is not a reason to ask a relay again.
  await page.locator('.now-sheet-close').click();
  await expect(page.locator('.now-sheet')).toHaveCount(0);
  await openPlayer(page);
  await expect(page.locator('.sheet-card-line')).toContainText('This rip is clean.');
  expect(await callsTo(page, 'remote_track_discussion')).toBe(1);
});

test('Napstrfy answers one comment rather than the room', async ({ page }) => {
  await openApp(page, { messages: [themFirst] });
  await openDiscussion(page);

  // The gesture belongs to the message it answers, and saying so is visible: the
  // box shows what it is replying to until it is sent or dropped.
  await page.locator('.discussion-message').first().locator('.discussion-reply').click();
  await expect(page.locator('.discussion-replying')).toContainText('Replying to Ada');
  await page.locator('.discussion-compose input').fill('Which rip is that?');
  await page.locator('.discussion-compose button').last().click();

  await expect.poll(() => callsTo(page, 'remote_send_track_discussion')).toBe(1);
  const sent = await page.evaluate(() => window.calls.find((call) => call.cmd === 'remote_send_track_discussion').args);
  expect(sent.replyTo).toBe(themFirst.eventId);
  expect(sent.content).toBe('Which rip is that?');
  // The answer carries what it answers, so the thread reads as a conversation
  // rather than a row of statements about nothing.
  await expect(page.locator('.discussion-message').last().locator('.discussion-quote')).toContainText('This rip is clean.');
  await expect(page.locator('.discussion-replying')).toHaveCount(0);
});

test('Napstrfy draws the line a reply answers', async ({ page }) => {
  const answer = {
    ...said(1, 'Me', id('c'), 'The 2011 remaster.'),
    replyTo: themFirst.eventId,
    reply: { author: 'Ada', excerpt: 'This rip is clean.' }
  };
  await openApp(page, { messages: [themFirst, answer] });
  await openDiscussion(page);

  const quote = page.locator('.discussion-message').last().locator('.discussion-quote');
  await expect(quote).toContainText('Ada');
  await expect(quote).toContainText('This rip is clean.');
  // Only the reply carries one: a message that answers nothing stays plain.
  await expect(page.locator('.discussion-message').first().locator('.discussion-quote')).toHaveCount(0);
});

test('a back press closes the conversation instead of leaving the app', async ({ page }) => {
  await openApp(page, { messages: [themFirst] });
  await openDiscussion(page);
  // The sheet has to tell Android there is somewhere to go back to. Without this
  // the press is never offered to the page, and Android leaves the app.
  await expect.poll(() => page.evaluate(() => window.backAvailable)).toBe(true);

  expect(await pressBack(page)).toBe('handled');
  await expect(page.locator('.discussion-view')).toHaveCount(0);
  expect(await page.evaluate(() => window.appExits)).toBe(0);
});

test('a back press out of the card\u2019s thread leaves the player sheet standing', async ({ page }) => {
  await openApp(page, { messages: [themFirst] });
  await openPlayer(page);
  await page.locator('.sheet-card').click();
  await expect(page.locator('.discussion-view')).toBeVisible();
  await expect.poll(() => page.evaluate(() => window.backAvailable)).toBe(true);

  // The thread is drawn over the sheet that opened it, so it is the first thing
  // back takes: the press closes the conversation, not the player underneath it.
  expect(await pressBack(page)).toBe('handled');
  await expect(page.locator('.discussion-view')).toHaveCount(0);
  await expect(page.locator('.now-sheet')).toBeVisible();
});
