// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { CANT_REACH_RETRYING, machineFooterMarkup, orderPairedMachines, renderPairedMachines, type PairedMachineRow } from './paired-machines';
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

  it('names the machine that needs pairing beside a connected one (cas-0739)', () => {
    expect(footerState([atlas, { ...atlas, id: 'studio', connected: true, connection: 'Connected', connectionState: { phase: 'live', degraded: false } }])).toBe('Atlas needs pairing');
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
  it('names the unsteady machine beside a connected one (cas-0739)', () => {
    expect(footerState([unsteady, { ...atlas, id: 'studio', connected: true, connection: 'Connected', connectionState: { phase: 'live', degraded: false } }])).toBe('Atlas unsteady');
  });
});

describe('the machine that is not connected comes first and the footer names it (cas-0739, journey F10)', () => {
  const live = (id: string, label: string): PairedMachineRow => ({ id, label, address: `${id}.test`, connection: 'Connected', connected: true, everConnected: true, lastSeen: '', connectionState: { phase: 'live', degraded: false } });
  const shed: PairedMachineRow = { id: 'shed', label: 'Shed NAS · Linux', address: 'shed.test', connection: CANT_REACH_RETRYING, connected: false, everConnected: false, lastSeen: '', connectionState: { phase: 'backoff', degraded: false } };
  const fleet = [live('atlas', 'Atlas · Linux'), live('studio', 'Studio Mac · macOS'), shed, live('forge', 'Forge · Linux')];

  it('names the one machine that cannot be reached, by its own name', () => {
    expect(footerState(fleet)).toBe("Shed NAS can't be reached");
  });

  it('names its state in words for each kind of outage', () => {
    const one = (connection: string) => footerState([live('atlas', 'Atlas · Linux'), { ...shed, connection }]);
    expect(one('Reconnecting')).toBe('Shed NAS reconnecting');
    expect(one('Needs pairing')).toBe('Shed NAS needs pairing');
    expect(one('Unreachable')).toBe("Shed NAS can't be reached");
    expect(one('Connecting…')).toBe('Shed NAS connecting');
  });

  it('counts several machines that are not connected', () => {
    expect(footerState([...fleet, { ...shed, id: 'attic', label: 'Attic · Linux', connection: 'Reconnecting' }])).toBe('2 not connected');
  });

  it('lists the machines that are not connected first, keeping each group in its order', () => {
    expect(orderPairedMachines(fleet).map((row) => row.id)).toEqual(['shed', 'atlas', 'studio', 'forge']);
    expect(orderPairedMachines([...fleet, { ...shed, id: 'attic' }]).map((row) => row.id)).toEqual(['shed', 'attic', 'atlas', 'studio', 'forge']);
  });

  it('moves rendered rows into that order only when asked, so an open list does not jump', () => {
    const container = document.createElement('div');
    renderPairedMachines(container, fleet, async () => undefined);
    const ids = () => [...container.querySelectorAll<HTMLElement>('[data-machine-id]')].map((node) => node.dataset.machineId);
    expect(ids()).toEqual(['atlas', 'studio', 'shed', 'forge']);
    renderPairedMachines(container, orderPairedMachines(fleet), async () => undefined, { reorder: false });
    expect(ids()).toEqual(['atlas', 'studio', 'shed', 'forge']);
    renderPairedMachines(container, orderPairedMachines(fleet), async () => undefined);
    expect(ids()).toEqual(['shed', 'atlas', 'studio', 'forge']);
  });
});
