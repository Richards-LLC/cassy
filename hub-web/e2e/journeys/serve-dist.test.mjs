import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { once } from 'node:events';
import { createServer } from 'node:net';
import { copyFile, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

test('standalone journey server builds current source without changing tracked dist and refuses stale snapshots', async (t) => {
  const repo = await mkdtemp(join(tmpdir(), 'cas-journey-source-'));
  t.after(() => rm(repo, { recursive: true, force: true }));
  for (const dir of ['scripts', 'hub-web/src', 'hub-web/dist', 'hub-web/e2e/journeys', 'tools']) {
    await mkdir(join(repo, dir), { recursive: true });
  }
  await copyFile(new URL('../../../scripts/journey_dist.py', import.meta.url), join(repo, 'scripts/journey_dist.py'));
  await copyFile(new URL('./serve-dist.mjs', import.meta.url), join(repo, 'hub-web/e2e/journeys/serve-dist.mjs'));
  await writeFile(join(repo, '.gitignore'), '/target/\n');
  await writeFile(join(repo, 'hub-web/src/app.txt'), 'current source');
  await writeFile(join(repo, 'hub-web/dist/index.html'), 'stale committed bundle');
  await writeFile(join(repo, 'tools/npm'), `#!/usr/bin/env python3
import os,sys
from pathlib import Path
assert not os.environ.get('RUSTC_WRAPPER') and not os.environ.get('CARGO_BUILD_RUSTC_WRAPPER'), 'JavaScript snapshot must not start a Rust compiler-cache server'
out = Path(sys.argv[sys.argv.index('--outDir') + 1])
out.mkdir(parents=True)
(out / 'index.html').write_text(Path('src/app.txt').read_text())
`, { mode: 0o755 });
  const git = (...args) => execFileSync('git', args, { cwd: repo, encoding: 'utf8' }).trim();
  git('init', '-q'); git('config', 'user.name', 'fixture'); git('config', 'user.email', 'fixture@example.test');
  git('add', '.'); git('commit', '-qm', 'source-only fixture');
  const source = git('rev-parse', 'HEAD');
  const reservation = createServer();
  reservation.listen(0, '127.0.0.1'); await once(reservation, 'listening');
  const port = reservation.address().port;
  await new Promise((resolve) => reservation.close(resolve));
  const children = [];
  const start = (manifest) => {
    const env = { ...process.env, PATH: `${join(repo, 'tools')}:${process.env.PATH}`,
      RUSTC_WRAPPER: 'sccache', CARGO_BUILD_RUSTC_WRAPPER: 'sccache' };
    delete env.JOURNEY_BUILD_MANIFEST;
    if (manifest) env.JOURNEY_BUILD_MANIFEST = manifest;
    const child = spawn(process.execPath, [join(repo, 'hub-web/e2e/journeys/serve-dist.mjs'), String(port)], {
      cwd: repo, env, stdio: ['ignore', 'pipe', 'pipe'],
    });
    children.push(child);
    child.output = '';
    child.stderr.on('data', (bytes) => { child.output += bytes; });
    return child;
  };
  t.after(async () => {
    for (const child of children) {
      if (child.exitCode === null && child.signalCode === null) {
        child.kill('SIGTERM'); await once(child, 'exit');
      }
    }
  });
  const child = start();
  await new Promise((resolve, reject) => {
    child.stdout.on('data', (bytes) => { if (bytes.toString().includes(`at http://127.0.0.1:${port}/commander/`)) resolve(); });
    child.once('exit', () => reject(new Error(child.output)));
    child.once('error', reject);
  });
  const response = await fetch(`http://127.0.0.1:${port}/commander/`);
  assert.equal(await response.text(), 'current source');
  assert.equal(response.headers.get('x-cassy-source-sha'), source);
  assert.match(response.headers.get('x-cassy-dist-sha256'), /^[a-f0-9]{64}$/);
  assert.equal(git('status', '--porcelain'), '');
  child.kill('SIGTERM'); await once(child, 'exit');

  // cas-5873: cancelling a run kills Playwright's runner, which launched the
  // server as a shell command in its own process group with a stdin pipe.
  const runner = spawn(process.execPath, ['-e', `
    const server = require('node:child_process').spawn(process.argv[1],
      { shell: true, detached: true, stdio: ['pipe', 'pipe', 'pipe'] });
    server.stdout.on('data', (bytes) => { if (String(bytes).includes('/commander/')) console.log(server.pid); });
  `, `'${process.execPath}' hub-web/e2e/journeys/serve-dist.mjs ${port}`], {
    cwd: repo, env: { ...process.env, PATH: `${join(repo, 'tools')}:${process.env.PATH}`, JOURNEY_BUILD_MANIFEST: '' },
    stdio: ['ignore', 'pipe', 'inherit'],
  });
  const [line] = await once(runner.stdout, 'data');
  const group = Number(String(line).trim());
  const alive = () => { try { process.kill(-group, 0); return true; } catch (error) { if (error.code === 'ESRCH') return false; throw error; } };
  t.after(() => { if (alive()) process.kill(-group, 'SIGKILL'); });
  runner.kill('SIGKILL'); await once(runner, 'exit');
  for (const deadline = Date.now() + 10_000; alive() && Date.now() < deadline;) {
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  assert.equal(alive(), false, 'the cancelled run left its webServer process group running');
  const rebind = createServer();
  rebind.listen(port, '127.0.0.1'); await once(rebind, 'listening');
  await new Promise((resolve) => rebind.close(resolve));

  const manifest = join(repo, 'target/journey-dist/journey-build.json');
  const snapshot = JSON.parse(await readFile(manifest, 'utf8'));
  for (const [field, value, message] of [
    ['source_sha', '0'.repeat(40), /source does not match/],
    ['dist_sha256', '0'.repeat(64), /snapshot bytes changed/],
  ]) {
    await writeFile(manifest, JSON.stringify({ ...snapshot, [field]: value }));
    const refused = start(manifest);
    const [code] = await once(refused, 'exit');
    assert.equal(code, 2);
    assert.match(refused.output, message);
  }
});

// GH #2291: use the canonical config and real Playwright startup, with one
// HTTP-only selected test. Only npm and memory capacity are fixture boundaries.
test('named journeys execute ready snapshots and report refused admission before tests start', async (t) => {
  const repo = await mkdtemp(join(tmpdir(), 'cas-named-startup-'));
  t.after(() => rm(repo, { recursive: true, force: true }));
  const hub = join(repo, 'hub-web');
  for (const dir of ['scripts', 'hub-web/src', 'hub-web/e2e/journeys', 'tools']) {
    await mkdir(join(repo, dir), { recursive: true });
  }
  for (const path of ['scripts/journey_dist.py', 'hub-web/playwright.config.ts',
    'hub-web/e2e/checkout-ports.mjs', 'hub-web/e2e/journeys/clock.ts', 'hub-web/e2e/journeys/serve-dist.mjs']) {
    await copyFile(new URL(`../../../${path}`, import.meta.url), join(repo, path));
  }
  const installed = new URL('../../node_modules', import.meta.url).pathname;
  await symlink(installed, join(hub, 'node_modules'), 'dir');
  await writeFile(join(repo, '.gitignore'), '/target/\n/hub-web/node_modules\n');
  await writeFile(join(hub, 'package.json'), '{"type":"module"}');
  await writeFile(join(hub, 'src/app.txt'), 'source-bound named journey');
  await writeFile(join(repo, 'tools/npm'), `#!/usr/bin/env python3
import sys
from pathlib import Path
out = Path(sys.argv[sys.argv.index('--outDir') + 1])
out.mkdir(parents=True)
(out / 'index.html').write_text(Path('src/app.txt').read_text())
`, { mode: 0o755 });
  await writeFile(join(repo, 'scripts/worker-memory.py'), `import os,sys
if os.environ.get('FIXTURE_REFUSE_MEMORY') == '1':
    print('worker memory admission: fixture capacity refused', flush=True)
    sys.exit(75)
command = sys.argv[sys.argv.index('--') + 1:]
os.execvp(command[0], command)
`);
  const executed = join(repo, 'target/executed');
  await writeFile(join(hub, 'e2e/journeys/startup.journey.ts'), `
    import { test, expect } from '@playwright/test';
    import { writeFileSync } from 'node:fs';
    test('named startup selected', async ({ request }) => {
      const response = await request.get('./');
      expect(await response.text()).toBe('source-bound named journey');
      writeFileSync(${JSON.stringify(executed)}, 'executed');
    });
  `);
  const git = (...args) => execFileSync('git', args, { cwd: repo, encoding: 'utf8' }).trim();
  git('init', '-q'); git('config', 'user.name', 'fixture'); git('config', 'user.email', 'fixture@example.test');
  git('add', '.'); git('commit', '-qm', 'named startup fixture');
  const reservation = createServer();
  reservation.listen(0, '127.0.0.1'); await once(reservation, 'listening');
  const port = reservation.address().port;
  await new Promise((resolve) => reservation.close(resolve));
  for (const refused of [false, true]) {
    await rm(join(repo, 'target'), { recursive: true, force: true });
    const report = join(repo, 'target/report.json');
    const env = { ...process.env, CI: '1', PATH: `${join(repo, 'tools')}:${process.env.PATH}`,
      HUB_JOURNEY_PORT: String(port), FIXTURE_REFUSE_MEMORY: refused ? '1' : '0',
      PLAYWRIGHT_JSON_OUTPUT_NAME: report, JOURNEY_OUTPUT: join(repo, 'target/results') };
    delete env.JOURNEY_BUILD_MANIFEST;
    const child = spawn(process.execPath, [join(hub, 'node_modules/@playwright/test/cli.js'),
      'test', '--project=journeys', '--grep', '^named startup selected$', '--workers=1', '--reporter=json'],
    { cwd: hub, env, stdio: ['ignore', 'pipe', 'pipe'] });
    t.after(() => { if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM'); });
    let output = '';
    child.stdout.on('data', bytes => { output += bytes; });
    child.stderr.on('data', bytes => { output += bytes; });
    const [code] = await once(child, 'exit');
    assert.equal(code, refused ? 75 : 0, output);
    if (refused) {
      const outcome = output.split('\n').flatMap(line => { try { return [JSON.parse(line)]; } catch { return []; } })
        .find(row => row.status === 'NOT_STARTED');
      assert.equal(outcome?.reason, 'memory_admission');
      await assert.rejects(readFile(executed), { code: 'ENOENT' });
    } else {
      assert.equal(JSON.parse(await readFile(report, 'utf8')).stats.expected, 1);
      assert.equal(await readFile(executed, 'utf8'), 'executed');
    }
  }
});
