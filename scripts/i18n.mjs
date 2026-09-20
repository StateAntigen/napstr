import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { parse } from 'svelte/compiler';
import ts from 'typescript';
import { websiteMessages } from '../website/build.mjs';
import { languages, untranslatedStatuses } from '../shared/i18n/core.js';

const root = new URL('../', import.meta.url);
const dynamicKeys = [
  'Downloads', 'Shared', 'Settings', 'Trollbox', 'Napstrfy', 'All audio', 'All audiobooks', 'local catalogue',
  'Local', 'Tor connected', 'Tor failed', 'Tor connecting', 'Tor unavailable', 'Ready', 'Waiting',
  'Rock', 'Soundtrack', 'Punk', 'Folk', 'Upbeat', 'Comedy', 'News', 'True Crime', 'Society & Culture',
  'Technology', 'History', 'Business', 'Science', 'Arts', 'Sports', 'Education', 'Music',
  'Play all', 'Play random', 'Repeat track', 'Play once'
];

export async function requiredMessages() {
  const keys = await websiteMessages();
  dynamicKeys.forEach((key) => keys.add(key));
  function walk(node) {
    if (!node || typeof node !== 'object') return;
    if (node.type === 'CallExpression' && ['$t', 'msg'].includes(node.callee?.name)) {
      const add = (expression) => {
        if (typeof expression?.value === 'string') keys.add(expression.value);
        if (expression?.type === 'ConditionalExpression') { add(expression.consequent); add(expression.alternate); }
      };
      add(node.arguments[0]);
    }
    for (const [key, value] of Object.entries(node)) {
      if (['metadata', 'loc', 'name_loc'].includes(key)) continue;
      if (Array.isArray(value)) value.forEach(walk);
      else if (value && typeof value === 'object') walk(value);
    }
  }
  for (const file of ['src/routes/+page.svelte', 'android/src/App.svelte', 'shared/i18n/LanguageSelect.svelte']) {
    walk(parse(await readFile(new URL(file, root), 'utf8'), { modern: true }));
  }
  const source = ts.createSourceFile('releases.js', await readFile(new URL('website/releases.js', root), 'utf8'), ts.ScriptTarget.Latest, true);
  const visit = (node) => {
    if (ts.isCallExpression(node) && node.expression.getText(source) === 't' && ts.isStringLiteral(node.arguments[0])) keys.add(node.arguments[0].text);
    ts.forEachChild(node, visit);
  };
  visit(source);
  return [...keys].filter((key) => !untranslatedStatuses.includes(key)).sort();
}

export function placeholders(value) { return [...value.matchAll(/\{(\w+)\}/g)].map((match) => match[1]).sort(); }

export async function untranslatedCopy() {
  const issues = [];
  const allowed = /^(Napstr|Napstrfy|napstrfy|Nostr|Tor|i|#napstr-trollbox|https:\/\/…|napstrfy:\/\/pair\/…|bc1qwgms685z3j69qtgalyjtrfuqg5f6pt302z0k60)$/;
  for (const file of ['src/routes/+page.svelte', 'android/src/App.svelte']) {
    const ast = parse(await readFile(new URL(file, root), 'utf8'), { modern: true });
    function walk(node) {
      if (!node || typeof node !== 'object') return;
      if (node.type === 'Text' && /[A-Za-z]/.test(node.data) && !allowed.test(node.data.trim())) issues.push(`${file}: ${node.data.trim()}`);
      if (node.type === 'Attribute') {
        if (['aria-label', 'title', 'placeholder', 'alt'].includes(node.name) && Array.isArray(node.value)) node.value.forEach(walk);
        return;
      }
      for (const [key, value] of Object.entries(node)) {
        if (['expression', 'metadata', 'loc', 'name_loc'].includes(key)) continue;
        if (Array.isArray(value)) value.forEach(walk);
        else if (value && typeof value === 'object') walk(value);
      }
    }
    walk(ast.fragment);
  }
  return issues;
}

export async function validateCatalogs() {
  const keys = await requiredMessages();
  const issues = await untranslatedCopy();
  for (const { code } of languages) {
    const catalog = JSON.parse(await readFile(new URL(`shared/i18n/locales/${code}.json`, root), 'utf8'));
    for (const key of keys) {
      if (typeof catalog[key] !== 'string' || !catalog[key].trim()) { issues.push(`${code}: missing ${key}`); continue; }
      if (JSON.stringify(placeholders(key)) !== JSON.stringify(placeholders(catalog[key]))) issues.push(`${code}: placeholders differ in ${key}`);
      if (/[<>]/.test(catalog[key])) issues.push(`${code}: unexpected HTML in ${key}`);
    }
    for (const key of Object.keys(catalog)) if (!keys.includes(key)) issues.push(`${code}: unused ${key}`);
  }
  return issues;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  if (process.argv.includes('--extract')) {
    const keys = await requiredMessages();
    await writeFile(new URL('shared/i18n/locales/en.json', root), JSON.stringify(Object.fromEntries(keys.map((key) => [key, key])), null, 2) + '\n');
    console.log(`Extracted ${keys.length} shared messages.`);
  } else {
    const issues = await validateCatalogs();
    if (issues.length) { console.error(issues.join('\n')); process.exitCode = 1; }
    else console.log('All ten catalogues have complete keys and matching placeholders.');
  }
}
