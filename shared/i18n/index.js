import en from './locales/en.json';
import zh from './locales/zh.json';
import hi from './locales/hi.json';
import es from './locales/es.json';
import ar from './locales/ar.json';
import fr from './locales/fr.json';
import bn from './locales/bn.json';
import pt from './locales/pt.json';
import id from './locales/id.json';
import ur from './locales/ur.json';
import { translate } from './core.js';
export * from './core.js';
export const catalogs = { en, zh, hi, es, ar, fr, bn, pt, id, ur };
/** @param {string} locale @param {Parameters<typeof translate>[2]} input @param {Record<string, unknown>} [values] */
export function translateMessage(locale, input, values) { return translate(catalogs, locale, input, values); }
