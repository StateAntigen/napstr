export const pages = {
  index: { title: 'Home', description: 'Public music discovery over Nostr and private audio transfers over Tor.' },
  download: { title: 'Download', description: 'Download Napstr for Windows, Linux, and macOS.' },
  napstrfy: { title: 'Napstrfy', description: 'Listen to your Napstr library on another computer or phone.' },
  nostr: { title: 'Nostr', description: 'How Napstr uses Nostr for public discovery and encrypted download requests.' },
  tor: { title: 'Tor', description: 'How Napstr uses Tor for private, verified audio transfers.' }
};

export const siteOrigin = 'https://napstr.net';
export function pageUrl(slug, language) {
  return `${siteOrigin}/${language === 'en' ? '' : language + '/'}${slug === 'index' ? '' : slug + '.html'}`;
}
const socialLocales = { en: 'en_GB', zh: 'zh_CN', hi: 'hi_IN', es: 'es_ES', ar: 'ar_SA', fr: 'fr_FR', bn: 'bn_BD', pt: 'pt_BR', id: 'id_ID', ur: 'ur_PK' };

export function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, (character) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[character]);
}

// One shared shell; content fragments contain only the unique body of each page.
export function template({ slug, language, languages, direction, content, t, catalog }) {
  const base = language === 'en' ? './' : '../';
  const canonical = pageUrl(slug, language);
  const product = slug === 'napstrfy' ? 'Napstrfy' : 'Napstr';
  const socialImage = `${siteOrigin}/assets/${product.toLowerCase()}-share.png`;
  const nav = Object.entries(pages).map(([name, page]) => `<a href="${name}.html"${slug === name ? ' class="active" aria-current="page"' : ''}>${t(page.title)}</a>`).join('');
  return `<!doctype html>
<html lang="${language}" dir="${direction}">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="description" content="${t(pages[slug].description)}">
  <meta name="github-repository" content="lnbits/napstr">
  <link rel="canonical" href="${canonical}">
  ${languages.map(({ code }) => `<link rel="alternate" hreflang="${code}" href="${pageUrl(slug, code)}">`).join('\n  ')}
  <link rel="alternate" hreflang="x-default" href="${pageUrl(slug, 'en')}">
  <meta property="og:type" content="website">
  <meta property="og:site_name" content="Napstr">
  <meta property="og:title" content="Napstr — ${t(pages[slug].title)}">
  <meta property="og:description" content="${t(pages[slug].description)}">
  <meta property="og:url" content="${canonical}">
  <meta property="og:locale" content="${socialLocales[language]}">
  ${languages.filter(({ code }) => code !== language).map(({ code }) => `<meta property="og:locale:alternate" content="${socialLocales[code]}">`).join('\n  ')}
  <meta property="og:image" content="${socialImage}">
  <meta property="og:image:secure_url" content="${socialImage}">
  <meta property="og:image:type" content="image/png">
  <meta property="og:image:width" content="1200">
  <meta property="og:image:height" content="630">
  <meta property="og:image:alt" content="${product}">
  <meta name="twitter:card" content="summary_large_image">
  <meta name="twitter:title" content="Napstr — ${t(pages[slug].title)}">
  <meta name="twitter:description" content="${t(pages[slug].description)}">
  <meta name="twitter:image" content="${socialImage}">
  <meta name="twitter:image:alt" content="${product}">
  <title>Napstr — ${t(pages[slug].title)}</title>
  <link rel="icon" type="image/png" href="${base}assets/favicon.png">
  <link rel="stylesheet" href="${base}styles.css">
  <link rel="stylesheet" href="${base}i18n/styles.css">
  <script id="page-translations" type="application/json">${JSON.stringify(catalog).replace(/</g, '\\u003c')}</script>
  <script type="module" src="${base}site.js"></script>
</head>
<body data-page="${slug}" data-release-product="${slug === 'napstrfy' ? 'napstrfy' : 'napstr'}">
  <div class="site-shell">
    <header class="masthead">
      <a class="brand" href="index.html" aria-label="${t('Napstr home')}"><img src="${base}assets/napstr-logo.png" alt="Napstr"></a>
      <blockquote>${t('Owning something means you are free to share and pass it on.')}</blockquote>
    </header>
    <div class="page-grid">
      <aside class="sidebar">
        <nav aria-label="${t('Main navigation')}">${nav}</nav>
        <p class="language-suggestion" data-language-suggestion hidden>${t('Language')}: <a data-suggested-language></a></p>
        <details class="language-links">
          <summary>${t('Language')}</summary>
          ${languages.map(({ code, name }) => `<a href="${base}${code === 'en' ? '' : code + '/'}${slug}.html" lang="${code}" dir="${directionForLanguage(code)}"${code === language ? ' aria-current="page"' : ''}>${escapeHtml(name)}</a>`).join('\n          ')}
        </details>
        <div class="sidebar-meta"><span>${t('Community Build')}</span><span>${t('Privacy First')}</span></div>
      </aside>
      <main class="content-panel${slug === 'download' || slug === 'napstrfy' ? ' download-panel' : ''}">
        <section class="content-body">${content}</section>
        <footer><nav aria-label="${t('Footer navigation')}">${nav}</nav><em><a href="https://github.com/lnbits/napstr/blob/main/LICENSE">${t('Napstr is FLOSS — MIT License')}</a></em></footer>
      </main>
    </div>
  </div>
</body>
</html>`;
}

function directionForLanguage(code) { return code === 'ar' || code === 'ur' ? 'rtl' : 'ltr'; }
