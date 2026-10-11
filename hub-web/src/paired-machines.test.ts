// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest';
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { CANT_REACH_RETRYING, machineFooterMarkup, orderPairedMachines, renderPairedMachines, type PairedMachineRow } from './paired-machines';
import { UNSTEADY } from './connection-state';
import { fleetControlGate } from './fleet-permissions';
import type { Scope } from './types';

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

  it('says the browser blocks or cannot connect, not "Can\'t reach" (cas-d043 G04)', () => {
    const blocked = { ...atlas, connection: 'Blocked by browser', everConnected: false, connectionState: { phase: 'backoff' as const, degraded: false, networkAccessHelp: 'Allow Local network access' } };
    expect(footerState([blocked])).toBe('Blocked by browser');
    const unsupported = { ...atlas, connection: "Browser can't connect", connectionState: { phase: 'failed' as const, degraded: false, fatal: true } };
    expect(footerState([unsupported])).toBe("Browser can't connect");
    expect(footerState([unsupported, { ...atlas, id: 'studio', connected: true, connection: 'Connected', connectionState: { phase: 'live', degraded: false } }])).toBe("Browser can't connect to Atlas");
    expect(footerState([blocked, { ...atlas, id: 'studio', connected: true, connection: 'Connected', connectionState: { phase: 'live', degraded: false } }])).toBe('Browser is blocking Atlas');
  });

  it('names the machine that needs pairing beside a connected one (cas-0739)', () => {
    expect(footerState([atlas, { ...atlas, id: 'studio', connected: true, connection: 'Connected', connectionState: { phase: 'live', degraded: false } }])).toBe('Needs pairing: Atlas');
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
    expect(footerState([unsteady, { ...atlas, id: 'studio', connected: true, connection: 'Connected', connectionState: { phase: 'live', degraded: false } }])).toBe('Unsteady: Atlas');
  });
});

