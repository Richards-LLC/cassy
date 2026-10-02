// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { CANT_REACH_RETRYING, machineFooterMarkup, type PairedMachineRow } from './paired-machines';
import { UNSTEADY } from './connection-state';

const atlas: PairedMachineRow = {
  id: 'atlas', label: 'Atlas', address: 'atlas.test', connection: 'Needs pairing',
  connected: false, everConnected: true, lastSeen: '',
  connectionState: { phase: 'failed', degraded: false, authFailure: 'revoked' },
};

function footerState(rows: PairedMachineRow[]): string {
  const footer = document.createElement('div');
  footer.innerHTML = machineFooterMarkup(rows, 1, 'test-build');
  return footer.querySelector('.machine-badge-state')!.textContent!;
}

describe('paired machine footer after authentication loss (cas-f698)', () => {
  it.each([true, false])('requests pairing for the only revoked machine, previously live: %s', (everConnected) => {
    expect(footerState([{ ...atlas, everConnected }])).toBe('Needs pairing');
  });

  it('requests pairing when every paired machine has lost authorization', () => {
    expect(footerState([atlas, { ...atlas, id: 'studio', label: 'Studio' }])).toBe('Needs pairing');
  });

  it('keeps reconnecting when the only machine has a transient outage', () => {
    expect(footerState([{ ...atlas, connection: 'Reconnecting', connectionState: { phase: 'backoff' as const, degraded: false } }])).toBe('Reconnecting');
  });

  it('keeps reconnecting for a mixed revoked and transient fleet, in either order', () => {
    const retrying = { ...atlas, id: 'studio', label: 'Studio', connection: 'Reconnecting', connectionState: { phase: 'backoff' as const, degraded: false } };
    expect(footerState([atlas, retrying])).toBe('Reconnecting');
    expect(footerState([retrying, atlas])).toBe('Reconnecting');
  });

  it('does not count a revoked machine as reconnecting while another never-live machine retries', () => {
    const retrying = { ...atlas, id: 'studio', connection: CANT_REACH_RETRYING, everConnected: false, connectionState: { phase: 'backoff' as const, degraded: false } };
    expect(footerState([atlas, retrying])).toBe(CANT_REACH_RETRYING);
  });

  it('keeps the connected count when a second machine needs pairing', () => {
    expect(footerState([atlas, { ...atlas, id: 'studio', connected: true, connection: 'Connected', connectionState: { phase: 'live', degraded: false } }])).toBe('1 connected');
  });
});

describe('an unsteady machine reads Unsteady in the footer, as in the header and row (cas-a6f0)', () => {
  const unsteady: PairedMachineRow = { ...atlas, connection: UNSTEADY, connectionState: { phase: 'live', degraded: true } };
  it('names the only machine unsteady, with the warning dot', () => {
    const footer = document.createElement('div');
    footer.innerHTML = machineFooterMarkup([unsteady], 1, 'test-build');
    expect(footer.querySelector('.machine-badge-state')!.textContent).toBe('Unsteady');
    expect(footer.querySelector('.pairing-dot')!.classList.contains('partial')).toBe(true);
  });
  it('says Reconnecting once another machine is actually down', () => {
    const retrying = { ...atlas, id: 'studio', connection: 'Reconnecting', connectionState: { phase: 'backoff' as const, degraded: false } };
    expect(footerState([unsteady, retrying])).toBe('Reconnecting');
  });
  it('keeps the connected count beside a connected machine', () => {
    expect(footerState([unsteady, { ...atlas, id: 'studio', connected: true, connection: 'Connected', connectionState: { phase: 'live', degraded: false } }])).toBe('1 connected');
  });
});
