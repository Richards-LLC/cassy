// Renders every round-3 Pebble state at 1280 and 390, light and dark.
import { mkdir, rm } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { launchBrowser } from '../../hub-mobile/browser-tools.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const STATES = ['thread-a', 'thread-b', 'list', 'evidence', 'empty', 'pairs'];
const VIEWPORTS = [
  { name: 'desktop', width: 1280, height: 800 },
  { name: 'mobile', width: 390, height: 844 },
];
const only = process.argv[2];
const out = resolve(here, 'png');
if (!only) await rm(out, { recursive: true, force: true });
await mkdir(out, { recursive: true });

const browser = await launchBrowser();
let overflows = 0;
try {
  for (const state of STATES) {
    if (only && only !== state) continue;
    for (const vp of VIEWPORTS) for (const scheme of ['light', 'dark']) {
      const context = await browser.newContext({
        viewport: { width: vp.width, height: vp.height },
        deviceScaleFactor: 2,
        colorScheme: scheme,
      });
      const page = await context.newPage();
      await page.goto(pathToFileURL(resolve(here, `${state}.html`)).href);
      await page.waitForTimeout(300);
      await page.screenshot({ path: resolve(out, `${state}-${vp.name}-${scheme}.png`), fullPage: true });
      const m = await page.evaluate(() => ({
        height: document.documentElement.scrollHeight,
        overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
      }));
      if (m.overflow) overflows += 1;
      console.log(`${state}-${vp.name}-${scheme}.png height=${m.height}${m.overflow ? ' XOVERFLOW' : ''}`);
      await context.close();
    }
  }
} finally {
  await browser.close();
}
console.log(overflows ? `FAIL: ${overflows} render(s) overflow horizontally` : 'no horizontal overflow');
process.exit(overflows ? 1 : 0);
