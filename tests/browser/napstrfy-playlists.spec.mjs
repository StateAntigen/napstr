import { test, expect } from '@playwright/test';
import { mockNative } from './helpers/native.mjs';

/**
 * Playlists on the phone.
 *
 * The playlist itself lives on the Napstr computer: this page lists what the
 * computer holds, and the editor writes every change back through the companion
 * channel rather than keeping a copy here. What these tests hold it to is
 * therefore the round trip — that a row the computer reports is the row on
 * screen, that the editor's order is the order the playlist is saved in, that a
 * publication carries the author's own words and their answer about suggested
 * ones, and that a delete withdraws a published playlist rather than leaving a
 * revision on the relays.
 */
const TRACK = 'a'.repeat(64);
const OTHER_TRACK = 'b'.repeat(64);
const NEW_ID = '22222222-2222-4222-8222-222222222222';

async function openPlaylists(page) {
  await mockNative(page);
  await page.goto('http://127.0.0.1:15174');
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
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
    displayName: 'Music computer',
    artist: '',
    mbid: '',
    image: '',
    tags: '',
    private: false,
    published: false,
    updatedAt: 1787000000,
    tracks: [],
    total: 0,
    ...overrides
  };
}

function member(position, fileId, title, artist) {
  return { position, fileId, title, artist, album: '' };
}

function calls(page, cmd) {
  return page.evaluate((name) => window.calls.filter((call) => call.cmd === name).map((call) => call.args), cmd);
}

test('the page lists what the computer holds, between search and podcasts', async ({ page }) => {
  await mockNative(page);
  await page.goto('http://127.0.0.1:15174');
  // The nav order is the phone's: Music, Search, Playlists, Podcasts, Audiobooks.
  const labels = await page.locator('.bottom-nav button').allTextContents();
  expect(labels.map((label) => label.replace(/[^A-Za-z]/g, ''))).toEqual([
    'Music',
    'Search',
    'Playlists',
    'Podcasts',
    'Audiobooks'
  ]);

  await seed(page, [
    row({ tracks: [member(1, TRACK, 'Enter Sandman', 'Metallica')] }),
    // A playlist that opens with a file this computer does not hold has no cover
    // to draw, and says so with a placeholder rather than a broken picture.
    row({ playlistId: NEW_ID, title: 'Late night', tracks: [member(1, OTHER_TRACK, 'Rooster', 'Alice in Chains')] })
  ]);
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
  const rows = page.locator('.playlist-row');
  await expect(rows.locator('strong')).toHaveText(['Driving', 'Late night']);
  await expect(rows.locator('small')).toHaveText(['1 Tracks · Draft', '1 Tracks · Draft']);
  // The row's picture is the album it opens with, resolved off the library.
  await expect(rows.first().locator('.playlist-row-art .artwork')).toHaveCount(1);
  await expect(rows.last().locator('.playlist-row-art-empty')).toHaveCount(1);
});

test('clicking a playlist shows it, and the tools are how it is changed', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, [row({ tracks: [member(1, TRACK, 'Enter Sandman', 'Metallica')] })]);
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
  await page.locator('.playlist-open').click();

  // The playlist, not the editor: the row named and its members listed.
  await expect(page.locator('.playlist-page h1')).toHaveText('Driving');
  await expect(page.locator('.playlist-page .album-tracks li')).toHaveCount(1);
  await expect(page.locator('.playlist-page .album-track-copy strong')).toHaveText('Enter Sandman');
  await expect(page.locator('.playlist-editor')).toHaveCount(0);

  const tools = await page.locator('.playlist-tools button').allTextContents();
  expect(tools.map((label) => label.replace(/\s+/g, ' ').trim())).toEqual([
    '+Add',
    '☰Edit',
    '</>Sort',
    'Name & details'
  ]);

  // Edit is one of the tools, not what clicking a playlist does.
  await page.locator('.playlist-tools button').filter({ hasText: 'Edit' }).click();
  await expect(page.locator('.playlist-member')).toHaveCount(1);
  await page.getByLabel('Back to the playlist').click();
  await expect(page.locator('.playlist-page h1')).toHaveText('Driving');
  // Back at the list: "Playlists" alone, not the sheet's "Smart playlists".
  await page.getByLabel('Playlists', { exact: true }).click();
  await expect(page.locator('.playlist-row strong')).toHaveText('Driving');
});

