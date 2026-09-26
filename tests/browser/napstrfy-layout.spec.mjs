import { test, expect } from '@playwright/test';
import { mockNative } from './helpers/native.mjs';

/**
 * The album preview is a full-window overlay on a phone, where there is no
 * column beside it to run under. In the three-column desktop window it has to
 * stop at the middle column instead.
 *
 * The preview is a SIBLING of `.app-shell` rather than a child of it, because it
 * stays fixed to the window while it scrolls. A rule written through the shell
 * -- `.app-shell.desktop .album-view` -- therefore matches nothing at all and
 * fails silently, leaving the overlay over the sidebar and the player. These
 * tests measure the geometry the user actually sees, so a selector that matches
 * nothing fails here instead of passing quietly.
 */
async function openDesktop(page) {
  await mockNative(page, { platform: 'linux' });
  await page.goto('http://127.0.0.1:15174');
  await expect(page.locator('.app-shell.desktop')).toBeVisible();
}

async function openAlbumView(page) {
  await page.locator('.track-row .track-more').first().click();
  await page.getByRole('button', { name: 'Go to album' }).click();
}

async function openPlaylist(page) {
  await page.getByRole('button', { name: 'Open the playlist' }).click();
}

/**
 * The bottom of the phone screen: the player bar and the nav. Neither has an
 * edge of its own any more - the bar is a card, and the nav's colour fades in -
 * so the page dissolves into them rather than stopping at a line.
 */
test('the nav has no edge and the page fades into the player', async ({ page }) => {
  await page.setViewportSize({ width: 412, height: 892 });
  await mockNative(page, { platform: 'android' });
  await page.goto('http://127.0.0.1:15174');
  const nav = page.locator('.bottom-nav');
  await expect(nav).toBeVisible();

  // No hairline on its top edge, and no solid panel either: the bar's own colour
  // fades in from fully transparent.
  await expect(nav).toHaveCSS('border-top-width', '0px');
  const navFade = await nav.evaluate((node) => getComputedStyle(node).backgroundImage);
  expect(navFade).toContain('linear-gradient');
  expect(navFade, 'the top of the bar is not painted at all').toMatch(/rgba\(8, 8, 10, 0\)/);

  // And the wash above it reaches well past the player bar, so what is left
  // visible of it is the fade above the bar. It never takes a press.
  const wash = await page.evaluate(() => {
    const style = getComputedStyle(document.querySelector('.app-shell'), '::after');
    return {
      content: style.content,
      background: style.backgroundImage,
      height: Number.parseFloat(style.height),
      z: Number(style.zIndex),
      events: style.pointerEvents
    };
  });
  expect(wash.content, 'the wash is drawn').not.toBe('none');
  expect(wash.background).toContain('linear-gradient');
  expect(wash.height, 'it starts high up').toBeGreaterThan(200);
  expect(wash.z, 'behind the player bar, which is at 5').toBeLessThan(5);
  expect(wash.events).toBe('none');
});

const geometry = (page, selector) => page.evaluate((selector) => {
  const box = (target) => {
    const node = document.querySelector(target);
    if (!node) return null;
    const { left, right, width } = node.getBoundingClientRect();
    return { left, right, width };
  };
  return { overlay: box(selector), sidebar: box('.bottom-nav'), player: box('.now-sheet.pinned'), width: innerWidth };
}, selector);

/**
 * The overlay begins at the sidebar's edge rather than the window's, and ends at
 * the player's edge rather than the window's. A full-window inset starts at 0
 * and runs to the far side, which is what the user sees when the rule misses.
 */
async function expectMiddleColumn(page, selector) {
  await expect(page.locator(selector)).toBeVisible();
  await expect(page.locator('.now-sheet.pinned')).toHaveCount(1);
  const view = await geometry(page, selector);
  expect(view.sidebar, 'the tab bar is the sidebar').not.toBeNull();
  expect(view.player, 'the player is pinned as the third column').not.toBeNull();
  expect(view.sidebar.right, 'the sidebar is a real column').toBeGreaterThan(0);
  expect(view.player.width, 'the player is a real column').toBeGreaterThan(0);
  expect(view.overlay.left, `${selector} clears the sidebar`).toBeGreaterThan(view.sidebar.right - slack);
  expect(view.overlay.right, `${selector} stops short of the player`).toBeLessThan(view.player.left + slack);
}

/** With one column there is nothing to run under, so the overlay covers it all. */
async function expectFullWindow(page, selector) {
  await expect(page.locator(selector)).toBeVisible();
  const view = await geometry(page, selector);
  expect(view.overlay.left).toBeLessThanOrEqual(1);
  expect(view.overlay.right).toBeGreaterThanOrEqual(view.width - slack);
}

// A scrollbar can be counted by the grid and not by a fixed element, so the two
// edges are compared with room for one rather than exactly.
const slack = 20;

for (const [name, open, selector] of [
  ['the album preview', openAlbumView, '.album-view'],
  ['the playlist', openPlaylist, '.queue-view']
]) {
  test(`Napstrfy desktop holds ${name} between the sidebar and the player`, async ({ page }) => {
    await page.setViewportSize({ width: 1180, height: 810 });
    await openDesktop(page);
    await open(page);
    await expectMiddleColumn(page, selector);
  });

  test(`Napstrfy in a single column still lets ${name} cover the window`, async ({ page }) => {
    await page.setViewportSize({ width: 1180, height: 810 });
    await openDesktop(page);
    // Opened wide, where the control that launches it is on screen -- the playlist
    // button lives in the now-playing sheet, which a narrow window only mounts
    // once something is playing -- then narrowed the way a user would. The third
    // column goes away, so there is nothing left to run under.
    await open(page);
    await page.setViewportSize({ width: 700, height: 810 });
    await expectFullWindow(page, selector);
  });
}
