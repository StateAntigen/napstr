import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { parse } from 'parse5';
import { buildWebsite } from '../website/build.mjs';
import { pages, pageUrl } from '../website/template.mjs';
import { languages, direction } from '../shared/i18n/core.js';

test('all generated pages keep legacy filenames, valid links, anchors and localized metadata', async () => {
  const output = await mkdtemp(join(tmpdir(), 'napstr-site-test-'));
  try {
    await buildWebsite(output);
    const sitemap = await readFile(join(output, 'sitemap.xml'), 'utf8');
    const locations = [...sitemap.matchAll(/<loc>([^<]+)<\/loc>/g)].map((match) => match[1]);
    assert.equal(locations.length, 50);
    assert.equal(new Set(locations).size, 50);
    assert.match(await readFile(join(output, 'robots.txt'), 'utf8'), /Sitemap: https:\/\/napstr.net\/sitemap.xml/);
    for (const product of ['napstr', 'napstrfy']) {
      const png = await readFile(join(output, `assets/${product}-share.png`));
      assert.equal(png.subarray(1, 4).toString(), 'PNG');
      assert.equal(png.readUInt32BE(16), 1200);
      assert.equal(png.readUInt32BE(20), 630);
    }
    const fonts = await readFile(join(output, 'i18n/fonts/fonts.css'), 'utf8');
    for (const [, font] of fonts.matchAll(/url\(\.\/([^\)]+)\)/g)) await access(join(output, 'i18n/fonts', font));
    for (const { code } of languages) for (const slug of Object.keys(pages)) {
      const directory = code === 'en' ? output : join(output, code);
      const html = await readFile(join(directory, `${slug}.html`), 'utf8');
      assert.match(html, new RegExp(`<html lang="${code}" dir="${direction(code)}">`));
      assert.match(html, /<details class="language-links">/);
      assert.doesNotMatch(html, /data-language-select/);
      assert.equal((html.match(/hreflang=/g) || []).length, 11);
      const canonical = pageUrl(slug, code);
      assert.ok(locations.includes(canonical));
      assert.ok(html.includes(`<link rel="canonical" href="${canonical}">`));
      assert.ok(html.includes(`<link rel="alternate" hreflang="${code}" href="${canonical}">`));
      assert.ok(html.includes(`<link rel="alternate" hreflang="x-default" href="${pageUrl(slug, 'en')}">`));
      const image = `https://napstr.net/assets/${slug === 'napstrfy' ? 'napstrfy' : 'napstr'}-share.png`;
      assert.ok(html.includes(`<meta property="og:image" content="${image}">`));
      assert.ok(html.includes(`<meta name="twitter:image" content="${image}">`));
      assert.ok(html.includes(`<meta property="og:url" content="${canonical}">`));
      for (const { code: alternate } of languages) {
        assert.ok(html.includes(`<link rel="alternate" hreflang="${alternate}" href="${pageUrl(slug, alternate)}">`));
      }
      if (slug === 'index') { assert.match(html, /id="about"/); assert.match(html, /id="how-it-works"/); }
      const links = [];
      function walk(node) {
        for (const attribute of node.attrs || []) if (['src', 'href'].includes(attribute.name)) links.push(attribute.value);
        for (const child of node.childNodes || []) walk(child);
      }
      walk(parse(html));
      for (const link of links) {
        if (!link || /^(https?:|#|mailto:|data:)/.test(link)) continue;
        await access(resolve(directory, link.split(/[?#]/)[0]));
      }
      for (const page of Object.keys(pages)) assert.match(html, new RegExp(`href="${page}.html"`));
    }
    assert.doesNotMatch(await readFile(join(output, 'es/index.html'), 'utf8'), /<h1>Napstr - own your music again<\/h1>/);
  } finally { await rm(output, { recursive: true, force: true }); }
});
