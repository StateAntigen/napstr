import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { languages, matchLanguage, resolveLanguage, direction, message, translate, untranslatedStatuses } from '../shared/i18n/core.js';
import { validateCatalogs } from '../scripts/i18n.mjs';

test('language matching respects saved choices, ordered preferences, region tags and fallback', () => {
  assert.equal(languages.length, 10);
  assert.equal(resolveLanguage('es', ['ar-SA']), 'es');
  assert.equal(resolveLanguage('auto', ['de-DE', 'fr-CA', 'en']), 'fr');
  assert.equal(resolveLanguage('invalid', ['pt_BR.UTF-8']), 'pt');
  assert.equal(resolveLanguage('auto', ['zz']), 'en');
  assert.equal(resolveLanguage('auto', []), 'en');
  assert.equal(matchLanguage('zh-Hans-CN'), 'zh');
  assert.equal(matchLanguage(null), undefined);
  assert.equal(matchLanguage(''), undefined);
  assert.equal(direction('ar'), 'rtl');
  assert.equal(direction('ur'), 'rtl');
  assert.equal(direction('hi'), 'ltr');
});

test('messages preserve variables, localize numbers and fall back without translating user content', () => {
  const catalogs = { en: { 'Hello {name}': 'Hello {name}', 'Tracks: {count}': 'Tracks: {count}' }, es: { 'Hello {name}': 'Hola {name}' } };
  assert.equal(translate(catalogs, 'es', message('Hello {name}', { name: 'Search <script>' })), 'Hola Search <script>');
  assert.equal(translate(catalogs, 'ar', 'Tracks: {count}', { count: 1234 }), `Tracks: ${new Intl.NumberFormat('ar').format(1234)}`);
  assert.equal(translate(catalogs, 'es', 'Unknown diagnostic'), 'Unknown diagnostic');
  for (const key of ['constructor', '__proto__', 'toString']) assert.equal(translate(catalogs, 'es', key), key);
  assert.equal(translate(catalogs, 'es', 'Hello {name}'), 'Hola {name}');
  assert.equal(translate(catalogs, 'es', ''), '');
  assert.equal(translate(catalogs, 'es', message('Hello {name}', { name: message('Hello {name}', { name: 'Ana' }) })), 'Hola Hola Ana');
  for (const status of untranslatedStatuses) assert.equal(translate({ es: { [status]: 'Wrong' } }, 'es', status), status);
});

test('every visible message has all ten translations with identical placeholders', async () => {
  assert.deepEqual(await validateCatalogs(), []);
  for (const { code } of languages.filter(({ code }) => code !== 'en')) {
    const catalog = JSON.parse(await readFile(new URL(`../shared/i18n/locales/${code}.json`, import.meta.url)));
    for (const key of ['Language', 'Settings', 'Search', 'Play']) {
      assert.notEqual(catalog[key], key, `${code}: ${key} must be translated`);
    }
  }
});
