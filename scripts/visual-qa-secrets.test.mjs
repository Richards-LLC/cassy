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

test('runner-owned tracing continues journeys with one safe warning and no harness trace', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'qa-runner-trace-'));
  try {
    const script = `
      import * as qa from ${JSON.stringify(moduleUrl)};
      const { playwright } = await qa.resolvePlaywright();
      const launch = playwright.chromium.launch.bind(playwright.chromium);
      playwright.chromium.launch = async options => {
        const browser = await launch(options);
        const newContext = browser.newContext.bind(browser);
        browser.newContext = async options => {
          const context = await newContext(options);
          await context.tracing.start({ snapshots: true });
          return context;
        };
        return browser;
      };
      const result = await qa.runVisualQa({
        urls: [${JSON.stringify(new URL('./visual-qa-fixtures/clean.html', import.meta.url).pathname)}],
        journey: { states: [{ name: 'first', steps: [{ wait: 1 }] }, { name: 'second', steps: [{ wait: 1 }] }] },
        schemes: ['light'], viewports: ['390x800'], artifactDir: ${JSON.stringify(dir)},
      });
      if (result.journeyRuns.some(run => run.trace || run.steps.some(step => step.status !== 'ok'))) throw new Error('Journey did not complete without a harness trace');
      console.log('runner fixture completed');
    `;
    const child = spawnSync(process.execPath, ['--input-type=module', '-e', script], { encoding: 'utf8', timeout: 45000 });
    assert.equal(child.status, 0, 'runner-owned tracing interrupted the journey');
    assert.match(child.stdout, /runner fixture completed/);
    assert.equal((child.stderr.match(/runner-owned trace/g) ?? []).length, 1);
    assert.match(child.stderr, /not scrubbed/);
    assert.equal(forbidden.test(child.stdout + child.stderr), false);
    const { readdir } = await import('node:fs/promises');
    assert.equal((await readdir(dir)).some(name => name.endsWith('.zip')), false);
  } finally { await rm(dir, { recursive: true, force: true }); }
});

// cas-b10d: the close gate counts assertions only from the runner's own
// test.trace, so signed-in evidence keeps runner tracing on and scrubs the
// finished zip. Entry names and event shapes are those a runner zip carries.
function storedZip(entries) {
  const crc = (bytes) => {
    let value = 0xffffffff;
    for (const byte of bytes) {
      value ^= byte;
      for (let bit = 0; bit < 8; bit++) value = (value >>> 1) ^ (value & 1 ? 0xedb88320 : 0);
    }
    return (value ^ 0xffffffff) >>> 0;
  };
  const bodies = [], directory = [];
  let offset = 0;
  for (const [entryName, text] of entries) {
    const name = Buffer.from(entryName), bytes = Buffer.from(text), sum = crc(bytes);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50); local.writeUInt16LE(20, 4);
    local.writeUInt32LE(sum, 14); local.writeUInt32LE(bytes.length, 18); local.writeUInt32LE(bytes.length, 22); local.writeUInt16LE(name.length, 26);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50); central.writeUInt16LE(20, 4); central.writeUInt16LE(20, 6);
    central.writeUInt32LE(sum, 16); central.writeUInt32LE(bytes.length, 20); central.writeUInt32LE(bytes.length, 24); central.writeUInt16LE(name.length, 28); central.writeUInt32LE(offset, 42);
    bodies.push(local, name, bytes); directory.push(central, name);
    offset += 30 + name.length + bytes.length;
  }
  const central = Buffer.concat(directory), footer = Buffer.alloc(22);
  footer.writeUInt32LE(0x06054b50); footer.writeUInt16LE(entries.length, 8); footer.writeUInt16LE(entries.length, 10);
  footer.writeUInt32LE(central.length, 12); footer.writeUInt32LE(offset, 16);
  return Buffer.concat([...bodies, central, footer]);
}

