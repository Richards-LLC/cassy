import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { runVisualQa, resolvePlaywright } from './visual-qa.mjs';
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
    assert.match(child.stdout, /CONTENT.*text nodes=0[\s\S]*main landmarks=0/);
    assert.match(child.stdout, /^FAIL page-not-ready[^\n]*\n  ready-selector-not-visible.*100 ms/);
    const report = JSON.parse(await readFile(join(artifactDir, 'visual-qa.json'), 'utf8'));
    assert.equal(report.readiness[0].readySelector, '#missing-shell');
    assert.ok(report.playwrightSource.length > 0, 'resolved browser provenance remains in the report');
    assert.ok(child.stdout.includes(`Playwright ${report.playwrightVersion}`));
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

// C957: real C5 ranking After PNG, the image omitted in C9's PASS screenshots.
// Control only the browser decoder boundary, without timing sleeps: the image
// becomes available when decode is requested, then Chromium decodes its real bytes.
test('captures wait for the After image decoder in resting and journey screenshots', async () => {
  const artifactDir = await artifacts();
  const png = await readFile(new URL('./visual-qa-fixtures/ranking.after.png', import.meta.url));
  const uri = 'data:image/png;base64,' + png.toString('base64');
  const path = join(artifactDir, 'after.html');
  await writeFile(path, html(`<main><h1>Ranking capture</h1>
    <img id="after" alt="After ranking" decoding="async" style="position:absolute;left:16px;top:96px;width:320px;height:94px"
      src="data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7"></main>
    <script>const image=document.querySelector('#after'); const nativeDecode=image.decode.bind(image);
      image.decode=()=>{ image.src=${JSON.stringify(uri)}; return nativeDecode(); };</script>`));
  const result = await runVisualQa({ ...matrix, urls: [path], artifactDir,
    journey: { states: [{ name: 'after' }] } });
  assert.equal(result.status, 'PASS', JSON.stringify(result.findings));
  assert.equal(result.screenshots.length, 2);
  const { playwright } = await resolvePlaywright();
  const executablePath = process.env.CHROME_PATH || ['google-chrome', 'chromium', 'chromium-browser']
    .map(name => spawnSync('which', [name], { encoding: 'utf8' })).find(result => result.status === 0)?.stdout.trim();
  const browser = await playwright.chromium.launch({ headless: true, executablePath });
  try {
    const page = await browser.newPage();
    for (const shot of result.screenshots) {
      const screenshot = 'data:image/png;base64,' + (await readFile(join(artifactDir, shot.path))).toString('base64');
      const pixel = await page.evaluate(async (uri) => {
        const image = new Image(); image.src = uri; await image.decode();
        const canvas = document.createElement('canvas'); canvas.width = image.width; canvas.height = image.height;
        const context = canvas.getContext('2d'); context.drawImage(image, 0, 0);
        return [...context.getImageData(17, 97, 1, 1).data];
      }, screenshot);
      // pin: original C5 PNG's paper pixel; a blank capture is white here.
      assert.deepEqual(pixel, [246, 243, 236, 255], shot.state || 'resting');
    }
  } finally { await browser.close(); }
});

test('undecoded images fail strict captures even with a visual allowlist', async () => {
  for (const mode of ['broken', 'pending']) {
    const artifactDir = await artifacts();
    const path = join(artifactDir, 'after.html');
    const allowlistPath = join(artifactDir, 'allowlist.json');
    await writeFile(allowlistPath, JSON.stringify({ entries: [{ type: '*', selector: '*', reason: 'Visual exceptions cannot approve undecoded evidence.' }] }));
    await writeFile(path, html(`<main><h1>Ranking capture</h1><img id="after" alt="After ranking" decoding="async"
      style="width:320px;height:94px" src="data:image/png;base64,broken">
      <img id="offscreen" alt="Offscreen" src="data:image/png;base64,broken" style="position:absolute;left:-9999px;width:50px;height:50px">
      <span style="position:absolute;width:1px;height:1px;overflow:hidden;clip:rect(0,0,0,0)"><img id="sr-only" alt="Screen reader helper" src="data:image/png;base64,broken" width="50" height="50"></span>
      <img id="tracking" alt="" src="data:image/png;base64,broken" width="1" height="1"></main>
      ${mode === 'pending' ? '<script>document.querySelector("#after").decode=()=>new Promise(()=>{});</script>' : ''}`));
    const result = await runBuiltinVisualQa({ ...matrix, urls: [path], artifactDir, readyTimeoutMs: 100, allowlistPath,
      journey: { states: [{ name: 'after' }] } });
    assert.equal(result.status, 'FAIL', mode);
    assert.equal(result.exitCode, 1);
    const failures = result.findings.filter(({ type }) => type === 'image-not-decoded');
    assert.equal(failures.length, 2, JSON.stringify(result.findings));
    // pin: offline reports retain the measured decode refusal's reason, not just the image label.
    assert.ok(result.markdown.includes(failures[0].reason), 'the report names the decode failure or timeout');
    assert.ok(failures.every(({ selector, textSample, reason }) => selector === '#after' && textSample === 'After ranking'
      && reason.includes(mode === 'pending' ? '100 ms' : 'failed')), JSON.stringify(failures));
  }
});

test('a child frame disappearing or navigating during image decode does not abort capture', async () => {
  for (const change of ['frameElement.remove()', 'frameElement.srcdoc="<p>Replacement frame</p>"']) {
    const result = await render(`<main><h1>Stable report</h1><p>The main document remains ready.</p></main>
      <iframe title="Ephemeral content" srcdoc='<img src="data:image/png;base64,broken" width="50" height="50"><script>
        document.querySelector("img").decode=()=>{${change};return new Promise(()=>{});};
      </script>'></iframe>`);
    assert.equal(result.status, 'PASS', JSON.stringify(result.findings));
    assert.equal(result.screenshots.length, 1);
  }
});
