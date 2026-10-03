import "../src/styles.css";
import { renderAttentionNoticeFixture } from "./attention-notice";
import { renderConversationFixture } from "./conversations";
import { attentionCounts, createAttentionItem } from "../src/attention";
import { renderAttentionPanel } from "../src/attention-view";
import { renderConnectionSurfaceInto } from "../src/connection-state-view";
import { DeferredRenderScheduler } from "../src/deferred-render";
import { FleetBoardRenderer, renderFleetBoardInto, type FleetBoardModel } from "../src/fleet-board";
import { pairingDialogCancellationActive } from "../src/pairing-dialog";
import { pairingExchangeFailure } from "../src/pairing-messages";
import { pairDialogMarkup } from "../src/pair-dialog-markup";
import { createPairingDraft } from "../src/pairing-draft";
import { DEFAULT_PAIRING_SCOPES } from "../src/pairing-relay";
import { renderDecision, shellSignature } from "../src/render-model";
import { operatorThreadMarkup } from "../src/operator-thread";
import { TranscriptView, type TranscriptSource } from "../src/transcript-view";
import type { AttentionItem, HubSession, Scope } from "../src/types";
import { LaunchSheet, type LaunchHost, type LaunchResult } from "../src/launch-session";
import type { GhosttyCell, GhosttyColor, GhosttyRow } from "../src/terminal/ghostty/core";

export const FIXTURE_NAMES = [
  "paired-machines", "paired-machines-down", "conversations-machine-down-long", "conversations-machine-label-overlong", "conversations-list", "conversation", "conversation-replied", "conversation-error",
  "conversation-thread", "conversation-evidence",
  "conversation-ask", "conversation-ask-answered", "conversation-blocker", "conversation-pairs",
  "conversation-attachment", "conversation-empty", "conversation-empty-long-machine", "conversation-composer", "conversation-keyboard",
  "conversation-sessions", "conversation-earlier", "conversation-dated", "conversation-clock-ahead", "conversations-sessions",
  "conversations-session-ended",
  "conversations-session-end-error",
  "conversation-needs-pairing",
  "fleet-populated",
  "fleet-twins",
  "fleet-empty",
  "session-canvas",
  "session-workers",
  "transcript",
  "attention-0",
  "attention-12",
  "attention-notice-details",
  "drawer-attention-open",
  "operator-thread",
  "connection-failed-retry",
  "pairing-step-1",
  "pairing-email",
  "pairing-code",
  "pairing-cleanup",
  "conversation-long-status",
  "conversation-loading-earlier",
  "conversation-opening",
  "conversation-mic-idle",
  "conversation-mic-listening",
  "conversation-mic-unavailable",
  "conversation-draft-too-long",
  "conversation-draft-not-saved",
  "conversation-unconfirmed-dismissed",
  "conversations-loading",
  "conversations-unpaired",
  "launch-form",
  "launch-browse",
  "launch-error",
  "launch-starting",
  "launch-grant",
  "launch-offline",
  "launch-account",
  "launch-account-unavailable",
  "launch-account-default-out",
  "launch-grant-command",
] as const;

export type FixtureName = (typeof FIXTURE_NAMES)[number];

const fixtureParam = new URLSearchParams(window.location.search).get("fixture") ?? "fleet-populated";
const fixtureName: FixtureName = FIXTURE_NAMES.includes(fixtureParam as FixtureName)
  ? fixtureParam as FixtureName
  : "fleet-populated";

const app = document.querySelector<HTMLDivElement>("#app");
if (!app) throw new Error("Fixture shell is missing #app");

function element<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

function button(label: string, className = ""): HTMLButtonElement {
  const node = element("button", className, label);
  node.type = "button";
  return node;
}

function session(name: string, supervisor: string, workers: string[], liveness: HubSession["liveness"] = "live"): HubSession {
  return { name, supervisor, workers, liveness };
}

