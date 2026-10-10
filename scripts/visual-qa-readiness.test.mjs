import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { runVisualQa } from './visual-qa.mjs';
import { runVisualQa as runBuiltinVisualQa } from '../cas-cli/src/builtins/skills/cas-ui-craft/scripts/visual-qa.mjs';

const fixture = fileURLToPath(new URL('./visual-qa-fixtures/loading-only.html', import.meta.url));
const matrix = { strict: true, schemes: ['light'], viewports: [{ name: 'phone', width: 390, height: 800 }] };
const artifacts = () => mkdtemp(join(process.env.VISUAL_QA_ARTIFACTS_ROOT || tmpdir(), 'readiness-'));
const html = (body) => `<!doctype html><html lang="en"><meta charset="utf-8">
  <meta name="visual-qa:requires-javascript" content="Fixture exercises asynchronous application startup.">
  <style>body { background: white; color: black; font: 16px sans-serif; }</style>${body}</html>`;
async function render(body, options = {}) {
  const artifactDir = await artifacts();
  const path = join(artifactDir, 'page.html');
  await writeFile(path, html(body));
  return runVisualQa({ ...matrix, urls: [path], artifactDir, readyTimeoutMs: 100, ...options });
}

test('spinner-only strict capture cannot PASS', async () => {
  const artifactDir = await artifacts();
  const result = await runVisualQa({ ...matrix, urls: [fixture], artifactDir, readyTimeoutMs: 150 });
  assert.equal(result.status, 'FAIL');
  assert.equal(result.exitCode, 1);
  assert.ok(result.findings.some(({ type, reason }) => type === 'page-not-ready' && /loading/.test(reason)));
  assert.equal(result.screenshots.length, 1);
  const report = JSON.parse(await readFile(join(artifactDir, 'visual-qa.json'), 'utf8'));
  assert.equal(report.readiness[0].textNodes, 0);
  assert.equal(report.readiness[0].mainLandmarks, 0);
  assert.match(result.markdown, /text nodes.*0/i);
});

test('readiness waits for a delayed visible application shell before measuring it', async () => {
  const result = await render(`<div id="app" aria-busy="true"><div role="progressbar">Loading</div></div>
    <script>setTimeout(() => { document.querySelector('#app').outerHTML = '<main id="admin-shell"><h1>Admin dashboard</h1><p>Application content loaded.</p></main>'; }, 250);</script>`,
  { readySelector: '#admin-shell', readyTimeoutMs: 1500 });
  assert.equal(result.status, 'PASS', JSON.stringify(result.findings));
  assert.equal(result.readiness[0].ready, true);
  assert.equal(result.readiness[0].mainLandmarks, 1);
  assert.equal(result.readiness[0].loadingIndicators, 0);
  assert.ok(result.readiness[0].textNodes >= 2);
});

test('automatic readiness also waits for a delayed shell without a selector', async () => {
  const result = await render(`<div class="spinner"></div>
    <script>setTimeout(() => { document.body.innerHTML = '<main><h1>Loaded application</h1></main>'; }, 200);</script>`,
  { readyTimeoutMs: 1500 });
  assert.equal(result.status, 'PASS', JSON.stringify(result.findings));
  assert.equal(result.readiness[0].mainLandmarks, 1);
});

test('missing or hidden readiness targets fail despite other rendered content', async () => {
  for (const readySelector of ['#absent', '#hidden']) {
    const result = await render('<main><h1>Public landing page</h1></main><div id="hidden" hidden>Admin</div>', { readySelector });
    assert.equal(result.status, 'FAIL');
    assert.equal(result.readiness[0].reason, 'ready-selector-not-visible');
    assert.ok(result.findings.some(({ type, selector }) => type === 'page-not-ready' && selector === readySelector));
  }
});

test('a matched target containing only a loader is not ready', async () => {
  const result = await runBuiltinVisualQa({ ...matrix, urls: [fixture], artifactDir: await artifacts(), readySelector: '#app', readyTimeoutMs: 100 });
  assert.equal(result.status, 'FAIL');
  assert.equal(result.readiness[0].reason, 'loading-or-placeholder-surface');
});

