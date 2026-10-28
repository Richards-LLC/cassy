#!/usr/bin/env node
/** Headless report capture, lifted from the SOW runner. Rust owns servers,
 * immutable checkout refs, leases and the overall deadline. This asset owns
 * one browser, isolated login contexts and masked PNG/JPGs; no session is saved.
 * Input is a private JSON plan. Output is one sanitized JSON result. */
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync, existsSync, realpathSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join, resolve, relative, isAbsolute, extname } from 'node:path';
import { pathToFileURL, fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

export async function resolvePlaywright(root) {
  for (const base of [root, join(root, 'hub-web'), process.cwd()]) {
    try { return createRequire(join(base, 'package.json'))('playwright'); } catch {}
    try { return createRequire(join(base, 'package.json'))('@playwright/test'); } catch {}
  }
  if (process.env.PLAYWRIGHT_MODULE) {
    return createRequire(import.meta.url)(process.env.PLAYWRIGHT_MODULE);
  }
  try { return await import('playwright'); } catch {}
  try {
    const bin = execFileSync('which', ['playwright'], { encoding: 'utf8' }).trim();
    const dir = resolve(dirname(bin), '../playwright');
    if (existsSync(join(dir, 'index.mjs'))) return import(pathToFileURL(join(dir, 'index.mjs')).href);
  } catch {}
  throw new Error('playwright-unavailable');
}

export function validateShots(shots, config) {
  if (!Array.isArray(shots) || shots.length === 0 || shots.length > config.max_shots) throw new Error('shot-budget');
  const ids = new Set();
  const bounds = config.viewports.map(v => v.split('x').map(Number));
  return shots.map(s => {
    if (!s || typeof s !== 'object' || typeof s.id !== 'string' || !/^[A-Za-z0-9][A-Za-z0-9_.-]{0,119}$/.test(s.id ?? '') || ids.has(s.id)) throw new Error('shot-id');
    ids.add(s.id);
    const viewport = s.viewport ?? [1280, 800];
    if (!Array.isArray(viewport) || viewport.length !== 2 || !viewport.every(Number.isSafeInteger)
      || !bounds.some(([w,h]) => viewport[0] === w && viewport[1] > 0 && viewport[1] <= h)) throw new Error('shot-viewport');
    for (const field of ['allowRedirect','fullPage']) if (s[field] !== undefined && typeof s[field] !== 'boolean') throw new Error('shot-flag');
    if (s.touches !== undefined && (!Array.isArray(s.touches) || !s.touches.every(v => typeof v === 'string' && v.trim()))) throw new Error('shot-touches');
    const scheme = s.scheme ?? 'light';
    if (!['light','dark'].includes(scheme)) throw new Error('shot-scheme');
    if (s.settleMs !== undefined && (!Number.isSafeInteger(s.settleMs) || s.settleMs < 0 || s.settleMs > 5000)) throw new Error('shot-settle');
    for (const field of ['ready','locator','surface']) if (s[field] !== undefined && (typeof s[field] !== 'string' || !s[field].trim())) throw new Error('shot-selector');
    if (s.mask !== undefined && (!Array.isArray(s.mask) || !s.mask.every(v => typeof v === 'string' && v.trim()))) throw new Error('shot-mask');
    if (s.url !== undefined) {
      const url = new URL(s.url);
      if (url.protocol !== 'file:' || s.source !== 'artifact' || !/^\d{4}-\d{2}-\d{2}$/.test(s.delivered ?? '')) throw new Error('shot-url');
      const rel = relative(config.root, realpathSync(fileURLToPath(url)));
      if (!rel || isAbsolute(rel) || rel.startsWith('..') || rel.split(/[\\/]/).some(p => ['.cas','.git'].includes(p)) || !['.html','.htm'].includes(extname(rel))) throw new Error('shot-artifact');
      s = { ...s, artifact_path: rel };
    } else if (typeof s.path !== 'string' || !s.path.startsWith('/') || s.path.startsWith('//') || /[\s\\]/.test(s.path)) throw new Error('shot-path');
    return { ...s, viewport, scheme, mask: [...new Set([...config.mask, ...(s.mask ?? [])])] };
  });
}

function credentials(path) {
  const lines = readFileSync(path, 'utf8').split(/\r?\n/);
  const pick = name => {
    const line = lines.find(v => v.toLowerCase().startsWith(`${name}:`));
    if (!line || !line.slice(name.length + 1).trim()) throw new Error('credentials-format');
    return line.slice(name.length + 1).trim();
  };
  return { email: pick('email'), password: pick('password') };
}

async function login(page, phase, plan) {
  const c = plan.config;
  if (!c.login_path) return;
  const account = credentials(plan.credentials_file);
  await page.goto(new URL(c.login_path, phase.base).href, { waitUntil: 'networkidle' });
  if (new URL(page.url()).origin !== new URL(phase.base).origin) throw new Error('third-party-auth');
  await page.locator(c.email_sel).first().fill(account.email);
  await page.locator(c.password_sel).first().fill(account.password);
  await page.locator(c.submit_sel).first().click();
  await page.waitForURL(new RegExp(c.success_url));
  if (new URL(page.url()).origin !== new URL(phase.base).origin) throw new Error('third-party-auth');
}

async function capture(plan) {
  const shots = validateShots(plan.shots, { ...plan.config, root: plan.root });
  // Before launching any browser, enforce that every artifact exists on both
  // requested revisions. The original project copy is never photographed.
  for (const phase of plan.phases) for (const s of shots) if (s.artifact_path) {
    const path = realpathSync(join(phase.root, s.artifact_path));
    if (!path.startsWith(realpathSync(phase.root) + '/')) throw new Error('shot-artifact');
  }
  const { chromium } = await resolvePlaywright(plan.root);
  const browser = await chromium.launch({ headless: true });
  let timer;
  const result = { captures: [], failures: [] };
  try {
    // Leave Rust a grace window to reap every resource if Node cannot finish.
    timer = setTimeout(() => void browser.close(), Math.max(1, plan.remaining_ms - 750));
    for (const phase of plan.phases) {
      const context = await browser.newContext({ viewport: { width:1280, height:800 }, deviceScaleFactor:2 });
      const page = await context.newPage();
      page.setDefaultTimeout(Math.min(30_000, plan.remaining_ms));
      page.setDefaultNavigationTimeout(Math.min(90_000, plan.remaining_ms));
      try {
        await login(page, phase, plan);
        for (const s of shots) {
          try {
            const [width,height] = s.viewport;
            await page.setViewportSize({ width,height });
            await page.emulateMedia({ colorScheme:s.scheme });
            const url = s.artifact_path ? pathToFileURL(join(phase.root,s.artifact_path)).href : new URL(s.path,phase.base).href;
            await page.goto(url, { waitUntil:'networkidle' });
            if (!s.artifact_path && new URL(page.url()).origin !== new URL(phase.base).origin) throw new Error('capture-origin');
            if (!s.allowRedirect && new URL(page.url()).pathname.replace(/\/+$/,'') !== new URL(url).pathname.replace(/\/+$/,'')) throw new Error('capture-redirect');
            if (s.ready) await page.locator(s.ready).first().waitFor({ state:'visible' });
            if (s.settleMs) await page.waitForTimeout(s.settleMs);
            if (Buffer.byteLength(await page.content()) > plan.config.max_html_mb * 1024 * 1024) throw new Error('capture-size');
            const mask = s.mask.map(sel => page.locator(sel));
            for (let i = 0; i < mask.length; i++) if (await mask[i].count() === 0) {
              const error = new Error('capture-mask');
              error.selector = s.mask[i];
              throw error;
            }
            const target = s.locator ? page.locator(s.locator).first() : page;
            const common = { mask, maskColor:'#8F8371', animations:'disabled', ...(s.locator ? {} : { fullPage:Boolean(s.fullPage) }) };
            const stem = `${s.id}.${phase.side}`;
            const png = `${stem}.png`, jpg = `${stem}.jpg`;
            await target.screenshot({ ...common,path:join(plan.out,png),type:'png' });
            await target.screenshot({ ...common,path:join(plan.out,jpg),type:'jpeg',quality:80,scale:'css' });
            const sha = path => createHash('sha256').update(readFileSync(path)).digest('hex');
            result.captures.push({ id:s.id,surface:s.surface ?? s.path ?? s.artifact_path,viewport:s.viewport,scheme:s.scheme,masked:s.mask,side:phase.side,
              capture:{png,jpg,sha256:sha(join(plan.out,png)),jpg_sha256:sha(join(plan.out,jpg)),ref:phase.ref,source:s.artifact_path?'artifact':phase.source,captured_at:new Date().toISOString()} });
          } catch (error) {
            const known = ['capture-origin','capture-redirect','capture-size','capture-mask'];
            if (error.message === 'capture-mask') {
              let finding = result.failures.find(f => f.id === s.id && f.code === 'capture-mask' && f.selector === error.selector);
              if (!finding) {
                finding = { id:s.id, code:'capture-mask', selector:error.selector, sides:[] };
                result.failures.push(finding);
              }
              if (!finding.sides.includes(phase.side)) finding.sides.push(phase.side);
              finding.message = `Mask ${JSON.stringify(finding.selector)} absent ${finding.sides.join(', ')}; fix CSS`;
            } else {
              result.failures.push({id:s.id,side:phase.side,code:known.includes(error.message)?error.message:'capture-shot',message:'Shot failed; verify its route, readiness and selectors'});
            }
          }
        }
      } catch {
        // Do not save an unmasked login failure screen or echo page URLs.
        for (const s of shots) result.failures.push({id:s.id,side:phase.side,code:'capture-login',message:'Login failed; verify QA account, local login and anchored signed-in URL'});
      } finally { await context.close().catch(() => {}); }
    }
  } finally { clearTimeout(timer); await browser.close(); }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const plan = JSON.parse(readFileSync(process.argv[2], 'utf8'));
  if (process.argv[3] === '--validate') {
    try { validateShots(plan.shots, { ...plan.config,root:plan.root }); process.stdout.write('{}\n'); }
    catch { process.stdout.write(JSON.stringify({code:'capture-catalog',message:'Invalid shot catalog: check IDs, local paths, selectors, framing and shot budget'})+'\n'); process.exitCode=1; }
  } else {
    capture(plan).then(result => process.stdout.write(JSON.stringify(result)+'\n')).catch(error => {
      const missing = error.message === 'playwright-unavailable';
      process.stdout.write(JSON.stringify({code:missing?'capture-runtime':'capture-unavailable',message:missing?'Playwright is unavailable; install it locally or set PLAYWRIGHT_MODULE':'Capture could not finish; verify its catalog, browser and budget'})+'\n');
      process.exitCode=missing?2:1;
    });
  }
}
