import { cloudBrand, escapeHtml } from './cloud-brand';
import { CANT_REACH_RETRYING, NEEDS_PAIRING, UNREACHABLE, UNSTEADY, machineConnectionLabel, type MachineConnectionLabelState } from './connection-state';
import type { FleetControlGate } from './fleet-permissions';
import { commandTokensMarkup } from './launch-session';
import { FACTORY_MANAGE_CAPABILITY, FACTORY_OPERATE_CAPABILITY } from './pairing-scopes';
export { CANT_REACH_RETRYING } from './connection-state';

export interface PairedMachineRow {
  /**
   * cas-97d58 F16: why this browser, not the machine, is the problem (a
   * browser permission or a missing browser feature). Shown in place of the
   * bare connection word ("Unreachable"), which reads as the machine's fault.
   */
  cause?: string;
  id: string;
  label: string;
  address: string;
  connection: string;
  connected: boolean;
  /** Live at least once in this visit; until then a retry is still "Connecting". */
  everConnected?: boolean;
  connectionState?: MachineConnectionLabelState;
  lastSeen: string;
  runtime?: string;
  /**
   * Fleet permissions on this pairing (cas-d382): managing workers and tasks
   * (factory:operate) and stopping and restarting them (factory:manage).
   */
  fleet?: { readonly operate: FleetControlGate; readonly manage: FleetControlGate };
}

export interface PairedMachineActions {
  readonly installations?: (id: string) => void;
  /** The one-time "Allow managing workers" grant for this machine. */
  readonly allowManagingWorkers?: (id: string) => Promise<void>;
  /** Copy a pairing command. */
  readonly copy?: (text: string) => Promise<void> | void;
}

/** One permission line: what it allows, its state in words, and the way to get it when missing. */
function fleetPermissionMarkup(id: string, kind: 'operate' | 'manage', name: string, gate: FleetControlGate, label: string): string {
  const key = `${kind}-${id.replace(/[^a-z0-9_-]/gi, '_')}`;
  const state = gate.allowed ? 'Allowed' : gate.state;
  const head = `<p class="fleet-permission-head"><span class="fleet-permission-name">${escapeHtml(name)}</span> <span class="fleet-permission-state" id="fleet-state-${key}">${escapeHtml(state)}</span></p>`;
  if (gate.allowed) return `<div class="fleet-permission" data-permission="${kind}" data-allowed="true">${head}</div>`;
  const reason = `<p class="field-hint fleet-permission-reason" id="fleet-reason-${key}">${escapeHtml(gate.reason)}</p>`;
  // The control the permission unlocks reads as unavailable, and says why,
  // but it is never the only cue: the state and the way to get it are words.
  const control = kind === 'manage'
    ? `<button type="button" class="fleet-permission-control" aria-disabled="true" aria-describedby="fleet-state-${key} fleet-reason-${key}">Stop and restart</button>`
    : '';
  const grant = gate.grantable
    ? `<div class="fleet-permission-grant"><button type="button" class="fleet-permission-allow" data-fleet-allow="${escapeHtml(id)}" aria-describedby="fleet-reason-${key}">Allow managing workers</button></div>`
    : `<p class="field-hint fleet-permission-invite" id="fleet-invite-${key}">Run this on ${escapeHtml(label)} and open the link it prints in this browser:</p><div class="pair-code-actions fleet-permission-command"><code>${commandTokensMarkup(gate.command)}</code><button type="button" class="fleet-permission-copy" data-command="${escapeHtml(gate.command)}" aria-describedby="fleet-invite-${key}">Copy command</button></div>`;
  return `<div class="fleet-permission" data-permission="${kind}" data-allowed="false">${head}${reason}${control}${grant}</div>`;
}

/** The Fleet permissions block of one paired machine. */
export function fleetPermissionsMarkup(row: Pick<PairedMachineRow, 'id' | 'label' | 'fleet'>): string {
  if (!row.fleet) return '';
  return `<h4 class="fleet-permissions-title">Fleet permissions</h4>${fleetPermissionMarkup(row.id, 'operate', FACTORY_OPERATE_CAPABILITY, row.fleet.operate, row.label)}${fleetPermissionMarkup(row.id, 'manage', FACTORY_MANAGE_CAPABILITY, row.fleet.manage, row.label)}`;
}

/**
 * The footer badge. While browser storage is still loading there is nothing to
 * count, so it says so instead of "0 paired machines · Not paired"; a machine
 * never live in this visit is "Connecting…", not "Reconnecting" (journey F14).
 */
