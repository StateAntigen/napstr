<script lang="ts">
  import { artUrl, coverFor, drawsFromDisk, type AlbumCover } from '$lib/artwork';

  /**
   * One album's artwork, resolved by the host.
   *
   * Every instance asks in the same tick, which `$lib/artwork` turns into one
   * batched call for the whole page. A missing or unreachable image is a blank
   * tile with a music note, never an error: the NIP is explicit that image URLs
   * rot and that a broken one is no reason to penalise anybody.
   *
   * `revision` is bumped by the page after a cover scan, which is what makes a
   * tile that answered "nothing known" ask again.
   */
  export let artist = '';
  export let album = '';
  export let preferThumb = false;
  export let size: 'small' | 'medium' | 'large' = 'small';
  export let revision = 0;

  let cover: AlbumCover | null = null;
  let failed = false;
  /** The copy on this computer could not be read, so the address is drawn. */
  let bypassLocal = false;
  let generation = 0;

  $: identity = `${artist}\u0000${album}\u0000${revision}`;
  $: {
    const current = ++generation;
    cover = null;
    failed = false;
    bypassLocal = false;
    // Reading `identity` here is what subscribes this block to it.
    if (identity) {
      void coverFor(artist, album).then((found) => {
        if (current === generation) cover = found;
      });
    }
  }
  $: url = failed ? '' : artUrl(cover, preferThumb, !bypassLocal);

  /**
   * A picture held on this computer can be evicted while a page is open, and the
   * directory can be emptied under a running window. The publisher's address is
   * the first fallback, a blank tile with a note is the last.
   */
  function onImageFailed() {
    if (!bypassLocal && drawsFromDisk(cover, preferThumb)) bypassLocal = true;
    else failed = true;
  }
  // Art somebody signed and published reads differently from art only this
  // computer resolved, and the tooltip is the cheapest honest way to say so.
  $: origin = cover?.author
    ? `Cover ${cover.source ? `(${cover.source}) ` : ''}published to Napstr`
    : 'Art resolved on this computer; not published';
</script>

{#if url}
  <span class="cover-art {size}" title={origin}>
    <img
      src={url}
      alt=""
      loading="lazy"
      decoding="async"
      onerror={onImageFailed}
    />
  </span>
{:else}
  <span class="cover-art {size} empty" aria-hidden="true">♪</span>
{/if}
