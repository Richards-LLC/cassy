import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import ts from 'typescript';
import { clockViolations } from './check-journey-clock.mjs';

test('rejects the real yesterday fixture defect and ambient Date aliases', () => {
  for (const code of [
    'const yesterday = new Date(Date.now() - 30 * 3_600_000).toISOString();',
    'const yesterday = new Date(); yesterday.setDate(yesterday.getDate() - 1);',
    'const now = Date.now; const at = now() - 30 * 3_600_000;',
    'const at = new globalThis.Date();',
    'const at = window.Date["now"]() - 30 * 3_600_000;',
    'const at = Date();',
    'await page.clock.install();',
  ]) assert.equal(clockViolations(code).length, 1, code);
});

test('accepts injected calendar days, explicit instants and real durations', () => {
  assert.deepEqual(clockViolations(`
    const yesterday = journeyDay(1);
    const skew = journeyStamp(300_000);
    const start = performance.now();
    const duration = performance.now() - start;
    const instant = new Date(JOURNEY_NOW);
    const parsed = Date.parse('2026-09-30T12:00:00Z');
    // Date.now() and new Date() are forbidden in code, not in this comment.
    const explanation = "new Date(Date.now() - 30 * 3600000)";
  `), []);
});

test('reports the offending file and source position', () => {
  assert.match(clockViolations('\nconst at = Date.now();', 'read-history.journey.ts')[0], /^read-history\.journey\.ts:2:12: ambient Date;/);
});

// Execute the actual clock module with controlled monotonic elapsed time.
// Ambient dates throw, so this also detects accidental wall-clock fallback.
function clock(instant) {
  let elapsed = 100;
  const exports = {};
  class ExplicitDate extends Date {
    constructor(...args) {
      assert.ok(args.length, 'clock module must never construct an ambient date');
      super(...args);
    }
    static now() { throw new Error('clock module read the system clock'); }
  }
  const source = readFileSync(new URL('../e2e/journeys/clock.ts', import.meta.url), 'utf8');
  const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS } }).outputText;
  runInNewContext(code, {
    exports, Date: ExplicitDate, process: { env: { HUB_JOURNEY_NOW: instant } },
    performance: { now: () => elapsed },
  });
  return { api: exports, advance: (ms) => { elapsed += ms; } };
}

test('calendar fixtures keep Today/Yesterday at both early-morning instants', () => {
  for (const instant of ['2026-09-30T00:30:00Z', '2026-09-30T05:59:00Z', undefined]) {
    const { api } = clock(instant);
    assert.equal(api.JOURNEY_TIMEZONE, 'UTC');
    assert.equal(api.journeyDay(0, 0, 1), '2026-09-30T00:01:00.000Z');
    assert.equal(api.journeyDay(1), '2026-09-29T12:00:00.000Z');
    assert.equal(api.journeyDay(2, 6, 36), '2026-09-28T06:36:00.000Z');
  }
  assert.equal(clock('2027-01-01T00:30:00Z').api.journeyDay(1), '2026-12-31T12:00:00.000Z');
});

test('protocol timestamps advance and reset for each test without the host clock', () => {
  const { api, advance } = clock('2026-09-30T05:59:00Z');
  api.startJourneyClock();
  advance(1000);
  assert.equal(api.journeyStamp(), '2026-09-30T05:59:01.000Z');
  assert.equal(api.journeyStamp(300_000), '2026-09-30T06:04:01.000Z');
  api.startJourneyClock();
  assert.equal(api.journeyStamp(), '2026-09-30T05:59:00.000Z');
});

test('rejects ambiguous clock overrides instead of reading host timezone', () => {
  for (const instant of ['', 'invalid', '2026-09-30T00:30:00', '2026-09-30T00:30:00-04:00']) {
    assert.throws(() => clock(instant), /HUB_JOURNEY_NOW must be an ISO UTC instant/);
  }
});
