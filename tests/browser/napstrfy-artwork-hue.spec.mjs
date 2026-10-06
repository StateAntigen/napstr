import { test, expect } from '@playwright/test';
import { mockNative, serveAudio } from './helpers/native.mjs';

// Where the drawer's colour comes from.
//
// It is read off the cover that is on screen, so the pixels of that picture have
// to be readable. They are only readable when the picture was fetched as a CORS
// request: a picture drawn from another origin without one taints the canvas, and
// every read off it throws. That is not a hypothetical here - on the phone every
// cover comes from the loopback server the app runs itself, which is a different
// origin to the page - so a spec that served covers from the page's own address
// would pass with the read completely broken. This one serves them the way the
// phone does, from another origin, and says so in the header the server really
// sends.
//
// The colours below are the fixture: a one-pixel picture of that colour is the
// input, and the hue the drawer ends up with is the output. Red is at the top of
// the colour wheel (hue 5); grey names no colour at all, which has to leave the
// app's own colour in place rather than invent one.

const id = (letter) => letter.repeat(64);

const track = (letter, title, album) => ({
  fileId: id(letter),
  filename: `${title}.wav`,
  title,
  artist: 'Rancid',
  album,
  format: 'WAV',
  mime: 'audio/wav',
  size: 1234567,
  tags: '',
  local: true,
  sources: []
});

const song = track('a', 'Ruby Soho', 'And Out Come the Wolves');

/** A one-pixel picture, truecolour: every pixel of it counts. */
const RED = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC',
  'base64'
);
const GREY = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGNoaGgAAAMEAYFL09IQAAAAAElFTkSuQmCC',
  'base64'
);

/** Where the phone's own server answers, which is not where the page was loaded from. */
const COVER_ORIGIN = 'http://127.0.0.1:15176';

/**
 * Play the host, with covers served the way the phone serves them.
 *
 * The address the claim came with is on a host nothing here serves, so a picture
 * that reached a publisher would fail to load rather than pass for a picture this
 * phone drew.
 */
async function openApp(page, { cover = RED, seed = 'red' } = {}) {
  await mockNative(page, { platform: 'android' });
  await page.route('**/fixture.wav', serveAudio);
  await page.addInitScript(
    ({ library, origin, seed }) => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      const name = (rendition) => `${seed}${rendition}`.padEnd(64, '0').slice(0, 64);
      window.remoteLibrary = library;
      window.__TAURI_INTERNALS__.invoke = async (cmd, args = {}) => {
        if (cmd === 'remote_covers') {
          return args.keys.map((key) => ({
            key,
            art: 'https://publisher.invalid/full.png',
            thumb: 'https://publisher.invalid/thumb.png',
            artHash: name('full'),
            thumbHash: name('thumb'),
            mbid: '',
            year: '',
            genre: '',
            collection: '',
            source: 'itunes',
            coverFileId: '',
            mime: 'image/png',
            author: '',
            seeder: false
          }));
        }
        if (cmd === 'remote_art') {
          return { url: `${origin}/${args.rendition}/${args.hash}`, hash: args.hash };
        }
        return invoke(cmd, args);
      };
    },
    { library: [song], origin: COVER_ORIGIN, seed }
  );
  await page.route(`${COVER_ORIGIN}/**`, (route) =>
    route.fulfill({
      contentType: 'image/png',
      headers: { 'Access-Control-Allow-Origin': '*' },
      body: cover
    })
  );
  await page.goto('http://127.0.0.1:15174');
}

/** Play the one track there is and open the drawer over it. */
async function playAndOpen(page) {
  await page.locator('.track-open').first().click();
  await expect.poll(() => page.evaluate(() => document.querySelector('audio')?.paused ?? true)).toBe(false);
  await page.locator('.now-open').click();
  await expect(page.locator('.now-sheet')).toBeVisible();
  // The filled part of the shape is what wears the colour, so the drawing has to
  // be there before it is asked about. It is not asked to be *visible*: this file
  // is silent, so every bar of it sits on the centre line and the path has no
  // height to be seen through. Its colour is a style either way, which is what
  // this is about.
  await expect(page.locator('.now-sheet-timeline.waves')).toBeVisible();
  await expect(page.locator('.now-sheet-wave path.played')).toHaveCount(1);
}

/** The hue the drawer settled on, which is what every tint inside it is built from. */
const sheetHue = (page) =>
  page.evaluate(() =>
    getComputedStyle(document.querySelector('.now-sheet')).getPropertyValue('--cover-hue').trim()
  );

/** The colour the filled part of the waveform is actually painted. */
const drawnFill = (page) =>
  page.evaluate(() => getComputedStyle(document.querySelector('.now-sheet-wave path.played')).fill);

/**
 * What the browser makes of a colour, so the comparison is between colours rather
 * than between two spellings of one.
 */
const paintOf = (page, colour) =>
  page.evaluate((value) => {
    const probe = document.createElement('span');
    probe.style.color = value;
    document.body.append(probe);
    const paint = getComputedStyle(probe).color;
    probe.remove();
    return paint;
  }, colour);

test('the drawer takes its colour from the cover, not from the track', async ({ page }) => {
  await openApp(page, { cover: RED });
  await playAndOpen(page);

  // Red, and specifically not the app's own colour, which is what a cover with no
  // readable pixels leaves behind.
  await expect.poll(() => sheetHue(page)).toBe('5');
  expect(await drawnFill(page)).toBe(await paintOf(page, 'hsl(5 100% 64%)'));
});

test('a cover with no colour in it leaves the app’s own colour in place', async ({ page }) => {
  await openApp(page, { cover: GREY, seed: 'grey' });
  await playAndOpen(page);

  // A grey sleeve has no colour to name, and the app does not invent one: the
  // fallback is its own, which is the violet it uses for everything else.
  await expect.poll(() => sheetHue(page)).toBe('247');
  expect(await drawnFill(page)).toBe(await paintOf(page, 'hsl(247 100% 64%)'));
});
