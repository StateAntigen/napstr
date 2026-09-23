import { test, expect } from '@playwright/test';
import { mockNative } from './helpers/native.mjs';

/**
 * The Playlists page is where a playlist is written down and, when its author
 * says so, published.
 *
 * These tests hold it to the parts that are easy to get wrong and invisible on
 * screen: the coordinate a row is filed under, that the order on screen is the
 * order that is saved, that a private playlist cannot be published at all, and
 * that the author's answer about search words travels with the event rather than
 * being guessed at publish time.
 */
const TRACK = 'a'.repeat(64);
const OTHER_TRACK = 'b'.repeat(64);
const NEW_ID = '22222222-2222-4222-8222-222222222222';

async function openPlaylists(page) {
  await mockNative(page, { platform: 'linux' });
  await page.goto('http://127.0.0.1:15173');
  await page.locator('.toolbar .tool-button').filter({ hasText: 'Playlists' }).click();
  await expect(page.locator('.playlist-view')).toBeVisible();
}

function seed(page, rows) {
  return page.evaluate((seeded) => {
    window.playlistStore = seeded;
  }, rows);
}

function row(overrides = {}) {
  return {
    playlistId: '11111111-1111-4111-8111-111111111111',
    title: 'Driving',
    author: 'c'.repeat(64),
    displayName: 'Original user',
    artist: '',
    mbid: '',
    tags: '',
    private: false,
    published: false,
    updatedAt: 1787000000,
    tracks: [],
    total: 0,
    ...overrides
  };
}

function member(fileId, title, artist) {
  return { position: 1, fileId, title, artist, album: '' };
}

function calls(page, cmd) {
  return page.evaluate((name) => window.calls.filter((call) => call.cmd === name).map((call) => call.args), cmd);
}

const refresh = (page) => page.locator('.playlist-view .classic-button[title="Refresh"]').click();
const save = (page) => page.getByRole('button', { name: 'Save', exact: true }).click();
const publish = (page) => page.getByRole('button', { name: 'Publish', exact: true }).click();
const back = (page) => page.locator('.playlist-title [title="Back to playlists"]').click();

test('the page lists what this computer holds, and says what each row is', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, [row({ private: true }), row({ playlistId: NEW_ID, title: 'Published mix', published: true })]);
  await refresh(page);

  const rows = page.locator('.playlist-row');
  await expect(rows).toHaveCount(2);
  const draft = rows.filter({ hasText: 'Driving' });
  await expect(draft.locator('b')).toHaveText('Driving');
  // A row says whether the relays have a copy of it, and nothing else: the
  // window has no opinion about a playlist being private.
  await expect(draft.locator('.playlist-badge')).toHaveText(['Draft']);
  // A playlist whose author is this computer is editable, so opening it lands in
  // the editor rather than in a read-only view.
  await expect(rows.filter({ hasText: 'Published mix' }).locator('.playlist-badge')).toHaveText(['Published']);
});

test('an empty list says so rather than looking broken', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, []);
  await refresh(page);
  await expect(page.locator('.playlist-view .empty-state')).toHaveText('No playlists yet');
});

test('a new playlist is filed under a fresh id and saved without being published', async ({ page }) => {
  await openPlaylists(page);
  await page.evaluate((id) => {
    window.playlistStore = [];
    window.nextPlaylistId = id;
  }, NEW_ID);
  await page.getByRole('button', { name: 'New playlist' }).click();
  await expect(page.locator('.playlist-id code')).toHaveText(NEW_ID);
  // A playlist with no name yet is not something to write down.
  await expect(page.getByRole('button', { name: 'Save', exact: true })).toBeDisabled();
  await page.getByLabel('Title', { exact: true }).fill('Driving');
  await save(page);
  await expect(page.locator('.playlist-notice')).toHaveText('Saved on this computer');

  const saved = await calls(page, 'save_playlist');
  expect(saved).toHaveLength(1);
  expect(saved[0].playlist.playlistId).toBe(NEW_ID);
  expect(saved[0].playlist.title).toBe('Driving');
  expect(saved[0].playlist.tracks).toEqual([]);
  expect(await calls(page, 'publish_playlist')).toHaveLength(0);

  // The host stamped the author, and the list that comes back is the row that
  // was written: one playlist under the coordinate, not two.
  await back(page);
  await expect(page.locator('.playlist-row b')).toHaveText('Driving');
  await expect(page.locator('.playlist-row .playlist-badge')).toHaveText(['Draft']);
});

test('the order on screen is the order that is saved', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, [
    row({
      tracks: [member(TRACK, 'Enter Sandman', 'Metallica'), member(OTHER_TRACK, 'Rooster', 'Alice In Chains')],
      total: 2
    })
  ]);
  await refresh(page);
  await page.locator('.playlist-open').click();
  await expect(page.locator('.playlist-member')).toHaveCount(2);
  await expect(page.locator('.playlist-member').first()).toContainText('Enter Sandman');

  await page.locator('.playlist-member').first().locator('[title="Move down"]').click();
  await expect(page.locator('.playlist-member').first()).toContainText('Rooster');
  await save(page);

  const [saved] = await calls(page, 'save_playlist');
  // Positions are written from the screen order rather than carried along with
  // the rows, and the display hints travel with the members they belong to.
  expect(saved.playlist.tracks.map((track) => track.fileId)).toEqual([OTHER_TRACK, TRACK]);
  expect(saved.playlist.tracks.map((track) => track.position)).toEqual([1, 2]);
  expect(saved.playlist.tracks.map((track) => track.title)).toEqual(['Rooster', 'Enter Sandman']);
});

