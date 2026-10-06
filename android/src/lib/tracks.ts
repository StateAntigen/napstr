/**
 * One row per file, in the order they were given.
 *
 * A list of tracks is drawn keyed by file id, and a keyed list that names one
 * file twice is a list Svelte refuses to draw at all - not a row out of place,
 * the whole screen missing, silently, for as long as the list holds the repeat.
 *
 * Repeats are reachable from several directions, so this is a boundary rather
 * than a defence against one caller: a shuffled library paged while its files
 * were still arriving can name on a later page what an earlier one already did,
 * the network's own list names one file once per holder, and a computer's queue
 * names one copy per file. None of those is a mistake the row is making - they
 * are all just two ways of reaching one file - so the file is taken once.
 *
 * A row with no file id is dropped too: it cannot be identified, cannot be
 * played from, and two of them are the same collision under another name.
 */
export function dedupeByFile<T extends { fileId?: string }>(rows: readonly T[]): T[] {
  const seen = new Set<string>();
  return rows.filter((row) => {
    const fileId = row?.fileId;
    if (!fileId || seen.has(fileId)) return false;
    seen.add(fileId);
    return true;
  });
}
