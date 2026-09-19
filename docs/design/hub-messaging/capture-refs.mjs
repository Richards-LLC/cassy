// Reference-UI capture for the hub messaging study (cas-c3c5).
// Usage: node docs/design/hub-messaging/capture-refs.mjs <manifest.json> [--only NN,NN]
// Manifest: [{ id:"01", slug:"telegram", title:"Telegram", category:"consumer",
//   url:"https://…", mode:"page"|"element"|"fullpage", selector?:"css", viewport?:{width,height},
//   scale?:2, scheme?:"light"|"dark", delay?:ms, waitFor?:"css", dismiss?:["css",…], hide?:["css",…],
//   scrollTo?:"css", clip?:{x,y,width,height}, ua?:"mobile" }]
// Writes refs/<id>-<slug>.png beside a receipts JSON naming URL, method and time.
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, resolve, basename } from 'node:path';
import { launchBrowser } from '../hub-mobile/browser-tools.mjs';

const [manifestPath, ...rest] = process.argv.slice(2);
if (!manifestPath) throw new Error('Usage: capture-refs.mjs <manifest.json> [--only NN,NN]');
const only = rest.includes('--only') ? rest[rest.indexOf('--only') + 1].split(',') : null;
const outDir = resolve(dirname(manifestPath));
await mkdir(outDir, { recursive: true });
const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
const receiptPath = resolve(outDir, basename(manifestPath).replace(/\.json$/, '') + '.receipts.json');
let receipts = [];
try { receipts = JSON.parse(await readFile(receiptPath, 'utf8')); } catch {}

const MOBILE_UA = 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1';
const DESKTOP_UA = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/153.0.0.0 Safari/537.36';

const browser = await launchBrowser();
try {
  for (const entry of manifest) {
    if (only && !only.includes(entry.id)) continue;
    const file = `${entry.id}-${entry.slug}.png`;
    const viewport = entry.viewport ?? { width: 1280, height: 800 };
    const context = await browser.newContext({
      viewport, deviceScaleFactor: entry.scale ?? 2, colorScheme: entry.scheme ?? 'light',
      userAgent: entry.ua === 'mobile' ? MOBILE_UA : DESKTOP_UA, locale: 'en-US',
      isMobile: entry.ua === 'mobile', hasTouch: entry.ua === 'mobile',
    });
    const page = await context.newPage();
    const started = Date.now();
    let status = 'ok', error = null, size = null;
    try {
      await page.goto(entry.url, { waitUntil: 'domcontentloaded', timeout: 45000 });
      await page.waitForLoadState('networkidle', { timeout: 15000 }).catch(() => {});
      for (const sel of entry.dismiss ?? []) {
        await page.locator(sel).first().click({ timeout: 3000 }).catch(() => {});
      }
      if (entry.hide?.length) {
        await page.addStyleTag({ content: entry.hide.map((s) => `${s}{display:none !important}`).join('\n') });
      }
      if (entry.waitFor) await page.locator(entry.waitFor).first().waitFor({ timeout: 20000 });
      if (entry.scrollTo) await page.locator(entry.scrollTo).first().scrollIntoViewIfNeeded();
      await page.waitForTimeout(entry.delay ?? 1500);
      const path = resolve(outDir, file);
      if (entry.mode === 'element') {
        const loc = page.locator(entry.selector).first();
        await loc.scrollIntoViewIfNeeded();
        await page.waitForTimeout(400);
        await loc.screenshot({ path });
        const box = await loc.boundingBox();
        size = box ? { width: Math.round(box.width), height: Math.round(box.height) } : null;
      } else {
        await page.screenshot({ path, fullPage: entry.mode === 'fullpage', clip: entry.clip });
        size = entry.clip ? { width: entry.clip.width, height: entry.clip.height } : viewport;
      }
    } catch (e) {
      status = 'failed'; error = String(e.message ?? e).split('\n')[0];
    }
    const receipt = {
      id: entry.id, file, title: entry.title, category: entry.category, url: entry.url,
      finalUrl: page.url(), mode: entry.mode, selector: entry.selector ?? null, viewport, scheme: entry.scheme ?? 'light',
      size, status, error, capturedAt: new Date().toISOString(), ms: Date.now() - started,
    };
    receipts = receipts.filter((r) => r.id !== entry.id).concat(receipt).sort((a, b) => a.id.localeCompare(b.id));
    console.log(`${status.padEnd(6)} ${file} ${size ? `${size.width}x${size.height}` : ''} ${error ?? ''}`);
    await context.close();
  }
} finally {
  await browser.close();
  await writeFile(receiptPath, JSON.stringify(receipts, null, 2) + '\n');
}
