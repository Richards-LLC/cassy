#!/usr/bin/env node
/**
 * Headless screenshot capture for branded reports (evidence and before/after shots).
 *
 * Usage:
 *   npm exec --yes --package=playwright -- node capture.mjs \
 *     --shots shots.json --base https://staging.example.com --out <dir> \
 *     [--env staging] [--login --credentials <file> --login-path /login \
 *      --success-url '/dashboard' [--email-sel ..] [--password-sel ..] [--submit-sel ..]] \
 *     [--only id1,id2] [--jpeg-quality 80] [--mask-color '#8F8371']
 *
 * shots.json: an array of
 *   {
 *     "id": "campaign-roster",          // file stem; reuse the same id across captures
 *     "path": "/campaigns/42",          // appended to --base
 *     "url": "file:///abs/report.html", // optional: absolute URL instead of --base + path
 *     "source": "env",                  // optional: env (default) | artifact, a file already
 *                                       //   delivered to the reader; recorded in the manifest
 *     "delivered": "2026-09-30",        // optional: delivery date of an artifact source
 *     "allowRedirect": false,           // optional: accept a final path other than the requested one
 *     "viewport": [1440, 900],          // optional, default 1440x900
 *     "ready": "[data-test=roster] tr", // optional selector that must be visible
 *     "locator": "main",                // optional: screenshot this element only
 *     "fullPage": false,                // optional
 *     "mask": [".email", ".phone"],     // optional: personal data to mask
 *     "scheme": "light",                // optional: light | dark
 *     "settleMs": 500                   // optional extra wait after ready
 *   }
 *
 * A shot whose final path differs from the requested path fails as "redirected to …"
 * unless it sets allowRedirect: that is how an expired or failed login shows.
 * --success-url is required with --login and must match only the signed-in URL: anchor it (^https://[^/]+/admin/),
 * because a sign-in URL often carries the target in a returnUrl parameter.
 *
 * Writes <out>/<id>.png (full resolution), <out>/<id>.jpg (quality 80,
 * ≤1440px wide, for embedding) and <out>/manifest.json. The login session lives
 * only in this process: no storage state is written to disk. The credentials
 * file holds `email:` and `password:` lines and is never echoed.
 * Exit code 1 when any shot fails; failures are listed in the manifest.
 */
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

function args(argv) {
  const out = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith('--')) { out._.push(a); continue; }
    const key = a.slice(2);
    const next = argv[i + 1];
    if (next === undefined || next.startsWith('--')) out[key] = true;
    else { out[key] = next; i++; }
  }
  return out;
}

export async function resolvePlaywright() {
  try { return await import('playwright'); } catch {}
  let dir = process.env.PLAYWRIGHT_MODULE ?? '';
  if (!dir) {
    try {
      const bin = execFileSync('which', ['playwright'], { encoding: 'utf8' }).trim();
      dir = join(dirname(bin), '..', 'playwright');
    } catch {}
  }
  if (!dir || !existsSync(join(dir, 'package.json'))) {
    throw new Error('Playwright not found. Run under `npm exec --yes --package=playwright -- node ...` or set PLAYWRIGHT_MODULE.');
  }
  return import(pathToFileURL(join(dir, 'index.mjs')).href);
}

function readCredentials(file) {
  const lines = readFileSync(file, 'utf8').split(/\r?\n/);
  const pick = (k) => {
    const line = lines.find((l) => l.toLowerCase().startsWith(`${k}:`));
    if (!line) throw new Error(`credentials file lacks a "${k}:" line`);
    return line.slice(k.length + 1).trim();
  };
  return { email: pick('email'), password: pick('password') };
}

async function login(page, base, o) {
  const creds = readCredentials(o.credentials);
  try {
    // networkidle: a client-rendered form drops values typed before its scripts attach.
    await page.goto(new URL(o['login-path'] ?? '/login', base).href, { waitUntil: 'networkidle', timeout: 90_000 });
    await page.locator(o['email-sel'] ?? 'input[type=email]').first().fill(creds.email);
    await page.locator(o['password-sel'] ?? 'input[type=password]').first().fill(creds.password);
    await page.locator(o['submit-sel'] ?? 'button[type=submit]').first().click();
    await page.waitForURL(new RegExp(o['success-url']), { timeout: 60_000 });
  } catch (err) {
    await page.screenshot({ path: join(resolve(o.out), 'login-failure.png') }).catch(() => {});
    throw new Error(`login failed at ${page.url()}: ${String(err?.message ?? err).split('\n')[0]} (login-failure.png saved)`);
  }
}