export function machineFooterMarkup(rows: readonly PairedMachineRow[], sessions: number, build: string, loading = false): string {
  const connected = rows.filter(row => row.connected).length;
  const machine = loading ? 'Paired machines' : rows.length === 1 ? rows[0].label : `${rows.length} paired machines`;
  const labels = rows.map(row => row.connectionState
    ? machineConnectionLabel(row.connectionState, row.everConnected ?? false)
    : row.connection);
  // Authorization loss needs a new pairing, so its live history must not make
  // another machine's first retry look like a reconnect (cas-f698).
  // Needs pairing and Unreachable (a failure that will not retry) never
  // reconnect on their own, so neither may make the footer say Reconnecting.
  const retryable = rows.filter((_row, index) => labels[index] !== NEEDS_PAIRING && labels[index] !== UNREACHABLE);
  const unreachable = retryable.length > 0 && retryable.every(row => row.connection === CANT_REACH_RETRYING);
  // cas-a6f0 (journey F8): machines still live with heartbeats unanswered
  // are unsteady, as the header and the row say, not reconnecting.
  const unsteady = retryable.length > 0 && labels.every(label => label === UNSTEADY || label === NEEDS_PAIRING);
  // cas-0739 (journey F10): with some machines connected and some not, name
  // the one that isn't ("Can't reach Shed NAS"), not "5 connected". The
  // outage words lead, so the phone footer's end ellipsis shortens the name
  // and never the verb ("Can't reach Build Server Ra…").
  const down = rows.filter(row => !row.connected);
  const partial = down.length === 1 ? `${outageWords(down[0]!.connection)} ${shortMachineName(down[0]!.label)}` : `${down.length} not connected`;
  const state = loading ? 'Loading…' : connected ? (connected === rows.length ? 'Connected' : partial)
    : !rows.length ? 'Not paired'
    : !retryable.length ? labels[0]
    : unsteady ? UNSTEADY
    : retryable.some(row => row.everConnected) ? 'Reconnecting'
    : unreachable ? CANT_REACH_RETRYING : 'Connecting…';
  // The dot shows the worst machine: green only when every machine is
  // connected, the warning tone when some are down (cas-b789) or unsteady.
  const dot = connected && connected === rows.length ? ' connected' : connected || state === UNSTEADY ? ' partial' : '';
  return `<button id="paired-machines-toggle" type="button" aria-haspopup="dialog"><span class="pairing-dot${dot}" aria-hidden="true"></span><span title="${escapeHtml(machine)}">${escapeHtml(machine)}</span><span class="machine-badge-state${/\s/.test(state.trim()) ? '' : ' whole'}" title="${escapeHtml(state)}">${escapeHtml(state)}</span></button><div class="hub-footer-meta"><span>${sessions} ${sessions === 1 ? 'conversation' : 'conversations'}</span><span title="Hub build">Hub ${escapeHtml(build)}</span></div>`;
}

/** "Shed NAS · Linux" reads "Shed NAS" where room is short. */
function shortMachineName(label: string): string {
  return label.split(' · ')[0]?.trim() || label.trim();
}

/** A machine's connection in the footer's words, put before its name: "Can't reach", "Reconnecting to", "Needs pairing:" (cas-0739). */
function outageWords(connection: string): string {
  if (connection === CANT_REACH_RETRYING || connection === UNREACHABLE) return "Can't reach";
  if (connection === NEEDS_PAIRING) return 'Needs pairing:';
  if (connection === UNSTEADY) return 'Unsteady:';
  if (connection === 'Reconnecting') return 'Reconnecting to';
  if (connection.startsWith('Connecting') || connection === 'Idle') return 'Connecting to';
  return `${connection}:`;
}

/**
 * The register lists machines that aren't connected first, each group in its
 * own order, so the one the footer names is on screen when the dialog opens
 * (cas-0739, journey F10).
 */
export function orderPairedMachines<T extends Pick<PairedMachineRow, 'connected'>>(rows: readonly T[]): T[] {
  return [...rows.filter(row => !row.connected), ...rows.filter(row => row.connected)];
}

export function pairedMachinesDialogMarkup(): string {
  return `<dialog id="paired-machines-dialog" aria-labelledby="paired-machines-title"><header class="machines-dialog-heading">${cloudBrand()}<button id="paired-machines-close" type="button" aria-label="Close paired machines">×</button></header><h2 id="paired-machines-title">Paired machines</h2><p class="machine-register-hint">Paired with this browser. Removing a machine here leaves its other devices connected.</p><div id="paired-machines-list"></div><p id="paired-machines-error" role="alert" hidden></p><button id="paired-machines-add" type="button">Pair a machine</button></dialog>`;
}

