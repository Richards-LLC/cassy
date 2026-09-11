import { cloudBrand, escapeHtml } from './cloud-brand';

export interface PairedMachineRow {
  id: string;
  label: string;
  address: string;
  connection: string;
  connected: boolean;
  lastSeen: string;
  runtime?: string;
}

export function machineFooterMarkup(rows: readonly PairedMachineRow[], sessions: number, build: string): string {
  const connected = rows.filter(row => row.connected).length;
  const machine = rows.length === 1 ? rows[0].label : `${rows.length} paired machines`;
  return `<button id="paired-machines-toggle" type="button" aria-haspopup="dialog"><span class="pairing-dot${connected ? ' connected' : ''}" aria-hidden="true"></span><span>${escapeHtml(machine)}</span><span class="machine-badge-state">${connected ? `${connected === rows.length ? 'Connected' : `${connected} connected`}` : rows.length ? 'Reconnecting' : 'Not paired'}</span></button><div class="hub-footer-meta"><span>${sessions} ${sessions === 1 ? 'conversation' : 'conversations'}</span><span title="Hub build">Hub ${escapeHtml(build)}</span></div>`;
}

export function pairedMachinesDialogMarkup(): string {
  return `<dialog id="paired-machines-dialog" aria-labelledby="paired-machines-title"><header class="machines-dialog-heading">${cloudBrand()}<button id="paired-machines-close" type="button" aria-label="Close paired machines">×</button></header><h2 id="paired-machines-title">Paired machines</h2><p class="machine-register-hint">Paired with this browser. Removing a machine here leaves its other devices connected.</p><div id="paired-machines-list"></div><p id="paired-machines-error" role="alert" hidden></p><button id="paired-machines-add" type="button">Pair a machine</button></dialog>`;
}

/** Keyed register: status ticks preserve focused controls and removal confirmation. */
export function renderPairedMachines(container: HTMLElement, rows: readonly PairedMachineRow[], remove: (id: string) => Promise<void>): void {
  const ids = new Set(rows.map(row => row.id));
  for (const node of container.querySelectorAll<HTMLElement>('[data-machine-id]')) if (!ids.has(node.dataset.machineId!)) node.remove();
  let empty = container.querySelector<HTMLElement>('.machine-register-empty');
  if (!rows.length && !empty) { empty = document.createElement('p'); empty.className = 'machine-register-empty'; empty.textContent = 'No machines paired with this browser.'; container.append(empty); }
  if (rows.length) empty?.remove();
  for (const row of rows) {
    let node = Array.from(container.children).find(node => (node as HTMLElement).dataset.machineId === row.id) as HTMLElement | undefined;
    if (!node) {
      node = document.createElement('section'); node.className = 'paired-machine'; node.dataset.machineId = row.id;
      node.innerHTML = '<h3></h3><p class="paired-machine-address"></p><p class="paired-machine-state"></p><p class="paired-machine-seen"></p><p class="paired-machine-runtime"></p><button type="button">Remove from this browser</button>';
      const button = node.querySelector('button')!;
      button.onclick = async () => {
        if (button.dataset.confirm !== 'true') { button.dataset.confirm = 'true'; button.textContent = 'Confirm removal'; return; }
        button.disabled = true;
        try { await remove(row.id); } finally { button.disabled = false; delete button.dataset.confirm; button.textContent = 'Remove from this browser'; }
      };
      button.onblur = () => { delete button.dataset.confirm; button.textContent = 'Remove from this browser'; };
      container.append(node);
    }
    const texts = { h3: row.label, '.paired-machine-address': row.address, '.paired-machine-state': row.connection, '.paired-machine-seen': row.lastSeen, '.paired-machine-runtime': row.runtime ? `Cassy ${row.runtime}` : 'Runtime not yet received' };
    for (const [selector, text] of Object.entries(texts)) { const target = node.querySelector(selector)!; if (target.textContent !== text) target.textContent = text; }
  }
}
