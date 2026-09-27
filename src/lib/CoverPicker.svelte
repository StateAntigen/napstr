<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';

  /**
   * Choose album art by hand.
   *
   * The automatic lookup ranks by primary type and an exact title, which is
   * right nearly always but cannot know that a person meant a different
   * pressing — or that a record has art filed under a name the tags spell
   * differently. This shows what MusicBrainz actually offers, with the art the
   * archive holds for each, so a person can settle it in one look.
   *
   * The query box opens on the exact search the automatic lookup sends, so
   * editing starts from what Napstr already asked rather than from nothing.
   */
  export let artist = '';
  export let album = '';
  /** The art shown right now, for comparison. */
  export let currentArt = '';
  export let onClose: () => void = () => {};
  export let onApplied: () => void = () => {};

  type Hit = {
    /** What the row is keyed by. Not always an MBID: an iTunes row is keyed by
     *  Apple's collection id, because there is no MusicBrainz group for it. */
    id: string;
    mbid: string;
    /** `musicbrainz` or `itunes`. */
    source: string;
    title: string;
    types: string;
    year: string;
    score: number;
    art: string;
    thumb: string;
    front: boolean;
    chosen: boolean;
    /** Why there is no art here, when that is not simply "nobody scanned it". */
    note: string;
  };

  let query = '';
  let imageUrl = '';
  let hits: Hit[] = [];
  let busy = false;
  let error = '';
  let note = '';
  /** True once a search has actually run, so the empty list can be explained. */
  let searched = false;
  /** The candidate whose art was just applied, to mark the row. */
  let appliedId = '';

  async function loadDefaultQuery() {
    try {
      query = await invoke<string>('cover_default_query', { artist, album });
    } catch {
      query = '';
    }
  }

  async function fire() {
    busy = true;
    error = '';
    note = '';
    appliedId = '';
    try {
      hits = await invoke<Hit[]>('cover_search_candidates', {
        artist,
        album,
        query: query.trim() ? query : null
      });
      searched = true;
      if (hits.length === 0) note = 'Neither MusicBrainz nor iTunes returned anything for that album.';
    } catch (failure) {
      error = String(failure);
      hits = [];
      searched = true;
    } finally {
      busy = false;
    }
  }

  /**
   * File a candidate from the list.
   *
   * `source: 'hit'` is what tells the host this art came from a row it offered
   * itself, so the art domain list is not applied to it: that list exists to
   * guard links a person types and art other people publish, and a list that
   * omitted the sources Napstr uses on purpose would break the feature it sits
   * beside.
   */
  async function use(hit: Hit) {
    busy = true;
    error = '';
    note = '';
    try {
      const result = await invoke<{ key: string; published: boolean; eventId: string; note: string }>(
        'cover_apply_pick',
        {
          pick: {
            artist,
            album,
            mbid: hit.mbid,
            art: hit.art,
            thumb: hit.thumb,
            title: hit.title,
            year: hit.year,
            source: 'hit'
          }
        }
      );
      appliedId = hit.id;
      note = result.note ? `${result.note} Key: ${result.key}` : `Saved under ${result.key}`;
      onApplied();
    } catch (failure) {
      error = String(failure);
    } finally {
      busy = false;
    }
  }

  /**
   * File art from a link a person pasted.
   *
   * This is art from an address nobody vouched for, so it is the case the art
   * domain list in the Covers tab applies to, and the host is what refuses it —
   * the window cannot be the only thing checking.
   */
  async function useUrl() {
    busy = true;
    error = '';
    note = '';
    appliedId = '';
    try {
      const result = await invoke<{ key: string; published: boolean; eventId: string; note: string }>(
        'cover_apply_pick',
        {
          pick: {
            artist,
            album,
            mbid: '',
            art: imageUrl.trim(),
            thumb: '',
            title: '',
            year: '',
            source: 'url'
          }
        }
      );
      appliedId = 'pasted';
      note = result.note ? `${result.note} Key: ${result.key}` : `Saved under ${result.key}`;
      onApplied();
    } catch (failure) {
      error = String(failure);
    } finally {
      busy = false;
    }
  }

  void loadDefaultQuery();
</script>

