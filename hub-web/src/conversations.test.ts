// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import { applyHistoryCursor, ConversationHistory, RECEIPT_REPLY_GRACE_MS, RECEIPT_TIMEOUT_MS, supervisorWorking, type HistoryCursor } from "./conversation-history";
import { ConversationList, conversationRowMarkup, filterConversationRows, truncateConversationPreview, type ConversationRow, conversationRowSpokenName } from "./conversation-list";
import { ConversationView } from "./conversation-view";
import { applePlatform, appearanceButtonMarkup, ATTACH_DISABLED_REASON, ATTACH_SUPPORTED, arrangeConversationShell, conversationNoMatchText, conversationSearchPlaceholder, conversationShellMarkup, dressComposer, fitMachineLine, hostMarkup, KEYBOARD_HINT_MEDIA_QUERY, paletteShortcutLabel } from "./conversation-shell";
import { renderConversationFixture } from "../fixtures/conversations";
import { projectName, projectBadge } from "./cloud-brand";

const reply = { notification_id: 42, reply_to: 41, message: 'Actual supervisor reply <safe>', summary: '', device_id: 'device', operator_label: 'Daniel' };
it('another tab settles adjacent held sends without manufacturing unsent chips', () => {
  const history = new ConversationHistory();
  history.hold('a', 'supervisor', 'first', 1_000);
  history.hold('b', 'supervisor', 'second', 1_001);
  history.synchronizePending([]);
  expect(history.pendingSends()).toEqual([]);
  expect(history.dismissedSends()).toEqual([]);
  expect(history.visibleEvents()).toEqual([]);
});
describe('conversation evidence', () => {
  it('a later reply keeps explicit Send again available until that send is confirmed (cas-9dc6)', () => {
    const history = new ConversationHistory();
    history.submit('unknown', 'supervisor', 'Did the Mac tests start?', 1_000);
    history.unconfirmSilent(16_000);
    history.reply({ ...reply, reply_to: null }, 17_000, undefined, undefined, 17_000);
    const event = history.events.find(event => event.kind === 'send')!;
    if (event.kind !== 'send') throw new Error('Missing send');
    expect(history.isFailedSend(event.value)).toBe(false);
    expect(history.canRetrySend(event.value)).toBe(true);
    history.acknowledge({ client_ref: 'unknown', notification_id: 99, target: 'supervisor', stamped: true });
    expect(history.canRetrySend(event.value)).toBe(false);
  });
  it('shows both paired devices and terminal input in the same history', () => {
    const history = new ConversationHistory();
    const row = (notification_id: number, text: string, device_id: string, operator_label: string, stamped = true) => ({
      notification_id, target: 'supervisor', text, state: 'acknowledged' as const,
      stamped, device_id, operator_label, at: `2026-09-29T15:00:0${notification_id}Z`,
    });
    history.hydrateSend(row(1, 'Phone message', 'phone', 'Pixel 10'));
    history.hydrateSend(row(2, 'Computer message', 'computer', 'Desktop'));
    history.hydrateSend(row(3, 'Terminal message', 'terminal', 'Terminal', false));
    const view = new ConversationView(document, history, 'supervisor');
    document.body.replaceChildren(view.element);
    view.update();
    expect([...view.element.querySelectorAll('.conversation-send-origin')].map(node => node.textContent))
      .toEqual(['from Pixel 10', 'from Desktop', 'from Terminal']);
    expect(history.events.map(event => event.kind === 'send' && event.value.text))
      .toEqual(['Phone message', 'Computer message', 'Terminal message']);
  });
  it('threads the supervisor answer under a terminal-typed question, once (cas-5c89)', () => {
    const history = new ConversationHistory();
    history.hydrateSend({ notification_id: 3, target: 'supervisor', text: 'What changed in Violet today?', state: 'acknowledged', stamped: false, device_id: 'terminal', operator_label: 'Terminal', at: '2026-10-10T13:34:00Z' });
    const answer = { notification_id: 4, reply_to: 3, message: 'Two fixes landed.', summary: 'Two fixes landed.', device_id: '*', kind: 'answer' as const, at: '2026-10-10T13:34:20Z' };
    history.hydrateReply(answer);
    history.hydrateReply(answer);
    const view = new ConversationView(document, history, 'supervisor');
    document.body.replaceChildren(view.element);
    view.update();
    expect(history.events.map(event => event.kind === 'send' ? `send:${event.value.text}` : `reply:${event.value.message}`))
      .toEqual(['send:What changed in Violet today?', 'reply:Two fixes landed.']);
    expect([...view.element.querySelectorAll('.conversation-send-origin')].map(node => node.textContent)).toEqual(['from Terminal']);
    const send = history.events.find(event => event.kind === 'send')!;
    if (send.kind !== 'send') throw new Error('Missing send');
    expect(send.value.state).toBe('replied');
    expect(view.element.textContent).toContain('Two fixes landed.');
  });
  it('shows the other device send live before its reply and labels the sender receipt', () => {
    const history = new ConversationHistory();
    history.submit('own', 'supervisor', 'Mine');
    history.acknowledge({ client_ref: 'own', notification_id: 40, target: 'supervisor', stamped: true, device_label: 'Desktop' });
    history.hydrateSend({ notification_id: 41, target: 'supervisor', text: 'Question', state: 'sending', stamped: true, device_id: 'phone', operator_label: 'Pixel 10', session: 'factory-a', at: '2026-09-29T16:00:00Z' });
    history.receive({ notification_id: 42, reply_to: 41, message: 'Answer', summary: '', device_id: 'phone' });
    const view = new ConversationView(document, history, 'supervisor');
    document.body.replaceChildren(view.element);
    view.update();
    expect([...view.element.querySelectorAll('.conversation-send-origin')].map(node => node.textContent)).toEqual(['from Desktop', 'from Pixel 10']);
    expect(history.events.map(event => event.kind === 'send' ? event.value.text : event.value.message)).toEqual(['Mine', 'Question', 'Answer']);
  });
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
  it('gives up on a missing receipt after a bounded wait, sooner once a later reply lands (cas-1622)', () => {
    const history = new ConversationHistory();
    const sent = 1_000_000;
    history.submit('own', 'supervisor', 'Instruction', sent);
    expect(history.hasPending()).toBe(true);
    expect(history.nextReceiptCheck(sent)).toBe(RECEIPT_TIMEOUT_MS);
    // A supervisor turn after it means its receipt should already be here; it gets a short grace.
    // The grace counts from when that turn arrived, not from the send (cas-1185).
    history.receive({ notification_id: 9, reply_to: null, message: 'Status', summary: '', device_id: 'd' }, sent + 500);
    const graceEnds = sent + 500 + RECEIPT_REPLY_GRACE_MS;
    expect(history.nextReceiptCheck(sent + 500)).toBe(RECEIPT_REPLY_GRACE_MS);
    expect(history.unconfirmSilent(graceEnds - 1)).toEqual([]);
    expect(history.unconfirmSilent(graceEnds)).toEqual(['own']);
    expect(history.events[0]).toMatchObject({ value: { state: 'unconfirmed' } });
    // Nothing is left to wait for, and the thread is no longer "working" on it.
    expect(history.nextReceiptCheck(graceEnds)).toBeUndefined();
    expect(history.hasPending()).toBe(false);
    expect(history.unconfirmSilent(sent + RECEIPT_TIMEOUT_MS)).toEqual([]);
    // A retry discards it once the new send is on the wire; a delivered send is never discarded.
    expect(history.discardRefused('own')).toBe(true);
    history.submit('ok', 'supervisor', 'Delivered one', sent + 3_000);
    history.acknowledge({ client_ref: 'ok', notification_id: 12, target: 'supervisor', stamped: true });
    expect(history.discardRefused('ok')).toBe(false);
    expect(history.unconfirmSilent(sent + 3_000 + RECEIPT_TIMEOUT_MS)).toEqual([]);
  });
  it('a supervisor turn crossing the send never flashes "Not confirmed" before a late receipt (cas-1185)', () => {
    const history = new ConversationHistory();
    const sent = 2_000_000;
    history.submit('own', 'supervisor', 'Ship it', sent);
    // The turn lands 100 ms after the send; the receipt, measured up to 3.35 s late, comes after it.
    history.receive({ notification_id: 20, reply_to: null, message: 'Working on the gate', summary: '', device_id: 'd' }, sent + 100);
    for (const late of [1_920, 3_350, sent + 100 + RECEIPT_REPLY_GRACE_MS - 1 - sent]) {
      expect(history.unconfirmSilent(sent + late), `still Sending… ${late} ms after the send`).toEqual([]);
    }
    expect(history.events[0]).toMatchObject({ value: { state: 'sending' } });
    // No "Not confirmed", so no Retry could have gone out: the late receipt lands on the one send.
    expect(history.acknowledge({ client_ref: 'own', notification_id: 21, target: 'supervisor', stamped: true })).toBe(true);
    expect(history.events.filter((event) => event.kind === 'send')).toHaveLength(1);
    expect(history.events[0]).toMatchObject({ value: { state: 'acknowledged', notificationId: 21 } });
    expect(history.nextReceiptCheck(sent + 3_350)).toBeUndefined();
  });
  it('a turn that arrives late still leaves the receipt its full grace, capped by the timeout (cas-1185)', () => {
    const history = new ConversationHistory();
    const sent = 3_000_000;
    history.submit('own', 'supervisor', 'Ship it', sent);
    history.receive({ notification_id: 30, reply_to: null, message: 'Status', summary: '', device_id: 'd' }, sent + 4_000);
    expect(history.unconfirmSilent(sent + 4_000 + RECEIPT_REPLY_GRACE_MS - 1)).toEqual([]);
    expect(history.unconfirmSilent(sent + 4_000 + RECEIPT_REPLY_GRACE_MS)).toEqual(['own']);
    // A turn arriving near the timeout does not extend the wait past it.
    const capped = new ConversationHistory();
    capped.submit('own', 'supervisor', 'Ship it', sent);
    capped.receive({ notification_id: 31, reply_to: null, message: 'Status', summary: '', device_id: 'd' }, sent + RECEIPT_TIMEOUT_MS - 1_000);
    expect(capped.nextReceiptCheck(sent)).toBe(RECEIPT_TIMEOUT_MS);
  });
  it('never times out a hydrated send or a refused one (cas-1622)', () => {
    const history = new ConversationHistory();
    history.hydrateSend({ notification_id: 11, target: 'supervisor', text: 'Stored', state: 'sending', stamped: true, device_id: 'phone', operator_label: 'Daniel', at: '2026-09-21T14:01:00Z' });
    history.submit('refused', 'supervisor', 'No', 5_000);
    history.reject('refused', 'forbidden');
    expect(history.nextReceiptCheck(Number.MAX_SAFE_INTEGER)).toBeUndefined();
    expect(history.unconfirmSilent(Number.MAX_SAFE_INTEGER)).toEqual([]);
  });
  it('hydrates durable sends and replies in order, then dedupes live receipts and replies', () => {
    const history = new ConversationHistory();
    history.hydrateReply({ notification_id: 12, reply_to: null, message: 'Later', summary: '', device_id: 'phone', at: '2026-09-21T14:02:00Z' });
    history.hydrateSend({ notification_id: 11, target: 'supervisor', text: 'Earlier', state: 'acknowledged', stamped: true, device_id: 'phone', operator_label: 'Daniel', at: '2026-09-21T14:01:00Z' });
    expect(history.events.map((event) => event.kind === 'send' ? event.value.text : event.value.message)).toEqual(['Earlier', 'Later']);
    history.acknowledge({ client_ref: null, notification_id: 11, target: 'supervisor', stamped: true });
    history.reply({ notification_id: 12, reply_to: null, message: 'Later', summary: '', device_id: 'phone' });
    expect(history.events).toHaveLength(2);
    expect(history.events[0]).toMatchObject({ value: { notificationId: 11, state: 'acknowledged' } });
  });
  it('never counts a refused send as the answer to an ask (cas-b438)', () => {
    const history = new ConversationHistory();
    history.reply({ notification_id: 4, reply_to: null, message: 'Tag it?', summary: '', device_id: 'd', kind: 'ask' });
    // Sending answers optimistically.
    history.submit('q', 'supervisor', 'Yes, go ahead', Date.now(), 4);
    expect(history.answered(4)?.id).toBe('q'); expect(history.pinnedAsk()).toBeUndefined();
    // Refused: nothing reached the supervisor, so the ask is waiting and pinned again.
    history.reject('q', 'no access');
    expect(history.answered(4)).toBeUndefined();
    expect(history.pinnedAsk()?.notification_id).toBe(4);
    expect(history.waiting().map((item) => item.notification_id)).toEqual([4]);
    // A retry replaces the refused send and answers the ask.
    expect(history.discardRefused('q')).toBe(true);
    history.submit('r', 'supervisor', 'Yes, go ahead', Date.now(), 4);
    expect(history.answered(4)?.id).toBe('r'); expect(history.pinnedAsk()).toBeUndefined();
    // A refused send beside a live one never shadows it, whatever the order.
    history.submit('late', 'supervisor', 'Hold', Date.now(), 4); history.reject('late', 'no access');
    expect(history.answered(4)?.id).toBe('r');
    history.acknowledge({ client_ref: 'r', notification_id: 9, target: 'supervisor', stamped: true });
    expect(history.answered(4)).toMatchObject({ id: 'r', state: 'acknowledged' });
  });
  it('never counts a refused send as acknowledging a blocker (cas-6874)', () => {
    const history = new ConversationHistory();
    history.reply({ notification_id: 7, reply_to: null, message: 'Gate red.', summary: '', device_id: 'd', kind: 'blocker' });
    expect(history.waiting().map((item) => item.notification_id)).toEqual([7]);
    // Sending acknowledges optimistically.
    history.submit('s', 'supervisor', 'Looking now', Date.now());
    expect(history.waiting()).toEqual([]);
    // Refused: the supervisor received nothing, so the blocker is waiting on the operator again.
    history.reject('s', 'offline');
    expect(history.waiting().map((item) => item.notification_id)).toEqual([7]);
    // The list row (attention = waiting().length, as renderConversationList feeds it) still flags it waiting on the operator.
    const container = document.createElement('nav');
    new ConversationList().render(container, [{ key: 'a:s', machineId: 'a', session: 's', supervisor: 'sup', host: 'Atlas', freshness: 'now', connection: 'Live', when: '10:00', attention: history.waiting().length, selected: false }], vi.fn());
    expect((container.firstElementChild as HTMLElement).dataset.waiting).toBe('true');
    expect(container.querySelector('.conversation-flag')?.getAttribute('aria-label')).toBe('Waiting for you');
    // A retry replaces the refused send and acknowledges the blocker.
    expect(history.discardRefused('s')).toBe(true);
    history.submit('r', 'supervisor', 'Looking now', Date.now());
    expect(history.waiting()).toEqual([]);
    history.acknowledge({ client_ref: 'r', notification_id: 12, target: 'supervisor', stamped: true });
    expect(history.waiting()).toEqual([]);
    // A later refused send beside the delivered one never un-acknowledges it.
    history.submit('late', 'supervisor', 'Also', Date.now()); history.reject('late', 'offline');
    expect(history.waiting()).toEqual([]);
  });
  it('keeps a blocker waiting when only a refused send follows it, even without a retry (cas-6874)', () => {
    const history = new ConversationHistory();
    history.submit('before', 'supervisor', 'Earlier', 1);
    history.reply({ notification_id: 8, reply_to: null, message: 'Need a key.', summary: '', device_id: 'd', kind: 'blocker' }, 2);
    history.submit('x', 'supervisor', 'Here', 3); history.reject('x', 'no access');
    // The send before the blocker never acknowledges it, and the refused one after it does not either.
    expect(history.waiting().map((item) => item.notification_id)).toEqual([8]);
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
    // Journey F7: the title is the project, then the machine; the codename is tertiary text beneath.
    expect(waiting.querySelector('.conversation-who > .conversation-title > strong.conversation-project')?.textContent).toBe('cas-src');
    expect(waiting.querySelector('.conversation-title > .conversation-project + .conversation-machine')).not.toBeNull();
    expect(waiting.querySelector('.conversation-who > .conversation-title + .conversation-supervisor')?.textContent).toBe('patient-pelican-9');
    expect(waiting.querySelector('.conversation-who')?.textContent).toBe('cas-srcAtlaspatient-pelican-9');
    expect(waiting.querySelector('.project-badge')).toBeNull();
    // P13: the title leads with the project, never a stray dot; the only separator rides with the machine.
    expect(waiting.querySelector('.conversation-who > .conversation-sep, .conversation-title > .conversation-sep')).toBeNull();
    expect(waiting.querySelectorAll('.conversation-sep')).toHaveLength(1);
    // P14: the codename is one unbreakable word.
    expect(waiting.querySelector('.conversation-supervisor')?.classList.contains('codename')).toBe(true);
    // The machine is named as text on every row, after the project, with its own wrapping separator.
    expect(waiting.querySelector('.conversation-machine')?.textContent).toBe('Atlas');
    expect(waiting.querySelector('.conversation-machine')?.innerHTML).toBe('<span class="conversation-sep" aria-hidden="true"></span><span class="conversation-machine-name">Atlas</span>');
    // cas-1ca1: the machine name can ellipsise; the title attribute keeps it whole.
    expect(waiting.querySelector('.conversation-machine')?.getAttribute('title')).toBe('Atlas');
    expect(unread.querySelector('.conversation-machine')?.textContent).toBe('Studio Mac');
    expect(unread.querySelector('.conversation-project')?.textContent).toBe('gabber-studio');
    expect(waiting.querySelector('.conversation-preview')?.textContent).toBe('Fix <it>?');
    expect(waiting.querySelector('script, it')).toBeNull();
    expect(waiting.querySelector('.conversation-when')?.className).toBe('conversation-when hot');
    expect(waiting.querySelector('.conversation-flag')?.getAttribute('aria-label')).toBe('Waiting for you');
    expect(waiting.querySelector('.conversation-marks > .conversation-flag')).not.toBeNull();
    expect(waiting.querySelector('.conversation-unread')).toBeNull();
    expect(waiting.dataset.waiting).toBe('true');
    expect(unread.querySelector('.conversation-unread')?.textContent).toBe('2');
    // P13: an unread row keeps its time at the headline end; the count sits beneath it.
    expect(unread.querySelector('.conversation-when')?.textContent).toBe('Tue');
    expect([...unread.children].map((child) => child.className)).toEqual(['conversation-avatar', 'conversation-who', 'conversation-when', 'conversation-preview bold', 'conversation-marks']);
    expect(unread.querySelector('.conversation-marks > .conversation-unread')).not.toBeNull();
    expect(unread.querySelector('.conversation-flag')).toBeNull();
    expect(unread.querySelector('.conversation-preview')?.className).toBe('conversation-preview bold');
    expect(quiet.querySelector('.conversation-when')?.textContent).toBe('Tue');
    expect(quiet.querySelector('.conversation-preview')?.textContent).toBe('Live');
    expect(quiet.querySelector('.conversation-preview')?.className).toBe('conversation-preview');
    expect(quiet.querySelector('.conversation-flag, .conversation-unread, .conversation-marks')).toBeNull();
  });
  it('titles a row with no project by its codename, not a status phrase (cas-1ca1 F03)', () => {
    const markup = conversationRowMarkup({ key: 'a:s', machineId: 'a', session: 's', supervisor: 'calm-otter-4', host: 'Atlas · Linux', freshness: 'now', connection: 'Live', attention: 0, selected: false });
    const row = document.createElement('div'); row.innerHTML = markup;
    expect(row.querySelector('.conversation-project')?.textContent).toBe('calm-otter-4');
    expect(row.querySelector('.conversation-project')?.classList.contains('codename')).toBe(true);
    expect(row.querySelector('.conversation-supervisor')).toBeNull();
    expect(row.textContent).not.toContain('Project unavailable');
    expect(row.querySelector('.conversation-machine-name')?.textContent).toBe('Atlas');
    const header = document.createElement('div');
    header.innerHTML = conversationShellMarkup({ selected: true, supervisor: 'calm-otter-4', host: 'Atlas · Linux', machineId: 'a', loaded: true, paired: true });
    expect(header.querySelector('.conversation-identity h1')?.textContent).toBe('calm-otter-4');
    expect(header.querySelector('.host-where')?.textContent).toBe('Atlas · Linux');
    expect(header.textContent).not.toContain('Project unavailable');
    expect(filterConversationRows([{ supervisor: 'calm-otter-4', host: 'Atlas · Linux' }], 'unavailable')).toEqual([]);
  });
  it('keeps the time and stacks both marks when a row is waiting and unread (P13)', () => {
    const list = new ConversationList(); const container = document.createElement('nav');
    list.render(container, [{ key: 'a:s', machineId: 'a', session: 's', supervisor: 'patient-pelican-9', projectDir: '/p/cas-src', host: 'Atlas', freshness: 'now', connection: 'Live', when: '09:58', attention: 2, unread: 3, selected: false }], vi.fn());
    const row = container.firstElementChild!;
    expect(row.querySelector('.conversation-when.hot')?.textContent).toBe('09:58');
    expect([...row.querySelector('.conversation-marks')!.children].map((child) => child.className)).toEqual(['conversation-unread', 'conversation-flag']);
    expect(row.querySelector('.conversation-flag')?.getAttribute('aria-label')).toBe('2 waiting for you');
  });
  it('gates the compose FAB on a paired machine and puts Appearance & commands in the header as a named icon button (P13)', () => {
    // A phone has no Ctrl K to press: the hint is dropped (3.30.0 journey F10).
    const phone = document.createElement('div');
    phone.innerHTML = conversationShellMarkup({ selected: false, loaded: true, paired: true, keyboardHint: false });
    expect(phone.querySelector<HTMLInputElement>('#conversation-search')!.placeholder).toBe('Search conversations');
    expect(KEYBOARD_HINT_MEDIA_QUERY).toBe('(any-pointer: fine) and (min-width: 500px)');
    const unpaired = document.createElement('div');
    unpaired.innerHTML = conversationShellMarkup({ selected: false, loaded: true, paired: false });
    expect(unpaired.querySelector('#compose-fab')).toBeNull();
    const loading = document.createElement('div');
    loading.innerHTML = conversationShellMarkup({ selected: false, loaded: false, paired: false });
    expect(loading.querySelector('#compose-fab')).toBeNull();
    const paired = document.createElement('div');
    paired.innerHTML = conversationShellMarkup({ selected: false, loaded: true, paired: true });
    expect(paired.querySelector('#compose-fab')?.getAttribute('aria-label')).toBe('Write to a supervisor');
    const toggle = paired.querySelector<HTMLButtonElement>('#command-palette-toggle')!;
    expect(toggle.closest('.conversation-list-heading > .conversation-list-top')).not.toBeNull();
    expect(toggle.getAttribute('aria-label')).toBe('Appearance & commands');
    expect(toggle.textContent).toBe('');
    expect(toggle.querySelector('svg')?.getAttribute('aria-hidden')).toBe('true');
    expect(paired.querySelector('.conversation-sidebar footer #command-palette-toggle')).toBeNull();
  });
  it('keeps the list heading to one row and names New session by its goal in both permission states (cas-865c)', () => {
    for (const launch of ['ready', 'grant'] as const) {
      const root = document.createElement('div');
      root.innerHTML = conversationShellMarkup({ selected: false, loaded: true, paired: true, launch });
      const title = root.querySelector('.conversation-list-title')!;
      // The heading row holds the h1 and New session only; Pair a machine sits with the appearance control.
      expect([...title.querySelectorAll('button')].map((button) => button.textContent)).toEqual(['+ New session']);
      expect(root.querySelector('#pair-toggle')?.closest('.conversation-list-top')).not.toBeNull();
      expect(root.textContent).not.toContain('Allow new sessions');
      expect(root.querySelector('#new-session-toggle')?.hasAttribute('data-launch-grant')).toBe(launch === 'grant');
    }
    const unpaired = document.createElement('div');
    unpaired.innerHTML = conversationShellMarkup({ selected: false, loaded: true, paired: false });
    expect(unpaired.querySelector('.conversation-list-actions')).toBeNull();
    expect(unpaired.querySelector('#pair-toggle')?.closest('.conversation-list-top')).not.toBeNull();
  });
  it('names the palette shortcut the way this keyboard prints it, the same on every surface (journey F16)', () => {
    expect(applePlatform({ platform: 'Linux x86_64' })).toBe(false);
    expect(applePlatform({ platform: 'Win32' })).toBe(false);
    expect(applePlatform({ platform: 'MacIntel' })).toBe(true);
    expect(applePlatform({ platform: 'iPad' })).toBe(true);
    expect(applePlatform({ platform: '', userAgentData: { platform: 'macOS' } })).toBe(true);
    expect(applePlatform({ platform: 'MacIntel', userAgentData: { platform: 'Windows' } })).toBe(false);
    expect(applePlatform(undefined)).toBe(false);
    expect(paletteShortcutLabel(false)).toBe('Ctrl K');
    expect(paletteShortcutLabel(true)).toBe('⌘K');
    expect(conversationSearchPlaceholder(true, 'Ctrl K')).toBe('Search conversations (Ctrl K)');
    expect(conversationSearchPlaceholder(true, '⌘K')).toBe('Search conversations (⌘K)');
    expect(conversationSearchPlaceholder(false, '⌘K')).toBe('Search conversations');
    expect(appearanceButtonMarkup('⌘K')).toContain('title="Appearance &amp; commands (⌘K twice)"');
    expect(appearanceButtonMarkup('Ctrl K')).toContain('title="Appearance &amp; commands (Ctrl K twice)"');
  });
  it('puts a visible "Search conversations (Ctrl K)" field at the top of the list once a machine is paired (journey F8)', () => {
    const paired = document.createElement('div');
    paired.innerHTML = conversationShellMarkup({ selected: false, loaded: true, paired: true, searchQuery: 'gab"ber' });
    const search = paired.querySelector<HTMLInputElement>('.conversation-list-heading #conversation-search')!;
    expect(search.type).toBe('search');
    expect(search.getAttribute('aria-label')).toBe('Search conversations');
    expect(search.placeholder).toBe('Search conversations (Ctrl K)');
    expect(search.getAttribute('aria-controls')).toBe('conversation-list');
    expect(search.value).toBe('gab"ber');
    // It sits above the rows it filters.
    expect(search.compareDocumentPosition(paired.querySelector('#conversation-list')!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const unpaired = document.createElement('div');
    unpaired.innerHTML = conversationShellMarkup({ selected: false, loaded: true, paired: false });
    expect(unpaired.querySelector('#conversation-search')).toBeNull();
    expect(conversationNoMatchText(' zzz ')).toBe('No conversation matches “zzz”. Search looks at project, machine and supervisor names.');
  });
  it('filters rows by project, machine or supervisor, every word, ignoring case (journey F8)', () => {
    const base = { session: 's', freshness: 'now', connection: 'Live', attention: 0, selected: false };
    const rows: ConversationRow[] = [
      { ...base, key: 'a:1', machineId: 'a', supervisor: 'patient-pelican-9', projectDir: '/p/cas-src', host: 'Atlas · Linux' },
      { ...base, key: 'b:1', machineId: 'b', supervisor: 'calm-otter-4', projectDir: '/p/gabber-studio', host: 'Studio Mac · macOS' },
      { ...base, key: 'a:2', machineId: 'a', supervisor: 'steady-heron-2', projectDir: '/p/petra-stella-cloud', host: 'Atlas · Linux' },
    ];
    const keys = (query: string) => filterConversationRows(rows, query).map((row) => row.key);
    expect(keys('')).toEqual(['a:1', 'b:1', 'a:2']);
    expect(keys('  ')).toEqual(['a:1', 'b:1', 'a:2']);
    expect(keys('Gabber')).toEqual(['b:1']);
    expect(keys('atlas')).toEqual(['a:1', 'a:2']);
    expect(keys('heron')).toEqual(['a:2']);
    expect(keys('atlas cas')).toEqual(['a:1']);
    // The project is matched by its name, not the path above it.
    expect(keys('/p/')).toEqual([]);
    expect(keys('zzz')).toEqual([]);
  });
  it('strips supervisor markdown from conversation-list previews', () => {
    const list = new ConversationList(); const container = document.createElement('nav');
    list.render(container, [{ key: 'a:s', machineId: 'a', session: 's', supervisor: 'sup', host: 'Atlas', freshness: 'now', connection: 'Live', attention: 0, preview: '**Ready**\n\n- `cargo check`', selected: false }], vi.fn());
    const preview = container.querySelector('.conversation-preview');
    expect(preview?.textContent).toBe('Ready cargo check');
    expect(preview?.textContent).not.toContain('**');
    expect(preview?.querySelector('code')).toBeNull();
  });
  it('truncates long conversation previews to 160 characters with an ellipsis', () => {
    const longReply = 'A supervisor reply that keeps going. '.repeat(40);
    const preview = truncateConversationPreview(longReply);
    expect(preview).toHaveLength(160);
    expect(preview.endsWith('…')).toBe(true);
    expect(conversationRowMarkup({ key: 'a:s', machineId: 'a', session: 's', supervisor: 'sup', host: 'Atlas', freshness: 'now', connection: 'Live', attention: 0, preview: longReply, selected: false })).toContain(`>${preview}</span>`);
  });
  it('names an unreachable session with a pending instruction on its row, over the last turn (cas-7294)', () => {
    const row: ConversationRow = { key: 'a:s', machineId: 'a', session: 's', supervisor: 'sup', host: 'Atlas', freshness: 'now', connection: 'Unreachable · message pending', attention: 0, preview: 'You: Keep this instruction visible.', selected: false };
    expect(conversationRowMarkup({ ...row, unreachable: true })).toContain('<span class="conversation-preview unreachable">Unreachable · message pending</span>');
    // A dropped connection names itself in place of the last turn, so the row agrees with the header (cas-a447).
    expect(conversationRowMarkup({ ...row, connection: 'Reconnecting', interrupted: true })).toContain('<span class="conversation-preview interrupted">Reconnecting</span>');
    expect(conversationRowMarkup({ ...row, connection: 'Live' })).toContain('<span class="conversation-preview">You: Keep this instruction visible.</span>');
  });
  it('marks a parked draft in another conversation, under any connection problem (cas-97d58 F19)', () => {
    const row: ConversationRow = { key: 'a:s', machineId: 'a', session: 's', supervisor: 'sup', host: 'Atlas', freshness: 'now', connection: 'Live', attention: 0, preview: 'Gate run 3 of 3 is going.', selected: false, draft: 'Please keep the release notes short' };
    expect(conversationRowMarkup(row)).toContain('<span class="conversation-preview draft">Draft: Please keep the release notes short</span>');
    expect(conversationRowSpokenName(row)).toContain('Draft: Please keep the release notes short');
    // The open conversation shows its draft in the composer, not the row.
    expect(conversationRowMarkup({ ...row, selected: true })).toContain('>Gate run 3 of 3 is going.</span>');
    // A connection problem still leads, so the row agrees with the header.
    expect(conversationRowMarkup({ ...row, connection: 'Reconnecting', interrupted: true })).toContain('<span class="conversation-preview interrupted">Reconnecting</span>');
    expect(conversationRowMarkup({ ...row, draft: '   ' })).toContain('<span class="conversation-preview">Gate run 3 of 3 is going.</span>');
  });
  it('renders the fixture list with a footer count equal to the rows rendered', () => {
    const app = document.createElement('div'); document.body.replaceChildren(app);
    renderConversationFixture(app, 'conversations-list');
    const rows = app.querySelectorAll('.conversation-row').length;
    expect(rows).toBe(6);
    expect(app.querySelector('.hub-footer-meta span')?.textContent).toBe(`${rows} conversations`);
    expect(new Set([...app.querySelectorAll('.conversation-row')].map((row) => row.className)).size).toBe(3);
    expect([...app.querySelectorAll('.conversation-project')].map((project) => project.textContent)).toContain('petra-stella-cloud');
  });
  it('puts the selected machine accent on the shell root and a compose FAB in the operator colour', () => {
    const shell = conversationShellMarkup({ selected: true, supervisor: 'patient-pelican-9', projectDir: '/projects/cas-src', host: 'Atlas · Linux', machineId: 'atlas-linux', loaded: true, paired: true });
    expect(shell).toContain('class="conversation-shell thread-open machine-accent-0"');
    expect(shell).toContain('id="compose-fab" class="compose-fab"');
    expect(conversationShellMarkup({ selected: false, loaded: true, paired: true })).toContain('class="conversation-shell">');
  });
  it('offers one primary Pair a machine on first run: the welcome, with the header chip primary only where the welcome is hidden (D3)', () => {
    const shell = document.createElement('div');
    shell.innerHTML = conversationShellMarkup({ selected: false, loaded: true, paired: false });
    expect(shell.querySelector('.conversation-shell')?.classList.contains('welcome-pairs')).toBe(true);
    expect(shell.querySelector('#empty-pair')?.classList.contains('primary')).toBe(true);
    expect(shell.querySelector('#pair-toggle')?.classList.contains('primary')).toBe(true);
    for (const model of [{ loaded: false, paired: false }, { loaded: true, paired: true }]) {
      shell.innerHTML = conversationShellMarkup({ selected: false, ...model });
      expect(shell.querySelector('.welcome-pairs')).toBeNull();
      expect(shell.querySelector('#empty-pair')).toBeNull();
      expect(shell.querySelector('#pair-toggle')?.classList.contains('primary')).toBe(false);
    }
  });
  it('renders one Pebble header above the thread with the back link, Raw output, Interrupt, avatar, badge and connection slot', () => {
    const shell = document.createElement('div');
    shell.innerHTML = conversationShellMarkup({ selected: true, supervisor: 'patient-pelican-9', projectDir: '/projects/cas-src', host: 'Atlas · Linux', machineId: 'atlas-linux', loaded: true, paired: true });
    const main = shell.querySelector('.conversation-main')!;
    expect(main.querySelectorAll('header')).toHaveLength(1);
    const header = main.querySelector('header.conversation-heading.thead')!;
    expect(header.querySelector('#conversation-back')?.textContent).toBe('‹ Conversations');
    // cas-0546: the conversation's own actions replace Terminal view; the
    // phone folds Raw output to its icon, and the aria-labels keep the full
    // names at every width.
    expect(header.querySelector('#conversation-terminal')).toBeNull();
    expect(header.querySelector('#conversation-raw-output')?.textContent).toBe('Raw output');
    expect(header.querySelector('#conversation-raw-output')?.getAttribute('aria-label')).toBe('Raw output');
    expect(header.querySelector('#conversation-interrupt')?.textContent).toBe('Interrupt');
    expect(header.querySelector('#conversation-interrupt')?.getAttribute('aria-label')).toBe('Interrupt the cas-src supervisor');
    expect(header.querySelector('#conversation-back')?.getAttribute('aria-label')).toBe('‹ Conversations');
    expect(header.querySelector('#conversation-back .back-glyph')?.textContent).toBe('‹');
    expect(header.querySelector('#conversation-back .back-label')?.textContent).toBe(' Conversations');
    expect(header.querySelector('.conversation-avatar')?.textContent).toBe('A');
    // Journey F7: the project is the title, once; machine and codename sit beneath it.
    expect(header.querySelector('h1')?.textContent).toBe('cas-src');
    expect(header.querySelector('h1 b')?.getAttribute('title')).toBe('cas-src');
    expect(header.querySelector('h1 .project-badge')).toBeNull();
    expect(header.textContent?.split('cas-src')).toHaveLength(2);
    expect(header.querySelector('.conversation-host')?.textContent).toBe('Atlas · Linux · patient-pelican-9');
    // The OS word is its own span, so a phone can drop it before the codename (journey F14).
    expect(header.querySelector('.host-where .host-os')?.textContent).toBe(' · Linux');
    // Machine, separator and codename are separate flex items so the machine name yields first (cas-e918 QA F01).
    expect([...header.querySelector('.host-where')!.children].map((node) => node.className)).toEqual(['host-machine', 'host-sep', 'codename']);
    expect(header.querySelector('.host-machine')?.textContent).toBe('Atlas · Linux');
    expect(header.querySelector('.host-where')?.getAttribute('title')).toBe('Atlas · Linux · patient-pelican-9');
    expect(header.querySelector('.conversation-host > .host-where > .codename')?.textContent).toBe('patient-pelican-9');
    expect(header.querySelector('.conversation-host > .host-where + #conversation-connection')).not.toBeNull();
    expect(header.querySelector('#conversation-connection')).not.toBeNull();
    expect(main.querySelector('.conversation-identity h1')).not.toBeNull(); expect(main.querySelectorAll('h1')).toHaveLength(1);
    const view = new ConversationView(document, new ConversationHistory(), { supervisor: 'patient-pelican-9', machine: 'Atlas', project: 'cas-src', header: false });
    shell.querySelector('#conversation-pane-slot')!.append(view.element);
    expect(shell.querySelectorAll('.thead')).toHaveLength(1);
  });
  it('dresses the composer as Pebble: pill field, no dead attach clip, send in the accent naming the supervisor', () => {
    const app = document.createElement('div');
    const template = document.createElement('template');
    template.innerHTML = '<div class="message"><h2><label for="message-text">Talk to x</label></h2><textarea id="message-text" placeholder="old"></textarea><div class="composer-actions"><button id="message-mic" type="button" aria-label="Start listening" aria-pressed="false"><svg class="mic-glyph" aria-hidden="true"></svg></button><button id="message-keyboard" type="button">Keyboard</button><button id="message-send" class="primary">Send message</button></div><p id="message-status" class="message-status" role="status" hidden></p></div>';
    const grid = document.createElement('section'); grid.id = 'pane-grid';
    const attention = document.createElement('section'); attention.id = 'attention-panel'; attention.hidden = true;
    app.append(arrangeConversationShell(document, { selected: true, supervisor: 'patient-pelican-9', projectDir: '/projects/cas-src', machineId: 'atlas-linux', loaded: true, paired: true }, { grid, composer: template.content.firstElementChild as HTMLElement, attention }));
    expect(app.querySelector('#conversation-pane-slot > #pane-grid')).toBe(grid);
    expect(app.querySelector<HTMLElement>('#conversation-attention-slot > #attention-panel')?.hidden).toBe(false);
    const composer = app.querySelector<HTMLElement>('#conversation-composer-slot > .message.conversation-composer')!;
    expect(composer).not.toBeNull();
    // cas-17e3: attaching has no transport yet, so the clip is not offered at all.
    expect(ATTACH_SUPPORTED).toBe(false);
    expect(composer.querySelector('.composer-clip')).toBeNull();
    expect(ATTACH_DISABLED_REASON).toContain('not supported yet');
    const mic = composer.querySelector<HTMLButtonElement>('#message-mic')!;
    expect(mic.getAttribute('aria-label')).toBe('Start listening');
    expect(mic.querySelector('.mic-glyph')).not.toBeNull();
    expect(mic.textContent).toBe('');
    expect(composer.querySelector<HTMLTextAreaElement>('#message-text')?.placeholder).toBe('Message the cas-src supervisor');
    const send = composer.querySelector<HTMLButtonElement>('#message-send')!;
    expect(send.classList.contains('send')).toBe(true);
    expect(send.textContent).toBe('Send');
    expect(send.getAttribute('aria-label')).toBe('Send to the cas-src supervisor');
    expect(send.querySelector('.send-glyph')).not.toBeNull();
    expect(app.querySelector('.conversation-shell')?.classList.contains('machine-accent-0')).toBe(true);
    // Dressing twice (every re-render) never stacks a second clip or label.
    dressComposer(composer, 'patient-pelican-9');
    expect(composer.querySelectorAll('.composer-clip')).toHaveLength(0); expect(send.querySelectorAll('.send-label')).toHaveLength(1);
  });
  it('shows only turns — never pane text — escapes replies, and never presents the operator as the supervisor', () => {
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, { supervisor: 'real-supervisor', machine: 'Atlas', project: 'cas-src' }); document.body.replaceChildren(view.element); view.update();
    expect(view.element.querySelector('.thead b')?.textContent).toBe('real-supervisor');
    // P14: project · machine, the order of the shell header and every list row.
    expect(view.element.querySelector('.thead .id span')?.textContent).toBe('cas-src · Atlas');
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
  const forbidden = { code: 'forbidden', retryable: false };
  expect(onMessageRejected.mock.calls).toEqual([['a', 'daemon', 'Refused by daemon'], ['a', 'legacy', 'forbidden', forbidden], ['b', 'mux', 'forbidden', forbidden]]);
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

describe("hostMarkup (journey F14)", () => {
  it("wraps only a trailing OS word, and escapes the label", () => {
    expect(hostMarkup("Studio Mac · macOS")).toBe('Studio Mac<span class="host-os"> · macOS</span>');
    expect(hostMarkup("Forge build box · Linux")).toBe('Forge build box<span class="host-os"> · Linux</span>');
    expect(hostMarkup("hub · staging")).toBe("hub · staging");
    expect(hostMarkup("pippenz-desktop")).toBe("pippenz-desktop");
    expect(hostMarkup("<b> · Windows")).toBe('&lt;b&gt;<span class="host-os"> · Windows</span>');
  });
  it("keeps the machine ahead of the codename at every width (cas-766c)", () => {
    const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "styles.css"), "utf8");
    expect(css).toContain(".conversation-identity .host-machine { flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; }");
    expect(css).toContain(".conversation-identity .host-where.machine-long > .host-machine { min-width: 16ch; }");
    expect(css).toContain(".conversation-identity .host-machine ~ .codename { flex: 0 1000 auto; min-width: min(8ch, 100%); overflow: hidden; text-overflow: ellipsis; }");
    // The OS word goes whenever the line is short of room, not only on a phone.
    // cas-8526: hidden from sight only, so the OS word is still heard; since
    // cas-0546 fitMachineLine marks it .sr-only (the house accessibility helper).
    expect(css).toMatch(/\n\.sr-only \{/);
    expect(css).not.toContain("  .conversation-identity .host-os { display: none; }");
  });
});

describe("fitMachineLine (cas-766c)", () => {
  // 10px mono: 1ch is 6px, so the machine keeps up to 96px and the codename 48px.
  const line = (sizes: { machine: number; machineNoOs?: number; codename: number; separator?: number }) => {
    const where = document.createElement("span"); where.className = "host-where";
    where.innerHTML = '<span class="host-machine">Forge build box with an unusual hostname<span class="host-os"> · Linux</span></span><span class="host-sep"> · </span><span class="codename">an-extraordinarily-long-supervisor-name</span>';
    const codename = where.querySelector<HTMLElement>(".codename")!;
    const machine = where.querySelector<HTMLElement>(".host-machine")!;
    codename.style.fontSize = "10px";
    Object.defineProperty(codename, "scrollWidth", { configurable: true, get: () => sizes.codename });
    Object.defineProperty(machine, "scrollWidth", { configurable: true, get: () => (where.querySelector(".host-os")!.classList.contains("sr-only") ? sizes.machineNoOs ?? sizes.machine : sizes.machine) });
    const separator = where.querySelector<HTMLElement>(".host-sep")!;
    separator.getBoundingClientRect = () => ({ width: sizes.separator ?? 18 } as DOMRect);
    document.body.append(where);
    return where;
  };
  const state = (where: HTMLElement) => ["os-dropped", "machine-long", "codename-squeezed"].filter((name) => name === "os-dropped" ? where.querySelector(".host-os")!.classList.contains("sr-only") : where.classList.contains(name));

  it("leaves a line that fits whole alone", () => {
    const where = line({ machine: 80, codename: 100 });
    fitMachineLine(where, 200);
    expect(state(where)).toEqual([]);
  });
  it("drops the OS word first, then lets the codename ellipsise beside the whole machine", () => {
    const where = line({ machine: 160, machineNoOs: 120, codename: 300 });
    // The whole machine (120px) + 18px separator + 8ch (48px) codename = 186px fits in 200.
    fitMachineLine(where, 200);
    expect(state(where)).toEqual(["os-dropped", "machine-long"]);
  });
  it("never cuts the machine while the codename shows (cas-d043 QA round 1)", () => {
    // 200px machine + 18 + 48 = 266px: the codename steps aside rather than cut "Build Server Rack Seven".
    const where = line({ machine: 240, machineNoOs: 200, codename: 300 });
    fitMachineLine(where, 200);
    expect(state(where)).toEqual(["os-dropped", "machine-long", "codename-squeezed"]);
    // Within two pixels of fitting is not fitting: rounding drew an ellipsis.
    const tight = line({ machine: 160, machineNoOs: 133, codename: 300 });
    fitMachineLine(tight, 200);
    expect(state(tight)).toContain("codename-squeezed");
  });
  it("steps the codename aside only when the machine and 8ch of codename cannot share the line", () => {
    const where = line({ machine: 240, machineNoOs: 200, codename: 300 });
    fitMachineLine(where, 150);
    expect(state(where)).toEqual(["os-dropped", "machine-long", "codename-squeezed"]);
    // A short machine leaves the codename its 8ch.
    const short = line({ machine: 40, codename: 300 });
    fitMachineLine(short, 120);
    expect(state(short)).toEqual(["os-dropped"]);
  });
  it("on the phone header shows the codename whole or not at all (cas-d043 QA round 1)", () => {
    // 120px machine + 18px separator + 300px codename does not fit 200: aside, though 8ch would fit.
    const where = line({ machine: 160, machineNoOs: 120, codename: 300 });
    fitMachineLine(where, 200, true);
    expect(state(where)).toEqual(["os-dropped", "machine-long", "codename-squeezed"]);
    const fits = line({ machine: 160, machineNoOs: 120, codename: 50 });
    fitMachineLine(fits, 200, true);
    expect(state(fits)).toEqual(["os-dropped", "machine-long"]);
  });
  it("hides the codename, not the machine, in the stylesheet, header and empty card alike", () => {
    const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "styles.css"), "utf8");
    // cas-8526: the codename steps aside from sight only; it is still heard.
    expect(css).toContain(".conversation-identity .host-where.codename-squeezed > :is(.codename, .host-sep) { position: absolute; width: var(--line-width); height: var(--line-width); min-width: 0; overflow: hidden; clip: rect(0, 0, 0, 0); clip-path: inset(50%); white-space: nowrap; }");
    expect(css).toContain(".thread .empty .proj2.codename-squeezed > :is(.codename, .proj2-sep) { position: absolute; width: var(--line-width); height: var(--line-width); min-width: 0; overflow: hidden; clip: rect(0, 0, 0, 0); clip-path: inset(50%); white-space: nowrap; }");
    expect(css).not.toMatch(/(host-where|proj2)\.(os-dropped|codename-squeezed)[^{]*\{ display: none; \}/);
    expect(css).not.toContain(".conversation-identity .conversation-host { display: none; }");
    expect(css).not.toContain("machine-squeezed > :is(");
  });
});

describe("supervisor working indicator (cas-5a8f)", () => {
  const live = { phase: "live" };
  it("a send held in this browser while the machine is unreachable is not supervisor execution", () => {
    const history = new ConversationHistory();
    history.hold("held", "supervisor", "Are you there?");
    expect(history.hasPending(), "the held send still keeps its session listed").toBe(true);
    expect(history.awaitingReply()).toBe(false);
    expect(supervisorWorking(history, { phase: "backoff" }, false)).toBe(false);
    expect(supervisorWorking(history, live, false), "held, so the supervisor has not seen it").toBe(false);
    // Reconnected: the held send goes out, and now it awaits the supervisor.
    expect(history.release("held")).toBe(true);
    expect(supervisorWorking(history, live, false)).toBe(true);
  });
  it("a send that went out says working only while the machine is live and paired", () => {
    const history = new ConversationHistory();
    history.submit("sent", "supervisor", "Run the tests");
    expect(supervisorWorking(history, live, false)).toBe(true);
    for (const machine of [{ phase: "backoff" }, { phase: "dialing" }, { phase: "failed" }, undefined]) {
      expect(supervisorWorking(history, machine, false), JSON.stringify(machine)).toBe(false);
    }
    expect(supervisorWorking(history, { phase: "live", authFailure: "revoked" }, false), "revoked pairing").toBe(false);
  });
  it("real pane output is independent evidence and keeps working", () => {
    const history = new ConversationHistory();
    expect(supervisorWorking(history, { phase: "backoff" }, true)).toBe(true);
    expect(supervisorWorking(history, live, false), "nothing pending, no output").toBe(false);
  });
});

describe("history cursor across a reattach (cas-2093)", () => {
  const fresh = (): HistoryCursor => ({ hasEarlier: false, loading: false, loaded: false });
  it("a newest page after the start was reached never brings Load earlier back", () => {
    const cursor = fresh();
    applyHistoryCursor(cursor, { has_earlier: true, next_before: 20 });
    expect(cursor).toMatchObject({ loaded: true, hasEarlier: true, nextBefore: 20 });
    cursor.loading = true;
    applyHistoryCursor(cursor, { has_earlier: true, next_before: 10 });
    expect(cursor).toMatchObject({ hasEarlier: true, nextBefore: 10, loading: false });
    cursor.loading = true;
    applyHistoryCursor(cursor, { has_earlier: false });
    expect(cursor).toMatchObject({ hasEarlier: false, nextBefore: undefined, loading: false });
    // The reconnect asks for the newest page again: it says has_earlier, about itself.
    applyHistoryCursor(cursor, { has_earlier: true, next_before: 20 });
    expect(cursor).toMatchObject({ hasEarlier: false, nextBefore: undefined });
  });
  it("a newest page never moves the cursor forward, and leaves a Load earlier in flight waiting for its own page", () => {
    const cursor = fresh();
    applyHistoryCursor(cursor, { has_earlier: true, next_before: 20 });
    cursor.loading = true;
    applyHistoryCursor(cursor, { has_earlier: true, next_before: 10 });
    cursor.loading = true;
    // The reattach's newest page lands while the next Load earlier is out.
    applyHistoryCursor(cursor, { has_earlier: true, next_before: 20 });
    expect(cursor).toMatchObject({ hasEarlier: true, nextBefore: 10, loading: true });
    applyHistoryCursor(cursor, { has_earlier: true, next_before: 5 });
    expect(cursor).toMatchObject({ nextBefore: 5, loading: false });
  });
});
