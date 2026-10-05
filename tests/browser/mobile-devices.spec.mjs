import { test, expect } from '@playwright/test';
import { mockNative } from './helpers/native.mjs';

/**
 * What a paired phone may do, changed from this window.
 *
 * The access a phone has is a property of the phone now rather than of the code
 * that let it in, so this list is the whole interface to it: a tick writes the
 * grant the host will enforce, and the list is read again rather than assumed.
 */
const readOnly = { browse: true, fetch: true, control: false, download: false, privileged: false };
const indexOnly = { browse: true, fetch: false, control: false, download: false, privileged: false };

const device = (rights = readOnly, overrides = {}) => ({
  endpointId: 'a'.repeat(64),
  name: 'Ada’s phone',
  pairedAt: '2026-09-20T10:00:00Z',
  lastSeen: '2026-09-28T10:00:00Z',
  rights,
  ...overrides
});

async function openPairedPhones(page, devices) {
  await mockNative(page, { app: 'napstr' });
  await page.addInitScript((rows) => { window.mobileDevices = rows; }, devices);
  await page.goto('http://127.0.0.1:15173');
  await expect(page.locator('.search-button')).toBeEnabled();
  await page.getByRole('button', { name: 'Napstrfy' }).click();
  await expect(page.locator('.paired-devices-card')).toBeVisible();
}

const callsTo = (page, cmd) => page.evaluate((name) => window.calls.filter((call) => call.cmd === name), cmd);

test('a paired phone’s access is five ticks, and a tick writes the grant', async ({ page }) => {
  await openPairedPhones(page, [device()]);

  const ticks = page.locator('.device-rights input');
  await expect(ticks).toHaveCount(5);
  // Paired read-only: the two reads are on, the three acts are off.
  await expect(ticks.nth(0)).toBeChecked();
  await expect(ticks.nth(1)).toBeChecked();
  await expect(ticks.nth(2)).not.toBeChecked();
  await expect(ticks.nth(3)).not.toBeChecked();
  await expect(ticks.nth(4)).not.toBeChecked();

  // The fourth tick is the right to reach the network, which is not the right
  // to sign: this is the whole reason it was separated out.
  await page.locator('.device-rights label').nth(3).click();
  await expect.poll(async () => (await callsTo(page, 'set_mobile_device_rights')).length).toBe(1);
  const [sent] = await callsTo(page, 'set_mobile_device_rights');
  expect(sent.args.endpointId).toBe(device().endpointId);
  // The whole grant travels, not the one tick that moved: the host stores what
  // it is given, and a partial write would be a way of granting by omission.
  expect(sent.args.rights).toEqual({ ...readOnly, download: true });
  // Then the list is read again, so what is on screen is the host's answer.
  await expect.poll(async () => (await callsTo(page, 'mobile_status')).length).toBeGreaterThan(1);
  await expect(ticks.nth(3)).toBeChecked();
  // And signing is still off: a phone lent the network may not publish.
  await expect(ticks.nth(4)).not.toBeChecked();
});

test('the list names each right instead of calling every device read-only', async ({ page }) => {
  await openPairedPhones(page, [device(indexOnly)]);

  await expect(page.locator('.device-rights label')).toHaveText([
    'Read the library',
    'Play and keep audio',
    'Control playback',
    'Download from the network',
    'Sign and publish as you'
  ]);
  // A device lent the index and nothing else: it may browse and may not play,
  // which is a combination the words "read only" could not describe.
  await expect(page.locator('.device-rights label').nth(0)).toHaveClass(/on/);
  await expect(page.locator('.device-rights label').nth(1)).not.toHaveClass(/on/);
  await expect(page.locator('.device-rights label').nth(4)).not.toHaveClass(/on/);
});
