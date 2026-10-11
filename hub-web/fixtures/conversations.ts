import { machineFooterMarkup, orderPairedMachines, pairedMachinesDialogMarkup, renderPairedMachines } from '../src/paired-machines';
import { fleetControlGate } from '../src/fleet-permissions';
import type { Scope } from '../src/types';
import { ConversationList, groupConversationRows, type ConversationRow } from '../src/conversation-list';
import { ConversationHistory } from '../src/conversation-history';
import { ConversationView } from '../src/conversation-view';
import { applyActionAvailability, applyKeyboardViewport, conversationListState, conversationShellMarkup, conversationSkeletonMarkup, dressComposer, ensureConversationStage, fitConversationHost, keyboardViewportHeight, rawOutputDrawerMarkup } from '../src/conversation-shell';
import { CONVERSATION_OPENING, fatalConnectionRecovery, lostConnectionBanner, pairingControlsReason, renderConnectionSurfaceInto } from '../src/connection-state-view';
import { TranscriptView, type TranscriptSource } from '../src/transcript-view';
import type { GhosttyCell, GhosttyColor, GhosttyRow } from '../src/terminal/ghostty/core';
import { applyDraftNote, applyMicState, composerMarkup, type MicState } from '../src/composer-markup';
import { syncContextRail } from '../src/context-rail';
import { installAttentionObjects, renderAskObject, renderBlockerObject } from '../src/attention-objects';
import { installAttachmentSheet } from '../src/attachment-sheet';
import type { ArtifactRef } from '../src/types';
import './pairs.css';
// Real operator row 20812 (1,079 characters): the long status D2 measured.
import LONG_STATUS from './hub-row-20812.txt?raw';

// Pebble 3: ask and blocker paint as the fused-tray objects in every fixture.
installAttentionObjects();
// Pebble 4: artifacts are dog-eared sheets in every fixture, as in the app.
installAttachmentSheet();

const ASK_TEXT = 'Gate run 33512 failed on that one warning. Fix it in-train — one worker, about ten minutes — or ship 3.26.0 with it allowlisted?';
const BLOCKER_TEXT = 'The release gate went red. The train is held; nothing was tagged or pushed to main.\nattention.rs:212 · needless_borrow';

/** The report-card case (cassy#910): one supervisor turn, two artifacts. */
export const REPORT_CARD_ATTACHMENTS: ArtifactRef[] = [
  { artifact_id: 'report/3.26.0/brief.pdf', name: '3.26.0 release brief.pdf', mime: 'application/pdf', size_bytes: 1_468_006, sha256: '9f2c6b1e4d7a3c5f8e0b2d4a6c8e1f3a5b7d9f0c2e4a6b8d0f1a3c5e7b9d1f2a' },
  { artifact_id: 'report/3.26.0/card.html', name: 'Release report card.html', mime: 'text/html', size_bytes: 88_064, sha256: '1a3c5e7b9d1f2a4c6e8b0d2f4a6c8e0b1d3f5a7c9e1b3d5f7a9c1e3b5d7f9a2c' },
];

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
    { ...base, key: 'studio-mac:two', machineId: 'studio-mac', session: 'two', supervisor: 'calm-otter-4', projectDir: '/projects/gabber-studio', host: 'Studio Mac · macOS', when: '09:41', preview: 'Pass two is green — every pack in one place.', unread: 2 },
    { ...base, key: 'atlas-linux:three', machineId: 'atlas-linux', session: 'three', supervisor: 'steady-heron-2', projectDir: '/projects/petra-stella-cloud', host: 'Atlas · Linux', when: 'Tue', freshness: 'Catalog checked 1m ago', preview: 'Preview is up for the alias merge.' },
    { ...base, key: 'studio-mac:four', machineId: 'studio-mac', session: 'four', supervisor: 'quiet-marten-7', projectDir: '/projects/openclaw', host: 'Studio Mac · macOS', when: 'Tue', preview: 'Rebased and pushed; nothing waiting.' },
    { ...base, key: 'bench-1:five', machineId: 'bench-1', session: 'five', supervisor: 'bright-otter-3', projectDir: '/projects/violet', host: 'Bench · Linux', when: 'Mon', freshness: 'Catalog checked 2h ago', preview: 'Posted both threads to the channel.' },
    { ...base, key: 'bench-1:six', machineId: 'bench-1', session: 'six', supervisor: 'calm-heron-5', projectDir: '/projects/cas-hub-static', host: 'Bench · Linux', when: 'Mon', freshness: 'Catalog checked 2h ago', preview: 'Nothing waiting on you.' },
  ];
}

/**
 * cas-55a4: three live gabber-studio sessions on one machine, grouped under
 * one heading with the most recent marked; this device may end them.
 */