<div
  class="art-picker-backdrop"
  role="presentation"
  onclick={(event) => {
    // Only a click on the backdrop itself closes, which keeps the panel free of
    // a click handler it would otherwise need just to stop propagation.
    if (event.target === event.currentTarget) onClose();
  }}
>
  <div class="art-picker" role="dialog" aria-modal="true" aria-label="Find album art">
    <header class="art-picker-head">
      <div>
        <b>Find album art</b>
        <small>{artist || 'Unknown artist'} · {album || 'Unknown album'}</small>
      </div>
      <button class="classic-button" onclick={onClose}>Close</button>
    </header>

    <div class="art-picker-query">
      <label for="art-picker-query">MusicBrainz search</label>
      <input
        id="art-picker-query"
        bind:value={query}
        spellcheck="false"
        placeholder={'release:"album" AND artist:"artist"'}
        onkeydown={(event) => {
          if (event.key === 'Enter') {
            event.preventDefault();
            void fire();
          }
        }}
      />
      <button class="classic-button primary" onclick={() => void fire()} disabled={busy || !query.trim()}>
        {busy ? 'Asking…' : 'Fire'}
      </button>
    </div>

    <p class="art-picker-hint">
      Lucene syntax: <code>release:</code>, <code>artist:</code>, <code>AND</code>, <code>OR</code>.
      Swap in a different spelling, add <code>country:US</code>, or search for a release group by name —
      the automatic lookup could not guess that. iTunes is always asked as well, by album and artist
      name rather than by this query.
    </p>

    <!--
      The last resort, and the only path where art arrives from an address
      nobody vouched for. It is deliberately plain: paste a link, see the
      picture, decide. What that link is allowed to be is the host's business,
      and the list it checks is in the Covers tab.
    -->
    <div class="art-picker-query">
      <label for="art-picker-url">Paste a link</label>
      <input
        id="art-picker-url"
        bind:value={imageUrl}
        spellcheck="false"
        placeholder="https://example.org/cover.jpg"
        onkeydown={(event) => {
          if (event.key === 'Enter') {
            event.preventDefault();
            void useUrl();
          }
        }}
      />
      <button
        class="classic-button"
        onclick={() => void useUrl()}
        disabled={busy || !imageUrl.trim().startsWith('https://')}
      >Use this URL</button>
    </div>
    {#if imageUrl.trim().startsWith('https://')}
      <div class="art-picker-preview">
        <img src={imageUrl.trim()} alt="" />
        <small>What that link holds, if the host will serve it.</small>
      </div>
    {/if}

    {#if currentArt}
      <p class="art-picker-hint">The art shown now is the small square on the left of each row below; pick the one that matches.</p>
    {/if}

    {#if error}<div class="art-picker-error">{error}</div>{/if}
    {#if note}<div class="art-picker-note">{note}</div>{/if}

    <div class="art-picker-results">
      {#each hits as hit (hit.id)}
        <div class:applied={appliedId === hit.id} class="art-picker-hit">
          <span class="art-picker-art">
            {#if hit.thumb || hit.art}
              <img src={hit.thumb || hit.art} alt="" loading="lazy" decoding="async" />
            {:else}
              <span class="art-picker-empty" aria-hidden="true">♪</span>
            {/if}
          </span>
          <div class="art-picker-meta">
            <b>{hit.title}</b>
            <small>
              {hit.source === 'itunes' ? 'iTunes' : 'MusicBrainz'}{hit.types ? ` · ${hit.types}` : ''}{hit.year ? ` · ${hit.year}` : ''}{hit.score ? ` · score ${hit.score}` : ''}
              {hit.front ? '' : ' · no front image'}
            </small>
            {#if hit.note}
              <!-- The record the source knows about, with the reason no picture
                   could be fetched for it. Dropping this row is what used to
                   make the album look absent from the search. -->
              <small class="art-picker-why">{hit.note}</small>
            {/if}
            <!-- The identifier, where there is one worth showing: a release-group
                 MBID for a MusicBrainz row, Apple's collection for a candidate
                 whose only name is that. A row that is merely "the second
                 source could not be reached" has neither. -->
            {#if hit.mbid || hit.art}
              <code title={hit.mbid ? 'MusicBrainz release group' : 'Apple collection, which is not an MBID'}>{hit.id}</code>
            {/if}
          </div>
          <div class="art-picker-actions">
            {#if hit.chosen}<span class="art-picker-chosen" title="What the automatic lookup would pick">automatic</span>{/if}
            <button
              class="classic-button primary"
              disabled={busy || !hit.art}
              onclick={() => void use(hit)}
            >Use this</button>
          </div>
        </div>
      {/each}
      {#if searched && hits.length === 0 && !error}
        <p class="empty-state compact">Nothing matched. Try loosening the search — a bare album title usually finds it.</p>
      {/if}
      {#if !searched}
        <p class="empty-state compact">Press Fire to ask MusicBrainz what it has for this album.</p>
      {/if}
    </div>
  </div>
</div>

<style>
  .art-picker-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(4, 6, 10, 0.72);
    display: grid;
    place-items: center;
    z-index: 200;
    padding: 24px;
  }
  .art-picker {
    width: min(860px, 100%);
    max-height: min(760px, 88vh);
    overflow: auto;
    background: #10141c;
    border: 1px solid #2a3446;
    border-radius: 10px;
    padding: 14px 16px 18px;
    color: #d7deea;
    font-size: 13px;
  }
  .art-picker-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 10px;
  }
  .art-picker-head b {
    display: block;
    font-size: 15px;
  }
  .art-picker-head small {
    color: #8d9ab0;
  }
  .art-picker-query {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: 10px;
    margin-top: 10px;
  }
  .art-picker-query label {
    color: #8d9ab0;
  }
  .art-picker-query input {
    background: #0a0e14;
    border: 1px solid #2a3446;
    border-radius: 6px;
    color: #e6ebf3;
    padding: 7px 9px;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12px;
  }
  .art-picker-hint {
    color: #8d9ab0;
    font-size: 12px;
    margin: 8px 0 0;
  }
  .art-picker-hint code,
  .art-picker-meta code {
    color: #9fb3d1;
    font-size: 11px;
  }
  .art-picker-error,
  .art-picker-note {
    margin-top: 10px;
    padding: 8px 10px;
    border-radius: 6px;
    font-size: 12px;
  }
  .art-picker-error {
    background: rgba(190, 60, 60, 0.16);
    border: 1px solid rgba(190, 60, 60, 0.5);
    color: #f0b6b6;
  }
  .art-picker-note {
    background: rgba(60, 130, 90, 0.14);
    border: 1px solid rgba(60, 130, 90, 0.45);
    color: #b6e0c6;
  }
  /* The pasted link, shown before it is filed: a person pasting a URL is
     judging a picture, and judging it from a filename is not judging it. */
  .art-picker-preview {
    margin-top: 10px;
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .art-picker-preview img {
    width: 96px;
    height: 96px;
    object-fit: cover;
    border-radius: 6px;
    border: 1px solid #222b3a;
    background: #06090d;
  }
  .art-picker-preview small {
    color: #8d9ab0;
    font-size: 12px;
  }
  .art-picker-results {
    margin-top: 12px;
    display: grid;
    gap: 8px;
  }
  .art-picker-hit {
    display: grid;
    grid-template-columns: 64px 1fr auto;
    align-items: center;
    gap: 12px;
    padding: 8px;
    border: 1px solid #222b3a;
    border-radius: 8px;
    background: #0c1017;
  }
  .art-picker-hit.applied {
    border-color: #3c825a;
  }
  .art-picker-art {
    width: 64px;
    height: 64px;
    display: grid;
    place-items: center;
    background: #06090d;
    border-radius: 6px;
    overflow: hidden;
  }
  .art-picker-art img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .art-picker-empty {
    color: #46536a;
    font-size: 22px;
  }
  .art-picker-meta {
    min-width: 0;
  }
  .art-picker-meta b {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .art-picker-meta small {
    display: block;
    color: #8d9ab0;
    margin: 2px 0;
  }
  /* Amber, not the green of the panel's own notice: this is not good news, it
     is the reason a row that is obviously the right record has no picture. */
  .art-picker-meta small.art-picker-why {
    color: #e0c07a;
  }
  .art-picker-meta code {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .art-picker-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .art-picker-chosen {
    font-size: 11px;
    color: #9fb3d1;
    border: 1px solid #2a3446;
    border-radius: 999px;
    padding: 2px 8px;
  }
</style>