function fleetModel(): FleetBoardModel {
  if (fixtureName === "fleet-twins") {
    const machines = [
      { id: "atlas", label: "Atlas · Linux", state: "live", phase: "Live", selected: true },
      { id: "attic", label: "Attic · Linux", state: "live", phase: "Live", selected: false },
      { id: "atlas2", label: "Atlas2 · Linux", state: "live", phase: "Live", selected: false },
    ];
    const names = ["brisk-otter-5", "patient-pelican-19", "patient-pelican-9"];
    return { machines, sessions: machines.flatMap((machine, index) =>
      (index === 0 ? names : index === 1 ? names.slice(0, 2) : names.slice(2)).map((name) => ({
        machineId: machine.id, machineLabel: machine.label, session: name, supervisor: name,
        project: "cas-src", role: "supervisor" as const, workerCount: 0, status: "live", current: false,
      }))) };
  }
  const machines = [
    { id: "atlas", label: "Atlas laptop", state: "live", phase: "Live", selected: true },
    { id: "forge", label: "Forge desktop", state: "degraded", phase: "Unsteady", selected: false },
  ];
  const entries = [
    { machineId: "atlas", machineLabel: "Atlas laptop", session: "bright-otter", role: "supervisor" as const, supervisor: "bright-otter", workerCount: 3, status: "live", title: "Commander design pass", phase: "editing" as const, current: false },
    { machineId: "atlas", machineLabel: "Atlas laptop", session: "calm-heron", role: "supervisor" as const, supervisor: "calm-heron", workerCount: 1, status: "live", title: "Release rehearsal", phase: "testing" as const, current: false },
    { machineId: "forge", machineLabel: "Forge desktop", session: "quiet-marten", role: "supervisor" as const, supervisor: "quiet-marten", workerCount: 6, status: "stale metadata", title: "Memory relevance audit", phase: "blocked" as const, current: false },
  ];
  return { machines, sessions: entries };
}

function attentionItems(count: number): AttentionItem[] {
  return Array.from({ length: count }, (_, index) => createAttentionItem({
    id: `event-${index + 1}`,
    machineId: "atlas",
    machineLabel: "Atlas laptop",
    session: "commander-session-with-a-long-codename",
    kind: index === 0 ? "daemon_disconnected" : index % 3 === 0 ? "checkpoint" : "reconnecting",
    createdAt: new Date(Date.now() - index * 60_000).toISOString(),
  }, {
    headline: index === 0 ? "Daemon connection lost" : `Connection attempt ${index + 1}`,
    detail: "The session remains available for inspection while the hub retries. Inspect the machine transport and keep this full explanation readable even on a phone.",
    severity: index === 0 ? "critical" : index % 3 === 0 ? "info" : "warning",
    action: index % 3 === 0 && index !== 0 ? "none" : "retry",
    fingerprint: `fixture-reconnecting-${index}`,
    payload: { fixture: fixtureName, event: index + 1 },
  }));
}

