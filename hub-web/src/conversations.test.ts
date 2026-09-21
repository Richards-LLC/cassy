// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { ConversationHistory } from "./conversation-history";
import { ConversationList, type ConversationRow } from "./conversation-list";
import { ConversationView } from "./conversation-view";
import { conversationShellMarkup } from "./conversation-shell";
import { renderConversationFixture } from "../fixtures/conversations";
import { projectName, projectBadge } from "./cloud-brand";

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
  it('renders the Pebble row: machine accent on every row of a machine, waiting and unread as distinct affordances', () => {
    const list = new ConversationList(); const container = document.createElement('nav'); document.body.replaceChildren(container);
    const base = { session: 's', freshness: 'Catalog checked just now', connection: 'Live', attention: 0, unread: 0, selected: false };
    list.render(container, [
      { ...base, key: 'atlas-linux:a', machineId: 'atlas-linux', supervisor: 'patient-pelican-9', projectDir: '/projects/cas-src', host: 'Atlas · Linux', when: '09:58', preview: 'Fix <it>?', attention: 1, selected: true },
      { ...base, key: 'studio-mac:b', machineId: 'studio-mac', supervisor: 'calm-otter-4', projectDir: '/projects/gabber-studio', host: 'Studio Mac · macOS', when: 'Tue', preview: 'Pass two is green.', unread: 2 },
      { ...base, key: 'atlas-linux:c', machineId: 'atlas-linux', supervisor: 'steady-heron-2', projectDir: '/projects/petra-stella-cloud', host: 'Atlas · Linux', when: 'Tue' },
    ], vi.fn());
    const [waiting, unread, quiet] = [...container.children] as HTMLButtonElement[];
    expect(waiting.className).toBe('conversation-row machine-accent-0');
    expect(quiet.className).toBe('conversation-row machine-accent-0');
    expect(unread.className).toBe('conversation-row machine-accent-1');
    expect(waiting.querySelector('.conversation-avatar')?.textContent).toBe('A');
    expect(waiting.querySelector('.conversation-supervisor')?.textContent).toBe('patient-pelican-9');
    expect(waiting.querySelector('.project-badge')?.textContent).toBe('cas-src');
    // Separator and project travel together so a wrapped project never leaves the dot dangling.
    expect(waiting.querySelector('.conversation-project')?.innerHTML).toBe('<span class="conversation-sep" aria-hidden="true"></span><span class="project-badge">cas-src</span>');
    expect(waiting.querySelector('.conversation-who > .conversation-sep')).toBeNull();
    expect(waiting.querySelector('.conversation-preview')?.textContent).toBe('Fix <it>?');
    expect(waiting.querySelector('script, it')).toBeNull();
    expect(waiting.querySelector('.conversation-when')?.className).toBe('conversation-when hot');
    expect(waiting.querySelector('.conversation-flag')?.getAttribute('aria-label')).toBe('Waiting for you');
    expect(waiting.querySelector('.conversation-unread')).toBeNull();
    expect(waiting.dataset.waiting).toBe('true');
    expect(unread.querySelector('.conversation-unread')?.textContent).toBe('2');
    expect(unread.querySelector('.conversation-when')).toBeNull();
    expect(unread.querySelector('.conversation-flag')).toBeNull();
    expect(unread.querySelector('.conversation-preview')?.className).toBe('conversation-preview bold');
    expect(quiet.querySelector('.conversation-when')?.textContent).toBe('Tue');
    expect(quiet.querySelector('.conversation-preview')?.textContent).toBe('Live');
    expect(quiet.querySelector('.conversation-preview')?.className).toBe('conversation-preview');
    expect(quiet.querySelector('.conversation-flag, .conversation-unread')).toBeNull();
  });
  it('renders the fixture list with a footer count equal to the rows rendered', () => {
    const app = document.createElement('div'); document.body.replaceChildren(app);
    renderConversationFixture(app, 'conversations-list');
    const rows = app.querySelectorAll('.conversation-row').length;
    expect(rows).toBe(6);
    expect(app.querySelector('.hub-footer-meta span')?.textContent).toBe(`${rows} conversations`);
    expect(new Set([...app.querySelectorAll('.conversation-row')].map((row) => row.className)).size).toBe(3);
    expect([...app.querySelectorAll('.project-badge')].map((badge) => badge.textContent)).toContain('petra-stella-cloud');
  });
  it('puts the selected machine accent on the shell root and a compose FAB in the operator colour', () => {
    const shell = conversationShellMarkup({ selected: true, supervisor: 'patient-pelican-9', projectDir: '/projects/cas-src', host: 'Atlas · Linux', machineId: 'atlas-linux', loaded: true, paired: true });
    expect(shell).toContain('class="conversation-shell thread-open machine-accent-0"');
    expect(shell).toContain('id="compose-fab" class="compose-fab"');
    expect(conversationShellMarkup({ selected: false, loaded: true, paired: false })).toContain('class="conversation-shell">');
  });
  it('shows only turns — never pane text — escapes replies, and never presents the operator as the supervisor', () => {
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, { supervisor: 'real-supervisor', machine: 'Atlas', project: 'cas-src' }); document.body.replaceChildren(view.element); view.update();
    expect(view.element.querySelector('.thead b')?.textContent).toBe('real-supervisor');
    expect(view.element.querySelector('.thead .id span')?.textContent).toBe('Atlas · cas-src');
    expect(view.element.querySelector('.conversation-pane')).toBeNull();
    expect(view.element.querySelector('.msgs')?.children).toHaveLength(0);
    history.reply({ ...reply, message: 'Actual reply <script>alert(1)</script>' }); view.update();
    expect(view.element.querySelector('.turn.sup .bub p')?.textContent).toBe('Actual reply <script>alert(1)</script>');
    expect(view.element.querySelector('script')).toBeNull();
    expect(view.element.querySelector('.turn.you')).toBeNull();
    expect(view.element.textContent).not.toContain('Live pane text');
  });
  it('keeps typed supervisor turns and renders artifact link rows', () => {
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, 'real-supervisor'); document.body.replaceChildren(view.element);
    history.reply({
      notification_id: 44,
      reply_to: null,
      message: 'A report is ready.',
      summary: 'receipt',
      device_id: 'device',
      kind: 'receipt',
      attachments: [{ artifact_id: 'report/1', name: 'report.pdf', mime: 'application/pdf', size_bytes: 42, sha256: 'a'.repeat(64) }],
    });
    view.update();
    const turn = view.element.querySelector<HTMLElement>('.conversation-turn');
    expect(turn?.dataset.kind).toBe('receipt');
    expect(turn?.classList.contains('receipt')).toBe(true);
    expect(turn?.querySelector('.tick')).not.toBeNull();
    expect(turn?.querySelector('a')?.getAttribute('href')).toBe('#artifact:report%2F1');
    expect(turn?.querySelector('a')?.textContent).toBe('report.pdf');
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

it('retains a destination until a correlated reply or refusal settles each send', () => {
  const history = new ConversationHistory();
  expect(history.hasPending()).toBe(false);
  history.submit('pending', 'supervisor', 'instruction');
  expect(history.hasPending()).toBe(true);
  history.acknowledge({ client_ref: 'pending', target: 'supervisor', notification_id: 7, stamped: true });
  expect(history.hasPending()).toBe(true);
  history.reply({ notification_id: 8, reply_to: 7, message: 'done', summary: '', device_id: 'fixture' });
  expect(history.hasPending()).toBe(false);
  history.submit('refused', 'supervisor', 'instruction'); history.reject('refused', 'no access');
  expect(history.hasPending()).toBe(false);
});
