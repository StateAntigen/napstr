import { languages, resolveLanguage, storageKey, translate } from './i18n/core.js';
import { loadLatestRelease } from './releases.js';

const language = document.documentElement.lang;
const catalog = JSON.parse(document.getElementById('page-translations').textContent);
export const t = (key, values) => translate({ [language]: catalog }, language, key, values);
let saved = 'auto';
try { saved = localStorage.getItem(storageKey) || 'auto'; } catch { /* Session-only choice. */ }
if (saved !== 'auto' && !languages.some(({ code }) => code === saved)) saved = 'auto';
const root = new URL(language === 'en' ? './' : '../', location.href);

function destinationFor(next) {
  const destination = new URL(`${next === 'en' ? '' : next + '/'}${document.body.dataset.page}.html`, root);
  destination.search = location.search;
  destination.hash = location.hash;
  return destination;
}

// Every URL keeps its language, including English and shared legacy links.
// Detect preferences to suggest a language; navigate only after a user action.
const suggestion = document.querySelector('[data-language-suggestion]');
const suggestionLink = document.querySelector('[data-suggested-language]');
function suggestLanguage() {
  const next = resolveLanguage(saved, navigator.languages);
  suggestion.hidden = next === language;
  suggestionLink.href = destinationFor(next).href;
  suggestionLink.lang = next;
  suggestionLink.dir = next === 'ar' || next === 'ur' ? 'rtl' : 'ltr';
  suggestionLink.textContent = languages.find(({ code }) => code === next).name;
}
suggestLanguage();
for (const link of document.querySelectorAll('.language-links a, [data-suggested-language]')) {
  link.href = destinationFor(link.lang).href;
  link.addEventListener('click', () => {
    // Keep normal links usable without JavaScript, including opening new tabs.
    link.href = destinationFor(link.lang).href;
    saved = link.lang;
    try { localStorage.setItem(storageKey, saved); } catch { /* Navigation still works. */ }
  });
}
window.addEventListener('languagechange', suggestLanguage);
if (document.querySelector('#release-status')) void loadLatestRelease(t, language);
