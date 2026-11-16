import { readFileSync, writeFileSync } from 'node:fs';
import { expect, test } from 'vitest';

test('deterministic', () => expect(2 + 2).toBe(4));
test('seeded one in three', () => {
  const state = process.env.STABILITY_FIXTURE_STATE;
  let invocation = 0;
  try { invocation = Number(readFileSync(state, 'utf8')); } catch {}
  writeFileSync(state, String(++invocation));
  expect(invocation % 3).not.toBe(0);
});
