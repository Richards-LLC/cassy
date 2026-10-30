import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import test from 'node:test';
import { checkoutPorts } from './checkout-ports.mjs';

const hubRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const playwrightCli = join(hubRoot, 'node_modules', '@playwright', 'test', 'cli.js');
const playwrightModule = pathToFileURL(join(hubRoot, 'node_modules', '@playwright', 'test', 'index.mjs')).href;
const STARTUP_TIMEOUT_MS = 20_000;

test('production journey servers keep the fail-closed listener contract', async () => {
  const config = await readFile(join(hubRoot, 'playwright.config.ts'), 'utf8');
  const distServer = await readFile(join(hubRoot, 'e2e', 'journeys', 'serve-dist.mjs'), 'utf8');
  assert.equal([...config.matchAll(/reuseExistingServer:\s*false/g)].length, 2);
  assert.match(config, /vite fixtures .*--strictPort/);
  assert.match(distServer, /\.listen\(port,\s*'127\.0\.0\.1'/);
  // A URL probe is answered by whichever caller's server holds the port, so
  // readiness must come from each launched server's own output.
  assert.equal([...config.matchAll(/wait:\s*\{\s*stdout:/g)].length, 2);
  assert.doesNotMatch(config, /^\s*url:/m);
});

function shellQuote(value) {
  return `'${String(value).replaceAll("'", "'\\''")}'`;
}

async function canBind(port) {
  const server = createServer();
  return new Promise((resolveAvailable) => {
    server.once('error', () => resolveAvailable(false));
    server.listen(port, '127.0.0.1', () => server.close(() => resolveAvailable(true)));
  });
}

async function availablePair(dir) {
  const ports = checkoutPorts(dir);
  return await canBind(ports.fixtures) && await canBind(ports.journeys) ? ports : null;
}

async function makeSameCheckout(root) {
  for (let index = 0; index < 1_024; index += 1) {
    const dir = join(root, `same-${index}`);
    await mkdir(dir);
    const ports = await availablePair(dir);
    if (ports) return { dirs: [dir, dir], ports };
  }
  throw new Error('no available checkout-derived port pair found in 1,024 candidates');
}

async function makeCollidingCheckouts(root) {
  const seen = new Map();
  for (let index = 0; index < 1_024; index += 1) {
    const dir = join(root, `collision-${index}`);
    await mkdir(dir);
    const ports = checkoutPorts(dir);
    const previous = seen.get(ports.fixtures);
    if (previous && previous.dir !== dir && await availablePair(dir)) {
      return { dirs: [previous.dir, dir], ports };
    }
    seen.set(ports.fixtures, { dir, ports });
  }
  throw new Error('no available collision between distinct checkout port pairs in 1,024 candidates');
}

async function waitFor(description, predicate, timeoutMs = STARTUP_TIMEOUT_MS) {
  const deadline = performance.now() + timeoutMs;
  while (performance.now() < deadline) {
    const value = await predicate();
    if (value) return value;
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 15));
  }
  throw new Error(`timed out waiting for ${description}`);
}

function callerFixture(root) {
  return join(root, 'owner-server.mjs');
}

async function writeServerFixture(root) {
  await writeFile(callerFixture(root), `
    import { createServer } from 'node:http';
    import { access, writeFile } from 'node:fs/promises';
    const [portText, readyFile, gateFile, token, role] = process.argv.slice(2);
    const port = Number(portText);
    const server = createServer((_request, response) => {
      response.writeHead(200, { 'content-type': 'text/plain' });
      response.end(token + ':' + role);
    });
    await writeFile(readyFile, JSON.stringify({ pid: process.pid, port, token, role }));
    while (true) {
      try { await access(gateFile); break; }
      catch { await new Promise((resolveDelay) => setTimeout(resolveDelay, 5)); }
    }
    server.once('error', async (error) => {
      console.error('listener-error:' + error.code);
      process.exit(71);
    });
    server.listen(port, '127.0.0.1', () => {
      console.log('listener-ready:' + token + ':' + role);
    });
  `);
  await writeFile(join(root, 'probe.spec.mjs'), `
    import { access } from 'node:fs/promises';
    import { test } from ${JSON.stringify(playwrightModule)};
    test('Playwright caller reaches its own configured fixture servers', async () => {
      while (true) {
        try { await access(${JSON.stringify(join(root, 'release-probe'))}); break; }
        catch { await new Promise((resolveDelay) => setTimeout(resolveDelay, 5)); }
      }
    });
  `);
}

async function writeConfig(root, ports, token) {
  const serverScript = callerFixture(root);
  const gateFile = join(root, 'release-startup');
  const state = join(root, token);
  await mkdir(state);
  const servers = [
    { role: 'fixtures', port: ports.fixtures },
    { role: 'journeys', port: ports.journeys },
  ];
  const webServer = servers.map(({ role, port }) => {
    const ready = join(state, `${role}.ready.json`);
    const command = [process.execPath, serverScript, port, ready, gateFile, token, role]
      .map(shellQuote)
      .join(' ');
    const listening = `listener-ready:${token}:${role}`;
    return `{ command: ${JSON.stringify(command)}, wait: { stdout: new RegExp(${JSON.stringify(listening)}) }, reuseExistingServer: false, timeout: ${STARTUP_TIMEOUT_MS} }`;
  }).join(',\n');
  const config = join(state, 'playwright.config.mjs');
  await writeFile(config, `
    import { defineConfig } from ${JSON.stringify(playwrightModule)};
    export default defineConfig({
      testDir: ${JSON.stringify(root)},
      testMatch: 'probe.spec.mjs',
      outputDir: ${JSON.stringify(join(state, 'results'))},
      reporter: 'dot',
      workers: 1,
      retries: 0,
      globalTimeout: ${STARTUP_TIMEOUT_MS * 2},
      webServer: [${webServer}],
    });
  `);
  return { token, state, config, gateFile, servers };
}

