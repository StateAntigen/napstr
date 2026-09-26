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
  // A phone, because that is what these screens are: a desktop window draws a
  // sidebar where a phone has its bottom bar, and the full-screen sheets have to
  // cover a phone's screen rather than a column of one.
  await mockNative(page, { platform: 'android' });
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

function member(position, fileId, title, artist, album = '') {
  return { position, fileId, title, artist, album };
}

function calls(page, cmd) {
  return page.evaluate((name) => window.calls.filter((call) => call.cmd === name).map((call) => call.args), cmd);
}

/**
 * The hardware back button, the way Android delivers it: the page is given the
 * press only while it advertises a destination, and answering consumes the flag.
 */
async function enableBack(page) {
  await page.addInitScript(() => {
    window.backAvailable = null;
    window.appExits = 0;
    window.NapstrfyBack = {
      setBackAvailable: (available) => { window.backAvailable = available; },
      setDrawerOpen: (open) => { window.backAvailable = open; }
    };
  });
}

function pressBack(page) {
  return page.evaluate(() => {
    if (!window.backAvailable) {
      window.appExits += 1;
      return 'exit';
    }
    window.backAvailable = false;
    window.dispatchEvent(new CustomEvent('napstrfy-back'));
    return 'handled';
  });
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
  await expect(page.locator('.playlist-sheet h1')).toHaveText('Driving');
  await expect(page.locator('.playlist-sheet .album-tracks li')).toHaveCount(1);
  await expect(page.locator('.playlist-sheet .track-copy strong')).toHaveText('Enter Sandman');
  // Nothing of the details or edit screen is on it.
  await expect(page.locator('.playlist-sheet .playlist-heading')).toHaveCount(0);

  // Nothing to scroll sideways to: a page wider than the phone is a page that
  // was sized wrong, which is what the playlist screen used to be.
  const sized = await page.evaluate(() => {
    const sheet = document.querySelector('.playlist-sheet');
    return {
      document: document.documentElement.scrollWidth,
      viewport: window.innerWidth,
      sheet: sheet?.scrollWidth ?? 0,
      sheetBox: sheet?.clientWidth ?? 0
    };
  });
  expect(sized.document).toBeLessThanOrEqual(sized.viewport);
  expect(sized.sheet).toBeLessThanOrEqual(sized.sheetBox);

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
  await expect(page.locator('.playlist-sheet h1')).toHaveText('Driving');
  // Back at the list: the sheet is closed and the row is there again.
  await page.getByLabel('Close the playlist').click();
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
  await expect(page.locator('.playlist-sheet .playlist-heading')).toHaveText('Edit playlist');
  await expect(page.locator('.playlist-sheet .playlist-save')).toHaveText('Save');
  // The album sheet's own back: an arrow in a circle, no text beside it.
  await expect(page.locator('.playlist-sheet .view-head .view-icon')).toHaveText('');

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

  await page.locator('.playlist-sheet .playlist-save').click();
  await expect(page.locator('.playlist-notice')).toHaveText('Saved on your computer');

  const [saved] = await calls(page, 'remote_save_playlist');
  expect(saved.playlist.tracks.map((track) => track.fileId)).toEqual([OTHER_TRACK]);
  expect(saved.playlist.tracks.map((track) => track.position)).toEqual([1]);
});

test('the add sheet lists what could go in, and toggles it in and out', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, [row({ title: 'Things to hear' })]);
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
  await page.locator('.playlist-open').click();
  await page.locator('.playlist-tools button').filter({ hasText: 'Add' }).click();

  // Three ways in, and the first is the computer's library itself.
  await expect(page.locator('.add-tabs button')).toHaveText(['Songs', 'Recently played', 'Liked Songs']);
  const rows = page.locator('.add-row');
  await expect(rows).toHaveCount(1);
  await expect(rows.first().locator('.picker-toggle')).not.toHaveClass(/on/);
  // The toggle is inside the press target, so the circle itself takes the tap
  // rather than the strip of dead space beside it.
  await expect(rows.first().locator('.track-open .picker-toggle')).toHaveCount(1);

  // Ticking adds it to the end and writes it down at once.
  await rows.first().locator('.picker-toggle').click();
  await expect(rows.first().locator('.picker-toggle')).toHaveClass(/on/);
  const [saved] = await calls(page, 'remote_save_playlist');
  expect(saved.playlist.tracks.map((track) => track.fileId)).toEqual([TRACK]);
  expect(saved.playlist.tracks[0].position).toBe(1);

  // Unticking takes it back out: anywhere on the row does the same thing.
  await rows.first().locator('.track-copy').click();
  await expect(rows.first().locator('.picker-toggle')).not.toHaveClass(/on/);
  const saves = await calls(page, 'remote_save_playlist');
  expect(saves[1].playlist.tracks).toEqual([]);

  await page.getByLabel('Close the add a track sheet').click();
  await expect(page.locator('.playlist-sheet .album-tracks li')).toHaveCount(0);
});

