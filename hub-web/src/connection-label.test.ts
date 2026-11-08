import { expect, it } from 'vitest';
import {
  BROWSER_BLOCKED,
  BROWSER_UNSUPPORTED,
  CANT_REACH_RETRYING,
  NEEDS_PAIRING,
  UNREACHABLE,
  machineConnectionLabel,
} from './connection-state';

const blocked = { networkAccessHelp: 'Allow Local network access' };

it('auth loss beats a browser block, on any visit history', () => {
  for (const everConnected of [true, false]) {
    expect(machineConnectionLabel({ phase: 'failed', degraded: false, authFailure: 'revoked', ...blocked }, everConnected)).toBe(NEEDS_PAIRING);
    expect(machineConnectionLabel({ phase: 'failed', degraded: false, authFailure: 'needs-pairing' }, everConnected)).toBe(NEEDS_PAIRING);
  }
  // cas-d043 G04: without an auth failure the browser is the cause.
  expect(machineConnectionLabel({ phase: 'backoff', degraded: false, ...blocked }, false)).toBe(BROWSER_BLOCKED);
  expect(machineConnectionLabel({ phase: 'failed', degraded: false, ...blocked }, true)).toBe(BROWSER_BLOCKED);
  expect(machineConnectionLabel({ phase: 'failed', degraded: false, fatal: true }, true)).toBe(BROWSER_UNSUPPORTED);
});

it('separates a never-live retry from a lost live connection', () => {
  // cas-b789: never live in this visit, retries continue.
  expect(machineConnectionLabel({ phase: 'backoff', degraded: false }, false)).toBe(CANT_REACH_RETRYING);
  expect(machineConnectionLabel({ phase: 'failed', degraded: false }, false)).toBe(CANT_REACH_RETRYING);
  // Live earlier in this visit: a retry is not "can't reach", a failure is unreachable.
  expect(machineConnectionLabel({ phase: 'backoff', degraded: false }, true)).not.toBe(CANT_REACH_RETRYING);
  expect(machineConnectionLabel({ phase: 'failed', degraded: false }, true)).toBe(UNREACHABLE);
});