function renderRail(machineCount: number, drawerOpen = false): HTMLElement {
  const navigation = element("aside", `machine-navigation${drawerOpen ? " drawer-open" : ""}`);
  navigation.setAttribute("aria-label", "Machines and sessions");
  const rail = element("div", "machine-rail");
  const mark = button("", "rail-control commander-mark");
  mark.setAttribute("aria-label", "Open machines and sessions");
  mark.append(element("span", "commander-mark-label", "Machines"));
  rail.append(mark);
  for (let index = 0; index < machineCount; index += 1) {
    const machine = index === 0 ? "Atlas laptop" : "Forge desktop";
    const item = button(index === 0 ? "AL" : "FD", `machine-icon${index === 0 ? " active" : ""}`);
    item.setAttribute("aria-label", `${machine}, ${index === 0 ? "live" : "unsteady"}`);
    item.append(element("span", `machine-state ${index === 0 ? "live" : "degraded"}`));
    rail.append(item);
  }
  const pair = button("", "rail-control pair-machine");
  pair.setAttribute("aria-label", "Pair a machine");
  pair.append(element("span", undefined, "+"), element("span", "pair-machine-label", "Pair"));
  rail.append(pair);
  navigation.append(rail);
  const drawer = element("div", "machine-drawer");
  if (!drawerOpen) {
    drawer.setAttribute("aria-hidden", "true");
    drawer.setAttribute("inert", "");
  }
  const drawerHeader = element("header", "drawer-header");
  drawerHeader.append(element("strong"), button("", "drawer-close"));
  const drawerTree = element("nav");
  drawerTree.id = "machine-tree";
  drawerTree.setAttribute("aria-label", "Machine sessions");
  // cas-bad9: the open drawer lists every machine and the selected machine's
  // sessions, in main.ts machineTreeGroup's markup, so a phone render shows
  // whether anything paints over a row.
  if (drawerOpen) {
    for (const [index, machine] of ["Atlas laptop", "Forge desktop", "Studio Mac"].entries()) {
      const group = element("section", `machine-group${index === 0 ? " active" : ""}`);
      const row = button("", "machine-row");
      row.append(element("span", `machine-state ${index === 1 ? "degraded" : "live"}`), element("strong", undefined, machine), element("small", undefined, index === 1 ? "Reconnecting" : "live"));
      group.append(row);
      if (index === 0) {
        const sessions = element("div", "session-tree");
        sessions.append(button("cas-src · bright-otter", "nav-item active"), button("gabber-studio · calm-otter", "nav-item"));
        group.append(sessions);
      }
      drawerTree.append(group);
    }
  }
  drawer.append(drawerHeader, drawerTree);
  navigation.append(drawer);
  return navigation;
}

function renderHeader(openSession: boolean): HTMLElement {
  const header = element("header", "session-header");
  const identity = element("div", "session-identity");
  const heading = element("h1", openSession ? "toolbar-session-title" : undefined);
  const picker = button("", "session-picker-toggle");
  picker.append(
    element("span", "session-picker-name", openSession ? (["session-canvas", "session-workers", "transcript", "attention-0", "attention-12", "operator-thread", "drawer-attention-open"].includes(fixtureName) ? "bright-otter" : "Penguinz-fierce-tiger-commander") : "Fleet overview"),
    element("span", "session-picker-caret", "▾"),
  );
  picker.lastElementChild?.setAttribute("aria-hidden", "true");
  picker.setAttribute("aria-haspopup", "dialog");
  heading.append(picker);
  identity.append(heading);
  header.append(identity);
  if (openSession) {
    header.append(element("span", "machine-chip", "Atlas laptop"));
    header.append(element("span", "mode-badge observer", "OBSERVER"));
    header.append(element("span", "connection-summary live", "● 42ms"));
    const actions = element("div", "actions");
    actions.append(button("Take control"), button("Interrupt", "danger"));
    header.append(actions);
  }
  return header;
}

function renderAttention(count: number): HTMLElement {
  const panel = element("section", "context-tab");
  panel.id = "attention-panel";
  renderAttentionPanel(panel, attentionItems(count), { dismiss: () => {}, act: () => {}, copy: () => {} });
  return panel;
}