test('the other two tabs come from this phone, not the computer', async ({ page }) => {
  // Both lists are read when the page starts, so they are written before it does.
  await page.addInitScript(() => {
    window.localStorage.setItem(
      'napstrfy-liked-music',
      JSON.stringify([
        {
          fileId: 'f'.repeat(64), filename: 'Liked.wav', title: 'Liked song', artist: 'Someone',
          album: 'Some album', format: 'WAV', mime: 'audio/wav', size: 100, tags: '', local: true, sources: []
        }
      ])
    );
    window.localStorage.setItem(
      'napstrfy-played-tracks',
      JSON.stringify([{ fileId: 'e'.repeat(64), title: 'Played song', artist: 'Somebody', album: 'Another album' }])
    );
  });
  await openPlaylists(page);
  await seed(page, [row({ title: 'Things to hear' })]);
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
  await page.locator('.playlist-open').click();
  await page.locator('.playlist-tools button').filter({ hasText: 'Add' }).click();

  await page.locator('.add-tabs button').filter({ hasText: 'Recently played' }).click();
  await expect(page.locator('.add-row strong')).toHaveText('Played song');
  // Neither tab asks the computer for anything: the one library page is the full
  // list's, and these two came off this phone.
  await page.locator('.add-tabs button').filter({ hasText: 'Liked Songs' }).click();
  await expect(page.locator('.add-row strong')).toHaveText('Liked song');
  expect((await calls(page, 'remote_library')).filter((call) => call.limit === 50)).toHaveLength(1);

  // A track this phone only knows from here can still be added, which is what
  // makes the two tabs worth having.
  await page.locator('.add-row').first().click();
  const [saved] = await calls(page, 'remote_save_playlist');
  expect(saved.playlist.tracks[0]).toMatchObject({ fileId: 'f'.repeat(64), title: 'Liked song' });
});

test('the add sheet asks the computer for the library a page at a time', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, [row({ title: 'Everything' })]);
  await page.evaluate(() => {
    window.remoteLibrary = Array.from({ length: 120 }, (_, index) => ({
      fileId: String(index).padStart(64, '0'),
      filename: `Track ${index}.wav`,
      title: `Track ${index}`,
      artist: 'Someone',
      album: 'Some album',
      format: 'WAV',
      mime: 'audio/wav',
      size: 100,
      tags: '',
      local: true,
      sources: []
    }));
  });
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
  await page.locator('.playlist-open').click();
  await page.locator('.playlist-tools button').filter({ hasText: 'Add' }).click();

  // The first page only, so a large library is not one answer. The music page
  // asks for its own library at startup, so the pages are counted by their size.
  await expect(page.locator('.add-row')).toHaveCount(50);
  const pages = async () => (await calls(page, 'remote_library')).filter((call) => call.limit === 50);
  expect((await pages()).at(-1)).toMatchObject({ query: '', offset: 0, limit: 50 });

  // Scrolling towards the end asks for the next one, and the rows accumulate.
  await page.locator('.add-list').evaluate((box) => { box.scrollTop = box.scrollHeight; });
  await expect(page.locator('.add-row')).toHaveCount(100);
  expect((await pages()).at(-1)).toMatchObject({ offset: 50, limit: 50 });

  await page.locator('.add-list').evaluate((box) => { box.scrollTop = box.scrollHeight; });
  await expect(page.locator('.add-row')).toHaveCount(120);
  // The end of the library is the end: nothing is asked for past it.
  await page.locator('.add-list').evaluate((box) => { box.scrollTop = box.scrollHeight; });
  await expect.poll(async () => (await pages()).length).toBe(3);
  await expect(page.locator('.add-row')).toHaveCount(120);
});