test('a track is added from the library, and only once', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, []);
  await page.getByRole('button', { name: 'New playlist' }).click();
  await page.getByLabel('Title', { exact: true }).fill('Driving');
  await expect(page.locator('.playlist-members .empty-state')).toHaveText('No tracks yet');

  await page.locator('.playlist-track-search input').fill('Search');
  await page.locator('.playlist-track-search button').click();
  await expect(page.locator('.playlist-candidate')).toHaveCount(1);
  await page.locator('.playlist-candidate button').click();
  await expect(page.locator('.playlist-member')).toHaveCount(1);
  // The same file is not a member twice, so its button goes quiet instead.
  await expect(page.locator('.playlist-candidate button')).toBeDisabled();

  await save(page);
  const [saved] = await calls(page, 'save_playlist');
  expect(saved.playlist.tracks.map((track) => track.fileId)).toEqual([TRACK]);
  expect(saved.playlist.tracks[0].position).toBe(1);
  expect(saved.playlist.tracks[0].title).toBe('Search');
});

test("the author's own words outrank a suggestion, and both answers travel", async ({ page }) => {
  await openPlaylists(page);
  await seed(page, []);
  await page.getByRole('button', { name: 'New playlist' }).click();
  await page.getByLabel('Title', { exact: true }).fill('Driving');
  const suggest = page.getByLabel('Suggest words from the title', { exact: true });
  const words = page.getByLabel('Search words', { exact: true });

  // Nothing of the author's and no suggestion accepted: the event has to say so,
  // because an empty tag list cannot.
  await suggest.uncheck();
  await publish(page);
  const [declined] = await calls(page, 'publish_playlist');
  expect(declined.suggestTags).toBe(false);
  expect(declined.playlist.tags).toBe('');

  // One word of their own turns the suggestion off, and their words are what
  // the event carries.
  await words.fill('rock, driving');
  await expect(suggest).toBeDisabled();
  await expect(page.locator('.playlist-fields .privacy-note').first()).toHaveCount(0);
  await publish(page);
  const publishes = await calls(page, 'publish_playlist');
  expect(publishes).toHaveLength(2);
  expect(publishes[1].playlist.tags).toBe('rock, driving');
});

test('a track this computer does not hold can be listed, and the row says where the name came from', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, []);
  await page.evaluate(() => {
    window.networkSearchResults = [
      {
        fileId: 'd'.repeat(64),
        filename: 'Notmine.wav',
        title: 'Not mine',
        artist: 'Someone Else',
        album: 'Other album',
        format: 'WAV',
        mime: 'audio/wav',
        size: 100,
        license: '',
        description: '',
        tags: '',
        sources: []
      }
    ];
  });
  // The search offers network rows only once the window knows it is connected.
  await expect(page.locator('.connection-box')).toContainText('Nostr connected');
  await page.getByRole('button', { name: 'New playlist' }).click();
  await page.getByLabel('Title', { exact: true }).fill('Things to hear');
  await page.locator('.playlist-track-search input').fill('Not mine');
  await page.locator('.playlist-track-search button').click();

  // Two rows: the one that is here, and the one only the network knows about.
  await expect(page.locator('.playlist-candidate')).toHaveCount(2);
  const remote = page.locator('.playlist-candidate').filter({ hasText: 'Not mine' });
  await expect(remote.locator('.playlist-badge')).toHaveText('Not on this computer');
  await remote.locator('button').click();

  // The member's name comes from the playlist, and the row says so. A track that
  // is here is not marked: its own entry is what names it.
  const member = page.locator('.playlist-member').filter({ hasText: 'Not mine' });
  await expect(member.locator('.playlist-badge')).toHaveText('From the playlist');
  await expect(page.locator('.playlist-member').filter({ hasText: 'Search' }).locator('.playlist-badge')).toHaveCount(0);

  await save(page);
  const [saved] = await calls(page, 'save_playlist');
  expect(saved.playlist.tracks).toHaveLength(1);
  expect(saved.playlist.tracks[0]).toMatchObject({
    position: 1,
    fileId: 'd'.repeat(64),
    title: 'Not mine',
    artist: 'Someone Else',
    album: 'Other album'
  });
});

test('a playlist that names a picture shows it, and can forget it', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, [row({ image: '9'.repeat(64) })]);
  await refresh(page);
  await page.locator('.playlist-open').click();
  const artwork = page.locator('.playlist-id').filter({ hasText: 'Artwork' });
  await expect(artwork.locator('code')).toHaveText('9'.repeat(64));
  await artwork.locator('button').click();
  await expect(artwork).toHaveCount(0);

  await save(page);
  const [saved] = await calls(page, 'save_playlist');
  expect(saved.playlist.image).toBe('');
});

test('deleting withdraws a published playlist and only forgets a draft', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, [
    row({ title: 'Published mix', published: true }),
    row({ playlistId: NEW_ID, title: 'Draft mix' })
  ]);
  await refresh(page);

  // The draft goes quietly: nothing was ever sent anywhere for it.
  await page.locator('.playlist-row').filter({ hasText: 'Draft mix' }).locator('[title="Delete"]').click();
  await expect(page.locator('.playlist-row')).toHaveCount(1);
  expect(await calls(page, 'delete_playlist')).toHaveLength(1);
  expect(await calls(page, 'withdraw_playlist')).toHaveLength(0);

  // The published one has to be taken back from the relays as well, or its
  // author would find it again on the next client that looked.
  await page.locator('.playlist-row').filter({ hasText: 'Published mix' }).locator('[title="Delete"]').click();
  await expect(page.locator('.playlist-row')).toHaveCount(0);
  expect(await calls(page, 'withdraw_playlist')).toHaveLength(1);
  expect(await calls(page, 'delete_playlist')).toHaveLength(1);
});