function renderContext(count: number): HTMLElement {
  const panel = element("aside", "context-panel");
  panel.setAttribute("aria-label", "Attention, workers, and tasks");
  const rail = element("div", "attention-rail");
  rail.append(button("›", "rail-control"));
  const counts = attentionCounts(attentionItems(count));
  const countButton = button(`${counts.critical + counts.warning + counts.info}`, "attention-rail-counts");
  countButton.setAttribute("aria-label", "Open attention");
  rail.append(countButton, button("✉", "mobile-message-toggle"));
  panel.append(rail);
  const body = element("div", "context-body");
  const tabs = element("div", "context-tabs");
  tabs.setAttribute("role", "tablist");
  // Production markup (src/main.ts context-tabs): the selected tab is aria-selected, not a class.
  const tab = (label: string, key: string, selected: boolean) => {
    const node = button(label);
    node.setAttribute("role", "tab");
    node.dataset.contextTab = key;
    node.setAttribute("aria-selected", String(selected));
    return node;
  };
  tabs.append(tab("Attention", "attention", true), tab("Workers & Tasks", "status", false));
  body.append(tabs, fixtureName === "operator-thread" ? renderOperatorComposer() : renderAttention(count));
  panel.append(body);
  return panel;
}

function renderOperatorComposer(): HTMLElement {
  const panel = element("section", "context-tab status-context");
  panel.dataset.contextContent = "status";
  const message = element("div", "message");
  message.append(element("h2", undefined, "Talk to bright-otter"));
  const thread = element("div");
  thread.innerHTML = operatorThreadMarkup([{
    notification_id: 93,
    reply_to: 41,
    message: "The deployment is confirmed and healthy.",
    summary: "deployment confirmed",
    device_id: "phone-7",
    operator_label: "Daniel",
  }]);
  message.append(thread, element("textarea"));
  const actions = element("div", "composer-actions");
  actions.append(button("Keyboard"), button("Send message", "primary"));
  message.append(actions, element("p", "message-status"), element("p", "message-delivery"));
  panel.append(message);
  return panel;
}

function paneHeader(title: string): HTMLElement {
  const header = element("header", "pane-header");
  // Exercise the production pane eyebrow, activity and view/search controls.
  header.append(element("span", "pane-status-dot live"), element("span", "pane-title", title), element("span", "pane-role", title === "bright-otter" ? "supervisor" : "worker"), element("span", "pane-last-activity", "1m ago"));
  const controls = element("div", "pane-layout-controls");
  for (const [label, cls] of [["Show terminal", "pane-view-toggle"], ["Find", "pane-search"]]) {
    const control = button(label, cls);
    control.setAttribute("aria-label", label);
    control.dataset.view = "transcript";
    controls.append(control);
  }
  header.append(controls);
  return header;
}

function renderTerminalPlaceholder(title: string, role: string, collapsed = false): HTMLElement {
  const pane = element("section", collapsed ? "pane collapsed" : "pane selected");
  pane.dataset.paneId = title;
  pane.dataset.paneRole = role;
  const mount = element("div", "terminal-mount");
  const canvas = document.createElement("canvas");
  canvas.className = "t3-ghostty-canvas";
  canvas.width = 800;
  canvas.height = 460;
  canvas.setAttribute("aria-label", `${title} terminal placeholder`);
  mount.append(canvas);
  pane.append(paneHeader(title), mount);
  return pane;
}

function renderHiddenWorkersNote(count: number): HTMLElement {
  const note = element("p", "hidden-workers");
  note.setAttribute("role", "status");
  const reveal = button("Show workers", "hidden-workers-reveal");
  reveal.title = "Show worker panes for debugging; reloads the Hub";
  note.append(element("span", "hidden-workers-label", `${count} workers hidden`), reveal);
  return note;
}

function renderRestingToast(): void {
  const toast = element("div", undefined, "A previous hub notice is resting.");
  toast.id = "toast";
  toast.setAttribute("role", "status");
  toast.setAttribute("aria-hidden", "true");
  document.body.append(toast);
}

function cell(text: string, foreground: GhosttyColor, background: GhosttyColor): GhosttyCell {
  return { text, wide: 0, foreground, background, bold: false, italic: false, invisible: false, strikethrough: false, overline: false, underline: false, selected: false };
}

function row(text: string, wrapsToNext = false, isWrapContinuation = false): GhosttyRow {
  const foreground = { r: 232, g: 235, b: 242 };
  const background = { r: 12, g: 14, b: 19 };
  return { cells: [...text].map((character) => cell(character, foreground, background)), text, wrapsToNext, isWrapContinuation };
}

