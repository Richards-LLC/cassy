import { machineFooterMarkup, pairedMachinesDialogMarkup, renderPairedMachines } from '../src/paired-machines';
import { ConversationList, type ConversationRow } from '../src/conversation-list';
import { ConversationHistory } from '../src/conversation-history';
import { ConversationView } from '../src/conversation-view';
import { conversationShellMarkup } from '../src/conversation-shell';
import { installAttentionObjects, renderAskObject, renderBlockerObject } from '../src/attention-objects';
import './pairs.css';

// Pebble 3: ask and blocker paint as the fused-tray objects in every fixture.
installAttentionObjects();

const ASK_TEXT = 'Gate run 33512 failed on that one warning. Fix it in-train — one worker, about ten minutes — or ship 3.26.0 with it allowlisted?';
const BLOCKER_TEXT = 'The release gate went red. The train is held; nothing was tagged or pushed to main.\nattention.rs:212 · needless_borrow';

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
  // The evidence state opens the Studio Mac thread so the supervisor pebbles take the second accent.
  const machine = state === 'conversation-evidence'
    ? { id: 'studio-mac', label: 'Studio Mac', host: 'Studio Mac · macOS', projectDir: '/projects/gabber-studio', project: 'gabber-studio' }
    : { id: 'atlas-linux', label: 'Atlas', host: 'Atlas · Linux', projectDir: '/projects/cas-src', project: 'cas-src' };
  app.innerHTML = conversationShellMarkup({ selected, supervisor, projectDir: machine.projectDir, host: machine.host, machineId: machine.id, loaded: true, paired: true });
  const listRows = fixtureConversationRows(selected);
  new ConversationList().render(app.querySelector('#conversation-list')!, listRows, () => {});
  const machines = FIXTURE_MACHINES;
  // The footer counts conversations, not machines: it must equal the rows rendered above.
  app.querySelector('#hub-footer-badges')!.innerHTML = machineFooterMarkup(machines, listRows.length, 'fixture');
  app.insertAdjacentHTML('beforeend', pairedMachinesDialogMarkup());
  renderPairedMachines(app.querySelector('#paired-machines-list')!, machines, async () => {});
  const dialog = app.querySelector<HTMLDialogElement>('#paired-machines-dialog')!;
  app.querySelector<HTMLElement>('#paired-machines-toggle')!.onclick = () => dialog.showModal();
  app.querySelector<HTMLElement>('#paired-machines-close')!.onclick = () => dialog.close();
  if (state === 'paired-machines') dialog.showModal();
  if (!selected) return;
  if (state === 'conversation-pairs') { renderPairsSheet(app, supervisor); return; }
  const history = new ConversationHistory();
  // Fixture clock: 09:41 today, so day and group timestamps are deterministic.
  const today = new Date(); today.setHours(9, 41, 0, 0);
  const at = (hh: number, mm: number) => new Date(today.getFullYear(), today.getMonth(), today.getDate(), hh, mm).getTime();
  const reply = (notification_id: number, reply_to: number | null, message: string, kind: 'answer' | 'status' | 'receipt' | 'ask' | 'blocker', when: number) =>
    history.reply({ notification_id, reply_to, message, summary: '', device_id: 'fixture', operator_label: 'Daniel', kind }, when);
  let working = false;
  if (state === 'conversation-thread') {
    // thread-a: directive, answer, receipt, coalesced statuses, question, answer, ask (Pebble 3 fallback), working.
    history.submit('directive', supervisor, 'Merge the two green lanes, then cut 3.26.0 once the gate is green.', at(9, 41));
    history.acknowledge({ client_ref: 'directive', notification_id: 41, target: supervisor, stamped: true });
    reply(42, 41, 'On it. Both lanes are green on their own CI — the gate starts after the second merge lands.', 'answer', at(9, 44));
    reply(43, 41, 'Both lanes are on the epic branch. The release gate is running.', 'receipt', at(9, 47));
    reply(44, null, 'Gate started · 0 of 14 targets', 'status', at(9, 49));
    reply(45, null, 'Gate 4 of 14 targets green', 'status', at(9, 51));
    reply(46, null, 'Gate 8 of 14 targets green', 'status', at(9, 53));
    reply(47, null, 'gate 11 of 14 targets green', 'status', at(9, 55));
    history.submit('question', supervisor, 'Did the tokens drift test move?', at(9, 56));
    history.acknowledge({ client_ref: 'question', notification_id: 48, target: supervisor, stamped: true });
    reply(49, 48, "No — unchanged since 3.25.3. The gate's only new failure is one lint warning.", 'answer', at(9, 57));
    reply(50, null, ASK_TEXT, 'ask', at(9, 58));
    working = true;
  } else if (state === 'conversation-ask' || state === 'conversation-ask-answered') {
    // thread-a's tail: the blocker with its evidence window, then the ask.
    // Unanswered, the ask is pinned above the composer with its chips
    // (the payload carries options here); answered, the tray shows the sent reply.
    history.submit('directive', supervisor, 'Merge the two green lanes, then cut 3.26.0 once the gate is green.', at(9, 41));
    history.acknowledge({ client_ref: 'directive', notification_id: 41, target: supervisor, stamped: true });
    reply(42, 41, 'On it. Both lanes are green on their own CI — the gate starts after the second merge lands.', 'answer', at(9, 44));
    reply(43, 41, 'Both lanes are on the epic branch. The release gate is running.', 'receipt', at(9, 47));
    reply(44, null, 'Gate started · 0 of 14 targets', 'status', at(9, 49));
    reply(47, null, 'gate 11 of 14 targets green', 'status', at(9, 55));
    reply(49, null, "No — unchanged since 3.25.3. The gate's only new failure is one lint warning.", 'answer', at(9, 57));
    reply(51, null, BLOCKER_TEXT, 'blocker', at(9, 58));
    history.reply({ notification_id: 52, reply_to: null, message: ASK_TEXT, summary: '', device_id: 'fixture', operator_label: 'Daniel', kind: 'ask', options: ['Fix in-train', 'Ship with allowlist'] }, at(9, 58));
    if (state === 'conversation-ask-answered') {
      history.submit('answer', supervisor, 'Fix in-train', at(10, 2), 52);
      history.acknowledge({ client_ref: 'answer', notification_id: 53, target: supervisor, stamped: true });
      reply(54, 53, 'Spawning one worker on the warning now.', 'answer', at(10, 3));
      working = true;
    }
  } else if (state === 'conversation-blocker') {
    // A blocker alone: nothing answers it, so the row waits and the composer stays plain.
    history.submit('directive', supervisor, 'Cut 3.26.0 once the gate is green.', at(9, 41));
    history.acknowledge({ client_ref: 'directive', notification_id: 41, target: supervisor, stamped: true });
    reply(42, 41, 'Gate is running.', 'answer', at(9, 44));
    reply(51, null, BLOCKER_TEXT, 'blocker', at(9, 58));
  } else if (state === 'conversation-evidence') {
    history.submit('flake', supervisor, 'Did pass two clear the flake?', at(9, 28));
    history.acknowledge({ client_ref: 'flake', notification_id: 61, target: supervisor, stamped: true });
    reply(62, 61, ['Yes — pass two is green. Every pack:', '', '| pack | cases | result |', '| --- | --- | --- |', '| core | 412 | pass |', '| ui | 388 | pass |', '| net | 211 | pass |', '| store | 174 | pass |', '| hooks | 96 | pass |', '| mcp | 143 | pass |', '| hub | 260 | pass |', '| cli | 318 | 1 flake |'].join('\n'), 'answer', at(9, 30));
    reply(63, 61, 'Tagged gabber-studio v2.4.1 and pushed.', 'receipt', at(9, 31));
    working = true;
  } else if (state !== 'conversation') {
    history.submit('fixture', supervisor, 'Please keep the project badge prominent.', at(9, 41));
    if (state === 'conversation-error') history.reject('fixture', 'The session no longer grants this device control.');
    else {
      history.acknowledge({ client_ref: 'fixture', notification_id: 41, target: supervisor, stamped: true });
      reply(42, 41, 'The project badge stays visible in the list and conversation header.', 'answer', at(9, 43));
    }
  }
  // Fixture respond: record the chip as an operator send answering the ask, exactly as main.ts does after the hub accepts it.
  const view = new ConversationView(document, history, { supervisor, machine: machine.label, project: machine.project, header: false, working: () => working, editMessage: () => {}, respond: (ask, text) => { history.submit(`quick-${ask.notification_id}`, supervisor, text, Date.now(), ask.notification_id); view.update(); } });
  app.querySelector('#conversation-pane-slot')!.append(view.element); view.update();
  app.querySelector('#conversation-composer-slot')!.innerHTML = `<div class="message conversation-composer"><h2><label for="message-text">Your message</label></h2><textarea id="message-text" placeholder="Write to ${supervisor}…"></textarea><div class="composer-actions"><button id="message-send" class="primary" type="button">Send to ${supervisor}</button></div></div>`;
  app.querySelector('#conversation-composer-slot')!.prepend(view.pinned);
  app.querySelector('#conversation-status-slot')!.innerHTML = '<p class="conversation-host">Supervisor conversations<br>In progress</p>';
  app.querySelector('#conversation-attention-slot')!.innerHTML = '<h2>Attention</h2><p class="conversation-host">One request needs your direction.</p>';
}

