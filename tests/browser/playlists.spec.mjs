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

/** One of this identity's own coordinates as the relays answer for it. */
function relayRow(overrides = {}) {
  return {
    playlistId: '11111111-1111-4111-8111-111111111111',
    title: 'Driving',
    author: 'c'.repeat(64),
    displayName: '',
    image: '',
    firstFileId: '',
    trackCount: 0,
    private: false,
    published: true,
    updatedAt: 1787000000,
    ...overrides
  };
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
  // What the relays answered for this identity: the published one is there and
  // the draft has never been anywhere, which are two different kinds of row.
  await page.evaluate((known) => {
    window.playlistRelayOwn = known;
  }, [relayRow({ playlistId: NEW_ID, title: 'Published mix' })]);
  await seed(page, [row({ private: true }), row({ playlistId: NEW_ID, title: 'Published mix', published: true })]);
  await refresh(page);

  const rows = page.locator('.playlist-row');
  await expect(rows).toHaveCount(2);
  const draft = rows.filter({ hasText: 'Driving' });
  await expect(draft.locator('b')).toHaveText('Driving');
  // A row nothing has ever been sent for is a draft, and says so: the window has
  // no opinion about a playlist being private, and nothing to offer to withdraw.
  await expect(draft.locator('.playlist-badge')).toHaveText(['Draft']);
  // A row the relays answer for is labelled where it lives, and keeps the
  // published/draft state beside it.
  await expect(rows.filter({ hasText: 'Published mix' }).locator('.playlist-badge')).toHaveText([
    'Held locally',
    'Published'
  ]);
  // Both tiers are told apart: ours first, then everybody else's.
  await expect(page.locator('.playlist-group')).toHaveText(['Your playlists']);
});

