import { readFile, writeFile, mkdir, cp, rm } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { parseFragment, serialize } from 'parse5';
import { languages, direction, translate } from '../shared/i18n/core.js';
import { pages, template, escapeHtml, pageUrl, siteOrigin } from './template.mjs';

export const websiteRoot = fileURLToPath(new URL('.', import.meta.url));
const untranslated = /^(Napstr|Napstrfy|NAPSTR|Nostr|Tor|Iroh|Windows|Linux|macOS|Android|iOS|Apple Silicon|Intel|SHA-256|NIP-\d+|https?:.*|NAPSTR_TOR_PATH|tor|\d.*)$/;
export function isCopy(value) { return /[A-Za-z]/.test(value) && !untranslated.test(value); }

export function renderContent(source, t, language) {
  const tree = parseFragment(source);
  function visit(node, skip = false) {
    skip ||= ['code', 'pre', 'script', 'style'].includes(node.tagName);
    if (!skip && node.nodeName === '#text') {
      const key = node.value.trim().replace(/\s+/g, ' ');
      if (isCopy(key)) node.value = node.value.replace(/\S[\s\S]*\S|\S/, () => t(key));
    }
    for (const attribute of node.attrs || []) {
      if (['alt', 'title', 'aria-label'].includes(attribute.name) && isCopy(attribute.value)) attribute.value = t(attribute.value);
      if (language !== 'en' && ['src', 'href'].includes(attribute.name) && /^(assets|downloads)\//.test(attribute.value)) attribute.value = '../' + attribute.value;
    }
    for (const child of node.childNodes || []) visit(child, skip);
  }
  visit(tree);
  return serialize(tree);
}

export async function websiteMessages() {
  const messages = new Set();
  const t = (value) => { messages.add(value); return value; };
  for (const slug of Object.keys(pages)) {
    const content = renderContent(await readFile(`${websiteRoot}content/${slug}.html`, 'utf8'), t, 'en');
    template({ slug, language: 'en', languages, direction: 'ltr', content, t, catalog: {} });
  }
  return messages;
}

export async function buildWebsite(output = `${websiteRoot}dist`) {
  const catalogs = Object.fromEntries(await Promise.all(languages.map(async ({ code }) =>
    [code, JSON.parse(await readFile(new URL(`../shared/i18n/locales/${code}.json`, import.meta.url), 'utf8'))])));
  await mkdir(output, { recursive: true });
  for (const { code } of languages) {
    const directory = code === 'en' ? output : `${output}/${code}`;
    await mkdir(directory, { recursive: true });
    const t = (key) => translate(catalogs, code, key);
    for (const slug of Object.keys(pages)) {
      const content = renderContent(await readFile(`${websiteRoot}content/${slug}.html`, 'utf8'), t, code);
      await writeFile(`${directory}/${slug}.html`, template({ slug, language: code, languages, direction: direction(code), content, t: (key) => escapeHtml(t(key)), catalog: catalogs[code] }));
    }
  }
  await cp(`${websiteRoot}assets`, `${output}/assets`, { recursive: true });
  for (const file of ['styles.css', 'site.js', 'releases.js']) await cp(`${websiteRoot}${file}`, `${output}/${file}`);
  await mkdir(`${output}/i18n`, { recursive: true });
  for (const file of ['core.js', 'styles.css']) await cp(new URL(`../shared/i18n/${file}`, import.meta.url), `${output}/i18n/${file}`);
  await cp(new URL('../shared/i18n/fonts', import.meta.url), `${output}/i18n/fonts`, { recursive: true });
  // Preserve optional manually hosted downloads and the custom domain configuration.
  for (const file of ['CNAME', 'downloads']) {
    try { await cp(`${websiteRoot}${file}`, `${output}/${file}`, { recursive: true }); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  await writeFile(`${output}/.nojekyll`, '');
  const urls = languages.flatMap(({ code }) => Object.keys(pages).map((slug) =>
    `  <url><loc>${escapeHtml(pageUrl(slug, code))}</loc></url>`));
  await writeFile(`${output}/sitemap.xml`, `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${urls.join('\n')}\n</urlset>\n`);
  await writeFile(`${output}/robots.txt`, `User-agent: *\nAllow: /\n\nSitemap: ${siteOrigin}/sitemap.xml\n`);
  return output;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  await rm(`${websiteRoot}dist`, { recursive: true, force: true });
  console.log(`Website built: ${await buildWebsite()}`);
}