export function fixtureSessionRows(): ConversationRow[] {
  const base = { connection: 'Live', attention: 0, unread: 0, selected: false, projectDir: '/projects/gabber-studio', machineId: 'atlas-linux', host: 'Atlas · Linux', canEnd: true };
  const now = Date.now();
  return groupConversationRows([
    { ...base, key: 'atlas-linux:noble', session: 'gabber-studio-noble-cheetah-84', supervisor: 'noble-cheetah-84', when: '14h', freshness: 'Last activity 14h ago · relay-watchdog → Commander', activityAt: now - 14 * 3_600_000, preview: 'Mixdown preview rendered: 3 stems.' },
    { ...base, key: 'atlas-linux:puma', session: 'gabber-studio-calm-puma-34', supervisor: 'calm-puma-34', when: '2m', freshness: 'Last activity 2m ago · supervisor → bright-robin-85', activityAt: now - 120_000, connection: 'Live', selected: true },
    { ...base, key: 'atlas-linux:shark', session: 'gabber-studio-wild-shark-68', supervisor: 'wild-shark-68', when: '40m', freshness: 'Last activity 40m ago · Commander → supervisor', activityAt: now - 2_400_000, preview: 'Stem export is at 60%.' },
    ...fixtureConversationRows(false).slice(0, 1).map((row) => ({ ...row, canEnd: false })),
  ]);
}

/**
 * cas-d6bf: seven live sessions of one project on one machine, as the list a
 * phone opens on. Each can be ended; one has unread turns and one is waiting.
 */
export function fixtureManySessionRows(): ConversationRow[] {
  const base = { connection: 'Live', attention: 0, unread: 0, selected: false, projectDir: '/projects/gabber-studio', machineId: 'atlas-linux', host: 'Atlas · Linux', canEnd: true };
  const now = Date.now();
  const names = ['calm-puma-34', 'wild-shark-68', 'noble-cheetah-84', 'quiet-otter-12', 'brave-lynx-7', 'swift-heron-51', 'amber-fox-29'];
  const previews = ['Stem export is at 60%.', 'Mixdown preview rendered: 3 stems.', 'Waiting on your approval for the mastering chain.', 'Nothing waiting on you.', 'Re-rendered the vocal bus.', 'Live', 'Live'];
  return groupConversationRows(names.map((supervisor, index) => ({
    ...base,
    key: `atlas-linux:${supervisor}`,
    session: `gabber-studio-${supervisor}`,
    supervisor,
    when: `${(index + 1) * 7}m`,
    freshness: `Last activity ${(index + 1) * 7}m ago`,
    activityAt: now - (index + 1) * 420_000,
    preview: previews[index],
    ...(index === 1 ? { unread: 2 } : {}),
    ...(index === 2 ? { attention: 1 } : {}),
  })));
}

