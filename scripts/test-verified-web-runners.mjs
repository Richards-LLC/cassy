// Real zero-match and nonzero runs of both installed native runners. No browser
// is needed: the Playwright fixture exercises the test runner, not page fixtures.
import { mkdtempSync, writeFileSync, readFileSync, rmSync, existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import assert from 'node:assert/strict';

const hub = resolve(dirname(fileURLToPath(import.meta.url)), '../hub-web');
const scratch = mkdtempSync(join(hub, 'e2e/.verified-fixture-'));
let passed = 0;
try {
  writeFileSync(join(scratch, 'vitest.config.mjs'), `export default { test: { include: ['${scratch}/*.test.js'], environment: 'node' } };`);
  writeFileSync(join(scratch, 'playwright.config.mjs'), `export default { testDir: '${scratch}', outputDir: '${scratch}/results', testMatch: '*.spec.js' };`);
  writeFileSync(join(scratch, 'proof.test.js'), `import { test, expect } from 'vitest'; test('proof', () => expect(2+2).toBe(4));`);
  writeFileSync(join(scratch, 'proof.spec.js'), `import { test, expect } from '@playwright/test'; test('proof', () => expect(2+2).toBe(4));`);
  for (const runner of ['vitest', 'playwright']) {
    const count = join(scratch, `${runner}.count`);
    const config = join(scratch, `${runner}.config.mjs`);
    for (const zero of [true, false]) {
      writeFileSync(count, '999\n');
      const args = [join(hub, 'scripts/run-verified-tests.mjs'), runner, '--config', config];
      if (zero) args.push('no-such-test', runner === 'vitest' ? '--passWithNoTests' : '--pass-with-no-tests');
      const result = spawnSync(process.execPath, args, { cwd: hub, encoding: 'utf8', env: { ...process.env, VERIFIED_TEST_COUNT_FILE: count, JOURNEY_OUTPUT: scratch } });
      process.stdout.write(result.stdout + result.stderr);
      assert.equal(result.status, zero ? 1 : 0, `${runner} ${zero ? 'zero' : 'pass'} run: ${result.error ?? ''}`);
      if (zero) {
        assert.match(result.stderr, /zero tests passed or missing test summary/);
        assert.equal(existsSync(count), false);
      } else assert.equal(readFileSync(count, 'utf8'), '1\n');
      console.log(`PASS native ${runner} ${zero ? 'zero-match refused' : 'one executed test receipted'}`);
      passed++;
    }
  }
} finally { rmSync(scratch, { recursive: true, force: true }); }
console.log(`native web runner fixtures: ${passed} passed`);
