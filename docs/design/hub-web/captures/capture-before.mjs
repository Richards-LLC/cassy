#!/usr/bin/env node
// Baseline ("before") captures of the 3.17.3 Commander for the Hub design pass (cas-e5c7).
// Drives the live local hub (http://127.0.0.1:4173) through a fresh headless Chrome profile:
// pairs it with a one-time invitation, walks the six screens at 1280 and 390 (dark), saves a
// PNG per screen × viewport, and runs the exact PAGE_INSPECTION from scripts/visual-qa.mjs on
// each screen so the counts per finding class are comparable with the strict gate Unit 7 runs.
//
// Usage: PAIR_URL="$(cas hub pair --origin http://127.0.0.1:4173 --json | jq -r .url)" \
//        node docs/design/hub-web/captures/capture-before.mjs [--out DIR]
import { mkdtempSync, readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { join, resolve, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, '../../../..');
const out = resolve(process.argv.includes('--out') ? process.argv[process.argv.indexOf('--out') + 1] : join(here, 'before'));
mkdirSync(out, { recursive: true });
const pairUrl = process.env.PAIR_URL;
if (!pairUrl) throw new Error('PAIR_URL is required (cas hub pair --origin http://127.0.0.1:4173 --json).');
const origin = new URL(pairUrl).origin;
const app = `${origin}/commander/`;

const qaSource = readFileSync(join(repo, 'scripts/visual-qa.mjs'), 'utf8');
const start = qaSource.indexOf('const PAGE_INSPECTION = ');
const end = qaSource.indexOf('\n};\n', start) + 2;
const inspection = new Function(`return (${qaSource.slice(start + 'const PAGE_INSPECTION = '.length, end)})`)();

const playwrightDir = process.env.PLAYWRIGHT_MODULE || '/home/pippenz/.npm/_npx/e41f203b7505f1fb/node_modules/playwright';
const pw = await import(join(playwrightDir, 'index.js'));
const chromium = pw.chromium ?? pw.default.chromium;
const profile = mkdtempSync(join(tmpdir(), 'commander-before-'));
const context = await chromium.launchPersistentContext(profile, {
  headless: true,
  executablePath: process.env.CHROME_PATH || '/usr/bin/google-chrome',
  colorScheme: 'dark',
  viewport: { width: 1280, height: 800 },
  reducedMotion: 'reduce',
});
const page = context.pages()[0] ?? await context.newPage();
page.on('console', (m) => { if (m.type() === 'error') console.error('[page]', m.text().slice(0, 200)); });

const VIEWPORTS = [
  { name: '1280', width: 1280, height: 800 },
  { name: '390', width: 390, height: 844 },
];
const results = [];
const wait = (ms) => page.waitForTimeout(ms);

async function capture(screen, viewport, prepare) {
  await page.setViewportSize({ width: viewport.width, height: viewport.height });
  await wait(300);
  if (prepare) await prepare();
  await wait(400);
  const file = `${screen}.${viewport.name}.dark.png`;
  await page.screenshot({ path: join(out, file), fullPage: false });
  const result = await page.evaluate(inspection, { colorScheme: 'dark', contrastLimit: 4.5, largeTextLimit: 3, boxTolerance: 1 });
  const counts = {};
  for (const f of result.findings) counts[f.type] = (counts[f.type] || 0) + 1;
  const infoCounts = {};
  for (const f of result.infos) infoCounts[f.type] = (infoCounts[f.type] || 0) + 1;
  results.push({ screen, viewport: viewport.name, file, counts, infoCounts, findings: result.findings.map(({ type, elementPath, textSample, ratio, foreground, background, reason }) => ({ type, elementPath, textSample, ratio, foreground, background, reason })) });
  console.log(`${screen} @${viewport.name}: ${JSON.stringify(counts)} info=${JSON.stringify(infoCounts)}`);
}

async function closeDialogs() {
  await page.evaluate(() => { for (const d of document.querySelectorAll('dialog[open]')) d.close(); });
}

// 1. Pairing dialog step 1 (no invitation): the create-code step.
await page.goto(app, { waitUntil: 'load' });
await page.waitForSelector('#empty-pair');
for (const vp of VIEWPORTS) {
  await capture('pairing-dialog-step1', vp, async () => {
    await closeDialogs();
    await page.locator('#empty-pair:visible, #pair-toggle:visible').first().click();
    await page.waitForSelector('#pair-dialog[open] #pair-create');
  });
}
await closeDialogs();

// 2. Pair through the one-time invitation (legacy fragment form) so the hub is live.
await page.goto(pairUrl, { waitUntil: 'load' });
await page.waitForSelector('#pair-dialog[open] #pair-form', { timeout: 15000 });
await page.setViewportSize({ width: 1280, height: 800 });
await page.click('#pair-use-page-origin').catch(() => {});
await page.fill('#pair-form input[name="url"]', origin);
await page.fill('#pair-form input[name="label"]', 'Soundwave Linux');
await page.fill('#pair-form input[name="device"]', 'cas-e5c7 baseline capture');
await page.fill('#pair-form input[name="operator"]', 'agile-octopus-74');
await page.click('#pair-form button[type="submit"]');
await page.waitForSelector('#fleet-board .fleet-session', { timeout: 30000 });
await wait(2500);

// 3. Fleet board.
for (const vp of VIEWPORTS) await capture('fleet-board', vp, closeDialogs);

// 4. Session canvas with panes: open the first session.
await page.setViewportSize({ width: 1280, height: 800 });
// Prefer a session that has workers so the canvas shows a supervisor pane plus a worker strip.
const cards = page.locator('#fleet-board .fleet-session');
let target = cards.first();
let best = -1;
for (let i = 0; i < await cards.count(); i += 1) {
  const text = await cards.nth(i).innerText();
  const score = (/\b[1-9]\d* workers?\b/.test(text) ? 2 : 0) + (/cas-src/.test(text) ? 1 : 0);
  if (score > best) { best = score; target = cards.nth(i); }
}
const openedSession = await target.getAttribute('data-fleet-session');
console.log('opening session', (await target.innerText()).replace(/\s+/g, ' '));
await target.click();
await page.waitForSelector('.pane', { timeout: 30000 });
await wait(6000);
for (const vp of VIEWPORTS) await capture('session-canvas', vp, closeDialogs);

// 5. Transcript view on the primary pane.
await page.setViewportSize({ width: 1280, height: 800 });
const toggle = page.locator('.pane-view-toggle').first();
if (await toggle.count()) {
  await toggle.click();
  await page.waitForSelector('.transcript', { timeout: 15000 }).catch(() => {});
  await wait(1500);
}
for (const vp of VIEWPORTS) await capture('transcript', vp, closeDialogs);
await page.setViewportSize({ width: 1280, height: 800 });
if (await toggle.count()) { await toggle.click(); await wait(800); }

// 6. Attention rail: desktop context panel expanded on the Attention tab; phone panel opened from the bar.
await capture('attention-rail', VIEWPORTS[0], async () => {
  await closeDialogs();
  const expand = page.locator('#attention-panel-toggle[aria-expanded="false"]');
  if (await expand.count()) await expand.click();
  await page.click('[data-context-tab="attention"]');
});
await capture('attention-rail', VIEWPORTS[1], async () => {
  await closeDialogs();
  const counts = page.locator('#attention-rail-counts:visible');
  if (await counts.count()) await counts.click();
  const tab = page.locator('[data-context-tab="attention"]:visible');
  if (await tab.count()) await tab.click();
  await page.waitForSelector('#attention-panel:visible', { timeout: 5000 }).catch(() => console.error('attention panel not visible at 390'));
});
await page.setViewportSize({ width: 1280, height: 800 });
await page.click('#context-panel-close').catch(() => {});

// 7a. Connection lost with a retained frame: the disconnected banner over the last terminal frame.
await page.route('**/v1/**', (route) => route.abort('connectionrefused'));
await context.setOffline(true);
await page.waitForSelector('.terminal-disconnected-banner', { timeout: 150000 }).catch(() => console.error('no .terminal-disconnected-banner within 120s'));
await wait(1500);
for (const vp of VIEWPORTS) await capture('connection-disconnected', vp, closeDialogs);

// 7b. Connection failed / retry card: open a session that has no retained frame while the hub is unreachable.
await page.setViewportSize({ width: 1280, height: 800 });
await page.click('#session-back');
await page.waitForSelector('#fleet-board .fleet-session', { timeout: 15000 });
const other = page.locator(`#fleet-board .fleet-session:not([data-fleet-session="${openedSession}"])`).first();
console.log('opening (offline)', await other.getAttribute('data-fleet-session'));
await other.click();
await page.waitForFunction((prev) => document.querySelector('#pane-grid')?.dataset.sessionKey?.endsWith(':' + prev) === false, openedSession, { timeout: 15000 }).catch(() => console.error('pane grid did not switch session'));
await page.waitForSelector('.terminal-state', { timeout: 90000 }).catch(() => console.error('no .terminal-state within 90s'));
// The connect clock shows its 5s and 15s states (amber step, Retry actions) before the card settles; capture after them.
await page.waitForSelector('.terminal-state .terminal-connecting-actions, .terminal-connect-failed', { timeout: 40000 }).catch(() => console.error('no failed/actions state within 40s'));
await wait(2000);
for (const vp of VIEWPORTS) await capture('connection-failed', vp, closeDialogs);

writeFileSync(join(out, 'baseline-visual-qa.json'), `${JSON.stringify({ generatedAt: new Date().toISOString(), app, scheme: 'dark', results }, null, 2)}\n`);
await context.close();
console.log('done →', out);