export function renderConversationFixture(app: HTMLElement, state: string): void {
  const sessions = state === 'conversation-sessions' || state === 'conversation-earlier';
  const supervisor = sessions ? 'calm-puma-34' : FIXTURE_SUPERVISOR;
  const selected = !['conversations-list', 'conversations-sessions', 'conversations-session-ended', 'conversations-session-end-error', 'conversations-loading', 'conversations-unpaired', 'paired-machines', 'paired-machines-down', 'conversations-machine-down-long', 'conversations-machine-label-overlong'].includes(state);
  // Catalog loading: nothing is known yet, so no machine, no row, no pairing offer.
  const loading = state === 'conversations-loading';
  // First run: the catalog is loaded and empty, so the welcome offers pairing.
  const unpaired = state === 'conversations-unpaired';
  // The evidence state opens the Studio Mac thread so the supervisor pebbles
  // take the second accent; the empty state opens Bench (third accent) as in
  // empty.html.
  const machine = state === 'conversation-evidence'
    ? { id: 'studio-mac', label: 'Studio Mac', host: 'Studio Mac · macOS', projectDir: '/projects/gabber-studio', project: 'gabber-studio' }
    : state === 'conversation-empty'
      ? { id: 'bench-1', label: 'Bench', host: 'Bench · Linux', projectDir: '/projects/cas-hub-static', project: 'cas-hub-static' }
    // cas-1451 / cas-6b75: an empty session on a machine with a long name, as
    // HUB-J9's rack: the card keeps the machine whole ahead of the codename,
    // and the header's Raw output and Interrupt stay full targets on a phone.
    : state === 'conversation-empty-long-machine'
      ? { id: 'bench-1', label: 'Build Server Rack Seven · Windows', host: 'Build Server Rack Seven · Windows', projectDir: '/projects/infra', project: 'infra' }
      : state === 'conversation-sessions' || state === 'conversation-earlier'
        ? { id: 'atlas-linux', label: 'Atlas', host: 'Atlas · Linux', projectDir: '/projects/gabber-studio', project: 'gabber-studio' }
      : { id: 'atlas-linux', label: 'Atlas', host: 'Atlas · Linux', projectDir: '/projects/cas-src', project: 'cas-src' };
  app.innerHTML = conversationShellMarkup({ selected, supervisor, projectDir: machine.projectDir, host: machine.host, machineId: machine.id, loaded: !loading, paired: !loading && !unpaired });
  // cas-b452: Atlas's pairing was revoked with its conversation open; its rows stay listed, reading Needs pairing.
  const needsPairing = state === 'conversation-needs-pairing';
  const listRows = loading || unpaired ? [] : sessions ? fixtureSessionRows() : state === 'conversations-sessions' || state === 'conversations-session-ended' || state === 'conversations-session-end-error' ? fixtureManySessionRows()
    : needsPairing ? fixtureConversationRows(selected).map((row) => row.machineId === 'atlas-linux' ? { ...row, connection: 'Needs pairing', interrupted: true } : row)
    : fixtureConversationRows(selected);
  const conversationList = new ConversationList();
  const listNode = app.querySelector<HTMLElement>('#conversation-list')!;
  // Ending a session drops its row, as main.ts does once the hub confirms.
  let shownRows = listRows;
  const endSession = async (ended: ConversationRow): Promise<void> => {
    if (state === 'conversations-session-end-error') throw new Error('End session request failed (500)');
    shownRows = groupConversationRows(shownRows.filter((row) => row.key !== ended.key));
    conversationList.render(listNode, shownRows, () => {}, endSession);
  };
  conversationList.render(listNode, listRows, () => {}, endSession);
  // The End session confirmation, open on the idle session (cas-55a4).
  if (state === 'conversation-sessions') app.querySelectorAll<HTMLButtonElement>('#conversation-list .conversation-end-ask')[1]?.click();
  // cas-f60a: noble-cheetah-84 just ended; the list says so where its row was.
  if (state === 'conversations-session-ended' || state === 'conversations-session-end-error') {
    const control = [...listNode.querySelectorAll<HTMLElement>('.conversation-end')].find((node) => node.previousElementSibling?.textContent?.includes('noble-cheetah-84'));
    control?.querySelector<HTMLButtonElement>('.conversation-end-ask')?.click();
    const confirm = control?.querySelector<HTMLButtonElement>('.conversation-end-confirm');
    if (state === 'conversations-session-end-error') confirm?.focus();
    confirm?.click();
  }
  // cas-0739: Bench can't be reached; the register lists it first and the footer names it.
  const machines = loading || unpaired ? [] : state === 'paired-machines-down'
    ? orderPairedMachines(FIXTURE_MACHINES.map((machine) => machine.id === 'bench-1' ? { ...machine, connection: "Can't reach · retrying", connected: false, lastSeen: 'Not yet seen in this visit' } : machine))
    : state === 'conversations-machine-down-long'
      // cas-0739 QA round 1: a long machine name down; the footer state must
      // not collide with the label (phone) or squeeze it (desktop).
      ? orderPairedMachines(FIXTURE_MACHINES.map((machine) => machine.id === 'bench-1' ? { ...machine, label: 'Build Server Rack Seven Downstairs · Windows', connection: "Can't reach · retrying", connected: false, lastSeen: 'Not yet seen in this visit' } : machine))
      : state === 'conversations-machine-label-overlong'
        // cas-c19d: one connected machine whose name is about 691px wide at
        // 390, so the phone footer shows the label itself beside its state.
        ? [{ ...FIXTURE_MACHINES[0]!, label: 'soundwave — a very long personal workstation name with several extra words and anunbrokentailthatneedstowrap' }]
      : needsPairing
        ? orderPairedMachines(FIXTURE_MACHINES.map((machine) => machine.id === 'atlas-linux' ? { ...machine, connection: 'Needs pairing', connected: false } : machine))
        : FIXTURE_MACHINES;
  // The list's empty line, exactly as main.ts renderConversationList sets it.
  const empty = app.querySelector<HTMLElement>('#conversation-empty')!;
  empty.hidden = listRows.length > 0;
  const listState = conversationListState(!loading, machines.map(() => ({ catalogReceived: true, phase: 'live' })));
  if (listState.kind === 'loading') empty.innerHTML = conversationSkeletonMarkup();
  else empty.textContent = listState.text;
  empty.dataset.state = listState.kind;
  // The footer counts conversations, not machines: it must equal the rows rendered above.
  app.querySelector('#hub-footer-badges')!.innerHTML = machineFooterMarkup(machines, listRows.length, 'fixture', loading);
  app.insertAdjacentHTML('beforeend', pairedMachinesDialogMarkup());
  // cas-d382: the register names each pairing's fleet permissions: one holds
  // both, one is a control pairing (may allow managing workers, Stop and
  // restart is not allowed), one is read-only (commands for both).
  const fleetScopes: Scope[][] = [
    ['machine-read', 'session-read', 'pane-read', 'pane-input', 'message-send', 'pane-interrupt', 'factory-operate', 'factory-manage'],
    ['machine-read', 'session-read', 'pane-read', 'pane-input', 'message-send', 'pane-interrupt'],
    ['machine-read', 'session-read', 'pane-read'],
  ];
  const registerRows = state === 'paired-machines'
    ? machines.map((machine, index) => {
      const scopes = fleetScopes[index % fleetScopes.length]!;
      return { ...machine, fleet: { operate: fleetControlGate(scopes, 'add-workers', 'https://commander.example'), manage: fleetControlGate(scopes, 'stop-worker', 'https://commander.example') } };
    })
    : machines;
  renderPairedMachines(app.querySelector('#paired-machines-list')!, registerRows, async () => {});
  const dialog = app.querySelector<HTMLDialogElement>('#paired-machines-dialog')!;
  app.querySelector<HTMLElement>('#paired-machines-toggle')!.onclick = () => dialog.showModal();
  app.querySelector<HTMLElement>('#paired-machines-close')!.onclick = () => dialog.close();
  if (state === 'paired-machines' || state === 'paired-machines-down') dialog.showModal();
  if (!selected) return;
  if (state === 'conversation-pairs') { renderPairsSheet(app, supervisor); return; }
  const history = new ConversationHistory();
  // Fixture clock: 09:41 today, so day and group timestamps are deterministic.
  const today = new Date(); today.setHours(9, 41, 0, 0);
  const at = (hh: number, mm: number) => new Date(today.getFullYear(), today.getMonth(), today.getDate(), hh, mm).getTime();
  const reply = (notification_id: number, reply_to: number | null, message: string, kind: 'answer' | 'status' | 'receipt' | 'ask' | 'blocker', when: number, attachments?: ArtifactRef[]) =>
    history.reply({ notification_id, reply_to, message, summary: '', device_id: 'fixture', operator_label: 'Daniel', kind, ...(attachments ? { attachments } : {}) }, when);
  let working = false;
  let echo: string | undefined;
  let draft = '';
  let loadingEarlier = false;
  if (state === 'conversation-long-status') {
    // D2: a real waiting-on-you update arriving as a status after an earlier one.
    history.submit('directive', supervisor, 'Where are we on the user-eye pilot?', at(13, 20));
    history.acknowledge({ client_ref: 'directive', notification_id: 20810, target: supervisor, stamped: true });
    reply(20811, null, 'Status 13:1xZ. Pilot run 2 of 3 in flight.', 'status', at(13, 22));
    reply(20812, null, LONG_STATUS.trim(), 'status', at(13, 25));
  } else if (state === 'conversation-loading-earlier') {
    // The thread's only loading signal: an earlier page is on its way (D5).
    loadingEarlier = true;
    reply(42, null, 'On it. Both lanes are green on their own CI — the gate starts after the second merge lands.', 'answer', at(9, 44));
    reply(43, null, 'Both lanes are on the epic branch. The release gate is running.', 'receipt', at(9, 47));
  } else if (state === 'conversation-thread') {
    // thread-a: directive, answer, receipt, coalesced statuses, question, answer, ask (Pebble 3 fallback), working.
    history.submit('directive', supervisor, 'Merge the two green lanes, then cut 3.26.0 once the gate is green.', at(9, 41));
    history.acknowledge({ client_ref: 'directive', notification_id: 41, target: supervisor, stamped: true });
    reply(42, 41, 'On it. Both lanes are green on their own CI — the gate starts after the second merge lands.', 'answer', at(9, 44));
    reply(43, 41, 'Both lanes are on the epic branch. The release gate is running.', 'receipt', at(9, 47));
    reply(44, null, 'Gate started · 0 of 14 targets', 'status', at(9, 49));
    reply(45, null, 'Gate 4 of 14 targets green', 'status', at(9, 51));
    reply(46, null, 'Gate 8 of 14 targets green', 'status', at(9, 53));
    reply(47, null, 'gate 11 of 14 targets green', 'status', at(9, 55));
    // thread-a's sheet at 09:55, after the last status: an attachment-only
    // turn is its sheet alone. History is ordered by time (durable replay), so
    // a sheet stamped inside the 09:49–09:55 run would split the coalesced
    // statuses in two (cas-7294).
    reply(51, null, '', 'answer', at(9, 55), [REPORT_CARD_ATTACHMENTS[0]!]);
    history.submit('question', supervisor, 'Did the tokens drift test move?', at(9, 56));
    history.acknowledge({ client_ref: 'question', notification_id: 48, target: supervisor, stamped: true });
    reply(49, 48, "No — unchanged since 3.25.3. The gate's only new failure is one lint warning.", 'answer', at(9, 57));
    reply(50, null, ASK_TEXT, 'ask', at(9, 58));
    working = true;
  } else if (state === 'conversation-ask' || state === 'conversation-ask-answered' || state === 'conversation-keyboard') {
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
  } else if (state === 'conversation-dated') {
    // cas-e829: yesterday's blocker shows its date; a later message that does
    // not answer it says only that the operator has written since.
    reply(51, null, BLOCKER_TEXT, 'blocker', at(17, 20) - 86_400_000);
    history.submit('later', supervisor, 'Looking at the gate now.', at(9, 30));
    history.acknowledge({ client_ref: 'later', notification_id: 60, target: supervisor, stamped: true });
    reply(61, 60, 'The lint warning is the only failure.', 'answer', at(9, 33));
    // An answer to a question from yesterday's ended session stays here and names it.
    history.reply({ notification_id: 62, reply_to: 3196200, reply_to_session: 'gabber-studio-noble-cheetah-84', message: 'The mixdown preview you asked about yesterday is in renders/.', summary: '', device_id: 'fixture', kind: 'answer' }, at(9, 35));
  } else if (state === 'conversation-clock-ahead') {
    // cas-9e33 / cas-24fe: a reload of a thread whose machine clock runs five
    // minutes ahead. The first answer arrived live before anything showed the
    // lead, so it stays unmarked, as that visit showed it. The answer after
    // the reload arrived once the lead was known and says so. Times are
    // relative to now: a machine stamp is only "ahead" of this browser's clock.
    const AHEAD = 5 * 60_000;
    const now = Date.now();
    const first = { notification_id: 71, reply_to: null, message: 'The release gate is green; tagging 3.26.0 now.', summary: '', device_id: 'fixture', operator_label: 'Daniel', kind: 'answer' as const, attachments: [] };
    const visit = new ConversationHistory();
    visit.receive(first, now - 30 * 60_000);
    history.seedArrivals(visit.arrivalsRecord());
    history.hydrateReply({ ...first, at: new Date(now - 30 * 60_000 + AHEAD).toISOString() }, now - 28 * 60_000);
    history.submit('push', supervisor, 'Push the tag when the notes are ready.', now - 20 * 60_000);
    history.acknowledge({ client_ref: 'push', notification_id: 70, target: supervisor, stamped: true });
    history.receive({ notification_id: 72, reply_to: null, message: 'Tagged and pushed. The release notes are drafting.', summary: '', device_id: 'fixture', operator_label: 'Daniel', kind: 'receipt', attachments: [] }, now - 4 * 60_000);
  } else if (state === 'conversation-evidence') {
    history.submit('flake', supervisor, 'Did pass two clear the flake?', at(9, 28));
    history.acknowledge({ client_ref: 'flake', notification_id: 61, target: supervisor, stamped: true });
    reply(62, 61, ['## Pass two is green', '', '**All 14 targets passed** with *one recorded flake*.', '', '- `cargo check` stayed green', '- Review the [receipt](https://example.com/reports/pass-two).', '  - No retry was needed', '', '1. Tagged gabber-studio v2.4.1', '2. Pushed the release branch.', '', 'Every pack:', '', '| pack | cases | result |', '| --- | --- | --- |', '| core | 412 | pass |', '| ui | 388 | pass |', '| net | 211 | pass |', '| store | 174 | pass |', '| hooks | 96 | pass |', '| mcp | 143 | pass |', '| hub | 260 | pass |', '| cli | 318 | 1 flake |'].join('\n'), 'answer', at(9, 30));
    reply(63, 61, 'Tagged gabber-studio v2.4.1 and pushed.', 'receipt', at(9, 31));
    working = true;
  } else if (state === 'conversation-attachment') {
    // The report-card case: a receipt with prose, then its two artifacts laid on the thread as sheets.
    history.submit('report', supervisor, 'Send me the release report when the gate is green.', at(9, 40));
    history.acknowledge({ client_ref: 'report', notification_id: 70, target: supervisor, stamped: true });
    reply(71, 70, 'Gate green on all 14 targets. 3.26.0 is tagged and the brief is attached; the report card has the per-target timings.', 'receipt', at(9, 52), REPORT_CARD_ATTACHMENTS);
  } else if (state === 'conversation-empty' || state === 'conversation-empty-long-machine') {
    // empty.html: nothing in the thread, the last thing said as a faint echo.
    echo = 'Promoted the hub to production on Monday.';
  } else if (state === 'conversation-terminal') {
    // cas-5c89: the operator typed at the supervisor's terminal and never sent
    // from a phone. Each supervisor answer is mirrored and threads under the
    // terminal question it answers, so the phone shows both halves.
    const stamp = (hh: number, mm: number) => new Date(at(hh, mm)).toISOString();
    history.hydrateSend({ notification_id: 301, target: 'supervisor', text: 'What changed in Violet today?', state: 'acknowledged', stamped: false, device_id: 'terminal', operator_label: 'Terminal', at: stamp(13, 24) });
    history.hydrateReply({ notification_id: 302, reply_to: 301, message: '**Two fixes landed.**\n- Setup no longer loops on a token sign-in cannot create.\n- Claude Code now shows whether Slack accepts its key.', summary: '', device_id: '*', kind: 'answer', attachments: [], at: stamp(13, 25) });
    history.hydrateSend({ notification_id: 303, target: 'supervisor', text: 'Is the release branch green?', state: 'acknowledged', stamped: false, device_id: 'terminal', operator_label: 'Terminal', at: stamp(13, 34) });
    history.hydrateReply({ notification_id: 304, reply_to: 303, message: 'Yes. Every check passed on the release branch; the tag goes out after the notes are reviewed.', summary: '', device_id: '*', kind: 'answer', attachments: [], at: stamp(13, 35) });
  } else if (state === 'conversation-sessions' || state === 'conversation-earlier') {
    // cas-55a4: other sessions' turns, beside the thread and never in it.
    const own = 'gabber-studio-calm-puma-34';
    history.currentSession = own;
    const stamp = (daysAgo: number, hh: number, mm: number) => new Date(at(hh, mm) - daysAgo * 86_400_000).toISOString();
    history.hydrateSend({ notification_id: 101, target: 'supervisor', text: 'Render the mixdown preview.', state: 'acknowledged', stamped: true, device_id: 'fixture', operator_label: 'Pixel 10', session: 'gabber-studio-noble-cheetah-84', at: stamp(1, 21, 30) });
    history.hydrateReply({ notification_id: 102, reply_to: null, message: 'Mixdown preview rendered: 3 stems.', summary: '', device_id: 'fixture', kind: 'answer', attachments: [], session: 'gabber-studio-noble-cheetah-84', at: stamp(1, 21, 41) });
    history.hydrateReply({ notification_id: 103, reply_to: null, message: 'The supervisor (sharp-stork-98) was told 9 minutes ago that cas-c3f1 is in progress — worker died: daring-robin-43.', summary: '', device_id: 'fixture', kind: 'blocker', attachments: [], session: 'gabber-studio-wise-lion-31', at: stamp(2, 17, 20) });
    if (state === 'conversation-earlier') {
      history.hydrateSend({ notification_id: 201, target: 'supervisor', text: 'Status on the stem export?', state: 'acknowledged', stamped: true, device_id: 'fixture', operator_label: 'Pixel 10', session: own, at: new Date(at(9, 30)).toISOString() });
      history.hydrateReply({ notification_id: 202, reply_to: 201, message: 'Stem export is at 60%; the bass stem is next.', summary: '', device_id: 'fixture', kind: 'answer', attachments: [], session: own, at: new Date(at(9, 33)).toISOString() });
    }
  } else if (state === 'conversation-composer') {
    // A phone composer mid-draft: the field holds text, the send pill is in the accent.
    reply(80, null, 'Rebased and pushed; nothing waiting.', 'answer', at(9, 30));
    draft = 'Cut 3.26.0 once the gate is green, then post the release notes.';
  } else if (state === 'conversation-draft-too-long') {
    // cas-adfc: a draft over the store's 64k bound; the composer says it will not survive a reload.
    reply(80, null, 'Rebased and pushed; nothing waiting.', 'answer', at(9, 30));
    draft = 'Release notes for 3.26.0, pasted whole. '.repeat(1_800);
  } else if (state === 'conversation-draft-not-saved') {
    // cas-f657: an ordinary draft the browser's full storage refused; the composer says it won't survive a reload.
    reply(80, null, 'Rebased and pushed; nothing waiting.', 'answer', at(9, 30));
    draft = 'Ship it once the Mac lane is green, and tag 3.26.1.';
  } else if (state === 'conversation-unconfirmed-dismissed') {
    // cas-6a96 (journey F36): three not confirmed and dismissed, then a run of
    // two still in the thread. The thread notice counts its two; the chip
    // counts the three dismissed and says so.
    for (const [id, text, mm] of [['a', 'Is the gate green?', 30], ['b', 'Did the Mac tests start?', 31], ['c', 'Ship it if both are green', 32]] as const) {
      history.submit(id, supervisor, text, at(9, mm));
      history.unconfirmSilent(at(9, mm) + 20_000);
    }
    for (const id of ['a', 'b', 'c']) history.dismissSend(id);
    history.submit('d', supervisor, 'Status, please', at(9, 34));
    history.acknowledge({ client_ref: 'd', notification_id: 90, target: supervisor, stamped: true });
    for (const [id, text, mm] of [['e', 'Gate still red?', 36], ['f', 'Hold the train', 37]] as const) {
      history.submit(id, supervisor, text, at(9, mm));
      history.unconfirmSilent(at(9, mm) + 20_000);
    }
  } else if (state === 'conversation-opening') {
    // cas-813a: the one opening line, while the first history page is on its way.
  } else if (state !== 'conversation') {
    history.submit('fixture', supervisor, 'Please keep the project badge prominent.', at(9, 41));
    if (state === 'conversation-error') history.reject('fixture', 'The session no longer grants this device control.');
    else {
      history.acknowledge({ client_ref: 'fixture', notification_id: 41, target: supervisor, stamped: true });
      reply(42, 41, 'The project badge stays visible in the list and conversation header.', 'answer', at(9, 43));
    }
  }
  // Fixture respond: record the chip as an operator send answering the ask, exactly as main.ts does after the hub accepts it.
  const sessionFixture = state === 'conversation-sessions';
  const view = new ConversationView(document, history, { supervisor, machine: machine.label, project: machine.project, header: false, working: () => working, echo: () => echo, ...(sessionFixture || state === 'conversation-empty-long-machine' ? { activity: () => ({ at: Date.now() - 120_000, label: 'supervisor → bright-robin-85' }) } : {}), hasEarlier: () => loadingEarlier, loadingEarlier: () => loadingEarlier, loadingHistory: () => state === 'conversation-opening', openingSince: () => Date.now() - 5_000, editMessage: () => {}, retryMessage: (send) => { history.discardRefused(send.id); history.submit(`retry-${send.id}`, supervisor, send.text, Date.now(), send.replyTo); view.update(); }, respond: (ask, text) => { history.submit(`quick-${ask.notification_id}`, supervisor, text, Date.now(), ask.notification_id); view.update(); syncContextRail(app, { history, progress: false, attention: 0 }); } });
  const paneSlot = app.querySelector<HTMLElement>('#conversation-pane-slot')!;
  if (state === 'connection-failed-retry' || state === 'connection-fatal-browser') renderConnectionStage(paneSlot, view, state);
  else paneSlot.append(view.element);
  view.update();
  // The earlier session the operator opened to read (cas-55a4).
  if (sessionFixture) { const open = view.element.querySelector<HTMLDetailsElement>('details.earlier-session'); if (open) open.open = true; }
  // cas-0546: the header's Raw output and Interrupt, available or saying why not, as main.ts syncConversationActions leaves them.
  const reasons = fixtureActionReasons(state, machine.host);
  applyActionAvailability(app.querySelector<HTMLButtonElement>('#conversation-raw-output'), app.querySelector<HTMLElement>('#conversation-raw-output-reason'), reasons.rawOutput);
  applyActionAvailability(app.querySelector<HTMLButtonElement>('#conversation-interrupt'), app.querySelector<HTMLElement>('#conversation-interrupt-reason'), reasons.interrupt);
  // The host line is fitted to the header's room exactly as main.ts does after every render and resize.
  fitConversationHost(document);
  window.addEventListener('resize', () => fitConversationHost(document), { passive: true });
  // The app's own composer region, dressed the way arrangeConversationShell dresses it; the pinned ask mounts above it.
  const slot = app.querySelector<HTMLElement>('#conversation-composer-slot')!;
  // Dictation writes interim words into the field while the mic listens.
  if (state === 'conversation-mic-listening') draft = 'Cut 3.26.0 once the gate is';
  // The production composer markup (main.ts renders the same builder), mic included.
  slot.innerHTML = composerMarkup(supervisor);
  dressComposer(slot.querySelector<HTMLElement>('.message')!, supervisor, machine.project);
  applyMicState(slot.querySelector<HTMLButtonElement>('#message-mic')!, fixtureMicState(state));
  slot.querySelector<HTMLTextAreaElement>('#message-text')!.value = draft;
  applyDraftNote(slot, state === 'conversation-draft-too-long' ? 'too-long' : state === 'conversation-draft-not-saved' ? 'not-saved' : false);
  // The dismissed-messages chip sits at the top of the composer region, as main.ts places it.
  if (!view.unsent.hidden) slot.prepend(view.unsent);
  if (state === 'conversation-keyboard') {
    // A phone keyboard on a browser that ignores interactive-widget: the visual
    // viewport is 300px shorter than the layout one; the shell follows it and
    // the field holds a draft while the pinned ask stays in view (cas-edc9).
    // A landscape phone (short axis under 30rem) is left alone: Android hands
    // a rotated phone a full-screen editor, so no page layout is visible.
    applyKeyboardViewport(document, window.innerHeight >= 480 ? keyboardViewportHeight(window.innerHeight, { height: window.innerHeight - 300 }) : undefined);
    slot.querySelector<HTMLTextAreaElement>('#message-text')!.value = 'Fix it in-train, then';
  } else applyKeyboardViewport(document, undefined);
  slot.prepend(view.pinned);
  // The desktop context rail (P10) shows only what this thread's own data
  // supports: open asks/blockers and attachments. The fixtures report no
  // status and no attention events, so those sections stay absent, and a
  // thread with neither folds the rail to its 48px track.
  syncContextRail(app, { history, progress: false, attention: 0 });
  if (state === 'conversation-raw-output') openRawOutputFixture(machine.project);
}