test('a playlist with no name cannot be added to, and says so', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, []);
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
  await page.locator('.playlist-new').click();
  // A new playlist opens where it can be named, and can be looked at before it is.
  await page.getByLabel('Back to the playlist').click();
  await page.locator('.playlist-tools button').filter({ hasText: 'Add' }).click();
  await page.locator('.add-row').first().click();

  await expect(page.locator('.add-view .error-card')).toHaveText('Give this playlist a name before adding tracks to it');
  expect(await calls(page, 'remote_save_playlist')).toHaveLength(0);
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
  await page.locator('.tag-input').fill('rock');
  await page.locator('.tag-input').press('Enter');
  await page.locator('.tag-input').fill('driving');
  await page.locator('.tag-input').press(',');
  await expect(page.locator('.tag-chip')).toHaveText(['rock', 'driving']);
  await expect(page.locator('.playlist-hint', { hasText: 'With no words of your own' })).toHaveCount(0);
  await expect(page.getByLabel('Suggest words from the title')).toBeDisabled();
  await page.getByRole('button', { name: 'Publish', exact: true }).click();
  const publishes = await calls(page, 'remote_publish_playlist');
  expect(publishes).toHaveLength(2);
  expect(publishes[1].playlist.tags).toBe('rock, driving');
});

test('a word is a chip: it is added, taken back and deleted', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, [row({ title: 'Words' })]);
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
  await page.locator('.playlist-open').click();
  await page.locator('.playlist-tools button').filter({ hasText: 'Name & details' }).click();

  // An empty box offers the word to type; a box with chips does not.
  await expect(page.locator('.tag-input')).toHaveAttribute('placeholder', 'Add a word');
  await page.locator('.tag-input').fill('metal');
  await page.locator('.tag-input').press('Enter');
  await expect(page.locator('.tag-chip')).toHaveText(['metal']);
  await expect(page.locator('.tag-input')).toHaveValue('');

  // A comma separates too, and a word already there is not added twice.
  await page.locator('.tag-input').fill('driving,');
  await page.locator('.tag-input').press('Enter');
  await page.locator('.tag-input').fill('metal');
  await page.locator('.tag-input').press('Enter');
  await expect(page.locator('.tag-chip')).toHaveText(['metal', 'driving']);

  // Backspace on an empty box takes the last word back, and the x takes any.
  await page.locator('.tag-input').press('Backspace');
  await expect(page.locator('.tag-chip')).toHaveText(['metal']);
  await page.locator('.tag-chip button').click();
  await expect(page.locator('.tag-chip')).toHaveCount(0);

  await page.getByRole('button', { name: 'Save', exact: true }).click();
  const [saved] = await calls(page, 'remote_save_playlist');
  expect(saved.playlist.tags).toBe('');
});

test('deleting asks first, and says which kind of delete it is', async ({ page }) => {
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
  await expect(page.locator('.actions-row.danger')).toBeVisible();
  await expect(page.locator('.actions-panel small')).toHaveText('Nothing on the relays points at this one, so it is only forgotten here.');

  // The first press only asks, and a cancel leaves the playlist where it was.
  expect(await calls(page, 'remote_delete_playlist')).toHaveLength(0);
  await page.locator('.actions-row').filter({ hasText: 'Cancel' }).click();
  await expect(page.locator('.actions-view')).toHaveCount(0);
  await expect(page.locator('.playlist-field input').first()).toHaveValue('Draft mix');
  expect(await calls(page, 'remote_delete_playlist')).toHaveLength(0);

  await page.getByRole('button', { name: 'Delete', exact: true }).click();
  await page.locator('.actions-row.danger').click();
  await expect(page.locator('.playlist-row')).toHaveCount(1);
  expect(await calls(page, 'remote_delete_playlist')).toHaveLength(1);
  expect(await calls(page, 'remote_withdraw_playlist')).toHaveLength(0);

  // The published one has to be taken back off the relays as well, and the
  // confirmation has to say which of the two it is offering.
  await page.locator('.playlist-open').click();
  await page.locator('.playlist-tools button').filter({ hasText: 'Name & details' }).click();
  await page.getByRole('button', { name: 'Withdraw', exact: true }).click();
  await expect(page.locator('.actions-panel small')).toHaveText('This is published, so it will be withdrawn from the relays as well.');
  await page.locator('.actions-row.danger').click();
  await expect(page.locator('.playlist-row')).toHaveCount(0);
  expect(await calls(page, 'remote_withdraw_playlist')).toHaveLength(1);
});

