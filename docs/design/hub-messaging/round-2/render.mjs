// Renders the three round-2 chat variants: {pebble, stack, capsule} × {1280, 390} × {light, dark}.
import { mkdir, rm } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { launchBrowser } from '../../hub-mobile/browser-tools.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const VARIANTS = [
  { key: 'v1-pebble', file: 'v1-pebble.html' },
  { key: 'v2-stack', file: 'v2-stack.html' },
  { key: 'v3-capsule', file: 'v3-capsule.html' },
];
const VIEWPORTS = [
  { name: 'desktop', width: 1280, height: 800 },
  { name: 'mobile', width: 390, height: 844 },
];
const only = process.argv[2];
const out = resolve(here, 'png');
if (!only) await rm(out, { recursive: true, force: true });
await mkdir(out, { recursive: true });

const browser = await launchBrowser();
try {
  for (const variant of VARIANTS) {
    if (only && only !== variant.key) continue;
    for (const vp of VIEWPORTS) for (const scheme of ['light', 'dark']) {
      const context = await browser.newContext({
        viewport: { width: vp.width, height: vp.height },
        deviceScaleFactor: 2,
        colorScheme: scheme,
      });
      const page = await context.newPage();
      await page.goto(pathToFileURL(resolve(here, variant.file)).href);
      await page.waitForTimeout(300);
      const file = resolve(out, `${variant.key}-${vp.name}-${scheme}.png`);
      await page.screenshot({ path: file, fullPage: true });
      const measured = await page.evaluate(() => ({
        height: document.documentElement.scrollHeight,
        overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
      }));
      console.log(`${variant.key}-${vp.name}-${scheme}.png height=${measured.height}${measured.overflow ? ' XOVERFLOW' : ''}`);
      await context.close();
    }
  }
} finally {
  await browser.close();
}
