import { test, expect } from '@playwright/test';
import { mockNative } from './helpers/native.mjs';

// The window is a legacy-mode component: `let` and `$:`, no runes. Its own comments
// record the trap - a template expression that names only a function is not re-run
// when the state that function reads changes, so a list goes on drawing the rows it
// already had while a caption beside it counts the new ones. The list-derived values
// were converted to `$:` names with their state passed in; these tests hold the panes
// whose helpers were still called from the markup to the same rule, because those are
// the ones that were missed - the Downloads library and the audiobook banner both sat
// frozen on their first render.

/** A library bigger than one page, with every row telling you which page it is on. */
async function seedBigLibrary(page, count = 105) {
  await mockNative(page, { app: 'napstr' });
  await page.addInitScript((total) => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    window.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
      if (cmd === 'get_snapshot') {
        const snapshot = await invoke(cmd, args);
        const files = Array.from({ length: total }, (_, index) => ({
          ...snapshot.files[0],
          fileId: index.toString(16).padStart(64, '0'),
          filename: `Track ${String(index + 1).padStart(3, '0')}.wav`,
          title: `Track ${String(index + 1).padStart(3, '0')}`,
          folder: ''
        }));
        return { ...snapshot, files };
      }
      if (cmd === 'search_catalog') return (await window.__TAURI_INTERNALS__.invoke('get_snapshot')).files;
      return invoke(cmd, args);
    };
  }, count);
  await page.goto('http://127.0.0.1:15173');
  await expect(page.locator('.search-button')).toBeEnabled({ timeout: 20_000 });
}

/** Two folders, each a published audiobook, so the Shared pane has one of each to draw. */
async function seedAudiobookFolders(page) {
  await mockNative(page, { app: 'napstr' });
  await page.addInitScript(() => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    window.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
      if (cmd === 'get_snapshot') {
        const snapshot = await invoke(cmd, args);
        const folders = [
          { folder: 'Audiobooks/Book A', title: 'Book A' },
          { folder: 'Audiobooks/Book B', title: 'Book B' }
        ];
        const files = folders.flatMap((entry, book) =>
          [0, 1].map((index) => ({
            ...snapshot.files[0],
            fileId: (book * 2 + index + 1).toString(16).padStart(64, '0'),
            filename: `${entry.title} ${index + 1}.wav`,
            title: `${entry.title} ${index + 1}`,
            folder: entry.folder
          }))
        );
        const chapter = (file) => ({
          position: 1, fileId: file.fileId, filename: file.filename, title: file.title,
          format: 'WAV', mime: 'audio/wav', size: 1234567
        });
        const audiobooks = folders.map((entry, book) => ({
          audiobookId: `${book}`.repeat(8),
          title: entry.title,
          author: `Author ${entry.title.slice(-1)}`,
          narrator: '',
          totalSize: 2469134,
          chapters: [chapter(files[book * 2]), chapter(files[book * 2 + 1])],
          sources: [],
          local: true,
          localFolder: entry.folder
        }));
        return { ...snapshot, files, audiobooks };
      }
      return invoke(cmd, args);
    };
  });
  await page.goto('http://127.0.0.1:15173');
  await expect(page.locator('.search-button')).toBeEnabled({ timeout: 20_000 });
}

/** Pick a view folder the way a person does: the picker, then the folder in it. */
async function chooseFolder(page, folder) {
  await page.locator('.folder-picker-toggle').click();
  await page.locator('.folder-picker-menu button[role="option"]', { hasText: folder }).click();
}

test('the Downloads library draws the page the pager says it is on', async ({ page }) => {
  await seedBigLibrary(page);
  // Index 1 of the tool row is Downloads.
  await page.locator('.tool-button').nth(1).click();

  const rows = page.locator('.tags-table tbody tr');
  await expect(rows).toHaveCount(100);
  await expect(rows.first()).toContainText('Track 001');
  await expect(page.locator('.results-pager')).toContainText('Page 1 of 2');

  await page.locator('.results-pager button').filter({ hasText: 'Next' }).click();

  // The caption is a derived value with its state named, so it has always moved. The
  // table is the thing under test: it must move with it, or the window is showing the
  // first hundred files under a heading that claims to be showing the last five.
  await expect(page.locator('.results-pager')).toContainText('Page 2 of 2');
  await expect(rows).toHaveCount(5);
  await expect(rows.first()).toContainText('Track 101');

  // And back again, because a list that only moves once is a different bug.
  await page.locator('.results-pager button').filter({ hasText: 'Previous' }).click();
  await expect(page.locator('.results-pager')).toContainText('Page 1 of 2');
  await expect(rows.first()).toContainText('Track 001');
});

test('the Shared pane describes the audiobook of the folder that is chosen', async ({ page }) => {
  await seedAudiobookFolders(page);
  // Index 2 of the tool row is Shared.
  await page.locator('.tool-button').nth(2).click();

  const banner = page.locator('.audiobook-folder-banner');
  // Nothing is chosen yet, and the banner belongs to a chosen folder.
  await expect(banner).toHaveCount(0);

  await chooseFolder(page, 'Audiobooks/Book A');
  await expect(banner.locator('b')).toHaveText('Book A');

  await chooseFolder(page, 'Audiobooks/Book B');
  // Called straight from the markup this was drawn once and went on describing Book A
  // while the folder, the file list and the picker had all moved to Book B.
  await expect(banner.locator('b')).toHaveText('Book B');
  await expect(banner).toContainText('Author B');
});