test('a private playlist cannot be published, and says why', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, [row({ title: 'Just for me' })]);
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
  await page.locator('.playlist-open').click();
  await page.locator('.playlist-tools button').filter({ hasText: 'Name & details' }).click();

  // Nothing is private until the author says so.
  await expect(page.getByLabel('Private')).not.toBeChecked();
  await expect(page.getByRole('button', { name: 'Publish', exact: true })).toBeEnabled();

  await page.getByLabel('Private').check();
  await expect(page.getByRole('button', { name: 'Publish', exact: true })).toBeDisabled();
  await expect(page.locator('.playlist-hint', { hasText: 'stays on this computer' })).toHaveText(
    'A private playlist stays on this computer and the phone it is paired with, and is never published.'
  );

  await page.getByRole('button', { name: 'Save', exact: true }).click();
  const [saved] = await calls(page, 'remote_save_playlist');
  expect(saved.playlist.private).toBe(true);

  // And it can be handed back to the relays again.
  await page.getByLabel('Private').uncheck();
  await expect(page.getByRole('button', { name: 'Publish', exact: true })).toBeEnabled();
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
  await expect(page.locator('.playlist-sheet .track-copy strong').first()).toHaveText('Rooster');

  await page.locator('.playlist-tools button').filter({ hasText: 'Sort' }).click();
  await page.locator('.actions-row').filter({ hasText: 'Title A to Z' }).click();

  // Sorting is one finished edit: the order on screen is the order that is sent,
  // with the positions renumbered from the top.
  await expect(page.locator('.playlist-sheet .track-copy strong').first()).toHaveText('Enter Sandman');
  const [saved] = await calls(page, 'remote_save_playlist');
  expect(saved.playlist.tracks.map((track) => track.fileId)).toEqual([TRACK, OTHER_TRACK]);
  expect(saved.playlist.tracks.map((track) => track.position)).toEqual([1, 2]);

  await page.locator('.playlist-tools button').filter({ hasText: 'Sort' }).click();
  await page.locator('.actions-row').filter({ hasText: 'Reverse the order' }).click();
  await expect(page.locator('.playlist-sheet .track-copy strong').first()).toHaveText('Rooster');
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
  await expect(page.locator('.playlist-sheet .album-tracks li.playing')).toHaveCount(1);
});

test('a track is added to a playlist from its menu, and taken out again', async ({ page }) => {
  await mockNative(page, { platform: 'android' });
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

test('every member row says where the track is, and carries its cover', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, [
    row({
      tracks: [
        member(1, TRACK, 'Enter Sandman', 'Metallica'),
        member(2, OTHER_TRACK, 'Rooster', 'Alice In Chains'),
        member(3, 'd'.repeat(64), 'Elsewhere', 'Somebody')
      ]
    })
  ]);
  // The computer holds the first two, and this phone has only the first; the
  // third is a file nothing here has.
  await page.evaluate((other) => {
    const library = (fileId, title, artist, album) => ({
      fileId, filename: `${title}.wav`, title, artist, album, format: 'WAV',
      mime: 'audio/wav', size: 100, tags: '', local: true, sources: []
    });
    window.remoteLibrary = [
      library('a'.repeat(64), 'Search', 'Settings', 'User album'),
      library(other, 'Rooster', 'Alice In Chains', 'Dirt')
    ];
  }, OTHER_TRACK);
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
  await page.locator('.playlist-open').click();

  // Three different answers, said on the row rather than left to be worked out.
  await expect(page.locator('.playlist-sheet .track-row')).toHaveCount(3);
  await expect(page.locator('.playlist-sheet .track-badge.phone')).toHaveCount(1);
  await expect(page.locator('.playlist-sheet .track-badge.computer')).toHaveCount(1);
  await expect(page.locator('.playlist-sheet .track-badge.network')).toHaveCount(1);

  // Every row draws a tile and offers its menu straight away, rather than the
  // menu appearing only once the track has been played.
  await expect(page.locator('.playlist-sheet .track-row .artwork')).toHaveCount(3);
  await expect(page.locator('.playlist-sheet .track-more')).toHaveCount(3);
});

