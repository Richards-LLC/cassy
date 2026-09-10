// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { ConversationHistory } from "./conversation-history";
import { ConversationList, type ConversationRow } from "./conversation-list";
import { ConversationView } from "./conversation-view";
import { projectName, projectBadge } from "./cloud-brand";
import type { GhosttyRow } from "./terminal/ghostty/core";

const reply = { notification_id: 42, reply_to: 41, message: 'Actual supervisor reply <safe>', summary: '', device_id: 'device', operator_label: 'Daniel' };
describe('conversation evidence', () => {
  it('does not acknowledge socket submission, foreign references, or foreign targets', () => {
    const history = new ConversationHistory();
    history.submit('own', 'supervisor', 'Instruction');
    expect(history.events[0]).toMatchObject({ value: { state: 'sending' } });
    expect(history.acknowledge({ client_ref: 'foreign', notification_id: 41, target: 'supervisor', stamped: true })).toBe(false);
    expect(history.acknowledge({ client_ref: 'own', notification_id: 41, target: 'another', stamped: true })).toBe(false);
    expect(history.acknowledge({ client_ref: 'own', notification_id: 41, target: 'supervisor', stamped: true })).toBe(true);
    expect(history.events[0]).toMatchObject({ value: { state: 'acknowledged', notificationId: 41 } });
    history.reply(reply); history.reply(reply);
    expect(history.events).toHaveLength(2);
    expect(history.events[0]).toMatchObject({ value: { state: 'replied' } });
    history.acknowledge({ client_ref: 'own', notification_id: 41, target: 'supervisor', stamped: true });
    expect(history.events[0]).toMatchObject({ value: { state: 'replied' } });
  });
  it('correlates out-of-order replies and keeps rejection isolated', () => {
    const a = new ConversationHistory(), b = new ConversationHistory();
    a.submit('nonce', 'supervisor', 'A'); b.submit('nonce', 'supervisor', 'B');
    a.reply(reply);
    a.acknowledge({ client_ref: 'nonce', notification_id: 41, target: 'supervisor', stamped: true });
    expect(a.events[0]).toMatchObject({ value: { state: 'replied' } });
    expect(b.events[0]).toMatchObject({ value: { state: 'sending' } });
    b.reject('nonce', 'Permission refused');
    expect(b.events[0]).toMatchObject({ value: { state: 'error', error: 'Permission refused' } });
    expect(a.reject('nonce', 'late rejection')).toBe(false);
  });
  it('derives safe prominent project names without guessing', () => {
    expect(projectName('/home/Projects/cas-src/')).toBe('cas-src');
    expect(projectName('C:\\Projects\\Studio One\\')).toBe('Studio One');
    expect(projectName(undefined)).toBe('Project unavailable');
    expect(projectBadge('/home/<img>')).toContain('&lt;img&gt;');
  });
  it('preserves a focused row across catalog updates and isolates identical session names', () => {
    const list = new ConversationList(); const container = document.createElement('nav'); document.body.replaceChildren(container);
    const row: ConversationRow = { key: 'a:same', machineId: 'a', session: 'same', supervisor: 'same', host: 'Atlas', freshness: 'Catalog checked now', connection: 'Live', attention: 0, selected: false };
    list.render(container, [row, { ...row, key: 'b:same', machineId: 'b' }], vi.fn());
    const node = container.firstElementChild as HTMLButtonElement; node.focus();
    list.render(container, [{ ...row, freshness: 'Catalog checked 1m ago' }, { ...row, key: 'b:same', machineId: 'b' }], vi.fn());
    expect(document.activeElement).toBe(node); expect(container.children).toHaveLength(2);
  });
  it('renders exact real rows, escapes replies, and never presents the operator as the supervisor', () => {
    const color = { r: 200, g: 200, b: 200 };
    const text = 'Actual pane <script>\t — keep this text';
    const row = { cells: [...text].map(text => ({ text, wide: 0, foreground: color, background: color, bold: false, italic: false, invisible: false, strikethrough: false, overline: false, underline: false, selected: false })), text, isWrapContinuation: false, wrapsToNext: false } as GhosttyRow;
    const focus = vi.fn();
    const source = { rows: () => [row], theme: () => ({ foreground: color, background: color }), hasScrollbackAbove: () => false, scrollRows: vi.fn(), scrollToBottom: vi.fn(), focus };
    const history = new ConversationHistory();
    const view = new ConversationView(document, source, history, 'real-supervisor'); document.body.replaceChildren(view.element); view.update();
    expect(view.element.querySelector('.conversation-line')?.textContent).toBe(text);
    history.reply(reply); view.update();
    expect(view.element.querySelector('.from-supervisor strong')?.textContent).toBe('real-supervisor');
    expect(view.element.querySelector('.from-supervisor p')?.textContent).toBe(reply.message);
    expect(view.element.querySelector('script')).toBeNull();
    view.element.click(); expect(focus).not.toHaveBeenCalled();
    view.update(); expect(view.element.querySelectorAll('.conversation-pane')).toHaveLength(1);
  });
});

// The gateway can reject before the daemon accepts a message. These are a
// message outcome, not a reason to replace the supervisor's whole pane.
import { HubConnectionSupervisor, type HubCallbacks } from './connection';
import type { StoredMachine } from './types';
it('routes correlated daemon, legacy Hub and multiplex Hub rejections to the addressed send', async () => {
  const onMessageRejected = vi.fn(), onSocketError = vi.fn();
  const connection = new HubConnectionSupervisor({} as StoredMachine, { onMessageRejected, onSocketError } as unknown as HubCallbacks);
  const internals = connection as unknown as { handleDaemonObject(session: string, payload: unknown): void; handleMachineMessage(input: string): Promise<void> };
  internals.handleDaemonObject('a', { Error: { client_ref: 'daemon', message: 'Refused by daemon' } });
  internals.handleDaemonObject('a', { error: 'forbidden', client_ref: 'legacy' });
  await internals.handleMachineMessage(JSON.stringify({ channel: 'pty:b', error: { code: 'forbidden', client_ref: 'mux' } }));
  expect(onMessageRejected.mock.calls).toEqual([['a', 'daemon', 'Refused by daemon'], ['a', 'legacy', 'forbidden'], ['b', 'mux', 'forbidden']]);
  expect(onSocketError).not.toHaveBeenCalled();
});
