// Captures the current hub conversation surface from the checked-in fixtures (hub-web/fixtures)
// at 1280×800 and 390×844, light and dark. Output: refs/today-<fixture>-<viewport>-<scheme>.png
import { mkdir, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { extname, join, resolve, dirname } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { launchBrowser } from '../hub-mobile/browser-tools.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '../../..');
const fixtureRoot = join(repoRoot, 'hub-web', 'fixtures');
const outDir = join(here, 'refs');
const buildDir = join(repoRoot, '.cas', 'hub-messaging-fixture-build');
const { build } = await import(pathToFileURL(join(repoRoot, 'hub-web/node_modules/vite/dist/node/index.js')).href);
await rm(buildDir, { recursive: true, force: true });
await build({ configFile: false, logLevel: 'silent', root: fixtureRoot, base: '/', build: { outDir: buildDir, emptyOutDir: true, assetsInlineLimit: 0, rollupOptions: { input: join(fixtureRoot, 'index.html'), output: { entryFileNames: 'fixture.js', chunkFileNames: 'chunk-[name].js', assetFileNames: 'assets/[name][extname]' } } } });
const types = { '.css': 'text/css', '.html': 'text/html', '.js': 'text/javascript', '.svg': 'image/svg+xml', '.woff2': 'font/woff2', '.wasm': 'application/wasm' };
const server = createServer(async (req, res) => {
  const p = decodeURIComponent(new URL(req.url ?? '/', 'http://x').pathname);
  const target = resolve(buildDir, `.${p === '/' ? '/index.html' : p}`);
  if (!target.startsWith(buildDir) || !existsSync(target)) { res.writeHead(404); res.end(); return; }
  res.writeHead(200, { 'content-type': types[extname(target)] ?? 'application/octet-stream' }); res.end(await readFile(target));
});
await new Promise((ok) => server.listen(0, '127.0.0.1', ok));
const origin = `http://127.0.0.1:${server.address().port}`;
await mkdir(outDir, { recursive: true });
const browser = await launchBrowser();
const fixtures = process.argv.slice(2).length ? process.argv.slice(2) : ['conversation-replied', 'conversation', 'transcript'];
try {
  for (const fixture of fixtures) for (const scheme of ['light', 'dark']) for (const vp of [{ name: 'desktop', width: 1280, height: 800 }, { name: 'phone', width: 390, height: 844 }]) {
    const context = await browser.newContext({ viewport: vp, deviceScaleFactor: 2, colorScheme: scheme });
    const page = await context.newPage();
    await page.goto(`${origin}/?fixture=${fixture}`, { waitUntil: 'networkidle' });
    await page.waitForTimeout(1200);
    const file = join(outDir, `today-${fixture}-${vp.name}-${scheme}.png`);
    await page.screenshot({ path: file });
    console.log('ok', file.replace(repoRoot + '/', ''));
    await context.close();
  }
} finally { await browser.close(); server.close(); await rm(buildDir, { recursive: true, force: true }); }
