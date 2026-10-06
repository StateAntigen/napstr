import test from 'node:test';
import assert from 'node:assert/strict';
import { dedupeByFile } from '../android/src/lib/tracks.ts';

// One row per file, because the list is drawn keyed by file and a repeat is not a
// row out of place: it is a list Svelte refuses to draw at all. The rule is the
// file, so it holds whatever the list was built from and whatever order it is in.

test('a file named twice is one row, where the first of them was', () => {
  const rows = dedupeByFile([{ fileId: 'a' }, { fileId: 'b' }, { fileId: 'a' }]);

  assert.deepEqual(rows.map((row) => row.fileId), ['a', 'b']);
});

test('the row that is kept is the one that arrived first, not a copy of it', () => {
  const first = { fileId: 'a', title: 'this phone’s copy' };
  const second = { fileId: 'a', title: 'the network’s copy' };

  const rows = dedupeByFile([first, second]);

  assert.equal(rows.length, 1);
  assert.equal(rows[0], first);
});

test('a row with no file id is dropped, because it names nothing to play', () => {
  const rows = dedupeByFile([{ fileId: '' }, { fileId: 'a' }, {}, { fileId: 'a' }]);

  assert.deepEqual(rows.map((row) => row.fileId), ['a']);
});

test('a list that names nothing of the same file is left exactly as it was', () => {
  const rows = [{ fileId: 'a' }, { fileId: 'b' }, { fileId: 'c' }];

  assert.equal(dedupeByFile(rows).length, 3);
  assert.equal(dedupeByFile([]).length, 0);
});
