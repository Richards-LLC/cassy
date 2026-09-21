// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { ConversationHistory } from "./conversation-history";
import { ConversationList, type ConversationRow } from "./conversation-list";
import { ConversationView } from "./conversation-view";
import { ATTACH_DISABLED_REASON, arrangeConversationShell, conversationShellMarkup, dressComposer } from "./conversation-shell";
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
  it('tracks asks and blockers waiting on the operator and the send that answers an ask (cas-43f9)', () => {
    const history = new ConversationHistory();
    const turn = (notification_id: number, kind: 'ask' | 'blocker' | 'answer', message = `m${notification_id}`) => ({ notification_id, reply_to: null, message, summary: '', device_id: 'd', kind });
    history.reply(turn(1, 'answer'));
    history.reply(turn(2, 'blocker', 'Gate red.'));
    history.reply(turn(3, 'ask', 'Fix or ship?'));
    history.reply(turn(4, 'ask', 'Tag it too?'));
    expect(history.waiting().map((item) => item.notification_id)).toEqual([2, 3, 4]);
    expect(history.pinnedAsk()?.notification_id).toBe(4);
    expect(history.answered(4)).toBeUndefined();
    history.submit('q', 'supervisor', 'Yes, go ahead', Date.now(), 4);
    expect(history.events.at(-1)).toMatchObject({ value: { replyTo: 4, state: 'sending' } });
    expect(history.answered(4)).toMatchObject({ id: 'q', text: 'Yes, go ahead' });
    // The send after the blocker acknowledges it; the other ask stays pinned until a send carries its id.
    expect(history.waiting().map((item) => item.notification_id)).toEqual([3]);
    expect(history.pinnedAsk()?.notification_id).toBe(3);
    history.submit('free', 'supervisor', 'Ship it', Date.now(), 3);
    expect(history.waiting()).toEqual([]); expect(history.pinnedAsk()).toBeUndefined();
    history.submit('plain', 'supervisor', 'No reference');
    expect(history.events.at(-1)).not.toHaveProperty('value.replyTo');
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
    // The machine is named as text on every row, after the project, with its own wrapping separator.
    expect(waiting.querySelector('.conversation-machine')?.textContent).toBe('Atlas');
    expect(waiting.querySelector('.conversation-machine')?.innerHTML).toBe('<span class="conversation-sep" aria-hidden="true"></span>Atlas');
    expect(unread.querySelector('.conversation-machine')?.textContent).toBe('Studio Mac');
    expect(waiting.querySelector('.conversation-who')?.textContent).toBe('patient-pelican-9cas-srcAtlas');
    expect(waiting.querySelector('.conversation-who > .conversation-meta > .conversation-project + .conversation-machine')).not.toBeNull();
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
  it('renders one Pebble header above the thread with the back link, Terminal view, avatar, badge and connection slot', () => {
    const shell = document.createElement('div');
    shell.innerHTML = conversationShellMarkup({ selected: true, supervisor: 'patient-pelican-9', projectDir: '/projects/cas-src', host: 'Atlas · Linux', machineId: 'atlas-linux', loaded: true, paired: true });
    const main = shell.querySelector('.conversation-main')!;
    expect(main.querySelectorAll('header')).toHaveLength(1);
    const header = main.querySelector('header.conversation-heading.thead')!;
    expect(header.querySelector('#conversation-back')?.textContent).toBe('‹ Conversations');
    expect(header.querySelector('#conversation-terminal')?.textContent).toBe('Terminal view');
    expect(header.querySelector('.conversation-avatar')?.textContent).toBe('A');
    expect(header.querySelector('h1 b')?.textContent).toBe('patient-pelican-9');
    expect(header.querySelector('h1 .project-badge')?.textContent).toBe('cas-src');
    expect(header.querySelector('.conversation-host')?.textContent).toBe('cas-src · Atlas · Linux');
    expect(header.querySelector('#conversation-connection')).not.toBeNull();
    expect(main.querySelector('.conversation-identity h1')).not.toBeNull(); expect(main.querySelectorAll('h1')).toHaveLength(1);
    const view = new ConversationView(document, new ConversationHistory(), { supervisor: 'patient-pelican-9', machine: 'Atlas', project: 'cas-src', header: false });
    shell.querySelector('#conversation-pane-slot')!.append(view.element);
    expect(shell.querySelectorAll('.thead')).toHaveLength(1);
  });
  it('dresses the composer as Pebble: pill field, disabled attach clip with its reason, send in the accent naming the supervisor', () => {
    const app = document.createElement('div');
    app.innerHTML = '<div class="shell"><div id="pane-grid"></div><div class="message"><h2><label for="message-text">Talk to x</label></h2><div class="operator-thread"></div><textarea id="message-text" placeholder="old"></textarea><div class="composer-actions"><button id="message-keyboard" type="button">Keyboard</button><button id="message-send" class="primary">Send message</button></div><p id="message-status" class="message-status" role="status" hidden></p></div><div id="status-view"></div><section id="attention-panel" hidden></section></div>';
    arrangeConversationShell(app, { selected: true, supervisor: 'patient-pelican-9', machineId: 'atlas-linux', loaded: true, paired: true });
    const composer = app.querySelector<HTMLElement>('#conversation-composer-slot > .message.conversation-composer')!;
    expect(composer).not.toBeNull();
    expect(composer.querySelector('.operator-thread')).toBeNull();
    const clip = composer.querySelector<HTMLButtonElement>('.composer-clip')!;
    expect(clip.disabled).toBe(true); expect(clip.title).toBe(ATTACH_DISABLED_REASON);
    expect(clip.nextElementSibling?.id).toBe('message-text');
    expect(composer.querySelector<HTMLTextAreaElement>('#message-text')?.placeholder).toBe('Message patient-pelican-9');
    const send = composer.querySelector<HTMLButtonElement>('#message-send')!;
    expect(send.classList.contains('send')).toBe(true);
    expect(send.textContent).toBe('Send to patient-pelican-9');
    expect(send.querySelector('.send-glyph')).not.toBeNull();
    expect(app.querySelector('.conversation-shell')?.classList.contains('machine-accent-0')).toBe(true);
    // Dressing twice (every re-render) never stacks a second clip or label.
    dressComposer(composer, 'patient-pelican-9');
    expect(composer.querySelectorAll('.composer-clip')).toHaveLength(1); expect(send.querySelectorAll('.send-label')).toHaveLength(1);
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
  it('keeps typed supervisor turns and lays the artifact on the thread as a sheet beside the bubble', () => {
    // The fixture module imported above installs the Pebble 4 sheet, as main.ts does.
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
    expect(turn?.querySelector('a')).toBeNull();
    const sheet = turn?.nextElementSibling as HTMLAnchorElement | null;
    expect(sheet?.classList.contains('sheet')).toBe(true);
    expect(sheet?.getAttribute('href')).toBe('#artifact:report%2F1');
    expect(sheet?.querySelector('.fname')?.textContent).toBe('report.pdf');
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