test('a member row drops the number for its album, and the page counts the size', async ({ page }) => {
  await openPlaylists(page);
  await seed(page, [
    row({
      tracks: [
        member(1, TRACK, 'Enter Sandman', 'Metallica', 'Metallica'),
        member(2, OTHER_TRACK, 'Rooster', 'Alice In Chains', 'Dirt'),
        member(3, 'd'.repeat(64), 'Elsewhere', 'Somebody', 'Some album')
      ]
    })
  ]);
  // The computer knows two of the three files, and how big they are; the third
  // is a file nothing here holds, so its share of the total is unknown.
  await page.evaluate(({ known, other }) => {
    const library = (fileId, title, artist, album, size) => ({
      fileId, filename: `${title}.wav`, title, artist, album, format: 'WAV',
      mime: 'audio/wav', size, tags: '', local: true, sources: []
    });
    window.remoteLibrary = [
      library(known, 'Enter Sandman', 'Metallica', 'Metallica', 12 * 1024 * 1024),
      library(other, 'Rooster', 'Alice In Chains', 'Dirt', 6 * 1024 * 1024)
    ];
  }, { known: TRACK, other: OTHER_TRACK });
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
  await page.locator('.playlist-open').click();

  // The album the row plays from, where the position number used to be - and
  // nothing numbered, so the two are never confusable.
  await expect(page.locator('.playlist-sheet .track-row .track-meta')).toHaveText([
    'Metallica',
    'Dirt',
    'Some album'
  ]);
  await expect(page.locator('.playlist-sheet .track-row strong')).toHaveText([
    'Enter Sandman',
    'Rooster',
    'Elsewhere'
  ]);

  // Eighteen megabytes across the members that are known, plus a "+" for the
  // one that is not: a lower bound says so rather than reading as the total. A
  // running time is not on offer at all - no duration is carried anywhere.
  await expect(page.locator('.playlist-sheet .album-meta')).toHaveText('3 Tracks · 18 MB+ · Draft');
});

test('the home screen carries a shelf of playlists', async ({ page }) => {
  // The library, a played album, and the playlists themselves all have to be
  // there before the page boots: the shelf is filled when the computer answers.
  const rows = [
    row({ title: 'Driving', tracks: [member(1, TRACK, 'Enter Sandman', 'Metallica')] }),
    row({ playlistId: NEW_ID, title: 'Late night' })
  ];
  await page.addInitScript(
    ({ seeded }) => {
      const library = (fileId, title, artist, album) => ({
        fileId, filename: `${title}.wav`, title, artist, album, format: 'WAV',
        mime: 'audio/wav', size: 100, tags: '', local: true, sources: []
      });
      window.remoteLibrary = [
        library('a'.repeat(64), 'Enter Sandman', 'Metallica', 'Metallica'),
        library('b'.repeat(64), 'Rooster', 'Alice In Chains', 'Dirt')
      ];
      window.localStorage.setItem(
        'napstrfy-played-albums',
        JSON.stringify([{ key: 'metallica|metallica', artist: 'Metallica', album: 'Metallica' }])
      );
      window.playlistStore = seeded;
    },
    { seeded: rows }
  );
  await mockNative(page, { platform: 'android' });
  await page.goto('http://127.0.0.1:15174');

  // Between the two album shelves, in the order the shelves are read in.
  const labels = page.locator('.album-shelf-block .section-label b');
  await expect(labels).toHaveText(['Last played', 'Playlists', 'Discover albums']);
  const shelf = page.locator('.album-shelf-block').filter({ hasText: 'Playlists' });
  await expect(shelf.locator('.album-card strong')).toHaveText(['Driving', 'Late night']);
  // The card draws the album the playlist opens with, from the library.
  await expect(shelf.locator('.album-card').first().locator('.artwork')).toHaveCount(1);
  await expect(shelf.locator('.album-card').last().locator('.card-art-empty')).toHaveCount(1);

  // Opening one from the shelf lands on the playlist itself.
  await shelf.locator('.album-open').first().click();
  await expect(page.locator('.playlist-sheet h1')).toHaveText('Driving');
});

