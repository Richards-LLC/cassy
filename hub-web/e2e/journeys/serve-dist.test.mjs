import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { once } from 'node:events';
import { createServer } from 'node:net';
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
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
import sys
from pathlib import Path
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
    const env = { ...process.env, PATH: `${join(repo, 'tools')}:${process.env.PATH}` };
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
  assert.equal(await readFile(join(repo, 'hub-web/dist/index.html'), 'utf8'), 'stale committed bundle');
  assert.equal(git('status', '--porcelain'), '');
  child.kill('SIGTERM'); await once(child, 'exit');
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