test('an empty list says how a playlist is made, and the plus opens a blank one', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, []);
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
  await expect(page.locator('.empty-library h2')).toHaveText('No playlists yet');

  await page.locator('.playlist-new').click();
  // Blank: a new playlist exists nowhere until it is saved, and the computer is
  // the side that minted the id it will be saved under. It opens where it can be
  // named, because a playlist with no name cannot be saved at all.
  await expect(page.locator('.playlist-field input').first()).toHaveValue('');
  await expect(page.getByRole('button', { name: 'Save', exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Publish', exact: true })).toBeDisabled();
  const minted = await calls(page, 'remote_new_playlist_id');
  expect(minted).toHaveLength(1);
});

test('dragging a row is what puts the playlist in order', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, [
    row({
      tracks: [
        member(1, TRACK, 'Enter Sandman', 'Metallica'),
        member(2, OTHER_TRACK, 'Rooster', 'Alice In Chains')
      ]
    })
  ]);
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
  await page.locator('.playlist-open').click();
  await page.locator('.playlist-tools button').filter({ hasText: 'Edit' }).click();
  await expect(page.locator('.playlist-member')).toHaveCount(2);
  await expect(page.locator('.playlist-member').first()).toContainText('Enter Sandman');
  // The screen names itself and carries its Save in the corner, rather than
  // repeating the playlist's name beside the arrow.
  await expect(page.locator('.playlist-edit .playlist-editor-head strong')).toHaveText('Edit playlist');
  await expect(page.locator('.playlist-edit .playlist-save')).toHaveText('Save');
  await expect(page.locator('.playlist-edit .playlist-back span')).toHaveCount(0);

  // The rows are the order: dragging one down past the other swaps them.
  const dragged = page.locator('.playlist-member').first();
  const grip = await dragged.locator('.playlist-grip').boundingBox();
  const size = await dragged.boundingBox();
  await page.mouse.move(grip.x + grip.width / 2, grip.y + grip.height / 2);
  await page.mouse.down();
  await page.mouse.move(grip.x + grip.width / 2, grip.y + grip.height / 2 + size.height, { steps: 8 });
  await page.mouse.up();
  await expect(page.locator('.playlist-member').first()).toContainText('Rooster');

  // The round minus on the left takes a row out of the list.
  await page.locator('.playlist-member').last().locator('.playlist-remove').click();
  await expect(page.locator('.playlist-member')).toHaveCount(1);

  await page.locator('.playlist-edit .playlist-save').click();
  await expect(page.locator('.playlist-notice')).toHaveText('Saved on your computer');

  const [saved] = await calls(page, 'remote_save_playlist');
  expect(saved.playlist.tracks.map((track) => track.fileId)).toEqual([OTHER_TRACK]);
  expect(saved.playlist.tracks.map((track) => track.position)).toEqual([1]);
});

test('a track is found on the computer or the network, and marked when this phone lacks it', async ({ page }) => {
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
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
  await page.locator('.playlist-new').click();
  await page.locator('.playlist-field input').first().fill('Things to hear');
  await page.getByLabel('Back to the playlist').click();
  // "+ Add" is the shortcut that writes through, rather than the editor's own
  // search, which fills a draft that Save is what keeps.
  await page.locator('.playlist-tools button').filter({ hasText: 'Add' }).click();
  await page.locator('.playlist-add-form input').fill('Not mine');
  await page.locator('.playlist-add-form button').click();

  const remote = page.locator('.actions-row').filter({ hasText: 'Not mine' });
  await expect(remote).toBeVisible();
  await remote.click();
  const [saved] = await calls(page, 'remote_save_playlist');
  expect(saved.playlist.tracks[0]).toMatchObject({
    position: 1,
    fileId: 'd'.repeat(64),
    title: 'Not mine',
    artist: 'Someone Else'
  });

  // The playlist on screen is the one the computer answered with, and a member
  // this phone has never fetched says so where it is edited.
  await page.getByLabel('Close the add a track sheet').click();
  await expect(page.locator('.playlist-page .album-tracks li')).toHaveCount(1);
  await expect(page.locator('.playlist-page .album-track-copy strong')).toHaveText('Not mine');
  await page.locator('.playlist-tools button').filter({ hasText: 'Edit' }).click();
  await expect(page.locator('.playlist-member .playlist-badge')).toHaveText('Not on this phone');
  // Its row still draws a tile, which falls back on its own when there is no
  // cover to look up.
  await expect(page.locator('.playlist-member-art .artwork')).toHaveCount(1);
});