function renderTranscript(): HTMLElement {
  const pane = element("section", "pane selected");
  pane.append(paneHeader("bright-otter"));
  const source: TranscriptSource = {
    rows: () => [
      row("# Build result"),
      row("const answer = 42;"),
      row("⎿ Tool: exec_command"),
      row("<unknown-block>unfamiliar output</unknown-block>"),
      row(`┌${"─".repeat(100)}┐`),
      row("$ cas factory status", false),
      row("  › supervisor is coordinating six workers", true),
      row("    across the Commander design pass", false, true),
      row("", false),
      row("  › visual QA receipt is ready to review", false),
    ],
    theme: () => ({ foreground: { r: 232, g: 235, b: 242 }, background: { r: 12, g: 14, b: 19 } }),
    hasScrollbackAbove: () => true,
    scrollRows: () => {},
    scrollToBottom: () => {},
    focus: () => {},
  };
  const view = new TranscriptView(document, source);
  view.update();
  const mount = element("div", "terminal-mount transcript-active");
  mount.append(view.element);
  pane.append(mount);
  return pane;
}

function renderConnection(): HTMLElement {
  const grid = element("section", "pane-grid");
  const snapshot = { phase: "failed" as const, stage: "dialing" as const, since: Date.now() - 16_000, attempt: 3, reason: "The machine did not answer its hub address.", retryInMs: 8_000, missedHeartbeats: 0, degraded: false };
  const card = element("div", "empty empty-pane-slot");
  renderConnectionSurfaceInto(card, "bright-otter", snapshot, { retry: () => {}, diagnose: () => {} });
  grid.append(card);
  return grid;
}

type PairingFixture = "pairing-step-1" | "pairing-email" | "pairing-code" | "pairing-cleanup";

/**
 * The production pairing dialog (the same pairDialogMarkup main.ts renders)
 * for one fixture state: the entry step with its email field, the email field
 * live under focus, the relay code, and the cleanup step.
 */
function renderPairing(view: PairingFixture): HTMLDialogElement {
  const origin = window.location.origin;
  const cleanup = view === "pairing-cleanup";
  const code = view === "pairing-code";
  const template = document.createElement("template");
  template.innerHTML = pairDialogMarkup({
    cleanupFailed: cleanup,
    cleanupContext: { cause: "failure", storeOpen: true, rollbackPending: true },
    pendingPairing: code
      ? { kind: "relay-request", pairingRequestId: "fixture", userCode: "K7MW-4H2Q", pollSecret: "fixture", controllerOrigin: origin, requestedScopes: DEFAULT_PAIRING_SCOPES, expiresAt: new Date(Date.now() + 600_000).toISOString(), interval: 5 }
      : null,
    draft: createPairingDraft(origin),
    status: cleanup ? "Pairing failed; cleanup is still being verified." : code ? "Waiting for a machine to claim the code…" : "",
    createInFlight: false,
    exchangeInFlight: false,
    relayOrigin: "https://petra-stella-cloud.vercel.app",
    pageOrigin: origin,
  });
  const dialog = template.content.querySelector<HTMLDialogElement>("#pair-dialog");
  if (!dialog) throw new Error("pairDialogMarkup rendered no #pair-dialog");
  // Exercise the production cancellation policy at the fixture seam. The
  // result is intentionally not shown; it prevents a fixture from drifting
  // into a flow that the real dialog would not allow.
  dialog.dataset.cancellationActive = String(pairingDialogCancellationActive({ createInFlight: false, exchangeInFlight: false, hasPendingPairing: code || cleanup }));
  const failure = pairingExchangeFailure({ status: 502, body: "", controllerOrigin: origin });
  dialog.dataset.failureCopy = failure.message;
  return dialog;
}

