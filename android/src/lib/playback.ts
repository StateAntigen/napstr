// Match the Android media bridge's limit; never publish huge native time values.
export const MAX_MEDIA_SECONDS = 14 * 24 * 60 * 60;

export function validDuration(value: number) {
  return Number.isFinite(value) && value > 0 && value <= MAX_MEDIA_SECONDS ? value : 0;
}

export function safePosition(value: number, duration = MAX_MEDIA_SECONDS) {
  return Number.isFinite(value) ? Math.max(0, Math.min(value, duration)) : 0;
}

/** Coalesce events, including urgent ones, without synchronous native callbacks. */
export function rateLimitedTask(run: () => void, interval = 1000) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let lastRun = -Infinity;
  return {
    request() {
      if (timer !== undefined) return;
      timer = setTimeout(() => {
        timer = undefined;
        lastRun = Date.now();
        run();
      }, Math.max(0, interval - (Date.now() - lastRun)));
    },
    cancel() {
      clearTimeout(timer);
      timer = undefined;
    }
  };
}

/** Only read duration, never seekable: that getter can emit durationchange in WebKit. */
export function durationMonitor(changed: (duration: number) => void) {
  let read: (() => number) | undefined;
  let retry: ReturnType<typeof setTimeout> | undefined;
  let reads = 0;
  let automaticReads = 0;
  const task = rateLimitedTask(() => {
    if (!read || reads >= 32) return;
    reads++;
    let duration = 0;
    try { duration = validDuration(read()); } catch { /* Keep playback usable. */ }
    changed(duration);
    clearTimeout(retry);
    // Eight startup checks recover metadata that arrives without an event.
    // Later metadata events can request a check, up to a firm per-source limit.
    if (!duration && ++automaticReads < 8 && reads < 32) {
      retry = setTimeout(() => task.request(), 1000);
    }
  });
  function stop() {
    task.cancel();
    clearTimeout(retry);
    read = undefined;
  }
  return {
    start(reader: () => number) {
      stop();
      reads = 0;
      automaticReads = 0;
      read = reader;
      task.request();
    },
    request() { if (read && reads < 32) task.request(); },
    stop
  };
}