test("publishing carries the author's words and their answer about suggested ones", async ({ page }) => {
  await openPlaylists(page);
  await seed(page, []);
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
  await page.locator('.playlist-new').click();
  await page.locator('.playlist-field input').first().fill('Driving');

  // Nothing of the author's, and the suggestion declined: the event cannot carry
  // the difference between "no" and "not asked", so the answer travels as
  // `suggestTags`.
  await page.getByLabel('Suggest words from the title').uncheck();
  await page.getByRole('button', { name: 'Publish', exact: true }).click();
  const [first] = await calls(page, 'remote_publish_playlist');
  expect(first.suggestTags).toBe(false);
  expect(first.playlist.tags).toBe('');

  // One word of their own turns the suggestion off - the question has been
  // answered - and their words are what the event carries.
  await expect(page.locator('.playlist-hint', { hasText: 'With no words of your own' })).toHaveText(
    'With no words of your own, this playlist is found by its name and its members alone.'
  );
  await page.locator('.playlist-field input').last().fill('rock, driving');
  await expect(page.locator('.playlist-hint', { hasText: 'With no words of your own' })).toHaveCount(0);
  await expect(page.getByLabel('Suggest words from the title')).toBeDisabled();
  await page.getByRole('button', { name: 'Publish', exact: true }).click();
  const publishes = await calls(page, 'remote_publish_playlist');
  expect(publishes).toHaveLength(2);
  expect(publishes[1].playlist.tags).toBe('rock, driving');
});

test('the details screen withdraws a published playlist and only forgets a draft', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, [
    row({ title: 'Published mix', published: true }),
    row({ playlistId: NEW_ID, title: 'Draft mix' })
  ]);
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();

  // A draft goes quietly: nothing was ever sent anywhere for it.
  await page.locator('.playlist-open').filter({ hasText: 'Draft mix' }).click();
  await page.locator('.playlist-tools button').filter({ hasText: 'Name & details' }).click();
  await page.getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(page.locator('.playlist-row')).toHaveCount(1);
  expect(await calls(page, 'remote_delete_playlist')).toHaveLength(1);
  expect(await calls(page, 'remote_withdraw_playlist')).toHaveLength(0);

  // The published one has to be taken back off the relays as well, and the
  // screen has to say which of the two it is offering.
  await page.locator('.playlist-open').click();
  await page.locator('.playlist-tools button').filter({ hasText: 'Name & details' }).click();
  await expect(page.getByRole('button', { name: 'Withdraw', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Withdraw', exact: true }).click();
  await expect(page.locator('.playlist-row')).toHaveCount(0);
  expect(await calls(page, 'remote_withdraw_playlist')).toHaveLength(1);
});

test('sorting reorders the playlist and writes the order down', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, [
    row({
      tracks: [
        member(1, OTHER_TRACK, 'Rooster', 'Alice In Chains'),
        member(2, TRACK, 'Enter Sandman', 'Metallica')
      ]
    })
  ]);
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
  await page.locator('.playlist-open').click();
  await expect(page.locator('.playlist-page .album-track-copy strong').first()).toHaveText('Rooster');

  await page.locator('.playlist-tools button').filter({ hasText: 'Sort' }).click();
  await page.locator('.actions-row').filter({ hasText: 'Title A to Z' }).click();

  // Sorting is one finished edit: the order on screen is the order that is sent,
  // with the positions renumbered from the top.
  await expect(page.locator('.playlist-page .album-track-copy strong').first()).toHaveText('Enter Sandman');
  const [saved] = await calls(page, 'remote_save_playlist');
  expect(saved.playlist.tracks.map((track) => track.fileId)).toEqual([TRACK, OTHER_TRACK]);
  expect(saved.playlist.tracks.map((track) => track.position)).toEqual([1, 2]);

  await page.locator('.playlist-tools button').filter({ hasText: 'Sort' }).click();
  await page.locator('.actions-row').filter({ hasText: 'Reverse the order' }).click();
  await expect(page.locator('.playlist-page .album-track-copy strong').first()).toHaveText('Rooster');
  const saving = await calls(page, 'remote_save_playlist');
  expect(saving[1].playlist.tracks.map((track) => track.fileId)).toEqual([OTHER_TRACK, TRACK]);
});

