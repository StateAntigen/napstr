import { test, expect } from '@playwright/test';
import { mockNative } from './helpers/native.mjs';

// The Covers tab had no test at all, which is how it grew into a wall of prose
// nobody had looked at. This plays a busy host so the tab can be looked at, and
// pins the shape it is supposed to have: one panel open at a time, a summary line
// that is always there, and the explanations in tooltips rather than on screen.
const key = (index) => `artist ${index}|album ${index}`;

async function openCovers(page, { status = {}, gaps = [], queue = [], log = [] } = {}) {
  await mockNative(page);
  await page.route('**/fixture.wav', (route) => route.fulfill({ contentType: 'audio/wav', body: '' }));
  await page.addInitScript(({ status, gaps, queue, log }) => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    window.__TAURI_INTERNALS__.invoke = async (cmd, args = {}) => {
      switch (cmd) {
        case 'cover_status':
          return {
            lookupExternal: true,
            publishClaims: true,
            running: true,
            current: 'Bicep — Glue',
            pending: 412,
            remaining: 96,
            published: 318,
            resolved: 74,
            alreadyCovered: 12,
            noArt: 8,
            failed: 2,
            backedOff: 1,
            stopped: false,
            allowedArtHosts: 'coverartarchive.org\narchive.org\nmzstatic.com',
            artCached: 1240,
            artBytes: 196_083_712,
            artPending: 96,
            artPaused: false,
            message: '318 published · 74 resolved here',
            ...status
          };
        case 'cover_candidates':
          return queue;
        case 'cover_missing':
          return gaps;
        case 'cover_lookup_log':
          return log;
        default:
          return invoke(cmd, args);
      }
    };
  }, { status, gaps, queue, log });
  await page.goto('http://127.0.0.1:15173');
  await page.locator('.tool-button', { hasText: 'Covers' }).click();
  await expect(page.locator('.cover-summary')).toBeVisible();
}

const busy = {
  queue: Array.from({ length: 12 }, (_, index) => ({
    key: key(index),
    artist: `Artist ${index}`,
    album: `Album ${index}`,
    trackCount: (index % 4) + 6,
    source: index % 3 === 0 ? 'library' : 'browsed'
  })),
  gaps: [
    { key: 'a|failed', artist: 'A', album: 'A record whose lookup fell over', tracks: 11, state: 'failed', source: 'library', note: 'MusicBrainz did not answer in 45 seconds; this is back-pressure, not a missing record' },
    { key: 'b|none', artist: 'B', album: 'A record nobody has scanned', tracks: 9, state: 'no_art', source: 'library', note: '' },
    { key: 'c|here', artist: 'C', album: 'A record resolved here and not signed', tracks: 4, state: 'resolved_here', source: 'library', note: '' },
    { key: 'd|never', artist: 'D', album: 'A record never looked up', tracks: 2, state: 'not_looked_up', source: 'library', note: '' }
  ],
  log: Array.from({ length: 8 }, (_, index) => ({
    at: new Date(Date.UTC(2026, 8, 27, 17, index)).toISOString(),
    key: key(index),
    artist: `Artist ${index}`,
    album: `Album ${index}`,
    outcome: ['published', 'resolved', 'no_art', 'throttled', 'failed', 'slow'][index % 6],
    source: 'musicbrainz',
    message: index % 3 === 0 ? 'Album not found in MusicBrainz; try the release group' : ''
  }))
};

test('the Covers tab is a summary line and one panel at a time', async ({ page }) => {
  await openCovers(page, busy);
  // The numbers somebody actually came for are together, in one line, and each
  // one says what it counts: a pass position, the pictures held and what they
  // take, the albums still to fetch, and the albums of this computer's own music
  // with nothing to show for them.
  const summary = page.locator('.cover-summary');
  await expect(summary).toContainText('96 of 412 this pass');
  await expect(summary).toContainText('1,240 pictures');
  await expect(summary).toContainText('187.0 MB');
  await expect(summary).toContainText('96 albums to fetch');
  await expect(summary).toContainText('4 albums without a claim');
  // The three panels are reachable and only one is drawn at a time, so the tab is
  // a quarter of the height it was.
  await expect(page.locator('.cover-panel-body')).toHaveCount(1);
  await expect(page.locator('.cover-candidate-list')).toBeVisible();
  const shot = (name) =>
    process.env.COVER_TAB_SCREENSHOTS
      ? page.screenshot({ path: `test-results/cover-tab-${name}.png`, fullPage: true })
      : Promise.resolve();
  await shot('waiting');

  await page.locator('.cover-subtab', { hasText: 'Covers found' }).click();
  await expect(page.locator('.cover-gap-list')).toBeVisible();
  await expect(page.locator('.cover-candidate-list')).toHaveCount(0);
  await expect(page.locator('.cover-panel-body')).toHaveCount(1);
  await shot('found');

  await page.locator('.cover-subtab', { hasText: 'Activity' }).click();
  await expect(page.locator('.cover-log-list')).toBeVisible();
  await expect(page.locator('.cover-gap-list')).toHaveCount(0);
  await expect(page.locator('.cover-panel-body')).toHaveCount(1);
  await shot('activity');

  await page.locator('.cover-subtab', { hasText: 'Settings' }).click();
  await expect(page.locator('#cover-art-hosts')).toBeVisible();
  await expect(page.locator('#cover-art-cache')).toBeVisible();
  await expect(page.locator('.cover-log-list')).toHaveCount(0);
  await expect(page.locator('.cover-panel-body')).toHaveCount(1);
  await shot('settings');
  // Nothing in the tab is a paragraph any more: every explanation is a title on
  // the thing it explains, which is what the tab was asked for.
  await expect(page.locator('.cover-scan-view .privacy-note')).toHaveCount(0);
});