test('a public playlist somebody else published is listed read-only, and saving one keeps a copy', async ({ page }) => {
  const theirs = '70e320fe962a67c58c61269ca1c5b4f0e128931267f32a92809aaf904425e376';
  await openPlaylists(page);
  await seed(page, [
    row({
      title: 'rock',
      author: theirs,
      displayName: 'Sean Parker',
      published: true,
      updatedAt: 1787000100,
      tracks: [member(TRACK, 'Enter Sandman', 'Metallica')],
      total: 1
    })
  ]);
  await refresh(page);

  // The second tier: somebody else's, with the name they publish under.
  await expect(page.locator('.playlist-group')).toHaveText(['From everyone else']);
  const rowEl = page.locator('.playlist-row').filter({ hasText: 'rock' });
  await expect(rowEl.locator('.playlist-badge')).toHaveText(['Read-only']);
  await expect(rowEl.locator('small')).toContainText('Sean Parker');
  // Nothing of theirs is deleted from here: the row has no remove button at all.
  await expect(rowEl.locator('.classic-button')).toHaveCount(0);

  // Opening it is a look, not an edit.
  await rowEl.locator('.playlist-open').click();
  await expect(page.getByLabel('Title', { exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Publish', exact: true })).toHaveCount(0);
  await expect(page.locator('.playlist-member [title="Move down"]')).toBeDisabled();
  // The only write on offer is the one that keeps a copy.
  await page.getByRole('button', { name: 'Save a copy' }).click();

  // Saving it files a copy of this identity's own, under an id of its own: the
  // revision never lands on the coordinate its author published under.
  const [saved] = await calls(page, 'save_playlist');
  expect(saved.playlist.author).toBe(theirs);
  await expect(page.locator('.playlist-id code')).toHaveText('33333333-3333-4333-8333-333333333333');
  await expect(page.locator('.playlist-notice')).toHaveText('Saved on this computer');
  await back(page);
  const mineRow = page.locator('.playlist-row').filter({ hasText: 'rock' });
  await expect(mineRow).toHaveCount(2);
  await expect(page.locator('.playlist-row').filter({ hasText: 'rock' }).locator('.playlist-badge')).toHaveText([
    'Draft',
    'Read-only'
  ]);
});

test('a playlist only the relays have can be restored, and one only here can be withdrawn', async ({ page }) => {
  await openPlaylists(page);
  // The relays have Night_Rider and this computer does not; this computer has
  // 'Bring me the playlist', which the relays answered without.
  await page.evaluate((known) => {
    window.playlistRelayOwn = known;
  }, [
    relayRow({
      playlistId: 'eb738de4-c6b7-47b0-ba32-b8948d06cbef',
      title: 'Night_Rider',
      trackCount: 100,
      updatedAt: 1787000200
    })
  ]);
  await seed(page, [row({ title: 'Bring me the playlist', published: true })]);
  await refresh(page);

  const night = page.locator('.playlist-row').filter({ hasText: 'Night_Rider' });
  await expect(night.locator('.playlist-badge')).toHaveText(['On the relays only']);
  const held = page.locator('.playlist-row').filter({ hasText: 'Bring me the playlist' });
  await expect(held.locator('.playlist-badge')).toHaveText(['Only on this computer']);
  await expect(held.locator('[title="Withdraw"]')).toHaveCount(1);

  // Restoring imports the revision the relays hold, under this identity's own
  // coordinate, and the row becomes one this computer holds rather than two.
  await night.locator('.classic-button').click();
  await expect(page.locator('.playlist-row').filter({ hasText: 'Night_Rider' })).toHaveCount(1);
  const [restored] = await calls(page, 'restore_playlist');
  expect(restored.playlistId).toBe('eb738de4-c6b7-47b0-ba32-b8948d06cbef');
  await expect(page.locator('.playlist-row').filter({ hasText: 'Night_Rider' }).locator('.playlist-badge')).toHaveText([
    'Held locally',
    'Published'
  ]);
  // Ours are listed before theirs, newest first inside each tier.
  await expect(page.locator('.playlist-row b').first()).toHaveText('Night_Rider');

  // Withdrawing is asked for, because the relays are told and it is not a local
  // delete: a dismissed confirmation changes nothing.
  page.once('dialog', (dialog) => dialog.dismiss());
  await held.locator('[title="Withdraw"]').click();
  await expect(page.locator('.playlist-row').filter({ hasText: 'Bring me the playlist' })).toHaveCount(1);
  expect(await calls(page, 'withdraw_playlist')).toHaveLength(0);

  page.once('dialog', (dialog) => dialog.accept());
  await held.locator('[title="Withdraw"]').click();
  await expect(page.locator('.playlist-row').filter({ hasText: 'Bring me the playlist' })).toHaveCount(0);
  expect(await calls(page, 'withdraw_playlist')).toHaveLength(1);
});

test('a look that failed never offers to withdraw anything', async ({ page }) => {
  await openPlaylists(page);
  await page.evaluate(() => {
    window.playlistRelayReadFails = true;
  });
  await seed(page, [row({ title: 'Published mix', published: true })]);
  await refresh(page);

  // "Nobody looked" is not "the relays do not have it".
  await expect(page.locator('.playlist-row').locator('.playlist-badge')).toHaveText([
    'Held locally',
    'Published'
  ]);
  await expect(page.locator('.playlist-row [title="Withdraw"]')).toHaveCount(0);
  await expect(page.locator('.playlist-row [title="Delete"]')).toHaveCount(1);
});

test('a member nobody seeds is shown as unavailable rather than dropped', async ({ page }) => {
  const seeded = 'd'.repeat(64);
  const gone = 'e'.repeat(64);
  await openPlaylists(page);
  // One member this computer holds, one others seed, and one nobody seeds at
  // all: all three are drawn, in position order, and each says what is out
  // there - an unavailable member is a member.
  await seed(page, [
    row({
      tracks: [
        member(TRACK, 'Search', 'Settings'),
        member(seeded, 'Rooster', 'Alice In Chains'),
        member(gone, 'Nothing at all', 'Nobody')
      ],
      total: 3
    })
  ]);
  await page.evaluate((counts) => {
    window.playlistMemberAvailability = counts;
  }, [
    { fileId: seeded, seeders: 3 },
    { fileId: gone, seeders: 0 }
  ]);
  await refresh(page);
  await page.locator('.playlist-open').click();

  const members = page.locator('.playlist-member');
  await expect(members).toHaveCount(3);
  // The member this computer holds needs no explanation.
  await expect(members.first().locator('.playlist-badge')).toHaveText(['This computer']);
  await expect(members.nth(1).locator('.playlist-badge')).toHaveText(['From the playlist', '3 seeders']);
  await expect(members.nth(2).locator('.playlist-badge')).toHaveText(['From the playlist', 'Unavailable']);
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
  // The relays have the published one, so removing it is a withdrawal; the draft
  // has never been anywhere and is only forgotten.
  await page.evaluate((known) => {
    window.playlistRelayOwn = known;
  }, [relayRow({ title: 'Published mix' })]);
  await refresh(page);

  // The draft goes quietly: nothing was ever sent anywhere for it.
  await page.locator('.playlist-row').filter({ hasText: 'Draft mix' }).locator('[title="Delete"]').click();
  await expect(page.locator('.playlist-row')).toHaveCount(1);
  expect(await calls(page, 'delete_playlist')).toHaveLength(1);
  expect(await calls(page, 'withdraw_playlist')).toHaveLength(0);

  // The published one has to be taken back from the relays as well, or its
  // author would find it again on the next client that looked. Taking it back is
  // the author's decision, so it is confirmed first.
  page.once('dialog', (dialog) => dialog.accept());
  await page.locator('.playlist-row').filter({ hasText: 'Published mix' }).locator('[title="Delete"]').click();
  await expect(page.locator('.playlist-row')).toHaveCount(0);
  expect(await calls(page, 'withdraw_playlist')).toHaveLength(1);
  expect(await calls(page, 'delete_playlist')).toHaveLength(1);
});
