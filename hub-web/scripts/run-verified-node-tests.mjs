#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const args = process.argv.slice(2);
const trial = process.env.CAS_WORKER_STABILITY_TRIAL === '1';
const countFile = process.env.VERIFIED_TEST_COUNT_FILE;
const reportFile = process.env.VERIFIED_TEST_REPORT_FILE;
if (!args.length) {
  console.error('verified-node-tests: FAIL: name at least one test file');
  process.exit(2);
}
if (countFile) {
  try { (await import('node:fs')).unlinkSync(countFile); } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
}

const hub = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const repo = resolve(hub, '..');
const report = trial ? spawnSync(process.execPath,
  ['--test', '--test-reporter=junit', ...args], { cwd: hub, encoding: 'utf8' }) :
  spawnSync(process.execPath, ['--test', ...args], { cwd: hub, stdio: 'inherit' });
if (report.error) throw report.error;
if (report.status !== 0) {
  if (trial && report.stdout) process.stdout.write(report.stdout);
  if (trial && report.stderr) process.stderr.write(report.stderr);
  process.exit(report.status ?? 1);
}

if (trial) {
  const passed = Number(report.stdout.match(/<!-- pass (\d+) -->/)?.[1] ?? 0);
  const failed = Number(report.stdout.match(/<!-- fail (\d+) -->/)?.[1] ?? 0);
  const skipped = Number(report.stdout.match(/<!-- skipped (\d+) -->/)?.[1] ?? 0);
  if (!passed || failed || skipped || !reportFile) {
    console.error('verified-node-tests: FAIL: missing, failed or skipped test summary');
    process.exit(1);
  }
  mkdirSync(dirname(resolve(reportFile)), { recursive: true });
  writeFileSync(resolve(reportFile), report.stdout);
  if (countFile) writeFileSync(resolve(countFile), `${passed}\n`);
  process.exit(0);
}

if (process.env.CAS_WORKER_STABILITY_TRIAL !== '1') {
  let scriptRoot = repo;
  while (scriptRoot !== resolve('/') && !existsSync(resolve(scriptRoot, 'scripts/worker-test-stability.py'))) {
    scriptRoot = resolve(scriptRoot, '..');
  }
  if (existsSync(resolve(scriptRoot, 'scripts/worker-test-stability.py'))) {
    const stability = spawnSync('python3', [resolve(scriptRoot, 'scripts/worker-test-stability.py'),
      '--run-adapter', 'node-test', '--', ...args], { cwd: scriptRoot, stdio: 'inherit' });
    if (stability.error) throw stability.error;
    if (stability.status !== 0) process.exit(stability.status ?? 1);
  } else if (process.env.CAS_FACTORY_MODE === '1') {
    throw new Error('cannot find worker stability adapter; no bound receipt');
  }
}
