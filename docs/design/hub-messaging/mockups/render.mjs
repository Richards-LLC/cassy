// Renders the three conversation-surface options: {ledger, desk, brief} × 2 states × {1280, 390} × {light, dark}.
import { mkdir, rm } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { launchBrowser } from '../../hub-mobile/browser-tools.mjs';
const here = dirname(fileURLToPath(import.meta.url));
const OPTIONS = { ledger: ['thread', 'evidence'], desk: ['queue', 'item'], brief: ['now', 'history'] };
const only = process.argv[2];
const out = resolve(here, 'png');
if (!only) await rm(out, { recursive: true, force: true });
await mkdir(out, { recursive: true });
const browser = await launchBrowser();
try {
  for (const [option, states] of Object.entries(OPTIONS)) {
    if (only && only !== option) continue;
    for (const state of states) for (const vp of [{ name: 'desktop', width: 1280, height: 800 }, { name: 'phone', width: 390, height: 844 }]) for (const scheme of ['light', 'dark']) {
      const context = await browser.newContext({ viewport: vp, deviceScaleFactor: 2, colorScheme: scheme });
      const page = await context.newPage();
      await page.goto(pathToFileURL(resolve(here, `${option}.html`)).href + `?state=${state}&vp=${vp.name}`);
      await page.waitForTimeout(300);
      const fixedSheet = (option === 'ledger' && state === 'evidence' && vp.name === 'phone');
      const file = resolve(out, `${option}-${state}-${vp.name}-${scheme}.png`);
      await page.screenshot({ path: file, fullPage: !fixedSheet });
      const h = await page.evaluate(() => document.documentElement.scrollHeight);
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth);
      console.log(`${option}-${state}-${vp.name}-${scheme}.png height=${h}${overflow ? ' XOVERFLOW' : ''}`);
      await context.close();
    }
  }
} finally { await browser.close(); }