/**
 * Why each header action can't run in a fixture state, in main.ts's words
 * (interruptUnavailableReason / rawOutputUnavailableReason); undefined is available.
 */
function fixtureActionReasons(state: string, host: string): { rawOutput?: string; interrupt?: string } {
  // cas-b452 / cas-6b75: the pairing is gone, so neither action can reach the machine.
  if (state === 'conversation-needs-pairing') return { rawOutput: pairingControlsReason(host), interrupt: pairingControlsReason(host) };
  // A first connection that failed: nothing has attached yet.
  if (state === 'connection-failed-retry') return { rawOutput: 'Raw output appears once the conversation is connected.', interrupt: 'Interrupt works once the conversation is connected.' };
  if (state === 'connection-fatal-browser') {
    const reason = `${fatalConnectionRecovery(FATAL_BROWSER_REASON)} Interrupt and raw output wait until then.`;
    return { rawOutput: reason, interrupt: reason };
  }
  // A pairing without pane-interrupt: Raw output reads, Interrupt says how to get the permission.
  if (state === 'conversation-interrupt-unavailable') return { interrupt: `This browser was paired without permission to interrupt. Run cas hub pair --origin ${window.location.origin} on ${host}, open the new pairing link here, and approve control access.` };
  return {};
}

const FATAL_BROWSER_REASON = 'This browser is missing AbortSignal.timeout, which Cassy Cloud needs. Update to Chrome 103, Edge 103, Firefox 100, or Safari 16 or newer.';