/** Keyed register: status ticks preserve focused controls and removal confirmation. */
export function renderPairedMachines(container: HTMLElement, rows: readonly PairedMachineRow[], remove: (id: string) => Promise<void>, options: { reorder?: boolean } & PairedMachineActions = {}): void {
  const ids = new Set(rows.map(row => row.id));
  for (const node of container.querySelectorAll<HTMLElement>('[data-machine-id]')) if (!ids.has(node.dataset.machineId!)) node.remove();
  let empty = container.querySelector<HTMLElement>('.machine-register-empty');
  if (!rows.length && !empty) { empty = document.createElement('p'); empty.className = 'machine-register-empty'; empty.textContent = 'No machines paired with this browser.'; container.append(empty); }
  if (rows.length) empty?.remove();
  for (const row of rows) {
    let node = Array.from(container.children).find(node => (node as HTMLElement).dataset.machineId === row.id) as HTMLElement | undefined;
    if (!node) {
      node = document.createElement('section'); node.className = 'paired-machine'; node.dataset.machineId = row.id;
      node.innerHTML = '<h3></h3><p class="paired-machine-address"></p><p class="paired-machine-state"></p><p class="paired-machine-seen"></p><p class="paired-machine-runtime"></p><section class="paired-machine-fleet" hidden></section><button type="button" class="paired-machine-remove">Remove from this browser</button>';
      const button = node.querySelector<HTMLButtonElement>('.paired-machine-remove')!;
      button.onclick = async () => {
        if (button.dataset.confirm !== 'true') { button.dataset.confirm = 'true'; button.textContent = 'Confirm removal'; return; }
        button.disabled = true;
        try { await remove(row.id); } finally { button.disabled = false; delete button.dataset.confirm; button.textContent = 'Remove from this browser'; }
      };
      button.onblur = () => { delete button.dataset.confirm; button.textContent = 'Remove from this browser'; };
      container.append(node);
    }
    const texts = { h3: row.label, '.paired-machine-address': row.address, '.paired-machine-state': row.cause ?? row.connection, '.paired-machine-seen': row.lastSeen, '.paired-machine-runtime': row.runtime ? `Cassy ${row.runtime}` : 'Version unknown until it connects' };
    for (const [selector, text] of Object.entries(texts)) { const target = node.querySelector(selector)!; if (target.textContent !== text) target.textContent = text; }
    renderFleetPermissions(node, row, options);
    if (options.installations) {
      let inventory = node.querySelector<HTMLButtonElement>('.paired-machine-installations');
      if (!inventory) {
        inventory = document.createElement('button'); inventory.type = 'button';
        inventory.className = 'paired-machine-installations';
        node.insertBefore(inventory, node.querySelector('.paired-machine-remove'));
      }
      inventory.textContent = `Browser installations on ${row.label}`;
      inventory.onclick = () => options.installations?.(row.id);
    }
  }
  // cas-0739: put rows in the given order. An open register passes
  // reorder: false so a status tick never moves a row under the operator's
  // finger or focus; it is ordered again before it next opens.
  if (options.reorder === false) return;
  const current = [...container.querySelectorAll<HTMLElement>('[data-machine-id]')];
  const nodes = rows.map(row => current.find(node => node.dataset.machineId === row.id)).filter((node): node is HTMLElement => Boolean(node));
  if (nodes.every((node, index) => current[index] === node)) return;
  const anchor = container.querySelector('.machine-register-empty');
  for (const node of nodes) container.insertBefore(node, anchor);
}

/**
 * Fill a machine's Fleet permissions block (cas-d382). Rebuilt only when what
 * it says changes, so a status tick never drops focus from its buttons.
 */
function renderFleetPermissions(node: HTMLElement, row: PairedMachineRow, actions: PairedMachineActions): void {
  const block = node.querySelector<HTMLElement>('.paired-machine-fleet');
  if (!block) return;
  const markup = fleetPermissionsMarkup(row);
  block.hidden = markup === '';
  if (block.dataset.markup === markup) return;
  block.dataset.markup = markup;
  block.setAttribute('aria-label', `Fleet permissions on ${row.label}`);
  block.innerHTML = markup;
  for (const copy of block.querySelectorAll<HTMLButtonElement>('.fleet-permission-copy')) {
    copy.onclick = () => { void actions.copy?.(copy.dataset.command ?? ''); copy.textContent = 'Copied'; };
  }
  const allow = block.querySelector<HTMLButtonElement>('.fleet-permission-allow');
  if (allow) {
    // Two deliberate presses, as Remove asks: the first says what it allows.
    allow.onclick = async () => {
      if (allow.dataset.confirm !== 'true') { allow.dataset.confirm = 'true'; allow.textContent = `Confirm: allow managing workers on ${row.label}`; return; }
      allow.disabled = true;
      try { await actions.allowManagingWorkers?.(row.id); } finally { allow.disabled = false; delete allow.dataset.confirm; allow.textContent = 'Allow managing workers'; }
    };
    allow.onblur = () => { delete allow.dataset.confirm; allow.textContent = 'Allow managing workers'; };
  }
  for (const control of block.querySelectorAll<HTMLButtonElement>('.fleet-permission-control')) {
    // Unavailable but focusable, so its description is reachable; a press does nothing.
    control.onclick = (event) => event.preventDefault();
  }
}