/** pairs.html: each attention object beside a calm pebble for scale. */
function renderPairsSheet(app: HTMLElement, supervisor: string): void {
  const history = new ConversationHistory();
  const ask = { notification_id: 1, reply_to: null, message: ASK_TEXT, summary: '', device_id: 'fixture', kind: 'ask' as const, options: ['Fix in-train', 'Ship with allowlist'], attachments: [] };
  const blocker = { notification_id: 2, reply_to: null, message: BLOCKER_TEXT, summary: '', device_id: 'fixture', kind: 'blocker' as const, attachments: [] };
  const context = (reply: typeof ask | typeof blocker) => ({ document, supervisor, reply, history, turn: { key: `reply:${reply.notification_id}`, side: 'supervisor' as const, kind: reply.kind, event: { kind: 'reply' as const, value: reply }, first: true, last: true }, body: () => { const p = document.createElement('p'); p.textContent = reply.message; return [p]; }, respond: () => {} });
  const calm = (text: string) => { const bub = document.createElement('div'); bub.className = 'bub group-first group-last'; const p = document.createElement('p'); p.textContent = text; bub.append(p); return bub; };
  const one = (object: HTMLElement, caption: string) => { const node = document.createElement('div'); node.className = 'one'; const cap = document.createElement('span'); cap.className = 'cap'; cap.textContent = caption; node.append(object, cap); return node; };
  const pair = (title: string, ...ones: HTMLElement[]) => { const section = document.createElement('section'); section.className = 'pair'; const h2 = document.createElement('h2'); h2.textContent = title; const side = document.createElement('div'); side.className = 'side'; side.append(...ones); section.append(h2, side); return section; };
  const sheet = document.createElement('div'); sheet.className = 'pairs thread';
  sheet.append(
    pair('Ask — silhouette', one(renderAskObject(ask, context(ask)), 'A · fused tray: body steps down into a deeper tray that holds the replies, one outline'), one(calm('Both lanes are on the epic branch.'), 'for scale · an ordinary calm pebble')),
    pair('Blocker — silhouette', one(renderBlockerObject(blocker, context(blocker)), 'A · fused object with an inset window for the evidence'), one(calm('Nothing was tagged or pushed to main.'), 'for scale · an ordinary calm pebble')),
  );
  app.querySelector('#conversation-pane-slot')!.append(sheet);
  app.querySelector('#conversation-composer-slot')!.innerHTML = '';
}
