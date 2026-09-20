import { test, expect } from '@playwright/test';
import { mockNative } from './helpers/native.mjs';

for (const chat of ['trollbox', 'track']) {
  test(`Napstr ${chat}: opens at newest, prepends 100 messages, and preserves reading position`, async ({ page }) => {
    await mockNative(page, { app: 'napstr' });
    await page.addInitScript(() => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      const callbacks = new Map(), handlers = new Map();
      window.__TAURI_INTERNALS__.transformCallback = (fn) => { const id = callbacks.size + 1; callbacks.set(id, fn); return id; };
      window.chatCalls = [];
      window.chatMessages = Array.from({ length: 250 }, (_, index) => ({ eventId: index.toString(16).padStart(64, '0'),
        createdAt: Math.floor(index / 8), content: `Message ${index}`, pubkey: 'd'.repeat(64), npub: 'npubother', displayName: 'Alice' }));
      window.appendChat = (topic) => {
        const index = window.chatMessages.length;
        window.chatMessages.push({ ...window.chatMessages[0], eventId: index.toString(16).padStart(64, '0'), createdAt: Math.floor(index / 8), content: `Message ${index}` });
        callbacks.get(handlers.get('napstr-public-chat'))?.({ event: 'napstr-public-chat', id: 1, payload: topic });
      };
      window.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
        if (cmd === 'plugin:event|listen') { handlers.set(args.event, args.handler); return 1; }
        if (cmd === 'get_trollbox_messages' || cmd === 'get_track_discussion_messages') {
          window.chatCalls.push({ cmd, ...args });
          const before = args?.before;
          const messages = window.chatMessages.filter((message) => !before || message.createdAt < before.createdAt
            || message.createdAt === before.createdAt && message.eventId < before.eventId).slice(-100);
          if (before && window.holdHistory) await new Promise((resolve) => { window.releaseHistory = resolve; });
          return messages;
        }
        if (cmd === 'network_browse') {
          const page = await invoke(cmd, args);
          return { ...page, results: [...page.results, { ...page.results[0], fileId: 'b'.repeat(64), filename: 'Another.wav', title: 'Another', sources: [{ pubkey: 'd'.repeat(64), npub: 'npubother', displayName: 'Alice' }] }] };
        }
        return invoke(cmd, args);
      };
    });
    await page.goto('http://127.0.0.1:15173');
    if (chat === 'trollbox') await page.locator('.tool-button').filter({ hasText: 'Trollbox' }).click();
    else await page.locator('.search-results-table tbody tr').filter({ hasText: 'Search' }).click();
    const log = page.locator(chat === 'trollbox' ? '.trollbox-log' : '.track-discussion-log');
    const messages = log.locator('.trollbox-message');
    const bottomGap = () => log.evaluate((node) => node.scrollHeight - node.clientHeight - node.scrollTop);
    await expect(messages).toHaveCount(100);
    await expect(messages.last()).toContainText('Message 249');
    await expect.poll(bottomGap).toBeLessThan(2);
    if (chat === 'track') {
      // A -> B -> A must not accept the first A's delayed history response.
      await page.evaluate(() => { window.holdHistory = true; });
      await log.evaluate((node) => { node.scrollTop = 0; node.dispatchEvent(new Event('scroll')); });
      await expect.poll(() => page.evaluate(() => typeof window.releaseHistory)).toBe('function');
      await page.evaluate(() => { window.holdHistory = false; });
      await page.locator('.search-results-table tbody tr').filter({ hasText: 'Another' }).click();
      await expect(log).toHaveAttribute('aria-busy', 'false');
      await expect.poll(bottomGap).toBeLessThan(2);
      await page.locator('.search-results-table tbody tr').filter({ hasText: 'Search' }).click();
      await expect(log).toHaveAttribute('aria-busy', 'false');
      await page.evaluate(() => window.releaseHistory());
      await page.waitForTimeout(100);
      await expect(messages).toHaveCount(100);
      await expect.poll(bottomGap).toBeLessThan(2);
    }
    const anchor = await log.evaluate((node) => {
      node.scrollTop = 0;
      const first = node.querySelector('.trollbox-message');
      const offset = first.getBoundingClientRect().top - node.getBoundingClientRect().top;
      node.dispatchEvent(new Event('scroll'));
      return offset;
    });
    await expect(messages).toHaveCount(200);
    await expect.poll(() => messages.nth(100).evaluate((node) => node.getBoundingClientRect().top - node.parentElement.getBoundingClientRect().top)).toBeCloseTo(anchor, 0);
    const topic = chat === 'trollbox' ? 'napstr-trollbox' : 'napstr-' + 'a'.repeat(64);
    const top = await log.evaluate((node) => node.scrollTop);
    await page.evaluate((topic) => window.appendChat(topic), topic);
    await expect(messages).toHaveCount(201);
    expect(await log.evaluate((node) => node.scrollTop)).toBeCloseTo(top, 0);
    await log.evaluate((node) => { node.scrollTop = 0; node.dispatchEvent(new Event('scroll')); });
    await expect(messages).toHaveCount(251);
    await expect(messages.first()).toContainText('Message 0');
    const olderCount = await page.evaluate(() => window.chatCalls.filter((call) => call.before).length);
    await log.evaluate((node) => { node.scrollTop = 0; node.dispatchEvent(new Event('scroll')); });
    await page.waitForTimeout(100);
    expect(await page.evaluate(() => window.chatCalls.filter((call) => call.before).length)).toBe(olderCount);
    await log.evaluate((node) => { node.scrollTop = node.scrollHeight; });
    await page.evaluate((topic) => window.appendChat(topic), topic);
    await expect(messages).toHaveCount(252);
    await expect.poll(bottomGap).toBeLessThan(2);
    // Remounting a populated log must also open at the newest message.
    await page.locator('.tool-button').filter({ hasText: 'Settings' }).click();
    await page.locator('.tool-button').filter({ hasText: chat === 'trollbox' ? 'Trollbox' : 'Search' }).click();
    await expect.poll(bottomGap).toBeLessThan(2);
    await expect(messages).toHaveCount(252);
  });
}

