#!/usr/bin/env node
// Guard the native runners' machine-readable summaries, rather than console
// labels (which may include skipped tests). The Rust runner uses this same
// VERIFIED_TEST_COUNT_FILE receipt contract.
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync, rmSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export function passedCount(runner, report) {
  const count = runner === 'vitest' ? report.numPassedTests : report.stats?.expected;
  if (!Number.isSafeInteger(count) || count < 1) {
    throw new Error(`${runner}: zero tests passed or missing test summary`);
  }
  return count;
}

export function runVerified(runner, args, { spawn = spawnSync, cwd = process.cwd(), env = process.env } = {}) {
  if (!['vitest', 'playwright'].includes(runner)) throw new Error('runner must be vitest or playwright');
  const countFile = env.VERIFIED_TEST_COUNT_FILE;
  if (countFile) rmSync(countFile, { force: true });
  const scratch = mkdtempSync(join(tmpdir(), 'cas-web-tests-'));
  const report = join(scratch, 'report.json');
  try {
    // Own the reporters/output so caller arguments cannot silently select a
    // console-only run or reuse a report from a previous command.
    if (args.some(arg => /^--(?:reporter|outputFile)/.test(arg))) throw new Error('runner owns reporter/outputFile options');
    // Playwright honours the caller's last explicit --workers (journey-eval.sh
    // passes 1..4) up to four; vitest stays at two (cas-3ae7).
    let requested;
    args.forEach((arg, index) => {
      if (arg.startsWith('--workers=')) requested = arg.slice('--workers='.length);
      else if (arg === '--workers') requested = args[index + 1];
    });
    args = args.filter((arg, index) => !/^--(?:maxWorkers|workers)(?:=|$)/.test(arg) && !/^--(?:maxWorkers|workers)$/.test(args[index - 1] ?? ''));
    const workers = /^\d+$/.test(requested ?? '') ? Math.min(Math.max(Number(requested), 1), 4) : 1;
    args.push(runner === 'vitest' ? '--maxWorkers=2' : `--workers=${workers}`);
    const nativeArgs = runner === 'vitest'
      ? ['run', ...args, '--reporter=default', '--reporter=json', `--outputFile=${report}`]
      : ['test', ...args, '--reporter=list,json'];
    const result = spawn('python3', [fileURLToPath(new URL('../../scripts/worker-memory.py', import.meta.url)), '--', process.execPath, resolve(cwd, `node_modules/${runner === 'vitest' ? 'vitest/vitest.mjs' : '@playwright/test/cli.js'}`), ...nativeArgs], {
      cwd, stdio: 'inherit', env: { ...env, PLAYWRIGHT_JSON_OUTPUT_NAME: report },
    });
    if (result.error) throw result.error;
    // Keep failures/skips and all native parts for the journey receipt, too.
    // An absent report still fails; it can never become a zero-impact run.
    if (result.status !== 0 && !existsSync(report)) return result.status ?? 1;
    const raw = readFileSync(report, 'utf8');
    if (runner === 'playwright') {
      // Keep the report consumed by journey-bundles.py, even though this run
      // uses a fresh scratch report to reject stale/absent summaries.
      const dest = resolve(cwd, env.JOURNEY_OUTPUT ?? 'e2e/.results');
      // Playwright creates outputDir itself when tests execute.
      writeFileSync(join(dest, 'report.json'), raw);
    }
    if (result.status !== 0) return result.status ?? 1;
    const count = passedCount(runner, JSON.parse(raw));
    if (countFile) writeFileSync(countFile, `${count}\n`);
    console.log(`verified-web-tests: PASS (${count} ${runner} tests passed)`);
    return 0;
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    process.exitCode = runVerified(process.argv[2], process.argv.slice(3));
  } catch (error) {
    console.error(`verified-web-tests: FAIL: ${error.message}`);
    process.exitCode = 1;
  }
}