/**
 * The open conversation's grid as main.ts renderConnectionSurface leaves it:
 * a first connection that failed shows the connection card in place of the
 * thread; a lost connection keeps the thread under the disconnected banner.
 */
function renderConnectionStage(paneSlot: HTMLElement, view: ConversationView, state: 'connection-failed-retry' | 'connection-fatal-browser'): void {
  const grid = document.createElement('section');
  grid.id = 'pane-grid';
  grid.className = 'pane-grid';
  grid.dataset.sessionKey = 'atlas-linux:one';
  paneSlot.append(grid);
  if (state === 'connection-failed-retry') {
    const placeholder = document.createElement('div');
    placeholder.className = 'empty';
    grid.append(placeholder);
    const snapshot = { phase: 'failed' as const, stage: 'dialing' as const, since: Date.now() - 16_000, attempt: 3, reason: 'The machine did not answer its hub address.', retryInMs: 8_000, missedHeartbeats: 0, degraded: false };
    renderConnectionSurfaceInto(placeholder, 'one', snapshot, { retry: () => {}, diagnose: () => {} }, Date.now(), { openingTitle: CONVERSATION_OPENING, quietOpening: true });
    return;
  }
  ensureConversationStage(grid).slot.append(view.element);
  const banner = document.createElement('div');
  banner.className = 'terminal-disconnected-banner';
  banner.setAttribute('role', 'status');
  banner.dataset.scope = 'machine';
  const words = document.createElement('span');
  words.className = 'banner-text';
  words.textContent = lostConnectionBanner('Atlas · Linux', true, FATAL_BROWSER_REASON);
  banner.append(words);
  grid.prepend(banner);
  grid.classList.add('terminal-disconnected');
}

