import { test, expect } from '@playwright/test';
import { mockNative } from './helpers/native.mjs';

/**
 * A row can say that people have been talking about a file.
 *
 * The mark is deliberately about people rather than comments, because comments
 * are what one author can produce on their own, and it is a window rather than
 * all of history, because a comment from 2019 is not a reason to look at
 * anything. What these tests pin down is the part the page owns: which rows are
 * asked about, and what the answer is allowed to look like.
 */
const id = (index) => index.toString(16).padStart(64, '0');

const result = (index, title) => ({
  fileId: id(index),
  filename: `${title}.mp3`,
  title,
  artist: 'Artist',
  album: 'Album',
  folder: '',
  path: `/music/${title}.mp3`,
  format: 'MP3',
  mime: 'audio/mpeg',
  size: 1_000_000,
  sources: [{ pubkey: 'd'.repeat(64), npub: 'npub1source', displayName: 'Source' }]
});

const talkedAbout = result(1, 'Talked about');
const quiet = result(2, 'Quiet');

async function search(page, query = 'talked') {
  await mockNative(page, { app: 'napstr' });
  await page.addInitScript(({ talkedAbout, quiet }) => {
    // Nothing of this computer's own, so the page has exactly the two files under
    // test: one people have commented on, and one they have not.
    window.localSearchResults = [];
    window.networkSearchResults = [talkedAbout, quiet];
    window.discussionActivity = {
      [talkedAbout.fileId]: { fileId: talkedAbout.fileId, authors: 3, messages: 7, lastAt: 1_800_000_000 }
    };
  }, { talkedAbout, quiet });
  await page.goto('http://127.0.0.1:15173');
  await expect(page.locator('.search-button')).toBeEnabled();
  await page.locator('#search-query').fill(query);
  await page.locator('.search-button').click();
  await expect(page.locator('.search-results-table tbody tr')).toHaveCount(2);
}

test('Napstr marks the results people have been talking about', async ({ page }) => {
  await search(page);
  const rows = page.locator('.search-results-table tbody tr');
  const marked = rows.filter({ hasText: 'Talked about' }).locator('.discussion-mark');
  await expect(marked).toHaveText('3');
  await expect(marked).toHaveAttribute('title', '3 people commented in the last 30 days');
  // A file nobody has said anything about carries nothing at all, rather than a
  // zero, so the eye is not drawn to rows that have no conversation.
  await expect(rows.filter({ hasText: 'Quiet' }).locator('.discussion-mark')).toHaveCount(0);

  // The thumbnail view is the same answer drawn differently.
  await page.locator('.view-toggle').last().click();
  const cards = page.locator('.result-card');
  await expect(cards).toHaveCount(2);
  await expect(cards.filter({ hasText: 'Talked about' }).locator('.discussion-mark')).toHaveText('3');
  await expect(cards.filter({ hasText: 'Quiet' }).locator('.discussion-mark')).toHaveCount(0);

  // All of that cost one question, naming every row on screen at once, and none of
  // the re-renders that followed asked about them again. (A question before this
  // one is the page that was on screen at boot, which the app had already loaded.)
  const questions = await page.evaluate(() =>
    window.calls.filter((call) => call.cmd === 'track_discussion_activity').map((call) => call.args.fileIds)
  );
  const aboutTheseRows = questions.filter((fileIds) => fileIds.includes(talkedAbout.fileId));
  expect(aboutTheseRows).toHaveLength(1);
  expect([...aboutTheseRows[0]].sort()).toEqual([talkedAbout.fileId, quiet.fileId].sort());
});

test('Napstr reads the discussion the mark points at', async ({ page }) => {
  // The mark is only worth drawing if the place it points at answers: selecting
  // the row is what opens the panel that reads the conversation itself.
  await search(page);
  await page.locator('.search-results-table tbody tr').filter({ hasText: 'Talked about' }).click();
  await expect(page.locator('.track-discussion')).toBeVisible();
  await expect(page.locator('.track-discussion-log')).toContainText('Search');
});
