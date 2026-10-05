import { test, expect } from '@playwright/test';
import { mockNative, serveAudio } from './helpers/native.mjs';

/**
 * This phone's own Nostr identity.
 *
 * A key made on the device, which is what the phone signs with: what it says in
 * public is signed here and published by a computer, rather than signed by the
 * computer. What these tests are about is the part a person touches - the public
 * half being readable, the secret being shown only when asked for, and a restored
 * key being the one the phone then signs with.
 */

const NPUB = 'npub1napstrfyidentityexample000000000000000000000000000000000000000';
const PUBKEY = 'a'.repeat(64);
const NSEC = 'nsec1napstrfysecretkeyexample0000000000000000000000000000000000000000';

async function openApp(page, { restored = null } = {}) {
  await mockNative(page, { platform: 'android' });
  await page.route('**/fixture.wav', serveAudio);
  await page.addInitScript(({ npub, pubkey, nsec, restored }) => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    window.identityExports = 0;
    window.identity = { pubkey, npub };
    window.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
      if (cmd === 'nostr_identity' || cmd === 'export_nostr_identity' || cmd === 'import_nostr_identity') {
        // Recorded the way the shared mock records everything else, so a test can
        // assert on what the app asked for and not only on what it drew.
        window.calls.push({ cmd, args });
      }
      if (cmd === 'nostr_identity') return window.identity;
      if (cmd === 'export_nostr_identity') {
        window.identityExports += 1;
        return nsec;
      }
      if (cmd === 'import_nostr_identity') {
        // The host decides: a paste that is not a key fails here, and the answer
        // the app draws is this one rather than its own optimism.
        if (restored && args.secret.trim() !== restored.secret) throw 'That is not a Nostr secret key';
        window.identity = restored ? restored.identity : window.identity;
        return window.identity;
      }
      return invoke(cmd, args);
    };
  }, { npub: NPUB, pubkey: PUBKEY, nsec: NSEC, restored });
  await page.goto('http://127.0.0.1:15174');
}

async function openSettings(page) {
  // By role and exactly: the fixture library happens to hold a track by an
  // artist called "Settings", which is also a label on the page.
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(page.locator('.settings-scroll')).toBeVisible();
}

test('the settings sheet names the key this phone signs with', async ({ page }) => {
  await openApp(page);
  await openSettings(page);

  const section = page.locator('.settings-section', { hasText: 'Your Nostr identity' });
  await expect(section).toBeVisible();
  // The public half, in full: it is what another person compares against, and a
  // truncated key is a key nobody can check.
  await expect(section.locator('.identity-row code')).toHaveAttribute('title', NPUB);
  await expect(section.locator('.identity-row code')).toHaveText(NPUB);
  // And the secret is not on the page until it is asked for.
  await expect(section.locator('textarea')).toHaveCount(0);
  expect(await page.evaluate(() => window.identityExports)).toBe(0);
});

test('the secret is shown only when it is asked for, and can be put away again', async ({ page }) => {
  await openApp(page);
  await openSettings(page);

  const section = page.locator('.settings-section', { hasText: 'Your Nostr identity' });
  await section.getByRole('button', { name: 'Export your secret key' }).click();

  const secret = section.locator('textarea[aria-label="Your secret key"]');
  await expect(secret).toHaveValue(NSEC);
  await expect(secret).toHaveAttribute('readonly', '');
  expect(await page.evaluate(() => window.identityExports)).toBe(1);
  // The warning travels with it: what this key can do is the reason it is not
  // simply drawn on the page.
  await expect(section).toContainText('Anyone who has this key can post as this phone');

  // Asked again, and it is away: the same button closes what it opened.
  await section.getByRole('button', { name: 'Export your secret key' }).click();
  await expect(secret).toHaveCount(0);
});

test('a restored key becomes the one this phone signs with', async ({ page }) => {
  const restored = { secret: 'nsec1restoredkey00000000000000000000000000000000000000000000000000', identity: { pubkey: 'b'.repeat(64), npub: 'npub1restoredkey00000000000000000000000000000000000000000000000000' } };
  const asked = [];
  page.on('dialog', (dialog) => {
    asked.push(dialog.message());
    void dialog.accept();
  });
  await openApp(page, { restored });
  await openSettings(page);

  const section = page.locator('.settings-section', { hasText: 'Your Nostr identity' });
  await section.getByRole('button', { name: 'Restore from a key' }).click();
  await section.locator('textarea[aria-label="The key to restore"]').fill(restored.secret);
  await section.getByRole('button', { name: 'Restore', exact: true }).click();

  // Asked first, and the question says what a different key costs: everything
  // this phone published belongs to the key that signed it.
  expect(asked.some((message) => message.includes('stay with the old key'))).toBe(true);
  await expect
    .poll(() => page.evaluate(() => window.calls.filter((call) => call.cmd === 'import_nostr_identity').map((call) => call.args.secret)))
    .toEqual([restored.secret]);
  // The row now reads the key it restored, rather than the one it made.
  await expect(section.locator('.identity-row code')).toHaveText(restored.identity.npub);
});

test('a paste that is not a key is refused without changing the identity', async ({ page }) => {
  const restored = { secret: 'nsec1restoredkey00000000000000000000000000000000000000000000000000', identity: { pubkey: 'b'.repeat(64), npub: 'npub1restoredkey00000000000000000000000000000000000000000000000000' } };
  page.on('dialog', (dialog) => void dialog.accept());
  await openApp(page, { restored });
  await openSettings(page);

  const section = page.locator('.settings-section', { hasText: 'Your Nostr identity' });
  await section.getByRole('button', { name: 'Restore from a key' }).click();
  await section.locator('textarea[aria-label="The key to restore"]').fill('not-a-key');
  await section.getByRole('button', { name: 'Restore', exact: true }).click();

  await expect(section.locator('.settings-error')).toContainText('That is not a Nostr secret key');
  await expect(section.locator('.identity-row code')).toHaveText(NPUB);
});
