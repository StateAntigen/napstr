<script lang="ts">
  /**
   * Where a track actually is: in this phone's audio cache, on the paired
   * Napstr computer, or only out on the network among its seeders.
   *
   * `local` on the wire means the host holds the file, so it is the middle
   * state; the phone's own cache has to be asked about separately.
   */
  import type { RemoteTrack } from './types';
  import { hostHue } from './hosts';

  /**
   * A grain of seed: pointed at both ends, no round end anywhere. What a seeder
   * is, in one shape, and wide enough once laid flat for a count to sit inside it.
   */
  const GRAIN = 'M12 2.8Q18.2 7.6 18.2 12.2 18.2 16.8 12 21.2 5.8 16.8 5.8 12.2 5.8 7.6 12 2.8Z';

  let { track, cached = false, pending = false, host = '', hostName = '' }: {
    track: RemoteTrack;
    cached?: boolean;
    pending?: boolean;
    /** The computer that answered with this row, when it was not this phone's own. */
    host?: string;
    /** What that computer is called, for the label. */
    hostName?: string;
  } = $props();
</script>

{#if pending}
  <span class="track-badge pending" role="img" aria-label="Downloading from Napstr">
    <i></i>
  </span>
{:else if cached}
  <span class="track-badge phone" role="img" aria-label="Cached on this phone">
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <rect x="6.6" y="2.4" width="10.8" height="19.2" rx="2.8" />
      <path d="M12 7.4v5.4" /><path d="M9.8 10.9 12 13.1l2.2-2.2" />
    </svg>
  </span>
{:else if track.local}
  <span
    class="track-badge computer"
    class:elsewhere={Boolean(host)}
    style={`--host-hue:${host ? hostHue(host) : 0}`}
    role="img"
    aria-label={host ? `Stored on ${hostName || 'another computer'}` : 'Stored on your Napstr computer'}
  >
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <rect x="2.8" y="4.2" width="18.4" height="12.4" rx="2.2" />
      <path d="M12 16.6V20.4" /><path d="M8.6 20.4h6.8" />
    </svg>
  </span>
{:else}
  <span
    class="track-badge network"
    class:nothing={track.sources.length === 0}
    role="img"
    aria-label={`${track.sources.length} ${track.sources.length === 1 ? 'seeder' : 'seeders'} on the network`}
  >
    <!-- The count is inside the grain rather than beside it, and the grain is laid
         flat so the digits have its length to sit in. -->
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <g transform="rotate(90 12 12)">
        <path class="frame" d={GRAIN} />
        {#if track.sources.length > 0}<path class="fill" d={GRAIN} />{/if}
      </g>
    </svg>
    {#if track.sources.length > 0}<b>{track.sources.length}</b>{/if}
  </span>
{/if}
