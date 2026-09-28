/**
 * Whether the connection this phone is spending is metered.
 *
 * The phone is the only side that knows this, so it comes from the Android
 * bridge rather than from the host, and it covers what looking at the connection
 * type alone would miss: a Wi-Fi hotspot from another phone is Wi-Fi and is still
 * somebody's data plan.
 *
 * A build with no bridge at all - the desktop window, a browser - has nothing to
 * spend and answers unmetered. A bridge that is there but cannot answer is
 * treated as metered, which is the same choice `NetworkBridge.kt` makes and for
 * the same reason: guessing wrong about a bill is worse than holding a track
 * back for a moment.
 */

type NetworkBridge = {
  metered?: () => boolean;
  kind?: () => string;
};

export type ConnectionKind = 'metered' | 'unmetered' | 'offline' | 'unknown';

/** The event the bridge dispatches when the connection changes. */
const NETWORK_EVENT = 'napstrfy-network';

function bridge(): NetworkBridge | undefined {
  const candidate = (window as unknown as { NapstrfyNetwork?: NetworkBridge }).NapstrfyNetwork;
  return candidate && typeof candidate.metered === 'function' ? candidate : undefined;
}

export function meteredNow(): boolean {
  const phone = bridge();
  if (!phone) return false;
  try {
    return phone.metered?.() === true;
  } catch {
    return true;
  }
}

export function connectionKind(): ConnectionKind {
  try {
    const kind = bridge()?.kind?.();
    if (kind === 'metered' || kind === 'unmetered' || kind === 'offline') return kind;
    return 'unknown';
  } catch {
    return 'unknown';
  }
}

/** Calls back when the phone switches connection, and stops when told to. */
export function watchNetwork(changed: () => void): () => void {
  window.addEventListener(NETWORK_EVENT, changed);
  return () => window.removeEventListener(NETWORK_EVENT, changed);
}
