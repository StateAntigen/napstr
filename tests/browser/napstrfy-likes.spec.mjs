import { test, expect } from '@playwright/test';
import { mockNative, serveAudio } from './helpers/native.mjs';

/**
 * Where a phone's likes and playlists live.
 *
 * Two places, on purpose: this phone keeps its own copy, so the liked page draws
 * with nothing reachable and a like lands instantly, and the computer it acts
 * through keeps the same set under the phone's key, so the list survives a
 * reinstall and can be read by the same key on another phone. Neither copy is
 * authoritative - they are merged when they meet - which is what these tests are
 * about.
 *
 * The lists are filed under the *phone's* key rather than the computer's, so
 * what moves when the home computer changes is this phone's own things. That
 * move is the other half: it is the moment a person is deciding which computer
 * their lists live on, so it happens when they say so and it says what it did.
 */

const id = (letter) => letter.repeat(64);

const track = (fileId, title) => ({
  fileId,
  filename: `${title}.mp3`,
  title,
  artist: 'Artist',
  album: 'Album',
  format: 'MP3',
  mime: 'audio/mpeg',
  size: 4_000_000,
  tags: '',
  local: true,
  sources: [],
  bitrateKbps: 128,
  sampleRateHz: 44_100,
  channels: 2,
  lossless: false,
  durationMs: 200_000
});

const mine = track(id('a'), 'My Song');
const other = track(id('b'), 'Another Song');

const full = { browse: true, fetch: true, control: true, download: true, privileged: true };

const myComputer = { endpointId: 'endpoint', desktopName: 'My Napstr', rights: full, home: true, included: true, online: true, mayDownload: true };
const second = { endpointId: id('f'), desktopName: 'Studio Napstr', rights: full, home: false, included: true, online: true, mayDownload: true };

async function openApp(page, { library = [mine, other], known = [myComputer], hostLikes = [], likes = [], orphans = [], synced = [], playlistRows = null, identity = null, carryFails = false } = {}) {
  await mockNative(page, { platform: 'android' });
  await page.route('**/fixture.wav', serveAudio);
  await page.addInitScript(({ library, known, hostLikes, likes, orphans, synced, playlistRows, identity, carryFails }) => {
    window.remoteHosts = known;
    window.remoteLibrary = library;
    window.hostLikes = hostLikes;
    window.playlistStore = playlistRows ?? [];
    window.carryFails = carryFails;
    if (identity) window.nostrIdentity = { pubkey: identity, npub: `npub1${'c'.repeat(58)}` };
    if (likes.length > 0) window.localStorage.setItem('napstrfy-liked-music', JSON.stringify(likes));
    if (orphans.length > 0) window.localStorage.setItem('napstrfy-liked-orphans', JSON.stringify(orphans));
    if (synced.length > 0) window.localStorage.setItem('napstrfy-likes-synced', JSON.stringify(synced));
  }, { library, known, hostLikes, likes, orphans, synced, playlistRows, identity, carryFails });
  await page.goto('http://127.0.0.1:15174');
}

/** The calls the phone made, by command name. */
const calls = (page, cmd) => page.evaluate((name) => window.calls.filter((call) => call.cmd === name).map((call) => call.args), cmd);

async function openLiked(page) {
  // The liked chip lives on the search page, which is where a person opens it
  // from - the library page has its own shelves.
  await page.locator('button[data-tab="search"]').click();
  await page.locator('.chips-row button', { hasText: 'Liked' }).click();
  await expect(page.locator('.library-heading h1')).toHaveText('Liked music');
}

async function openSettings(page) {
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(page.locator('.settings-scroll')).toBeVisible();
}

test('a like is written down on this phone and on the computer it acts through', async ({ page }) => {
  await openApp(page, { library: [mine] });
  await page.locator('button[data-tab="music"]').click();
  await expect(page.locator('.track-row')).toHaveCount(1);

  // The track's own menu, which is where the like lives.
  await page.locator('.track-row .track-more').first().click();
  await page.getByRole('button', { name: 'Add to Liked Songs' }).click();

  // The phone's copy first, because that is the one that is drawn: it is there
  // before any computer is asked, which is what makes a like feel instant.
  await expect
    .poll(() => page.evaluate(() => JSON.parse(window.localStorage.getItem('napstrfy-liked-music') ?? '[]').map((row) => row.fileId)))
    .toEqual([mine.fileId]);

  // And the computer's copy, as ids under this phone's key.
  await expect.poll(() => calls(page, 'remote_set_likes')).toContainEqual({ fileIds: [mine.fileId] });
});

test('a like the computer holds and this phone does not is merged in rather than lost', async ({ page }) => {
  // The computer knows about a like this phone has never seen: a reinstall, or
  // the same key on another phone. The id becomes a record here, drawn from the
  // library the computer answers with - and because the two lists are now the
  // same one, nothing is sent back for sending's sake.
  await openApp(page, { library: [mine, other], hostLikes: [other.fileId] });

  await openLiked(page);
  await expect(page.locator('.track-row strong')).toHaveText(['Another Song']);
  await expect
    .poll(() => page.evaluate(() => JSON.parse(window.localStorage.getItem('napstrfy-liked-music') ?? '[]').map((row) => row.fileId)))
    .toEqual([other.fileId]);
  expect(await calls(page, 'remote_set_likes')).toEqual([]);
});