test('the tools pin to a title bar that fades in as they reach the top', async ({ page }) => {
  await openPlaylists(page);
  const tracks = Array.from({ length: 12 }, (_, index) =>
    member(index + 1, `${String(index + 1).padStart(2, '0')}${'f'.repeat(62)}`, `Song ${index + 1}`, 'Metallica')
  );
  await seed(page, [row({ tracks })]);
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
  await page.locator('.playlist-open').click();

  const scroller = page.locator('.playlist-sheet .album-scroll');
  const pin = page.locator('.playlist-sheet .playlist-pin');
  const bar = page.locator('.playlist-titlebar');
  const tools = page.locator('.playlist-sheet .playlist-tools');
  const arrow = page.locator('.playlist-sheet .view-head .view-icon');
  const disc = page.locator('.playlist-pin-play');

  // At rest there is no bar at all, and the out-of-flow band it occupies does
  // not swallow a press that belongs to the play button behind it. The disc that
  // joins the bar is out of the way for the same reason.
  await expect(pin).toHaveCSS('position', 'sticky');
  await expect(bar).toHaveCSS('opacity', '0');
  await expect(bar).toHaveCSS('pointer-events', 'none');
  await expect(disc).toHaveCSS('opacity', '0');
  await expect(disc).toHaveCSS('pointer-events', 'none');
  const playBox = await page.locator('.playlist-sheet .album-play-all:not(.playlist-pin-play)').boundingBox();
  expect(await page.evaluate(({ x, y }) => Boolean(document.elementFromPoint(x, y)?.closest('.album-play-all')),
    { x: playBox.x + playBox.width / 2, y: playBox.y + playBox.height / 2 })).toBe(true);

  // Scrolled to the end the bar is up, and the tools are pinned under it.
  await scroller.evaluate((node) => { node.scrollTop = node.scrollHeight; });
  await expect(pin).toHaveClass(/pinned/);
  await expect(bar).toHaveCSS('opacity', '1');
  await expect(bar).toHaveCSS('pointer-events', 'auto');
  await expect(bar.locator('strong')).toHaveText('Driving');
  // The name and nothing else: no count, no size, no status line.
  await expect(bar.locator('small')).toHaveCount(0);
  const [barBox, toolsBox, arrowBox] = [
    await bar.boundingBox(), await tools.boundingBox(), await arrow.boundingBox()
  ];
  // One band in the status area the sheet's arrow floats over: the bar takes the
  // very top of the screen - nothing of the page shows under the status bar -
  // and the tool row starts where the bar ends rather than in mid page.
  expect(barBox.y).toBeLessThanOrEqual(1);
  expect(barBox.y + barBox.height - toolsBox.y).toBeLessThanOrEqual(1);
  expect(toolsBox.y).toBeLessThanOrEqual(arrowBox.y + arrowBox.height + 4);
  // And the arrow is still what you hit in that band.
  expect(await page.evaluate(({ x, y }) => Boolean(document.elementFromPoint(x, y)?.closest('.view-head')),
    { x: arrowBox.x + arrowBox.width / 2, y: arrowBox.y + arrowBox.height / 2 })).toBe(true);

  // The disc is held with its middle on the bar's lower edge: half over the bar
  // and half over the tool row, and on top of both of them.
  await expect(disc).toHaveCSS('opacity', '1');
  const discBox = await disc.boundingBox();
  expect(Math.abs(discBox.y + discBox.height / 2 - (barBox.y + barBox.height))).toBeLessThanOrEqual(1);
  expect(toolsBox.y - discBox.y).toBeGreaterThan(0);
  expect(discBox.y + discBox.height - toolsBox.y).toBeGreaterThan(0);
  expect(await page.evaluate(({ x, y }) => Boolean(document.elementFromPoint(x, y)?.closest('.playlist-pin-play')),
    { x: discBox.x + discBox.width / 2, y: discBox.y + discBox.height / 2 })).toBe(true);

  // Scrolling back up puts it away again.
  await scroller.evaluate((node) => { node.scrollTop = 0; });
  await expect(pin).not.toHaveClass(/pinned/);
  await expect(bar).toHaveCSS('opacity', '0');
  await expect(disc).toHaveCSS('opacity', '0');
});