describe('the machine that is not connected comes first and the footer names it (cas-0739, journey F10)', () => {
  const live = (id: string, label: string): PairedMachineRow => ({ id, label, address: `${id}.test`, connection: 'Connected', connected: true, everConnected: true, lastSeen: '', connectionState: { phase: 'live', degraded: false } });
  const shed: PairedMachineRow = { id: 'shed', label: 'Shed NAS · Linux', address: 'shed.test', connection: CANT_REACH_RETRYING, connected: false, everConnected: false, lastSeen: '', connectionState: { phase: 'backoff', degraded: false } };
  const fleet = [live('atlas', 'Atlas · Linux'), live('studio', 'Studio Mac · macOS'), shed, live('forge', 'Forge · Linux')];

  it('names the one machine that cannot be reached, by its own name', () => {
    expect(footerState(fleet)).toBe("Can't reach Shed NAS");
  });

  // cas-cae9: the outage words lead and the name follows, so the phone
  // footer's end ellipsis shortens the name, never the verb.
  it('names its state in words for each kind of outage, before the name', () => {
    const one = (connection: string) => footerState([live('atlas', 'Atlas · Linux'), { ...shed, connection }]);
    expect(one('Reconnecting')).toBe('Reconnecting to Shed NAS');
    expect(one('Needs pairing')).toBe('Needs pairing: Shed NAS');
    expect(one('Unreachable')).toBe("Can't reach Shed NAS");
    expect(one(CANT_REACH_RETRYING)).toBe("Can't reach Shed NAS");
    expect(one('Connecting…')).toBe('Connecting to Shed NAS');
    expect(one('Idle')).toBe('Connecting to Shed NAS');
    expect(one(UNSTEADY)).toBe('Unsteady: Shed NAS');
  });

  it('keeps a long name whole in the title and safe as text (cas-0739 QA round 1)', () => {
    const footer = document.createElement('div');
    const rack = { ...shed, label: 'Build Server Rack Seven Downstairs <b>x</b> · Windows' };
    footer.innerHTML = machineFooterMarkup([live('atlas', 'Atlas · Linux'), rack], 1, 'test-build');
    const state = footer.querySelector<HTMLElement>('.machine-badge-state')!;
    expect(state.textContent).toBe("Can't reach Build Server Rack Seven Downstairs <b>x</b>");
    expect(state.title).toBe(state.textContent);
    expect(state.querySelector('b')).toBeNull();
    // The label and the state are separate spans, so CSS can let the state yield.
    expect([...footer.querySelector('#paired-machines-toggle')!.children].map((child) => child.className)).toEqual(['pairing-dot partial', '', 'machine-badge-state']);
  });

  it('keeps a one-word footer state whole, so the label yields instead of "Conne…" (cas-97d58 F07)', () => {
    const footer = document.createElement('div');
    footer.innerHTML = machineFooterMarkup([live('atlas', 'Atlas · Linux')], 1, 'test-build');
    expect(footer.querySelector('.machine-badge-state')?.className).toBe('machine-badge-state whole');
    const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'styles.css'), 'utf8');
    expect(css).toContain('.conversation-sidebar #paired-machines-toggle .machine-badge-state.whole { flex: none; min-width: auto; }');
  });

  it('counts several machines that are not connected', () => {
    expect(footerState([...fleet, { ...shed, id: 'attic', label: 'Attic · Linux', connection: 'Reconnecting' }])).toBe('2 not connected');
  });

  it('lists the machines that are not connected first, keeping each group in its order', () => {
    expect(orderPairedMachines(fleet).map((row) => row.id)).toEqual(['shed', 'atlas', 'studio', 'forge']);
    expect(orderPairedMachines([...fleet, { ...shed, id: 'attic' }]).map((row) => row.id)).toEqual(['shed', 'attic', 'atlas', 'studio', 'forge']);
  });

  it('names a browser-side cause on the row instead of a bare Unreachable (cas-97d58 F16)', () => {
    const container = document.createElement('div');
    const blocked = { ...shed, connection: 'Unreachable', cause: "This browser is blocking the connection. Allow Local network access for this site in the browser's settings." };
    renderPairedMachines(container, [blocked], async () => undefined);
    expect(container.querySelector('.paired-machine-state')?.textContent).toBe(blocked.cause);
    renderPairedMachines(container, [{ ...shed, connection: 'Unreachable' }], async () => undefined);
    expect(container.querySelector('.paired-machine-state')?.textContent).toBe('Unreachable');
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

describe('an overlong machine label on the phone footer (cas-c19d)', () => {
  const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'styles.css'), 'utf8');
  const name = 'soundwave — a very long personal workstation name with several extra words and anunbrokentailthatneedstowrap';
  const live = (id: string, label: string): PairedMachineRow => ({ id, label, address: `${id}.test`, connection: 'Connected', connected: true, everConnected: true, lastSeen: '', connectionState: { phase: 'live', degraded: false } });

  it('carries its full name as a title, since the phone footer may ellipsise it', () => {
    const footer = document.createElement('div');
    footer.innerHTML = machineFooterMarkup([live('atlas', name)], 1, 'test-build');
    const label = footer.querySelector('#paired-machines-toggle')!.children[1] as HTMLElement;
    expect(label.textContent).toBe(name);
    expect(label.title).toBe(name);
  });

  it('lets the label shrink and ellipsise after the state has yielded, so the two never overlap', () => {
    const rule = (selector: string) => {
      const at = css.indexOf(`${selector} {`);
      return at < 0 ? '' : css.slice(at, css.indexOf('}', at));
    };
    const label = rule('  .conversation-sidebar #paired-machines-toggle > span:nth-child(2)');
    expect(label).toContain('flex: 0 1 auto');
    expect(label).toContain('min-width: 0');
    expect(label).toContain('text-overflow: ellipsis');
    expect(label).toContain('overflow: hidden');
    // The state still gives way first: its shrink weight dwarfs the label's.
    const state = rule('  .conversation-sidebar #paired-machines-toggle .machine-badge-state');
    expect(state).toContain('flex: 1 1000 auto');
    // ...but never below a 4em stub, so an overlong name cannot hide it entirely.
    expect(state).toContain('min-width: 4em');
  });
});