function appendOpenPairingDialog(view: PairingFixture): void {
  const dialog = renderPairing(view);
  app.append(dialog);
  dialog.showModal();
  if (!dialog.open) throw new Error("Pairing fixture dialog did not open");
  if (view === "pairing-email") {
    // The live field: focused and empty, so its placeholder and edge are what is measured (D4).
    const email = dialog.querySelector<HTMLInputElement>("#pair-email");
    if (!email) throw new Error("Production pairing dialog has no #pair-email field");
    email.focus();
  }
}

type LaunchFixture = "launch-form" | "launch-browse" | "launch-error" | "launch-starting" | "launch-grant" | "launch-grant-command" | "launch-offline" | "launch-account" | "launch-account-unavailable" | "launch-account-default-out";

/**
 * The production New session sheet (LaunchSheet, cas-0f51) driven through its
 * own controls to one state, over a host that answers like a hub.
 */
async function openLaunchSheet(view: LaunchFixture): Promise<void> {
  const control: Scope[] = ["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt"];
  const target = (id: string) => ({ kind: "project" as const, id });
  const root = { id: "root-code", name: "code", path: "/home/dev/code" };
  const host: LaunchHost = {
    machines: () => [
      // cas-0e14: the offline view's machine is reconnecting.
      { id: "atlas", label: "Atlas · Linux", scopes: [...control, "session-launch"], ...(view === "launch-offline" ? { connection: "Reconnecting" } : {}) },
      // cas-cee5: a read-only pairing cannot allow launch here, so the sheet shows the command to run on the machine.
      { id: "studio", label: "Studio Mac · macOS", scopes: view === "launch-grant-command" ? ["machine-read", "session-read"] : control },
    ],
    currentMachineId: () => (view === "launch-grant" || view === "launch-grant-command" ? "studio" : "atlas"),
    origin: window.location.origin,
    projects: async () => ({
      projects: [
        { id: "p-ledger", name: "ledger-api", path: "/home/dev/ledger-api", last_touched_at: "2026-09-28T08:00:00Z", touch_count: 12, running_session: null, target: target("p-ledger") },
        { id: "p-cas", name: "cas-src", path: "/home/dev/cas-src", last_touched_at: "2026-09-27T18:00:00Z", touch_count: 90, running_session: "patient-pelican-9", target: target("p-cas") },
        { id: "p-old", name: "old-notes-with-a-rather-long-project-name", path: "/home/dev/archive/2025/old-notes-with-a-rather-long-project-name", last_touched_at: "2026-03-01T09:00:00Z", touch_count: 2, running_session: null, target: target("p-old") },
      ],
      browse_roots: [root],
    }),
    profiles: async () => ({
      claude: { installed: true, profiles: [
        // cas-c107: the default account logged out, so one row shows the
        // account's "Default" beside the logged-out Copy control.
        { name: "main", logged_in: view !== "launch-account-default-out", is_default: true },
        { name: "support@petrastella.io", logged_in: true, is_default: false },
        { name: "customer-success-escalations@petrastella-international.example", logged_in: true, is_default: false },
        { name: "old@petrastella.io", logged_in: false, is_default: false },
      ] },
      codex: { installed: true, profiles: [], error: "cli_probe_failed" },
      grok: { installed: true, profiles: [] },
    }),
    browse: async () => ({ root, path: "clients", truncated: true, entries: [
      { name: "archive", path: "clients/archive", launchable: false, project_id: null, target: null },
      { name: "acme-portal", path: "clients/acme-portal", launchable: true, project_id: null, target: { kind: "browse", root_id: root.id, path: "clients/acme-portal" } },
    ] }),
    launch: (): Promise<LaunchResult> => view === "launch-error"
      ? Promise.resolve({ ok: false, status: 422, code: "not_logged_in", detail: "claude: not logged in for profile main (run `claude /login`)" })
      : new Promise(() => {}),
    sessionListed: async () => false,
    open: () => {},
    copy: async () => {},
  };
  const sheet = new LaunchSheet(host);
  sheet.open(view === "launch-grant" || view === "launch-grant-command" ? "studio" : "atlas");
  const dialog = document.querySelector<HTMLDialogElement>("#launch-dialog");
  if (!dialog?.open) throw new Error("Launch fixture sheet did not open");
  const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
  await settle();
  if (view === "launch-browse") {
    dialog.querySelector<HTMLButtonElement>("#launch-tab-browse")!.click();
    await settle();
    dialog.querySelector<HTMLInputElement>('[data-launch-list="browse"] input[type=radio]')?.click();
    return;
  }
  if (view === "launch-account" || view === "launch-account-unavailable" || view === "launch-account-default-out") {
    dialog.querySelector<HTMLInputElement>('[data-launch-list="known"] input[type=radio]')!.click();
    if (view === "launch-account-unavailable") dialog.querySelector<HTMLInputElement>('input[name="launch-cli"][value="codex"]')!.click();
    else dialog.querySelector<HTMLInputElement>('input[name="launch-account"][value^="customer-success"]')!.click();
    return;
  }
  if (view === "launch-form" || view === "launch-error" || view === "launch-starting") {
    dialog.querySelector<HTMLInputElement>('[data-launch-list="known"] input[type=radio]')!.click();
    if (view === "launch-form") return;
    dialog.querySelector<HTMLButtonElement>('[data-launch-action="start"]')!.click();
    await settle();
    if (view === "launch-error") dialog.querySelector<HTMLDetailsElement>(".launch-error-detail")!.open = true;
  }
}

