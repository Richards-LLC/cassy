import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import test from 'node:test';

const hubRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const playwrightCli = join(hubRoot, 'node_modules', '@playwright', 'test', 'cli.js');
const playwrightModule = pathToFileURL(join(hubRoot, 'node_modules', '@playwright', 'test', 'index.mjs')).href;

test('project selection starts only the servers consumed by the selected tests', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'cas-project-servers-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const selected = join(root, 'started.jsonl');
  const server = join(root, 'server.mjs');
  await writeFile(server, `
    import { appendFileSync } from 'node:fs';
    appendFileSync(${JSON.stringify(selected)}, JSON.stringify(process.argv[2]) + '\\n');
    console.log('selected-server-ready');
    setInterval(() => {}, 1000);
  `);
  await writeFile(join(root, 'chromium.spec.mjs'), `
    import { test } from ${JSON.stringify(playwrightModule)};
    test('selected project runs', async () => {});
  `);
  const config = join(root, 'playwright.config.mjs');
  await writeFile(config, `
    import base from ${JSON.stringify(pathToFileURL(join(hubRoot, 'playwright.config.ts')).href)};
    export default {
      ...base, reporter: 'dot', workers: 1,
      projects: base.projects.map(project => ({
        ...project, testDir: ${JSON.stringify(root)}, testMatch: 'chromium.spec.mjs', testIgnore: [],
      })),
      webServer: base.webServer.map(server => ({
        ...server,
        command: [process.execPath, ${JSON.stringify(server)}, server.command.includes('vite fixtures') ? 'fixtures' : 'journeys']
          .map(value => JSON.stringify(value)).join(' '),
        wait: { stdout: /selected-server-ready/ },
      })),
    };
  `);
  for (const [args, expected] of [
    [['--project=journeys'], ['journeys']],
    // An equals-form option consumes one value; this positional spec filter
    // must not also select the chromium project.
    [['--project=journeys', 'chromium'], ['journeys']],
    [['--project', 'chromium'], ['fixtures']],
    [['--project=JOUR*'], ['journeys']],
    [[], ['fixtures', 'journeys']],
  ]) {
    await writeFile(selected, '');
    const child = spawn(process.execPath, [playwrightCli, 'test', '--config', config, ...args], {
      cwd: hubRoot, env: { ...process.env, CI: '1' }, stdio: ['ignore', 'pipe', 'pipe'],
    });
    let output = '';
    child.stdout.on('data', bytes => { output += bytes; });
    child.stderr.on('data', bytes => { output += bytes; });
    t.after(() => { if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM'); });
    const result = await exited(child);
    assert.equal(result.code, 0, output);
    const actual = (await readFile(selected, 'utf8')).trim().split('\n').filter(Boolean).map(JSON.parse);
    assert.deepEqual(actual, expected, `${args.join(' ') || 'all projects'} starts only consumed servers`);
  }
});

function exited(child) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return Promise.resolve({ code: child.exitCode, signal: child.signalCode });
  }
  return new Promise((resolveExit, rejectExit) => {
    child.once('error', rejectExit);
    child.once('exit', (code, signal) => resolveExit({ code, signal }));
  });
}
