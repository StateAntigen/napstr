<script lang="ts">
  import { onMount } from 'svelte';
  import { clearCoverCache, debugCoverState } from './artwork';
  import { clearCoverEvents, coverEvents, subscribeCoverEvents, type CoverEvent } from './coverDebug';
  import type { CompanionStatus, RemoteTrack } from './types';

  let { tracks = [], status }: { tracks?: RemoteTrack[]; status: CompanionStatus } = $props();

  let open = $state(false);
  let events = $state<CoverEvent[]>(coverEvents());
  let rows = $state<ReturnType<typeof debugCoverState>>([]);

  onMount(() => subscribeCoverEvents(() => { events = [...coverEvents()]; }));

  $effect(() => {
    // Re-probe whenever the panel opens or the list changes.
    if (open) rows = debugCoverState(tracks);
    return () => {};
  });

  const counts = $derived({
    requests: events.filter((event) => event.kind === 'request').length,
    answers: events.filter((event) => event.kind === 'answer').length,
    errors: events.filter((event) => event.kind === 'error').length,
    skips: events.filter((event) => event.kind === 'skip').length,
    cached: events.filter((event) => event.kind === 'cached').length
  });

  function stamp(at: number) {
    return new Date(at).toLocaleTimeString();
  }

  function retry() {
    clearCoverCache();
    clearCoverEvents();
    window.location.reload();
  }
</script>

<button class="cover-debug-pill" class:active={open} onclick={() => (open = !open)} aria-label="Cover art debug">
  ☰ art
</button>

{#if open}
  <section class="cover-debug" aria-label="Cover art debug">
    <header>
      <strong>Cover art debug</strong>
      <button onclick={() => (open = false)} aria-label="Close debug panel">✕</button>
    </header>

    <p class="cover-debug-status">
      {status.paired ? status.desktopName || 'paired' : 'not paired'} ·
      {status.connected ? 'connected' : 'offline'} · {status.streamOnly ? 'read-only' : 'full access'}
    </p>

    <p class="cover-debug-counts">
      {counts.requests} requests · {counts.answers} answers · {counts.errors} errors ·
      {counts.cached} cached · {counts.skips} no-key
    </p>

    <div class="cover-debug-actions">
      <button onclick={retry}>Clear cache & reload</button>
      <button onclick={() => (rows = debugCoverState(tracks))}>Refresh list</button>
      <button onclick={() => clearCoverEvents()}>Clear log</button>
    </div>

    <h4>Tracks ({rows.length})</h4>
    <ul class="cover-debug-rows">
      {#each rows as row (row.fileId)}
        <li class:resolved={row.state === 'resolved'}>
          <b>{row.state}</b>
          <span>{row.key || '—'}</span>
          <em>{row.title}</em>
        </li>
      {/each}
    </ul>

    <h4>Events ({events.length})</h4>
    <ul class="cover-debug-events">
      {#each events as event (event.id)}
        <li class={event.kind}>
          <b>{stamp(event.at)} {event.kind}</b>
          <span>{event.detail}</span>
        </li>
      {/each}
    </ul>
  </section>
{/if}
