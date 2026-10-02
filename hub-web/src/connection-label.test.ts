import { expect, it } from 'vitest';
import { machineConnectionLabel } from './connection-state';

it.each([
  ['revoked', { phase: 'failed', authFailure: 'revoked' }, 'Needs pairing'],
  // The transport maps unknown_key to the needs-pairing auth failure.
  ['unknown key', { phase: 'failed', authFailure: 'needs-pairing' }, 'Needs pairing'],
  ['transient after a live visit', { phase: 'backoff' }, 'Reconnecting'],
  ['connected', { phase: 'live' }, 'Live'],
  ['degraded', { phase: 'live', degraded: true }, 'Unsteady'],
  ['fatal', { phase: 'failed', fatal: true }, 'Unreachable'],
  ['first attempt', { phase: 'dialing' }, 'Connecting'],
] as const)('preserves the shared machine vocabulary for %s', (_case, state, expected) => {
  expect(machineConnectionLabel({ degraded: false, ...state }, true)).toBe(expected);
});

it('distinguishes a never-live retry from a first connection and auth loss', () => {
  expect(machineConnectionLabel({ phase: 'backoff', degraded: false }, false)).toBe("Can't reach · retrying");
  expect(machineConnectionLabel({ phase: 'failed', degraded: false, authFailure: 'revoked' }, false)).toBe('Needs pairing');
  expect(machineConnectionLabel(undefined)).toBe('Idle');
});