test('playing a playlist queues the members this phone holds', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, [
    row({
      tracks: [
        member(1, OTHER_TRACK, 'Rooster', 'Alice In Chains'),
        member(2, TRACK, 'Enter Sandman', 'Metallica')
      ]
    })
  ]);
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
  await page.locator('.playlist-open').click();
  await page.getByLabel('Play the playlist').click();

  // Only the file this phone can actually play is fetched: a queue that names a
  // member it cannot open would stop on it.
  await expect.poll(async () => (await calls(page, 'cache_remote_audio')).length).toBeGreaterThan(0);
  const [cached] = await calls(page, 'cache_remote_audio');
  expect(cached.track.fileId).toBe(TRACK);
  await expect(page.locator('.playlist-page .album-tracks li.playing')).toHaveCount(1);
});

test('a track is added to a playlist from its menu, and taken out again', async ({ page }) => {
  await mockNative(page);
  await page.goto('http://127.0.0.1:15174');
  await seed(page, [
    row({ title: 'Driving' }),
    row({ playlistId: NEW_ID, title: 'Late night', tracks: [member(1, TRACK, 'Enter Sandman', 'Metallica')] })
  ]);

  await page.locator('.track-row .track-more').first().click();
  await page.locator('.actions-row').filter({ hasText: 'Add to playlist' }).click();

  // The playlists it could go in, with the one that already holds it ticked.
  const picker = page.locator('.picker-row');
  await expect(picker).toHaveCount(2);
  await expect(page.locator('.picker-toggle.on')).toHaveCount(1);
  await expect(picker.filter({ hasText: 'Late night' }).locator('.picker-toggle')).toHaveClass(/on/);
  // The membership answer is one lookup, not a page of members per playlist.
  expect(await calls(page, 'remote_playlists_containing')).toHaveLength(1);

  // Ticking adds it to the end of that playlist.
  await picker.filter({ hasText: 'Driving' }).click();
  await expect(picker.filter({ hasText: 'Driving' }).locator('.picker-toggle')).toHaveClass(/on/);
  const [added] = await calls(page, 'remote_save_playlist');
  expect(added.playlist.playlistId).toBe('11111111-1111-4111-8111-111111111111');
  expect(added.playlist.tracks.map((track) => track.fileId)).toEqual([TRACK]);
  expect(added.playlist.tracks[0].position).toBe(1);
  // The row says how many members the playlist has now.
  await expect(picker.filter({ hasText: 'Driving' })).toContainText('1 Tracks');

  // Unticking takes it back out, and the rest are renumbered from the top.
  await picker.filter({ hasText: 'Late night' }).click();
  await expect(picker.filter({ hasText: 'Late night' }).locator('.picker-toggle')).not.toHaveClass(/on/);
  const saves = await calls(page, 'remote_save_playlist');
  expect(saves[1].playlist.playlistId).toBe(NEW_ID);
  expect(saves[1].playlist.tracks).toEqual([]);
  await expect(picker.filter({ hasText: 'Late night' })).toContainText('0 Tracks');
});