describe('fleet permissions on a paired machine (cas-d382)', () => {
  const ORIGIN = 'https://commander.example';
  const CONTROL: Scope[] = ['machine-read', 'session-read', 'pane-read', 'pane-input', 'message-send', 'pane-interrupt'];
  const row = (scopes: Scope[]): PairedMachineRow => ({
    ...atlas, connection: 'Connected', connected: true,
    fleet: { operate: fleetControlGate(scopes, 'add-workers', ORIGIN), manage: fleetControlGate(scopes, 'stop-worker', ORIGIN) },
  });

  it('labels a missing permission in words, with the command beside it, never only a disabled control', async () => {
    const container = document.createElement('div'); document.body.replaceChildren(container);
    const copy = vi.fn();
    const allow = vi.fn(async () => undefined);
    renderPairedMachines(container, [row(CONTROL)], async () => undefined, { copy, allowManagingWorkers: allow });
    const block = container.querySelector<HTMLElement>('.paired-machine-fleet')!;
    expect(block.hidden).toBe(false);
    expect(block.getAttribute('aria-label')).toBe('Fleet permissions on Atlas');
    const manage = block.querySelector<HTMLElement>('[data-permission="manage"]')!;
    expect(manage.querySelector('.fleet-permission-name')?.textContent).toBe('Stop and restart workers and sessions');
    // cas-a217: the list says which permission allows write grants.
    expect(manage.querySelector('.fleet-permission-also')?.textContent).toBe('Also needed to grant a task write access outside its worktree.');
    expect(block.querySelector('[data-permission="operate"] .fleet-permission-also')).toBeNull();
    expect(manage.querySelector('.fleet-permission-state')?.textContent).toBe('Not allowed on this pairing');
    const stop = manage.querySelector<HTMLButtonElement>('.fleet-permission-control')!;
    expect(stop.getAttribute('aria-disabled')).toBe('true');
    expect(stop.disabled).toBe(false); // focusable, so its description is reachable
    expect(stop.getAttribute('aria-describedby')?.split(' ').map((id) => document.getElementById(id)?.textContent)).toEqual([
      'Not allowed on this pairing', 'Needs the Stop and restart workers and sessions permission.',
    ]);
    const command = 'cas hub pair --origin https://commander.example --scopes machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt,factory:manage';
    expect(manage.querySelector('code')?.textContent).toBe(command);
    manage.querySelector<HTMLButtonElement>('.fleet-permission-copy')!.click();
    expect(copy).toHaveBeenCalledWith(command);
    // A control pairing may allow managing workers itself: two presses, then the grant.
    const operate = block.querySelector<HTMLElement>('[data-permission="operate"]')!;
    expect(operate.querySelector('.fleet-permission-state')?.textContent).toBe('Not allowed on this pairing');
    const button = operate.querySelector<HTMLButtonElement>('.fleet-permission-allow')!;
    expect(operate.querySelector('code')).toBeNull();
    await button.onclick!(new MouseEvent('click') as PointerEvent);
    expect(button.textContent).toBe('Confirm: allow managing workers on Atlas');
    expect(allow).not.toHaveBeenCalled();
    await button.onclick!(new MouseEvent('click') as PointerEvent);
    expect(allow).toHaveBeenCalledWith('atlas');
  });

  it('says Allowed when the pairing holds it, and keeps focus across a status tick', () => {
    const container = document.createElement('div'); document.body.replaceChildren(container);
    renderPairedMachines(container, [row([...CONTROL, 'factory-operate', 'factory-manage'])], async () => undefined);
    const states = [...container.querySelectorAll('.fleet-permission-state')].map((node) => node.textContent);
    expect(states).toEqual(['Allowed', 'Allowed']);
    expect(container.querySelector('.fleet-permission-control, .fleet-permission-allow, .fleet-permission-copy')).toBeNull();
    renderPairedMachines(container, [row(CONTROL)], async () => undefined);
    const copy = container.querySelector<HTMLButtonElement>('.fleet-permission-copy')!;
    copy.focus();
    renderPairedMachines(container, [{ ...row(CONTROL), lastSeen: 'Last seen now' }], async () => undefined);
    expect(document.activeElement).toBe(copy);
    // A read-only pairing cannot allow it itself: the command instead.
    renderPairedMachines(container, [row(['machine-read', 'session-read', 'pane-read'])], async () => undefined);
    const operate = container.querySelector<HTMLElement>('[data-permission="operate"]')!;
    expect(operate.querySelector('.fleet-permission-allow')).toBeNull();
    expect(operate.querySelector('code')?.textContent).toBe('cas hub pair --origin https://commander.example --scopes machine:read,session:read,pane:read,factory:operate');
  });
});
