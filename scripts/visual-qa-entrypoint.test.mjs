import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtemp, readFile, rm, symlink, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const fixture = join(root, 'scripts/visual-qa-fixtures/clean.html');

for (const [name, directory] of [
  ['root', 'scripts'],
  ['builtin', 'cas-cli/src/builtins/skills/cas-ui-craft/scripts'],
]) {
  test(`${name} CLI invoked through a symlinked skills directory publishes captures`, async () => {
    const dir = await mkdtemp(join(tmpdir(), 'qa-entrypoint-'));
    try {
      await symlink(join(root, directory), join(dir, 'skills'), 'dir');
      const output = join(dir, 'evidence');
      const child = spawnSync(process.execPath, [join(dir, 'skills/visual-qa.mjs'),
        '--strict', '--scheme', 'light', '--viewport', '390x800', '--artifact-dir', output, fixture], { encoding: 'utf8' });
      assert.equal(child.status, 0, child.stdout + child.stderr);
      const report = JSON.parse(await readFile(join(output, 'visual-qa.json'), 'utf8'));
      assert.equal(report.status, 'PASS');
      assert.equal(report.screenshots.length, 1);
      await access(join(output, report.screenshots[0].path));
      assert.match(child.stdout, /PASS/);
    } finally { await rm(dir, { recursive: true, force: true }); }
  });
}

test('symlinked CLI with no inputs fails with a message', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'qa-empty-cli-'));
  try {
    const script = join(dir, 'visual-qa.mjs');
    await symlink(join(root, 'scripts/visual-qa.mjs'), script);
    const child = spawnSync(process.execPath, [script], { encoding: 'utf8' });
    assert.equal(child.status, 2);
    assert.match(child.stdout + child.stderr, /Usage:|[Nn]o captures|URL.*required/);
  } finally { await rm(dir, { recursive: true, force: true }); }
});

for (const matrix of ['schemes', 'viewports']) {
  test(`empty ${matrix} cannot produce a passing zero-capture run`, async () => {
    const dir = await mkdtemp(join(tmpdir(), 'qa-empty-matrix-'));
    try {
      const child = spawnSync(process.execPath, ['--input-type=module', '-e', `
        import { runVisualQa } from ${JSON.stringify(new URL('./visual-qa.mjs', import.meta.url).href)};
        try {
          const result = await runVisualQa(${JSON.stringify({ urls: [fixture], artifactDir: dir, [matrix]: [] })});
          process.exitCode = result.exitCode;
        } catch (error) {
          console.error(error.message);
          process.exitCode = 2;
        }
      `], { encoding: 'utf8' });
      assert.equal(child.status, 2, child.stdout + child.stderr);
      assert.match(child.stderr, /[Nn]o captures/);
    } finally { await rm(dir, { recursive: true, force: true }); }
  });
}

test('hub fixture CLI enters its help path through a symlink', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'qa-hub-entrypoint-'));
  try {
    const script = join(dir, 'visual-qa.mjs');
    await symlink(join(root, 'hub-web/scripts/visual-qa.mjs'), script);
    const child = spawnSync(process.execPath, [script, '--help'], { encoding: 'utf8' });
    assert.equal(child.status, 0, child.stdout + child.stderr);
    assert.match(child.stdout, /Usage: npm run visual-qa/);
  } finally { await rm(dir, { recursive: true, force: true }); }
});