test('a long album or title is cut off rather than widening the sheet', async ({ page }) => {
  // A phone, because that is where the row is narrow enough for the text to
  // matter: on a desktop window the same name simply fits.
  await page.setViewportSize({ width: 412, height: 892 });
  await openPlaylists(page);
  const longTitle = 'Enter Sandman (Remastered 2026 Deluxe Edition With Bonus Tracks And Commentary)';
  const longAlbum = 'A Very Long Album Name That Would Otherwise Push The Whole Sheet Sideways Forever';
  await seed(page, [row({ tracks: [member(1, TRACK, longTitle, 'Metallica', longAlbum)] })]);
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
  await page.locator('.playlist-open').click();

  const name = page.locator('.playlist-sheet .track-copy strong');
  const album = page.locator('.playlist-sheet .track-meta');
  await expect(name).toHaveText(longTitle);
  await expect(album).toHaveText(longAlbum);
  // Cut off inside their own row: the text is there, the boxes are not wider.
  expect(await name.evaluate((node) => node.scrollWidth > node.clientWidth)).toBe(true);
  expect(await album.evaluate((node) => node.scrollWidth > node.clientWidth)).toBe(true);
  // So there is still nothing to scroll sideways to. The rows carry the sheet's
  // own width and no more.
  const sized = await page.evaluate(() => {
    const sheet = document.querySelector('.playlist-sheet');
    return {
      document: document.documentElement.scrollWidth,
      viewport: window.innerWidth,
      sheet: sheet?.scrollWidth ?? 0,
      sheetBox: sheet?.clientWidth ?? 0
    };
  });
  expect(sized.document).toBeLessThanOrEqual(sized.viewport);
  expect(sized.sheet).toBeLessThanOrEqual(sized.sheetBox);
});

test('back walks out of a playlist, its screens and its sheets', async ({ page }) => {
  await enableBack(page);
  await openPlaylists(page);
  await seed(page, [row({ tracks: [member(1, TRACK, 'Enter Sandman', 'Metallica')] })]);
  await page.locator('.bottom-nav button').filter({ hasText: 'Playlists' }).click();
  await page.locator('.playlist-open').click();
  await expect(page.locator('.playlist-sheet')).toBeVisible();
  await expect.poll(() => page.evaluate(() => window.backAvailable)).toBe(true);

  // A sheet over the playlist is the first thing a press closes.
  await page.locator('.playlist-tools button').filter({ hasText: 'Sort' }).click();
  await expect(page.locator('.actions-view')).toBeVisible();
  expect(await pressBack(page)).toBe('handled');
  await expect(page.locator('.actions-view')).toHaveCount(0);
  await expect(page.locator('.playlist-sheet')).toBeVisible();

  // Then the edit screen hands back to the playlist it came from.
  await page.locator('.playlist-tools button').filter({ hasText: 'Edit' }).click();
  await expect(page.locator('.playlist-sheet')).toBeVisible();
  expect(await pressBack(page)).toBe('handled');
  await expect(page.locator('.playlist-sheet')).toBeVisible();

  // And the playlist hands back to the list, which is the step before the tab.
  expect(await pressBack(page)).toBe('handled');
  await expect(page.locator('.playlist-row')).toBeVisible();
  expect(await pressBack(page)).toBe('handled');
  await expect(page.locator('.library-heading h1')).toHaveText('Your music');
});
