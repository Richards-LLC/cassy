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
