// Framework-independent locale selection and plain-text message formatting.
export const languages = [
  { code: 'en', name: 'English' },
  { code: 'zh', name: '中文（简体）' },
  { code: 'hi', name: 'हिन्दी' },
  { code: 'es', name: 'Español' },
  { code: 'ar', name: 'العربية' },
  { code: 'fr', name: 'Français' },
  { code: 'bn', name: 'বাংলা' },
  { code: 'pt', name: 'Português' },
  { code: 'id', name: 'Bahasa Indonesia' },
  { code: 'ur', name: 'اردو' }
];
export const storageKey = 'napstr-language';
export const untranslatedStatuses = ['Verified · Complete', 'Downloading'];

/** @param {string} key @returns {string | null} */
export function readStoredPreference(key) {
  try { return globalThis.localStorage?.getItem(key) ?? null; }
  catch { return null; }
}

/** @param {unknown} value */
export function matchLanguage(value) {
  if (typeof value !== 'string') return undefined;
  const code = value.trim().replace(/_/g, '-').split(/[.@]/)[0].split('-')[0].toLowerCase();
  return languages.find((language) => language.code === code)?.code;
}

/** @param {unknown} preference @param {readonly string[]} [preferred] */
export function resolveLanguage(preference, preferred = []) {
  if (preference !== 'auto' && languages.some(({ code }) => code === preference)) return /** @type {string} */ (preference);
  for (const candidate of preferred) {
    const match = matchLanguage(candidate);
    if (match) return match;
  }
  return 'en';
}

/** @param {string} locale */
export function direction(locale) { return locale === 'ar' || locale === 'ur' ? 'rtl' : 'ltr'; }

/** @typedef {{key: string, values: Record<string, unknown>}} Message */
/** Store messages, not translated text, so pending notices also change language. */
/** @param {string} key @param {Record<string, unknown>} [values] @returns {Message} */
export function message(key, values = {}) { return { key, values }; }

/** @param {Record<string, Record<string, string>>} catalogs @param {string} locale
 * @param {string | Message | null | undefined} input @param {Record<string, unknown>} [values] @returns {string} */
export function translate(catalogs, locale, input, values = {}) {
  if (!input) return '';
  const key = typeof input === 'string' ? input : input.key;
  if (typeof input !== 'string') values = input.values;
  if (untranslatedStatuses.includes(key)) return key;
  const lookup = (/** @type {Record<string, string> | undefined} */ catalog) =>
    catalog && Object.prototype.hasOwnProperty.call(catalog, key) && typeof catalog[key] === 'string' ? catalog[key] : undefined;
  const text = lookup(catalogs[locale]) ?? lookup(catalogs.en) ?? key;
  return text.replace(/\{(\w+)\}/g, (placeholder, name) => {
    if (!Object.prototype.hasOwnProperty.call(values, name)) return placeholder;
    const value = values[name];
    if (typeof value === 'number') return new Intl.NumberFormat(locale).format(value);
    if (value && typeof value === 'object' && 'key' in value) return translate(catalogs, locale, /** @type {Message} */ (value));
    return String(value ?? '');
  });
}