const samePath = (a, b) => new URL(a).pathname.replace(/\/+$/, '') === new URL(b).pathname.replace(/\/+$/, '');

async function main() {
  const o = args(process.argv.slice(2));
  if (!o.shots || !o.base || !o.out) {
    console.error('usage: capture.mjs --shots shots.json --base <url> --out <dir> [--env name] [--login --credentials <file> ...]');
    process.exit(2);
  }
  if (o.login && (typeof o['success-url'] !== 'string' || !o['success-url'].trim())) {
    console.error("login failed: --login requires --success-url <regex>.\nExample: --success-url '^https://[^/]+/admin/' (use your signed-in route).");
    process.exit(2);
  }
  const shots = JSON.parse(readFileSync(o.shots, 'utf8'));
  const only = o.only ? new Set(String(o.only).split(',')) : null;
  const quality = Number(o['jpeg-quality'] ?? 80);
  // A neutral mask reads as redaction in a report; Playwright's default is magenta.
  const maskColor = o['mask-color'] ?? '#8F8371';
  const out = resolve(o.out);
  mkdirSync(out, { recursive: true });

  const { chromium } = await resolvePlaywright();
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2 });
  const page = await context.newPage();
  if (o.login) {
    if (!o.credentials) throw new Error('--login needs --credentials <file>');
    await login(page, o.base, o);
  }

  const manifest = { base: o.base, env: o.env ?? null, captured_at: new Date().toISOString(), shots: [], failures: [] };
  for (const s of shots) {
    if (only && !only.has(s.id)) continue;
    const [w, h] = s.viewport ?? [1440, 900];
    const url = s.url ?? new URL(s.path ?? '/', o.base).href;
    try {
      await page.setViewportSize({ width: w, height: h });
      await page.emulateMedia({ colorScheme: s.scheme ?? 'light' });
      await page.goto(url, { waitUntil: 'networkidle', timeout: 90_000 });
      if (!s.allowRedirect && !samePath(page.url(), url)) throw new Error(`redirected to ${page.url()}`);
      if (s.ready) await page.locator(s.ready).first().waitFor({ state: 'visible', timeout: 30_000 });
      if (s.settleMs) await page.waitForTimeout(s.settleMs);
      const mask = (s.mask ?? []).map((sel) => page.locator(sel));
      const target = s.locator ? page.locator(s.locator).first() : page;
      const common = { mask, maskColor, animations: 'disabled', ...(s.locator ? {} : { fullPage: Boolean(s.fullPage) }) };
      const png = join(out, `${s.id}.png`);
      await target.screenshot({ ...common, path: png, type: 'png' });
      // Embedding copy: 1x scale keeps the width ≤ the viewport (≤1440px by default).
      const jpg = join(out, `${s.id}.jpg`);
      await target.screenshot({ ...common, path: jpg, type: 'jpeg', quality, scale: 'css' });
      const sha = (f) => createHash('sha256').update(readFileSync(f)).digest('hex');
      manifest.shots.push({ id: s.id, url, final_url: page.url(), source: s.source ?? 'env', delivered: s.delivered ?? null,
        viewport: [w, h], scheme: s.scheme ?? 'light',
        captured_at: new Date().toISOString(), png: `${s.id}.png`, jpg: `${s.id}.jpg`,
        png_sha256: sha(png), jpg_sha256: sha(jpg), jpg_bytes: readFileSync(jpg).length });
      console.log(`ok   ${s.id}  ${page.url()}`);
    } catch (err) {
      manifest.failures.push({ id: s.id, url, error: String(err?.message ?? err).split('\n')[0] });
      console.log(`FAIL ${s.id}  ${url}  ${String(err?.message ?? err).split('\n')[0]}`);
    }
  }
  await browser.close();
  writeFileSync(join(out, 'manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(`${manifest.shots.length} captured, ${manifest.failures.length} failed → ${join(out, 'manifest.json')}`);
  process.exit(manifest.failures.length ? 1 : 0);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((err) => { console.error(String(err?.message ?? err)); process.exit(1); });
}
