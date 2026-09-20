import { writable, derived } from 'svelte/store';
import { direction, languages, resolveLanguage, storageKey, translateMessage } from './index.js';
export { message } from './index.js';
/** @typedef {import('./core.js').Message} Message */

export const preference = writable('auto');
export const locale = writable('en');
export const t = derived(locale, (language) =>
  /** @param {string | Message | null | undefined} input @param {Record<string, unknown>} [values] */
  (input, values) => translateMessage(language, input, values));

/** Initialize only in the browser. The optional native reader supplies the OS language. */
/** @param {(() => Promise<string | null>) | undefined} [readNativeLocale] */
export function initializeLocale(readNativeLocale) {
  let alive = true;
  let nativeLanguage = '';
  let nativeRequest = 0;
  let selected = 'auto';
  let stored = '';
  try { selected = localStorage.getItem(storageKey) || 'auto'; } catch { /* Private browsing. */ }
  if (!languages.some(({ code }) => code === selected)) selected = 'auto';
  preference.set(selected);
  const apply = () => {
    const value = resolveLanguage(selected, [nativeLanguage, ...navigator.languages, navigator.language]);
    locale.set(value);
    document.documentElement.lang = value;
    document.documentElement.dir = direction(value);
  };
  const unsubscribe = preference.subscribe((value) => {
    selected = value;
    apply();
    // Avoid writing an unchanged value back across windows after a storage event.
    if (stored !== selected) {
      stored = selected;
      try { if (localStorage.getItem(storageKey) !== selected) localStorage.setItem(storageKey, selected); } catch { /* Session-only. */ }
    }
  });
  const refresh = () => {
    apply();
    const request = ++nativeRequest;
    if (readNativeLocale) void readNativeLocale().then((value) => {
      if (alive && request === nativeRequest) { nativeLanguage = value || ''; apply(); }
    }).catch(() => { /* Webview/browser preferences remain available. */ });
  };
  const storageChanged = (/** @type {StorageEvent} */ event) => {
    if (event.key === storageKey || event.key === null) {
      preference.set(languages.some(({ code }) => code === event.newValue) ? event.newValue || 'auto' : 'auto');
    }
  };
  refresh();
  window.addEventListener('languagechange', refresh);
  window.addEventListener('focus', refresh);
  window.addEventListener('storage', storageChanged);
  return () => {
    alive = false;
    unsubscribe();
    window.removeEventListener('languagechange', refresh);
    window.removeEventListener('focus', refresh);
    window.removeEventListener('storage', storageChanged);
  };
}