test('a like made with nothing reachable is sent when a computer is back', async ({ page }) => {
  // The like is on this phone and nowhere else, because it was made while the
  // computer was asleep - or from the queue, with the app offline. This is the
  // moment the two copies meet, and the computer is the one that has to catch
  // up.
  await openApp(page, { library: [mine], likes: [mine], hostLikes: [] });

  await expect.poll(() => calls(page, 'remote_set_likes')).toContainEqual({ fileIds: [mine.fileId] });
  await expect.poll(() => page.evaluate(() => window.hostLikes)).toEqual([mine.fileId]);
});

test('a like removed on this phone is not put back by the other copy', async ({ page }) => {
  // The one way a merge can lose a decision: the computer still holds a like
  // that was removed here while it could not be told, and a plain union would
  // add it straight back. What tells the two apart is what the copies agreed on
  // last time, so this is the test for that.
  await openApp(page, { library: [other], hostLikes: [other.fileId], synced: [other.fileId] });

  await openLiked(page);
  await expect(page.locator('.track-row')).toHaveCount(0);
  expect(await page.evaluate(() => JSON.parse(window.localStorage.getItem('napstrfy-liked-music') ?? '[]'))).toEqual([]);
  // And the removal is what goes over, so the computer stops holding it either.
  await expect.poll(() => calls(page, 'remote_set_likes')).toContainEqual({ fileIds: [] });
  await expect.poll(() => page.evaluate(() => window.hostLikes)).toEqual([]);
});

test('a like this phone cannot draw yet is kept until the library can describe it', async ({ page }) => {
  // A computer that has not indexed the file yet answers `libraryByIds` with
  // nothing. The like is still a like, so it is written down rather than
  // dropped - and the computer already holds it, so there is nothing to send.
  await openApp(page, { library: [], hostLikes: [id('9')] });

  await expect
    .poll(() => page.evaluate(() => JSON.parse(window.localStorage.getItem('napstrfy-liked-orphans') ?? '[]')))
    .toEqual([id('9')]);
  expect(await calls(page, 'remote_set_likes')).toEqual([]);
});

test('a playlist filed under the computer\'s key is not this phone\'s to change', async ({ page }) => {
  // The phone's key and the computer's are different keys, and a playlist
  // belongs to the key that wrote it. The computer's own playlist is therefore
  // read-only here even though the phone reaches it through that computer -
  // editing it would be a copy, which is a different action from editing.
  await openApp(page, {
    identity: id('5'),
    playlistRows: [
      { playlistId: 'aaaaaaaa-1111-4111-8111-111111111111', title: 'Mine', author: id('5'), tracks: [] },
      { playlistId: 'bbbbbbbb-1111-4111-8111-111111111111', title: 'Theirs', author: id('c'), tracks: [] }
    ]
  });
  await page.locator('button[data-tab="playlists"]').click();

  await expect(page.locator('.playlist-row', { hasText: 'Mine' })).toHaveCount(1);
  await expect(page.locator('.playlist-row', { hasText: 'Theirs' })).toHaveCount(1);
  // One of the two is offered as an editable playlist and the other as a
  // read-only one, and which is which is the whole point.
  await expect(page.locator('.playlist-row', { hasText: 'Theirs' })).toContainText('Read-only');
});

test('choosing a new home computer carries this phone\'s lists across', async ({ page }) => {
  await openApp(page, {
    known: [myComputer, second],
    library: [mine],
    likes: [mine],
    hostLikes: [mine.fileId]
  });
  await openSettings(page);

  await page.locator('.computer-row', { hasText: 'Studio Napstr' })
    .getByRole('button', { name: 'Make Studio Napstr the home computer' })
    .click();

  // Both computers are named, and the phone's own liked list travels with the
  // request: a like made while nothing was reachable is part of what lands on
  // the new computer rather than something only this phone knows about.
  await expect
    .poll(() => calls(page, 'carry_own_data'))
    .toEqual([{ from: myComputer.endpointId, to: second.endpointId, likes: [mine.fileId] }]);
  await expect(page.locator('.notice, .settings-scroll')).toBeVisible();
  await expect.poll(() => page.evaluate(() => document.body.textContent ?? '')).toContain('Moved');
});

test('a carry that cannot reach the computer being left says nothing was lost', async ({ page }) => {
  await openApp(page, {
    known: [myComputer, second],
    library: [mine],
    likes: [mine],
    hostLikes: [mine.fileId],
    carryFails: true
  });
  await openSettings(page);

  await page.locator('.computer-row', { hasText: 'Studio Napstr' })
    .getByRole('button', { name: 'Make Studio Napstr the home computer' })
    .click();

  // The failure is reported as a failure, and the thing a person would worry
  // about is answered in the same sentence: the lists are still where they were.
  await expect.poll(() => page.evaluate(() => document.body.textContent ?? '')).toContain('Nothing was lost');
  expect(await page.evaluate(() => JSON.parse(window.localStorage.getItem('napstrfy-liked-music') ?? '[]').length)).toBe(1);
  // And the choice itself still happened: a move that failed is not a reason to
  // leave the phone pointed at the old computer.
  await expect.poll(() => calls(page, 'set_mobile_home_host')).toEqual([{ endpointId: second.endpointId }]);
});