test('busy roots, progressbars, plain spinners and empty pages cannot pass', async () => {
  for (const body of [
    '<main aria-busy="true"><p>Loading...</p></main>',
    '<div role="progressbar" style="width:32px;height:32px"></div>',
    '<div class="spinner" style="width:32px;height:32px"></div><p>Loading...</p>',
    '<main></main>',
  ]) {
    const result = await render(body);
    assert.equal(result.status, 'FAIL', body);
    assert.ok(result.findings.some(({ type }) => type === 'page-not-ready'), body);
  }
});

test('a background loading region does not reject a rendered main surface', async () => {
  const result = await render('<main><h1>Dashboard</h1><p>The main surface rendered.</p></main><aside aria-busy="true"><p>Loading notifications...</p></aside>');
  assert.equal(result.status, 'PASS', JSON.stringify(result.findings));
  assert.equal(result.readiness[0].loadingIndicators, 1);
});

test('header text cannot make a dominant loading application pass', async () => {
  for (const body of [
    '<header>Administration</header><main aria-busy="true" style="height:90vh"><div role="progressbar">Loading...</div></main>',
    '<header>Administration</header><div style="height:90vh"><div class="spinner" style="width:32px;height:32px"></div></div>',
  ]) {
    const result = await render(body);
    assert.equal(result.status, 'FAIL');
    assert.equal(result.readiness[0].dominantLoadingSurface, true);
  }
});

test('text-only and image-only documents do not require a main landmark', async () => {
  for (const body of ['<p>Small static report.</p>', '<svg width="100" height="100" role="img" aria-label="Chart"><circle cx="50" cy="50" r="30"/></svg>']) {
    const result = await render(body);
    assert.equal(result.status, 'PASS', JSON.stringify(result.findings));
    assert.equal(result.readiness[0].mainLandmarks, 0);
  }
});

test('visual allowlists cannot suppress an unready page', async () => {
  const artifactDir = await artifacts();
  const allowlistPath = join(artifactDir, 'allowlist.json');
  await writeFile(allowlistPath, JSON.stringify({ entries: [{ type: '*', selector: '*', reason: 'Reviewed visuals.' }] }));
  const result = await runVisualQa({ ...matrix, urls: [fixture], artifactDir, readyTimeoutMs: 100, allowlistPath });
  assert.equal(result.status, 'FAIL');
  assert.equal(result.counts['page-not-ready'], 1);
});

test('both CLI copies honor readiness options and publish a clear failure reason', async () => {
  for (const script of ['./visual-qa.mjs', '../cas-cli/src/builtins/skills/cas-ui-craft/scripts/visual-qa.mjs']) {
    const artifactDir = await artifacts();
    const child = spawnSync(process.execPath, [fileURLToPath(new URL(script, import.meta.url)), '--strict',
      '--ready-selector', '#missing-shell', '--ready-timeout-ms', '100', '--scheme', 'light', '--viewport', '390x800', '--artifact-dir', artifactDir, fixture], { encoding: 'utf8' });
    assert.equal(child.status, 1, child.stdout + child.stderr);
    assert.match(child.stdout, /CONTENT.*text nodes=0.*main landmarks=0/);
    assert.match(child.stdout, /FAIL page-not-ready.*ready-selector-not-visible.*100 ms/);
    const report = JSON.parse(await readFile(join(artifactDir, 'visual-qa.json'), 'utf8'));
    assert.equal(report.readiness[0].readySelector, '#missing-shell');
  }
});

test('invalid readiness options fail before rendering', async () => {
  for (const readyTimeoutMs of [0, -1, NaN, Infinity, '150']) {
    await assert.rejects(runVisualQa({ urls: [fixture], readyTimeoutMs }), /positive finite number/);
  }
  await assert.rejects(runVisualQa({ urls: [fixture], readySelector: '' }), /nonempty CSS selector/);
});

test('canonical and builtin visual QA copies are identical', async () => {
  assert.equal(await readFile(new URL('./visual-qa.mjs', import.meta.url), 'utf8'),
    await readFile(new URL('../cas-cli/src/builtins/skills/cas-ui-craft/scripts/visual-qa.mjs', import.meta.url), 'utf8'));
});
