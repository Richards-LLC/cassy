import { machineFooterMarkup, pairedMachinesDialogMarkup, renderPairedMachines } from '../src/paired-machines';
import { ConversationList } from '../src/conversation-list';
import { ConversationHistory } from '../src/conversation-history';
import { ConversationView } from '../src/conversation-view';
import { conversationShellMarkup } from '../src/conversation-shell';
import type { GhosttyRow } from '../src/terminal/ghostty/core';

export function renderConversationFixture(app: HTMLElement, state: string): void {
  const supervisor = 'patient-pelican-9';
  const selected = !['conversations-list', 'paired-machines'].includes(state);
  app.innerHTML = conversationShellMarkup({ selected, supervisor, projectDir: '/projects/cas-src', host: 'Atlas · Linux', loaded: true, paired: true });
  new ConversationList().render(app.querySelector('#conversation-list')!, [
    { key: 'atlas:one', machineId: 'atlas', session: 'one', supervisor, projectDir: '/projects/cas-src', host: 'Atlas · Linux', freshness: 'Catalog checked just now', connection: 'Live', attention: 1, selected },
    { key: 'studio:two', machineId: 'studio', session: 'two', supervisor: 'calm-otter-4', projectDir: '/projects/gabber-studio', host: 'Studio Mac · macOS', freshness: 'Catalog checked 1m ago', connection: 'Live', attention: 0, selected: false },
  ], () => {});
  const machines = [{ id: 'atlas', label: 'Atlas · Linux', address: 'atlas.test', connection: 'Connected', connected: true, lastSeen: 'Last seen just now', runtime: '3.25.3' }, { id: 'studio', label: 'Studio Mac · macOS', address: 'studio.test', connection: 'Connected', connected: true, lastSeen: 'Last seen just now', runtime: '3.25.3' }];
  app.querySelector('#hub-footer-badges')!.innerHTML = machineFooterMarkup(machines, 2, 'fixture');
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
