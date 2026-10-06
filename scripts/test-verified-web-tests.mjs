import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync, rmSync, mkdirSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { runVerified } from '../hub-web/scripts/run-verified-tests.mjs';

for (const runner of ['vitest', 'playwright']) {
  for (const scenario of ['zero', 'skipped', 'missing', 'pass', 'failed']) {
    test(`${runner}: ${scenario} native run`, () => {
      const cwd = mkdtempSync(join(tmpdir(), 'cas-web-guard-fixture-'));
      const count = join(cwd, 'count');
      mkdirSync(join(cwd, 'e2e/.results'), { recursive: true });
      writeFileSync(count, '999\n');
      const spawn = (exe, args, opts) => {
        const file = runner === 'vitest' ? args.find(arg => arg.startsWith('--outputFile=')).split('=')[1] : opts.env.PLAYWRIGHT_JSON_OUTPUT_NAME;
        if (scenario !== 'missing') writeFileSync(file, JSON.stringify(runner === 'vitest'
          ? { numPassedTests: scenario === 'pass' ? 2 : 0, numPendingTests: scenario === 'skipped' ? 8 : 0 }
          : { stats: { expected: scenario === 'pass' ? 2 : 0, skipped: scenario === 'skipped' ? 8 : 0 } }));
        return { status: scenario === 'failed' ? 7 : 0 };
      };
      try {
        const run = () => runVerified(runner, [], { cwd, env: { VERIFIED_TEST_COUNT_FILE: count }, spawn });
        if (scenario === 'pass') {
          assert.equal(run(), 0);
          assert.equal(readFileSync(count, 'utf8'), '2\n');
          if (runner === 'playwright') assert.equal(JSON.parse(readFileSync(join(cwd, 'e2e/.results/report.json'))).stats.expected, 2);
        } else if (scenario === 'failed') {
          assert.equal(run(), 7);
          if (runner === 'playwright') assert.equal(JSON.parse(readFileSync(join(cwd, 'e2e/.results/report.json'))).stats.expected, 0);
        }
        else assert.throws(run, scenario === 'missing' ? /ENOENT/ : /zero tests/);
        if (scenario !== 'pass') assert.equal(existsSync(count), false);
      } finally { rmSync(cwd, { recursive: true, force: true }); }
    });
  }
}

for (const runner of ['vitest', 'playwright']) {
  test(`${runner}: host guard and authoritative worker cap`, () => {
    const cwd = mkdtempSync(join(tmpdir(), 'cas-web-admission-fixture-'));
    mkdirSync(join(cwd, 'e2e/.results'), { recursive: true });
    try {
      const flag = runner === 'vitest' ? '--maxWorkers' : '--workers';
      runVerified(runner, [flag, '50'], { cwd, env: {}, spawn: (exe, args, opts) => {
        assert.equal(exe, 'python3');
        assert.match(args[0], /scripts\/worker-memory\.py$/);
        assert.equal(args[1], '--');
        assert.equal(args.filter(arg => arg.startsWith(flag)).join(), flag + (runner === 'vitest' ? '=2' : '=4'));
        assert.equal(args.includes('50'), false);
        const report = runner === 'vitest' ? args.find(arg => arg.startsWith('--outputFile=')).slice(13) : opts.env.PLAYWRIGHT_JSON_OUTPUT_NAME;
        writeFileSync(report, JSON.stringify(runner === 'vitest' ? {numPassedTests: 1} : {stats: {expected: 1}}));
        return {status: 0};
      }});
    } finally { rmSync(cwd, {recursive: true, force: true}); }
  });
}

// cas-3ae7: journey-eval.sh's explicit 1..4 reaches Playwright; unset stays 1.
for (const [args, expected] of [[[], '--workers=1'], [['--workers=3'], '--workers=3'], [['--workers', '2'], '--workers=2'], [['--workers=50%'], '--workers=1']]) {
  test(`playwright: caller worker request ${JSON.stringify(args)} becomes ${expected}`, () => {
    const cwd = mkdtempSync(join(tmpdir(), 'cas-web-workers-fixture-'));
    mkdirSync(join(cwd, 'e2e/.results'), { recursive: true });
    try {
      runVerified('playwright', args, { cwd, env: {}, spawn: (exe, spawned, opts) => {
        assert.deepEqual(spawned.filter(arg => arg.startsWith('--workers')), [expected]);
        writeFileSync(opts.env.PLAYWRIGHT_JSON_OUTPUT_NAME, JSON.stringify({stats: {expected: 1}}));
        return {status: 0};
      }});
    } finally { rmSync(cwd, {recursive: true, force: true}); }
  });
}