async function zipEntries(path) {
  const { readFile } = await import('node:fs/promises');
  const { inflateRawSync } = await import('node:zlib');
  const zip = await readFile(path), entries = new Map();
  let end = zip.length - 22;
  while (zip.readUInt32LE(end) !== 0x06054b50) end--;
  let cursor = zip.readUInt32LE(end + 16);
  for (let index = 0; index < zip.readUInt16LE(end + 10); index++) {
    const method = zip.readUInt16LE(cursor + 10), size = zip.readUInt32LE(cursor + 20), nameLength = zip.readUInt16LE(cursor + 28);
    const name = zip.subarray(cursor + 46, cursor + 46 + nameLength).toString();
    const local = zip.readUInt32LE(cursor + 42);
    const start = local + 30 + zip.readUInt16LE(local + 26) + zip.readUInt16LE(local + 28);
    const bytes = zip.subarray(start, start + size);
    entries.set(name, (method === 8 ? inflateRawSync(bytes) : bytes).toString());
    cursor += 46 + nameLength + zip.readUInt16LE(cursor + 30) + zip.readUInt16LE(cursor + 32);
  }
  return entries;
}

const runnerEvents = [
  { type: 'before', callId: 'expect@1', method: 'expect', title: 'Expect "toHaveText"', params: { expected: 'Signed in' } },
  { type: 'after', callId: 'expect@1', endTime: 2 },
  { type: 'before', callId: 'pw:api@2', method: 'pw:api', title: 'Navigate to "/account"' },
  { type: 'after', callId: 'pw:api@2', endTime: 3 },
];

function signedInRunnerZip() {
  const network = {
    type: 'resource-snapshot',
    snapshot: {
      request: { url: '/account', headers: [{ name: 'Authorization', value: `Bearer ${idToken}` }, { name: 'Cookie', value: `__session=${cookie}` }], cookies: [{ name: '__session', value: cookie }] },
      response: { headers: [{ name: 'Set-Cookie', value: `__session=${cookie}; HttpOnly` }] },
    },
  };
  const page = { type: 'before', callId: 'call@3', method: 'evaluate', params: { arg: JSON.stringify({ refreshToken, idToken }) } };
  return storedZip([
    ['test.trace', runnerEvents.map((event) => JSON.stringify(event)).join('\n')],
    ['trace.trace', JSON.stringify(page)],
    ['trace.network', JSON.stringify(network)],
  ]);
}

test('a scrubbed runner trace keeps its assertions and drops every credential (cas-b10d)', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'qa-runner-trace-'));
  try {
    const raw = join(dir, 'raw.zip');
    await writeFile(raw, signedInRunnerZip());
    for (const [label, scrub] of [
      ['module', (output) => qa.scrubQaTraceFile(raw, output)],
      ['cli', (output) => {
        const child = spawnSync(process.execPath, [new URL(moduleUrl).pathname, '--scrub-trace', raw, output], {
          encoding: 'utf8', env: { ...process.env, QA_TRACE_SECRETS: cookie },
        });
        assert.equal(child.status, 0, child.stderr);
        assert.match(child.stdout, /^SCRUBBED /);
        assert.equal(forbidden.test(child.stdout + child.stderr), false, 'credential reached CLI output');
      }],
    ]) {
      const output = join(dir, `${label}-trace.zip`);
      await scrub(output);
      const entries = await zipEntries(output);
      assert.deepEqual([...entries.keys()], ['test.trace', 'trace.trace', 'trace.network'], label);
      assert.deepEqual(entries.get('test.trace').split('\n').map((line) => JSON.parse(line)), runnerEvents,
        `${label}: the runner's assertion events are unchanged`);
      for (const [name, text] of entries) assert.equal(forbidden.test(text), false, `${label}: ${name} still carries a credential`);
    }
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test('a failed runner-trace scrub publishes nothing (cas-b10d)', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'qa-runner-trace-bad-'));
  try {
    const raw = join(dir, 'raw.zip'), output = join(dir, 'trace.zip');
    await writeFile(raw, `not a zip ${cookie}`);
    await writeFile(output, 'stale');
    await assert.rejects(qa.scrubQaTraceFile(raw, output), (error) => !forbidden.test(error.message));
    const { existsSync } = await import('node:fs');
    assert.equal(existsSync(output), false);
  } finally { await rm(dir, { recursive: true, force: true }); }
});