function transcriptRow(text: string, wrapsToNext = false, isWrapContinuation = false): GhosttyRow {
  const foreground: GhosttyColor = { r: 232, g: 235, b: 242 };
  const background: GhosttyColor = { r: 12, g: 14, b: 19 };
  const cell = (character: string): GhosttyCell => ({ text: character, wide: 0, foreground, background, bold: false, italic: false, invisible: false, strikethrough: false, overline: false, underline: false, selected: false });
  return { cells: [...text].map(cell), text, wrapsToNext, isWrapContinuation };
}

/**
 * The Raw output drawer open over the conversation (cas-0546), built as
 * main.ts rawOutputDialog/openRawOutput build it, over a supervisor screen
 * with markdown-looking lines, a wrapped run and a wide diagram row.
 */
function openRawOutputFixture(project: string): void {
  const dialog = document.createElement('dialog');
  dialog.id = 'raw-output';
  dialog.className = 'raw-output-drawer';
  dialog.setAttribute('aria-labelledby', 'raw-output-title');
  dialog.setAttribute('aria-describedby', 'raw-output-subject');
  dialog.innerHTML = rawOutputDrawerMarkup();
  document.body.append(dialog);
  dialog.querySelector<HTMLButtonElement>('.raw-output-close')!.onclick = () => dialog.close();
  dialog.querySelector<HTMLElement>('#raw-output-subject')!.textContent = `What the ${project} supervisor's terminal shows, as text. Read-only.`;
  const source: TranscriptSource = {
    rows: () => [
      transcriptRow('# Build result'),
      transcriptRow('const answer = 42;'),
      transcriptRow('⎿ Tool: exec_command'),
      transcriptRow('<unknown-block>unfamiliar output</unknown-block>'),
      transcriptRow(`┌${'─'.repeat(100)}┐`),
      transcriptRow('$ cas factory status'),
      transcriptRow('  › supervisor is coordinating six workers', true),
      transcriptRow('    across the Commander design pass', false, true),
      transcriptRow(''),
      transcriptRow('  › visual QA receipt is ready to review'),
    ],
    theme: () => ({ foreground: { r: 232, g: 235, b: 242 }, background: { r: 12, g: 14, b: 19 } }),
    hasScrollbackAbove: () => true,
    scrollRows: () => {},
    scrollToBottom: () => {},
  };
  const view = new TranscriptView(document, source);
  view.element.setAttribute('aria-label', 'Raw output');
  dialog.querySelector<HTMLElement>('.raw-output-body')!.append(view.element);
  dialog.showModal();
  document.querySelector<HTMLButtonElement>('#conversation-raw-output')?.setAttribute('aria-expanded', 'true');
  view.update();
}

/**
 * The mic as main.ts syncSpeechComposer leaves it: dictation available and
 * idle once detection settles; listening; or typing-only when the browser has
 * no speech recognition (the unavailable reason is the production sentence).
 */
function fixtureMicState(state: string): MicState {
  if (state === 'conversation-mic-listening') return { mode: 'speech', listening: true, detail: '' };
  if (state === 'conversation-mic-unavailable') return { mode: 'typing', listening: false, detail: '' };
  return { mode: 'speech', listening: false, detail: '' };
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
