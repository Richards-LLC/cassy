import { expect, it } from 'vitest';
import { machineConnectionLabel } from './connection-state';

it.each([
  ['revoked', { phase: 'failed', authFailure: 'revoked' }, 'Needs pairing'],
  // The transport maps unknown_key to the needs-pairing auth failure.
  ['unknown key', { phase: 'failed', authFailure: 'needs-pairing' }, 'Needs pairing'],
  ['transient after a live visit', { phase: 'backoff' }, 'Reconnecting'],
  ['connected', { phase: 'live' }, 'Live'],
  ['degraded', { phase: 'live', degraded: true }, 'Unsteady'],
  // cas-d043 G04: every fatal transport failure is this browser's, so it says so.
  ['fatal', { phase: 'failed', fatal: true }, "Browser can't connect"],
  ['transient failure after a live visit', { phase: 'failed' }, 'Unreachable'],
  ['blocked by a browser permission', { phase: 'backoff', networkAccessHelp: 'Allow Local network access' }, 'Blocked by browser'],
  ['first attempt', { phase: 'dialing' }, 'Connecting'],
] as const)('preserves the shared machine vocabulary for %s', (_case, state, expected) => {
  expect(machineConnectionLabel({ degraded: false, ...state }, true)).toBe(expected);
});

it('distinguishes a never-live retry from a first connection and auth loss', () => {
  expect(machineConnectionLabel({ phase: 'backoff', degraded: false }, false)).toBe("Can't reach · retrying");
  expect(machineConnectionLabel({ phase: 'failed', degraded: false, authFailure: 'revoked' }, false)).toBe('Needs pairing');
  expect(machineConnectionLabel(undefined)).toBe('Idle');
});

it('names the browser, not the machine, when a permission blocks a never-live machine (cas-d043 G04)', () => {
  expect(machineConnectionLabel({ phase: 'backoff', degraded: false, networkAccessHelp: 'Allow Local network access' }, false)).toBe('Blocked by browser');
  expect(machineConnectionLabel({ phase: 'failed', degraded: false, networkAccessHelp: 'Allow Local network access' }, false)).toBe('Blocked by browser');
  // Auth loss still needs pairing, whatever the browser says.
  expect(machineConnectionLabel({ phase: 'failed', degraded: false, authFailure: 'revoked', networkAccessHelp: 'x' }, false)).toBe('Needs pairing');
});
