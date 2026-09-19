// Renders the report at 1280×800 and 390×844, light and dark (viewport only), for review.
// Usage: node render-report.mjs <report.html> <outDir> [--full]
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { launchBrowser } from '../hub-mobile/browser-tools.mjs';
const [file, outDir, flag] = process.argv.slice(2);
await mkdir(outDir, { recursive: true });
const browser = await launchBrowser();
try {
  for (const vp of [{ name: '1280', width: 1280, height: 800 }, { name: '390', width: 390, height: 844 }]) for (const scheme of ['light', 'dark']) {
    const context = await browser.newContext({ viewport: vp, deviceScaleFactor: 1, colorScheme: scheme });
    const page = await context.newPage();
    await page.goto(pathToFileURL(resolve(file)).href, { waitUntil: 'load' });
    await page.waitForTimeout(1500);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth);
    await page.screenshot({ path: resolve(outDir, `report-${vp.name}-${scheme}.png`), fullPage: flag === '--full' });
    console.log(`report-${vp.name}-${scheme}.png xoverflow=${overflow}`);
    await context.close();
  }
} finally { await browser.close(); }
