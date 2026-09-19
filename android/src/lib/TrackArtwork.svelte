<script lang="ts">
  import { artworkHue, coverFor, coverRevision } from './artwork';
  import type { RemoteTrack } from './types';

  let { track, lookup = false, large = false }: { track: RemoteTrack; lookup?: boolean; large?: boolean } = $props();
  let image = $state('');
  let failed = $state(false);
  let hue = $derived(artworkHue(track.fileId));

  $effect(() => {
    // Reading the revision subscribes this tile to it: when the host says its
    // art has changed, an album it had nothing for a moment ago is asked about
    // again, so art appears here as the desktop finds it.
    void $coverRevision;
    let alive = true;
    image = '';
    failed = false;
    if (lookup) {
      void coverFor(track).then((cover) => {
        if (!alive || !cover) return;
        // Dense grids prefer the published thumbnail; a large tile wants the
        // full front cover, falling back to whichever the publisher gave us.
        image = large ? cover.art || cover.thumb : cover.thumb || cover.art;
      });
    }
    return () => { alive = false; };
  });
</script>

<div class:large class="artwork" style={`--cover-hue:${hue}`}>
  {#if image && !failed}<img src={image} alt="" onerror={() => (failed = true)} />{:else}<img class="fallback" src="/napstr-logo-small.png" alt="" />{/if}
</div>