function renderShell(): void {
  const machineCount = fixtureName === "fleet-empty" ? 0 : 2;
  const openSession = ["session-canvas", "session-workers", "transcript", "attention-0", "attention-12", "operator-thread", "connection-failed-retry", "drawer-attention-open"].includes(fixtureName);
  const drawerOpen = fixtureName === "drawer-attention-open";
  const shell = element("div", `shell ${["session-canvas", "session-workers", "transcript"].includes(fixtureName) ? "attention-collapsed" : "attention-expanded"}${fixtureName === "fleet-empty" ? " fleet-empty" : ""}${drawerOpen ? " drawer-open" : ""}`);
  shell.dataset.fixture = fixtureName;
  const signature = shellSignature({
    machineId: machineCount ? "atlas" : undefined,
    session: openSession ? "bright-otter" : undefined,
    machineIds: machineCount ? ["atlas", "forge"] : [],
    sessionKeys: openSession ? ["atlas/bright-otter"] : [],
    catalogLoaded: true,
    drawerOpen,
    attentionCollapsed: false,
    contextTab: "attention",
    fleetEmpty: fixtureName === "fleet-empty",
    supervisor: openSession ? "bright-otter" : undefined,
    backLabel: undefined,
    compatibility: undefined,
    leaseHeldByMe: false,
    leaseController: undefined,
    controlDisabled: false,
    commandPaletteOpen: false,
    sessionPickerOpen: false,
    pairingView: fixtureName.startsWith("pairing") ? fixtureName : "",
  });
  shell.dataset.renderDecision = renderDecision({ signatureChanged: true, composing: false });
  const scheduler = new DeferredRenderScheduler({ render: () => {}, afterGesture: (run) => run() });
  scheduler.settled();
  shell.dataset.renderSignature = signature;
  shell.append(renderRail(machineCount, drawerOpen));
  const main = element("main");
  main.append(renderHeader(openSession));
  if (fixtureName === "fleet-populated" || fixtureName === "fleet-twins") {
    const grid = element("section", "pane-grid");
    const board = element("div", "fleet-board");
    board.setAttribute("aria-label", "Fleet");
    if (fixtureName === "fleet-twins") new FleetBoardRenderer().render(board, fleetModel(), { open: () => {} });
    else renderFleetBoardInto(board, fleetModel(), { open: () => {} });
    grid.append(board);
    main.append(grid);
  } else if (fixtureName === "fleet-empty") {
    const grid = element("section", "pane-grid");
    const empty = element("div", "empty empty-pane-slot");
    empty.append(element("p", "empty-title", "No machine paired yet"), element("p", "empty-hint", "Pair the machine your sessions run on. You will get a code to approve there."), button("Pair a machine", "primary"));
    grid.append(empty);
    main.append(grid);
  } else if (fixtureName === "session-workers") {
    // Workers revealed through the explicit off-by-default control (cas-6261).
    const grid = element("section", "pane-grid pane-layout");
    const primary = element("div", "primary-pane-slot");
    primary.append(renderTerminalPlaceholder("bright-otter", "supervisor"));
    const secondary = element("div", "secondary-pane-strip");
    const collapsed = renderTerminalPlaceholder("agile-octopus", "worker", true);
    if (!matchMedia("(max-width: 53rem)").matches) collapsed.classList.remove("collapsed");
    secondary.append(collapsed, renderTerminalPlaceholder("steady-badger", "worker", true));
    grid.append(primary, secondary);
    main.append(grid);
  } else if (["session-canvas", "attention-0", "attention-12", "operator-thread", "drawer-attention-open"].includes(fixtureName)) {
    // The default view: supervisor only, with the hidden-workers line (cas-6261).
    const grid = element("section", "pane-grid pane-layout workers-hidden");
    const primary = element("div", "primary-pane-slot");
    primary.append(renderTerminalPlaceholder("bright-otter", "supervisor"));
    const secondary = element("div", "secondary-pane-strip");
    secondary.append(renderHiddenWorkersNote(2));
    grid.append(primary, secondary);
    main.append(grid);
  } else if (fixtureName === "transcript") {
    const grid = element("section", "pane-grid pane-layout single-pane");
    const primary = element("div", "primary-pane-slot");
    primary.append(renderTranscript());
    grid.append(primary, element("div", "secondary-pane-strip"));
    main.append(grid);
  } else if (fixtureName === "connection-failed-retry") {
    main.append(renderConnection());
  } else {
    const grid = element("section", "pane-grid");
    const empty = element("div", "empty empty-pane-slot");
    if (fixtureName.startsWith("launch")) empty.append(element("p", "empty-title", "New session"), element("p", "empty-hint", "The New session sheet is open over the workspace."));
    else empty.append(element("p", "empty-title", "Pairing workspace"), element("p", "empty-hint", "The pairing dialog is open so the machine can be authorized."));
    grid.append(empty);
    main.append(grid);
  }
  shell.append(main);
  if (["attention-0", "attention-12", "operator-thread", "drawer-attention-open"].includes(fixtureName)) shell.append(renderContext(fixtureName === "attention-12" ? 12 : 0));
  app.replaceChildren(shell);
  renderRestingToast();
  if (fixtureName.startsWith("pairing")) appendOpenPairingDialog(fixtureName as PairingFixture);
  if (fixtureName.startsWith("launch")) void openLaunchSheet(fixtureName as LaunchFixture);
  if (fixtureName === "fleet-populated" && new URLSearchParams(window.location.search).has("broken")) {
    const style = document.createElement("style");
    style.textContent = ".fixture-broken-contrast { color: var(--bg-panel); background: var(--bg-panel); }";
    document.head.append(style);
    const defect = element("p", "fixture-broken-contrast", "Deliberate contrast defect");
    document.querySelector("main")?.append(defect);
  }
}

if (fixtureName === "attention-notice-details") renderAttentionNoticeFixture(app);
else if (fixtureName.startsWith("conversation") || fixtureName === "paired-machines" || fixtureName === "paired-machines-down") renderConversationFixture(app, fixtureName);
else renderShell();
