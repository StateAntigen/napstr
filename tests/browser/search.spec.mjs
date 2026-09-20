import { test, expect } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { mockNative } from './helpers/native.mjs';
const catalogs = Object.fromEntries(await Promise.all(['en', 'zh', 'ar'].map(async (code) => [code, JSON.parse(await readFile(new URL(`../../shared/i18n/locales/${code}.json`, import.meta.url)))])));

for (const code of ['en', 'zh', 'ar']) {
  test(`Napstr ${code}: search, pagination and Tor-to-Local update without leaving the screen`, async ({ page }) => {
    await mockNative(page, { app: 'napstr', saved: code });
    await page.addInitScript(() => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      const track = (index, prefix) => ({ fileId: index.toString(16).padStart(64, '0'), filename: `${prefix} ${index}.mp3`,
        title: `${prefix} ${String(index).padStart(3, '0')}`, artist: '', album: '', format: 'MP3', mime: 'audio/mpeg', size: 1000000,
        folder: '', path: `/music/${index}.mp3`, sources: [{ pubkey: 'd'.repeat(64), npub: 'npub1source', displayName: 'Source' }] });
      const callbacks = new Map();
      const handlers = new Map();
      window.__TAURI_INTERNALS__.transformCallback = (callback) => { const id = callbacks.size + 1; callbacks.set(id, callback); return id; };
      window.emitNative = (event) => callbacks.get(handlers.get(event))?.({ event, id: 1, payload: null });
      window.localFiles = [];
      window.downloads = [];
      window.finishDownload = () => {
        window.localFiles = [track(200, 'New')];
        window.downloads = [];
        window.emitNative('napstr-transfers-changed');
      };
      window.addLocalFiles = () => {
        window.localFiles = Array.from({ length: 105 }, (_, index) => track(index, 'Library'));
        window.emitNative('napstr-library-changed');
      };
      window.__TAURI_INTERNALS__.invoke = async (cmd, args = {}) => {
        if (cmd === 'plugin:event|listen') { handlers.set(args.event, args.handler); return 1; }
        if (cmd === 'get_snapshot') return { ...(await invoke(cmd, args)), files: window.localFiles, transfers: window.downloads };
        if (cmd === 'network_browse') return { results: [], cursor: null, totalAvailable: 0 };
        if (cmd === 'network_search') return args.query === 'first'
          ? Array.from({ length: 115 }, (_, index) => track(index, 'First'))
          : [track(200, 'New'), track(201, 'New')];
        if (cmd === 'resolve_catalogue_user' || cmd === 'search_catalog') return [];
        if (cmd === 'get_transfers') return window.downloads;
        if (cmd === 'request_network_download') {
          window.downloads = [{ ...track(200, 'New'), id: -1, progress: 100, status: 'Downloading', speed: '1 MB/s', destination: '' }];
          return 'request';
        }
        return invoke(cmd, args);
      };
    });
    await page.goto('http://127.0.0.1:15173');
    await expect(page.locator('.search-button')).toBeEnabled();
    const rows = page.locator('.search-results-table tbody tr');
    await page.locator('#search-query').fill('first');
    await page.locator('.search-button').click();
    await expect(rows).toHaveCount(100);
    await page.locator('.results-pane .results-pager button').last().click();
    await expect(rows).toHaveCount(15);
    await expect(rows.first()).toContainText('First 100');
    await page.locator('#search-query').fill('second');
    await page.locator('.search-button').click();
    await expect(rows).toHaveCount(2);
    await expect(rows.first()).toContainText('New 200');
    await expect(rows.first().locator('td').nth(4)).toHaveText('Tor');
    await page.locator('.detail-actions button.primary').click();
    await expect(page.locator('.mini-row')).toHaveCount(1);
    await page.evaluate(() => window.finishDownload());
    await expect(rows.first().locator('td').nth(4)).toHaveText(catalogs[code].Local);
    await expect(page.locator('.detail-actions button.primary')).toHaveText(catalogs[code]['▶ Play']);
    // The shared library uses the same helper pattern: an event must update it
    // in place, and its pager must actually replace the displayed rows.
    await page.locator('.tool-button').filter({ hasText: catalogs[code].Shared }).click();
    await expect(page.locator('.shared-table tbody tr')).toHaveCount(1);
    await page.evaluate(() => window.addLocalFiles());
    await expect(page.locator('.shared-table tbody tr')).toHaveCount(100);
    await page.locator('.full-panel .results-pager button').last().click();
    await expect(page.locator('.shared-table tbody tr')).toHaveCount(5);
  });

  test(`Napstr ${code}: Surprise me fills 50 unowned tracks across pages and ignores old filters`, async ({ page }) => {
    await mockNative(page, { app: 'napstr', saved: code });
    await page.addInitScript(() => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      const track = (index) => ({ fileId: index.toString(16).padStart(64, '0'), filename: `Track ${index}.mp3`,
        title: `Track ${index}`, artist: 'Search', album: '', format: 'MP3', mime: 'audio/mpeg', size: 1000000,
        sources: [{ pubkey: 'd'.repeat(64), npub: 'npub1source', displayName: 'Source' }] });
      const owned = Array.from({ length: 80 }, (_, index) => track(index));
      window.surpriseCalls = [];
      window.__TAURI_INTERNALS__.invoke = async (cmd, args = {}) => {
        if (cmd === 'get_snapshot') return { ...(await invoke(cmd, args)), files: owned, transfers: [] };
        if (cmd === 'network_browse' && args.unownedOnly) {
          window.surpriseCalls.push(args);
          const number = Number(args.cursor?.sessionId || 0);
          // Include owned entries and duplicates defensively, and leave an
          // empty middle page to exercise continuation past unavailable metadata.
          const pages = [owned, [], Array.from({ length: 20 }, (_, i) => track(80 + i)),
            Array.from({ length: 40 }, (_, i) => track(90 + i))];
          return { results: pages[number], cursor: number < 3 ? { sessionId: String(number + 1) } : null, totalAvailable: 130 };
        }
        return invoke(cmd, args);
      };
    });
    await page.goto('http://127.0.0.1:15173');
    await expect(page.locator('.surprise-button')).toBeEnabled();
    await page.locator('#format').selectOption('Audiobooks');
    await expect(page.locator('.surprise-button')).toBeEnabled();
    await page.locator('#search-query').fill('An unrelated old search');
    await page.locator('.advanced-toggle').click();
    await page.locator('.advanced-row input[type="number"]').fill('99');
    await page.locator('.advanced-row input:not([type])').fill('1 B');
    await page.locator('.surprise-button').click();
    const rows = page.locator('.results-pane tbody tr');
    await expect(rows).toHaveCount(50);
    await expect(page.locator('.surprise-button')).toBeEnabled();
    expect(await page.locator('#format').inputValue()).toBe('Audio only');
    expect(await page.locator('#search-query').inputValue()).toBe('');
    expect(await page.evaluate(() => window.surpriseCalls.length)).toBe(4);
    const names = await rows.locator('td:first-child').allTextContents();
    expect(new Set(names).size).toBe(50);
    expect(names.every((name) => Number(name.match(/Track (\d+)/)[1]) >= 80)).toBe(true);
    // Selection and Download All must continue to work with translated labels.
    await rows.first().click();
    await rows.nth(4).click({ modifiers: ['Shift'] });
    await expect(page.locator('.detail-actions button.primary')).toHaveText(catalogs[code]['⇩ Download All']);
    await expect(page.locator('.detail-actions button.primary')).toBeEnabled();
  });
}