function startCaller(config) {
  const child = spawn(process.execPath, [playwrightCli, 'test', '--config', config.config], {
    cwd: hubRoot,
    env: { ...process.env, CI: '1' },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  child.output = '';
  child.stdout.setEncoding('utf8').on('data', (chunk) => { child.output += chunk; });
  child.stderr.setEncoding('utf8').on('data', (chunk) => { child.output += chunk; });
  return child;
}

function exited(child) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return Promise.resolve({ code: child.exitCode, signal: child.signalCode });
  }
  return new Promise((resolveExit, rejectExit) => {
    child.once('error', rejectExit);
    child.once('exit', (code, signal) => resolveExit({ code, signal }));
  });
}

async function readReady(config, role) {
  try {
    return JSON.parse(await readFile(join(config.state, `${role}.ready.json`), 'utf8'));
  } catch {
    return null;
  }
}

async function getBodyWhenReady(url) {
  return waitFor(`listener ${url}`, async () => {
    try {
      const response = await fetch(url, { signal: AbortSignal.timeout(1_000) });
      return response.ok ? response.text() : null;
    } catch {
      return null;
    }
  });
}

function processIsAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    if (error.code === 'ESRCH') return false;
    throw error;
  }
}

async function runCollisionScenario(root, dirs, ports, name) {
  await writeServerFixture(root);
  const left = await writeConfig(root, ports, `${name}-left`);
  const right = await writeConfig(root, ports, `${name}-right`);
  const callers = [left, right];
  const children = callers.map(startCaller);
  try {
    await waitFor(`${name}: both callers reach the held startup gate`, async () => {
      const [leftReady, rightReady] = await Promise.all(callers.map((caller) => readReady(caller, 'fixtures')));
      return leftReady && rightReady ? [leftReady, rightReady] : null;
    });
    await writeFile(left.gateFile, 'release');

    const fixtureBody = await getBodyWhenReady(`http://127.0.0.1:${ports.fixtures}/`);
    const winners = callers.flatMap((caller, index) => fixtureBody === `${caller.token}:fixtures` ? [index] : []);
    assert.equal(winners.length, 1, `${name}: exactly one caller's fixture server must own the pair, got ${fixtureBody}`);
    const winnerIndex = winners[0];
    const loserIndex = 1 - winnerIndex;
    const winner = callers[winnerIndex];
    const loser = callers[loserIndex];
    const fixtureReady = await readReady(winner, 'fixtures');
    const journeyReady = await waitFor(`${name}: winning journey server startup`, async () => readReady(winner, 'journeys'));
    const loserJourneyReady = await readReady(loser, 'journeys');
    assert.equal(loserJourneyReady, null, `${name}: the rejected caller must not start a second server for the pair`);
    assert.deepEqual(dirs.map((dir) => checkoutPorts(dir)), [ports, ports], `${name}: caller paths must resolve to the same exact pair`);

    const journeyBody = await getBodyWhenReady(`http://127.0.0.1:${ports.journeys}/commander/`);
    assert.equal(fixtureBody, `${winner.token}:fixtures`);
    assert.equal(journeyBody, `${winner.token}:journeys`);
    assert.equal(fixtureReady.token, winner.token);
    assert.equal(journeyReady.token, winner.token);
    const loserResult = await exited(children[loserIndex]);
    assert.notEqual(loserResult.code, 0, `${name}: competing Playwright caller must fail closed`);

    await writeFile(join(root, 'release-probe'), 'verified');
    const winnerResult = await exited(children[winnerIndex]);
    assert.equal(winnerResult.code, 0, `${name}: winning Playwright caller should finish cleanly\n${children[winnerIndex].output}`);

    for (const config of callers) {
      for (const { role } of config.servers) {
        const ready = await readReady(config, role);
        if (!ready) continue;
        await waitFor(`${name}: ${config.token}/${role} process-death receipt`, async () => !processIsAlive(ready.pid));
      }
    }
    // Exit 71 is the fixture's EADDRINUSE exit: the loser failed on its own
    // server's bind, never attaching to the winner's listener to run tests.
    assert.match(children[loserIndex].output, /config\.webServer was not able to start\. Exit code: 71/,
      `${name}: failed caller should report its own server's port conflict\n${children[loserIndex].output}`);
    assert.doesNotMatch(children[loserIndex].output, /Running \d+ test/,
      `${name}: the rejected caller must not run tests against another caller's server\n${children[loserIndex].output}`);
  } finally {
    await Promise.all(children.map(async (child) => {
      if (child.exitCode === null && child.signalCode === null) {
        child.kill('SIGTERM');
        await exited(child);
      }
    }));
    for (const config of callers) {
      for (const { role } of config.servers) {
        const ready = await readReady(config, role);
        if (!ready || ready.token !== config.token || !processIsAlive(ready.pid)) continue;
        try { process.kill(ready.pid, 'SIGTERM'); }
        catch (error) { if (error.code !== 'ESRCH') throw error; }
      }
    }
  }
}

test('same-checkout concurrent Playwright callers fail closed and retain server identity', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'cas-same-checkout-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const fixture = await makeSameCheckout(root);
  await runCollisionScenario(root, fixture.dirs, fixture.ports, 'same-checkout');
});

test('colliding-checkout concurrent Playwright callers fail closed and retain server identity', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'cas-colliding-checkouts-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const fixture = await makeCollidingCheckouts(root);
  assert.notEqual(fixture.dirs[0], fixture.dirs[1]);
  await runCollisionScenario(root, fixture.dirs, fixture.ports, 'colliding-checkout');
});
