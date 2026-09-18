// Extra QA renders: JS disabled, print media, landscape phone, and a network-request count.
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { launchBrowser } from '../hub-mobile/browser-tools.mjs';
const [file, outDir] = process.argv.slice(2);
const url = pathToFileURL(resolve(file)).href;
const browser = await launchBrowser();
try {
  let ctx = await browser.newContext({ viewport: { width: 1280, height: 800 }, javaScriptEnabled: false });
  let page = await ctx.newPage();
  await page.goto(url, { waitUntil: 'load' }); await page.waitForTimeout(1200);
  await page.screenshot({ path: resolve(outDir, 'js-off-1280.png') });
  const detailsOpen = await page.evaluate(() => [...document.querySelectorAll('details')].map(d => d.open));
  const h1 = await page.evaluate(() => document.querySelectorAll('h1').length);
  const textLen = await page.evaluate(() => document.body.innerText.length);
  console.log('js-off: details open', detailsOpen, 'h1 count', h1, 'text chars', textLen);
  await ctx.close();
  ctx = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  page = await ctx.newPage();
  const requests = [];
  page.on('request', r => requests.push(r.url()));
  await page.goto(url, { waitUntil: 'load' }); await page.waitForTimeout(1200);
  const external = requests.filter(u => !u.startsWith('file:') && !u.startsWith('data:'));
  console.log('requests', requests.length, 'external', external.length, external.slice(0, 3));
  const textLenJs = await page.evaluate(() => document.body.innerText.length);
  console.log('js-on text chars', textLenJs);
  await page.emulateMedia({ media: 'print' }); await page.waitForTimeout(400);
  await page.screenshot({ path: resolve(outDir, 'print-1280.png') });
  await page.pdf({ path: resolve(outDir, 'report.pdf'), format: 'A4', printBackground: true });
  await ctx.close();
  ctx = await browser.newContext({ viewport: { width: 844, height: 390 }, colorScheme: 'dark' });
  page = await ctx.newPage();
  await page.goto(url, { waitUntil: 'load' }); await page.waitForTimeout(1200);
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth);
  await page.screenshot({ path: resolve(outDir, 'landscape-844-dark.png') });
  console.log('landscape 844x390 xoverflow', overflow);
  await ctx.close();
} finally { await browser.close(); }
