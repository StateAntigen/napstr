// Run manually with node website/social-cards.mjs after changing the branding.
// PNGs are committed assets; normal website builds do not need a browser.
import { chromium } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

const logo = (await readFile(new URL('./assets/napstr-logo.png', import.meta.url))).toString('base64');
const icon = (await readFile(new URL('../src-tauri/icons/icon.png', import.meta.url))).toString('base64');
const browser = await chromium.launch(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE
  ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {});
try {
  const page = await browser.newPage({ viewport: { width: 1200, height: 630 }, deviceScaleFactor: 1 });
  for (const product of ['Napstr', 'Napstrfy']) {
    const companion = product === 'Napstrfy';
    await page.setContent(`<!doctype html><html lang="en"><meta charset="utf-8"><style>
      * { box-sizing: border-box; }
      body { margin: 0; width: 1200px; height: 630px; padding: 38px; background: #8995aa; font-family: Arial, sans-serif; }
      .window { height: 100%; background: #d6d6d6; border: 3px solid; border-color: #f7f7f7 #484848 #484848 #f7f7f7; box-shadow: 12px 12px 0 #68748b; }
      .bar { height: 45px; margin: 4px; padding: 8px 14px; background: linear-gradient(90deg, #151657, #424e94); color: white; font-size: 23px; font-weight: bold; display: flex; justify-content: space-between; }
      .controls { letter-spacing: 6px; }
      main { padding: 35px 46px 25px; }
      .brand { height: 178px; display: flex; align-items: center; gap: 18px; }
      .logo { width: 525px; }
      .icon { width: 170px; height: 170px; }
      .wordmark { font-size: 94px; font-style: italic; font-weight: 900; letter-spacing: -6px; color: white; -webkit-text-stroke: 2px #242364; }
      h1 { color: #17174b; font-size: 48px; letter-spacing: -1.5px; margin: 22px 0 17px; }
      .bottom { border-top: 2px solid #aaa; padding-top: 22px; display: flex; justify-content: space-between; align-items: center; }
      .services { color: #373753; font-size: 25px; }
      .url { background: #efefef; border: 2px solid; border-color: white #888 #888 white; padding: 10px 18px; color: #17174b; font-size: 25px; font-weight: bold; }
    </style><body><div class="window"><div class="bar"><span>${product}</span><span class="controls">− □ ×</span></div><main>
      <div class="brand">${companion ? `<img class="icon" src="data:image/png;base64,${icon}"><span class="wordmark">napstrfy</span>` : `<img class="logo" src="data:image/png;base64,${logo}">`}</div>
      <h1>${companion ? 'Your music. On your other devices.' : 'Own your music again.'}</h1>
      <div class="bottom"><span class="services">${companion ? 'Your Napstr library · Connected by Iroh' : 'Music discovery · Nostr · Tor'}</span><span class="url">napstr.net</span></div>
    </main></div></body></html>`);
    await page.locator('img').evaluateAll((images) => Promise.all(images.map((image) => image.decode())));
    await page.screenshot({ path: fileURLToPath(new URL(`./assets/${product.toLowerCase()}-share.png`, import.meta.url)) });
  }
} finally { await browser.close(); }
