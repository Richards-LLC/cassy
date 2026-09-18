// Element screenshot of a report section for review: node element-shot.mjs <html> <selector> <out.png> [width] [scheme]
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { launchBrowser } from '../hub-mobile/browser-tools.mjs';
const [file, selector, out, width = '1280', scheme = 'light'] = process.argv.slice(2);
const browser = await launchBrowser();
try {
  const context = await browser.newContext({ viewport: { width: Number(width), height: 800 }, deviceScaleFactor: 1, colorScheme: scheme });
  const page = await context.newPage();
  await page.goto(pathToFileURL(resolve(file)).href, { waitUntil: 'load' });
  await page.waitForTimeout(1500);
  await page.locator(selector).first().screenshot({ path: out });
  console.log('ok', out);
} finally { await browser.close(); }
