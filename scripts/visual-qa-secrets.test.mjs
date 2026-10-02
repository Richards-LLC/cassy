import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync, execFileSync } from 'node:child_process';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
const moduleUrl = process.env.QA_TEST_MODULE || new URL('./visual-qa.mjs', import.meta.url).href;
const qa = await import(moduleUrl);

// Synthetic credentials only. Scan outputs rather than printing trace payloads.
const idToken = 'eyJhbGciOiJub25lIn0.eyJzdWIiOiJmaXh0dXJlIn0.syntheticSignature';
const refreshToken = 'AMf-vB_synthetic_refresh_credential_1234567890';
const cookie = 'synthetic-session-cookie';
const forbidden = new RegExp([idToken, refreshToken, cookie].map((s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|'));

test('CLI errors redact authenticated request text on stdout and stderr', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'qa-secrets-cli-'));
  try {
    const journey = join(dir, 'journey.json');
    await writeFile(journey, JSON.stringify({ states: [{ name: `Authorization: Bearer ${idToken}\nCookie: __session=${cookie}\nrefreshToken=${refreshToken}`, routes: [{}] }] }));
    const child = spawnSync(process.execPath, [new URL(moduleUrl).pathname, '--journey', journey], { encoding: 'utf8' });
    assert.equal(child.status, 2);
    assert.equal(forbidden.test(child.stdout + child.stderr), false, 'credential reached CLI output');
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test('logging boundary removes header and token values while preserving diagnostics', () => {
  const redact = qa.redactQaText ?? String;
  const text = `route.fetch: Target closed\nAuthorization: Bearer ${idToken}\n  - Cookie: sid=${cookie}\nSet-Cookie: __session=${cookie}; HttpOnly\nx-firebase-auth: ${refreshToken}\nrefreshToken="${refreshToken}"\n${idToken}`;
  const output = redact(text);
  assert.equal(forbidden.test(output), false, 'credential reached logging boundary');
  assert.match(output, /Target closed/);
});

test('signed-in trace and in-flight route teardown contain no seeded credentials', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'qa-secrets-trace-'));
  try {
    const script = `
      import { createServer } from 'node:http';
      import * as qa from ${JSON.stringify(moduleUrl)};
      const tokens = ${JSON.stringify({ idToken, refreshToken, cookie })};
      const server = createServer((req, res) => {
        if (req.url === '/slow') return;
        res.writeHead(200, { 'Content-Type': 'text/html', 'Set-Cookie': 'app-session=' + tokens.cookie });
        res.end('<body>Authenticated fixture</body>');
      });
      await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
      const url = 'http://127.0.0.1:' + server.address().port;
      const { playwright } = await qa.resolvePlaywright();
      const browser = await playwright.chromium.launch({ headless: true });
      try {
        const context = await browser.newContext({ extraHTTPHeaders: { Authorization: 'Bearer ' + tokens.idToken } });
        await context.addCookies([{ name: 'app-session', value: tokens.cookie, url }]);
        await context.tracing.start({ snapshots: true, screenshots: true });
        const page = await context.newPage();
        await page.goto(url);
        // Deliberately seed during tracing: the shared saver must also scrub
        // caller mistakes and opaque storage values, not only request headers.
        await page.evaluate(tokens => localStorage.setItem('firebase:authUser:fixture', JSON.stringify(tokens)), tokens);
        let started;
        const inFlight = new Promise(resolve => started = resolve);
        await page.route('**/slow', async route => {
          started();
          const response = await route.fetch();
          await route.fulfill({ response });
        });
        await page.evaluate(() => { void fetch('/slow'); });
        await inFlight;
        if (qa.saveQaTrace) await qa.saveQaTrace(context, ${JSON.stringify(join(dir, 'trace.zip'))});
        else await context.tracing.stop({ path: ${JSON.stringify(join(dir, 'trace.zip'))} });
        if (qa.closeQaContext) await qa.closeQaContext(context);
        else await context.close();
        await new Promise(resolve => setTimeout(resolve, 50));
        await qa.runVisualQa({
          urls: [url], journey: { states: [{ name: 'signed-in', steps: [{ wait: 1 }] }] },
          schemes: ['light'], viewports: ['390x800'], artifactDir: ${JSON.stringify(join(dir, 'report'))},
          extraHTTPHeaders: { Authorization: 'Bearer ' + tokens.idToken },
          storageState: { cookies: [], origins: [{ origin: url, localStorage: [{ name: 'firebase:authUser:fixture', value: JSON.stringify(tokens) }] }] },
        });
      } finally {
        await browser.close();
        server.closeAllConnections();
        await new Promise(resolve => server.close(resolve));
      }
      console.log('fixture completed');
    `;
    const child = spawnSync(process.execPath, ['--input-type=module', '-e', script], { encoding: 'utf8', timeout: 45000 });
    // Decompress every archive entry; scanning compressed ZIP bytes is insufficient.
    const trace = execFileSync('unzip', ['-p', join(dir, 'trace.zip')], { maxBuffer: 16 * 1024 * 1024 }).toString();
    assert.equal(forbidden.test(child.stdout + child.stderr), false, 'credential reached teardown output');
    assert.equal(forbidden.test(trace), false, 'credential reached trace entry');
    assert.equal(child.status, 0, 'teardown child failed');
    assert.match(trace, /Authenticated fixture/);
    execFileSync('unzip', ['-t', join(dir, 'trace.zip')]);
    const { readdir, readFile } = await import('node:fs/promises');
    for (const name of await readdir(join(dir, 'report'))) {
      if (name.endsWith('.zip')) {
        const content = execFileSync('unzip', ['-p', join(dir, 'report', name)], { maxBuffer: 16 * 1024 * 1024 }).toString();
        assert.equal(forbidden.test(content), false, 'credential reached visual-qa signed-in trace');
      } else if (/\.(json|md)$/.test(name)) {
        assert.equal(forbidden.test(await readFile(join(dir, 'report', name), 'utf8')), false, 'credential reached visual-qa report');
      }
    }
    assert.match(trace, /"type":"before"/);
  } finally { await rm(dir, { recursive: true, force: true }); }
});


test('failed trace scrub publishes no raw archive and redacts the error', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'qa-secrets-invalid-'));
  try {
    const path = join(dir, 'trace.zip');
    const context = { tracing: { stop: async ({ path }) => writeFile(path, 'invalid archive') } };
    await assert.rejects(qa.saveQaTrace(context, path), /Invalid QA trace|bounds/);
    const { readdir } = await import('node:fs/promises');
    assert.deepEqual(await readdir(dir), []);
  } finally { await rm(dir, { recursive: true, force: true }); }
});


test('installed builtin uses the same safe capture implementation', async () => {
  const { readFile } = await import('node:fs/promises');
  const root = await readFile(new URL('./visual-qa.mjs', import.meta.url), 'utf8');
  const builtin = await readFile(new URL('../cas-cli/src/builtins/skills/cas-ui-craft/scripts/visual-qa.mjs', import.meta.url), 'utf8');
  assert.equal(root, builtin);
});
