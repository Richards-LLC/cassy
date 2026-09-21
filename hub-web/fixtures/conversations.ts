import { machineFooterMarkup, pairedMachinesDialogMarkup, renderPairedMachines } from '../src/paired-machines';
import { ConversationList, type ConversationRow } from '../src/conversation-list';
import { ConversationHistory } from '../src/conversation-history';
import { ConversationView } from '../src/conversation-view';
import { conversationShellMarkup } from '../src/conversation-shell';
import type { GhosttyRow } from '../src/terminal/ghostty/core';

// The Pebble list from docs/design/hub-messaging/round-3/list.html: three
// machines across six projects, Atlas and Studio Mac each on two projects, the
// selected Atlas row waiting on the operator, Studio Mac carrying two unread.
// Machine ids are chosen so machine-accent.ts lands them on indigo, green and
// violet in that order (see machine-accent.test.ts).
export const FIXTURE_MACHINES = [
  { id: 'atlas-linux', label: 'Atlas · Linux', address: 'atlas.test', connection: 'Connected', connected: true, lastSeen: 'Last seen just now', runtime: '3.26.0' },
  { id: 'studio-mac', label: 'Studio Mac · macOS', address: 'studio.test', connection: 'Connected', connected: true, lastSeen: 'Last seen just now', runtime: '3.26.0' },
  { id: 'bench-1', label: 'Bench · Linux', address: 'bench.test', connection: 'Connected', connected: true, lastSeen: 'Last seen 2h ago', runtime: '3.26.0' },
];

export const FIXTURE_SUPERVISOR = 'patient-pelican-9';

export function fixtureConversationRows(selected: boolean): ConversationRow[] {
  const base = { freshness: 'Catalog checked just now', connection: 'Live', attention: 0, unread: 0, selected: false };
  return [
    { ...base, key: 'atlas-linux:one', machineId: 'atlas-linux', session: 'one', supervisor: FIXTURE_SUPERVISOR, projectDir: '/projects/cas-src', host: 'Atlas · Linux', when: '09:58', preview: 'Fix the warning in-train, or ship allowlisted?', attention: 1, selected },
    { ...base, key: 'studio-mac:two', machineId: 'studio-mac', session: 'two', supervisor: 'calm-otter-4', projectDir: '/projects/gabber-studio', host: 'Studio Mac · macOS', preview: 'Pass two is green — every pack in one place.', unread: 2 },
    { ...base, key: 'atlas-linux:three', machineId: 'atlas-linux', session: 'three', supervisor: 'steady-heron-2', projectDir: '/projects/petra-stella-cloud', host: 'Atlas · Linux', when: 'Tue', freshness: 'Catalog checked 1m ago', preview: 'Preview is up for the alias merge.' },
    { ...base, key: 'studio-mac:four', machineId: 'studio-mac', session: 'four', supervisor: 'quiet-marten-7', projectDir: '/projects/openclaw', host: 'Studio Mac · macOS', when: 'Tue', preview: 'Rebased and pushed; nothing waiting.' },
    { ...base, key: 'bench-1:five', machineId: 'bench-1', session: 'five', supervisor: 'bright-otter-3', projectDir: '/projects/mecha-cassy', host: 'Bench · Linux', when: 'Mon', freshness: 'Catalog checked 2h ago', preview: 'Posted both threads to the channel.' },
    { ...base, key: 'bench-1:six', machineId: 'bench-1', session: 'six', supervisor: 'calm-heron-5', projectDir: '/projects/cas-hub-static', host: 'Bench · Linux', when: 'Mon', freshness: 'Catalog checked 2h ago', preview: 'Nothing waiting on you.' },
  ];
}

export function renderConversationFixture(app: HTMLElement, state: string): void {
  const supervisor = FIXTURE_SUPERVISOR;
  const selected = !['conversations-list', 'paired-machines'].includes(state);
  app.innerHTML = conversationShellMarkup({ selected, supervisor, projectDir: '/projects/cas-src', host: 'Atlas · Linux', machineId: 'atlas-linux', loaded: true, paired: true });
  new ConversationList().render(app.querySelector('#conversation-list')!, fixtureConversationRows(selected), () => {});
  const machines = FIXTURE_MACHINES;
  app.querySelector('#hub-footer-badges')!.innerHTML = machineFooterMarkup(machines, machines.length, 'fixture');
  app.insertAdjacentHTML('beforeend', pairedMachinesDialogMarkup());
  renderPairedMachines(app.querySelector('#paired-machines-list')!, machines, async () => {});
  const dialog = app.querySelector<HTMLDialogElement>('#paired-machines-dialog')!;
  app.querySelector<HTMLElement>('#paired-machines-toggle')!.onclick = () => dialog.showModal();
  app.querySelector<HTMLElement>('#paired-machines-close')!.onclick = () => dialog.close();
  if (state === 'paired-machines') dialog.showModal();
  if (!selected) return;
  const history = new ConversationHistory();
  if (state !== 'conversation') {
    history.submit('fixture', supervisor, 'Please keep the project badge prominent.');
    if (state === 'conversation-error') history.reject('fixture', 'The session no longer grants this device control.');
    else {
      history.acknowledge({ client_ref: 'fixture', notification_id: 41, target: supervisor, stamped: true });
      history.reply({ notification_id: 42, reply_to: 41, message: 'The project badge stays visible in the list and conversation header.', summary: '', device_id: 'fixture', operator_label: 'Daniel' });
    }
  }
  const fg = { r: 232, g: 235, b: 242 }, bg = { r: 12, g: 14, b: 19 };
  const rows: GhosttyRow[] = ['The supervisor conversation is ready for review.', '', 'I kept the project visible while you read and reply.', '', '    const project = "cas-src";', '    const instruction = "Keep the real supervisor text.";', '', 'The focused checks passed. I am waiting for your direction.'].map(text => ({ text, wrapsToNext: false, isWrapContinuation: false, cells: [...text].map(text => ({ text, wide: 0, foreground: fg, background: bg, bold: false, italic: false, invisible: false, strikethrough: false, overline: false, underline: false, selected: false })) }));
  const view = new ConversationView(document, { rows: () => rows, theme: () => ({ foreground: fg, background: bg }), hasScrollbackAbove: () => false, scrollRows: () => {}, scrollToBottom: () => {}, focus: () => {} }, history, supervisor, () => {});
  app.querySelector('#conversation-pane-slot')!.append(view.element); view.update();
  app.querySelector('#conversation-composer-slot')!.innerHTML = `<div class="message conversation-composer"><h2><label for="message-text">Your message</label></h2><textarea id="message-text" placeholder="Write to ${supervisor}…"></textarea><div class="composer-actions"><button id="message-send" class="primary" type="button">Send to ${supervisor}</button></div></div>`;
  app.querySelector('#conversation-status-slot')!.innerHTML = '<p class="conversation-host">Supervisor conversations<br>In progress</p>';
  app.querySelector('#conversation-attention-slot')!.innerHTML = '<h2>Attention</h2><p class="conversation-host">One request needs your direction.</p>';
}
