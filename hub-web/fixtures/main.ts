import "../src/styles.css";
import { attentionCounts, createAttentionItem } from "../src/attention";
import { renderAttentionPanel } from "../src/attention-view";
import { renderConnectionSurfaceInto } from "../src/connection-state-view";
import { DeferredRenderScheduler } from "../src/deferred-render";
import { renderFleetBoardInto, type FleetBoardModel } from "../src/fleet-board";
import { pairingDialogCancellationActive } from "../src/pairing-dialog";
import { cleanupStepCopy } from "../src/pairing-cleanup";
import { pairingExchangeFailure } from "../src/pairing-messages";
import { renderDecision, shellSignature } from "../src/render-model";
import { operatorThreadMarkup } from "../src/operator-thread";
import { TranscriptView, type TranscriptSource } from "../src/transcript-view";
import type { AttentionItem, HubSession } from "../src/types";
import type { GhosttyCell, GhosttyColor, GhosttyRow } from "../src/terminal/ghostty/core";

export const FIXTURE_NAMES = [
  "fleet-populated",
  "fleet-empty",
  "session-canvas",
  "session-workers",
  "transcript",
  "attention-0",
  "attention-12",
  "operator-thread",
  "connection-failed-retry",
  "pairing-step-1",
  "pairing-cleanup",
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
  const machines = [
    { id: "atlas", label: "Atlas laptop", state: "live", phase: "Live", selected: true },
    { id: "forge", label: "Forge desktop", state: "degraded", phase: "Degraded", selected: false },
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

function renderRail(machineCount: number): HTMLElement {
  const navigation = element("aside", "machine-navigation");
  navigation.setAttribute("aria-label", "Machines and sessions");
  const rail = element("div", "machine-rail");
  const mark = button("", "rail-control commander-mark");
  mark.setAttribute("aria-label", "Open machines and sessions");
  mark.append(element("span", "commander-mark-label", "Machines"));
  rail.append(mark);
  for (let index = 0; index < machineCount; index += 1) {
    const machine = index === 0 ? "Atlas laptop" : "Forge desktop";
    const item = button(index === 0 ? "AL" : "FD", `machine-icon${index === 0 ? " active" : ""}`);
    item.setAttribute("aria-label", `${machine}, ${index === 0 ? "live" : "degraded"}`);
    item.append(element("span", `machine-state ${index === 0 ? "live" : "degraded"}`));
    rail.append(item);
  }
  const pair = button("", "rail-control pair-machine");
  pair.setAttribute("aria-label", "Pair a machine");
  pair.append(element("span", undefined, "+"), element("span", "pair-machine-label", "Pair"));
  rail.append(pair);
  navigation.append(rail);
  const drawer = element("div", "machine-drawer");
  drawer.setAttribute("aria-hidden", "true");
  drawer.setAttribute("inert", "");
  const drawerHeader = element("header", "drawer-header");
  drawerHeader.append(element("strong"), button("", "drawer-close"));
  const drawerTree = element("nav");
  drawerTree.id = "machine-tree";
  drawerTree.setAttribute("aria-label", "Machine sessions");
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
    element("span", "session-picker-name", openSession ? (["session-canvas", "session-workers", "transcript", "attention-0", "attention-12", "operator-thread"].includes(fixtureName) ? "bright-otter" : "Penguinz-fierce-tiger-commander") : "Fleet overview"),
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
  tabs.append(button("Attention", "active"), button("Workers & Tasks"));
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

function renderPairing(cleanup: boolean): HTMLDialogElement {
  const dialog = document.createElement("dialog");
  dialog.id = "pair-dialog";
  const flow = element("section", `pair-flow${cleanup ? " pair-cleanup" : ""}`);
  if (cleanup) {
    const copy = cleanupStepCopy({ cause: "failure", storeOpen: true, rollbackPending: true });
    flow.append(element("h2", undefined, copy.title), element("p", undefined, `${copy.discarded} ${copy.outstanding}`), element("p", undefined, copy.next));
    const status = element("p", "pair-status", "Pairing failed; cleanup is still being verified.");
    status.setAttribute("role", "status");
    flow.append(status);
    const actions = element("div", "dialog-actions");
    actions.append(button("Close"), button("Retry cleanup", "primary"));
    flow.append(actions);
  } else {
    flow.append(element("h2", undefined, "Pair a machine"), element("p", undefined, "Create a one-time pairing code and approve it on the machine you want to monitor."));
    const code = element("code", "pair-code", "K7MW-4H2Q");
    flow.append(code);
    const details = element("dl", "pair-details");
    const capability = element("div");
    capability.append(element("dt", undefined, "This browser will be able to"), element("dd", "pair-summary", "Read machine, session, and pane state"));
    const origin = element("div");
    origin.append(element("dt", undefined, "Cassy Commander origin"), element("dd", undefined, window.location.origin));
    details.append(capability, origin);
    flow.append(details);
    const status = element("p", "pair-status", "Waiting for approval on the machine.");
    status.setAttribute("role", "status");
    flow.append(status);
    const actions = element("div", "dialog-actions");
    actions.append(button("Cancel"), button("Create pairing code", "primary"));
    flow.append(actions);
  }
  // Exercise the production cancellation policy at the fixture seam. The
  // result is intentionally not shown; it prevents a fixture from drifting
  // into a flow that the real dialog would not allow.
  dialog.dataset.cancellationActive = String(pairingDialogCancellationActive({ createInFlight: !cleanup, exchangeInFlight: false, hasPendingPairing: cleanup }));
  const failure = pairingExchangeFailure({ status: 502, body: "", controllerOrigin: window.location.origin });
  dialog.dataset.failureCopy = failure.message;
  dialog.append(flow);
  return dialog;
}

function appendOpenPairingDialog(cleanup: boolean): void {
  const dialog = renderPairing(cleanup);
  app.append(dialog);
  dialog.showModal();
  if (!dialog.open) throw new Error("Pairing fixture dialog did not open");
}

function renderShell(): void {
  const machineCount = fixtureName === "fleet-empty" ? 0 : 2;
  const openSession = ["session-canvas", "session-workers", "transcript", "attention-0", "attention-12", "operator-thread", "connection-failed-retry"].includes(fixtureName);
  const shell = element("div", `shell ${["session-canvas", "session-workers", "transcript"].includes(fixtureName) ? "attention-collapsed" : "attention-expanded"}${fixtureName === "fleet-empty" ? " fleet-empty" : ""}`);
  shell.dataset.fixture = fixtureName;
  const signature = shellSignature({
    machineId: machineCount ? "atlas" : undefined,
    session: openSession ? "bright-otter" : undefined,
    machineIds: machineCount ? ["atlas", "forge"] : [],
    sessionKeys: openSession ? ["atlas/bright-otter"] : [],
    catalogLoaded: true,
    drawerOpen: false,
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
  shell.append(renderRail(machineCount));
  const main = element("main");
  main.append(renderHeader(openSession));
  if (fixtureName === "fleet-populated") {
    const grid = element("section", "pane-grid");
    const board = element("div", "fleet-board");
    board.setAttribute("aria-label", "Fleet");
    renderFleetBoardInto(board, fleetModel(), { open: () => {} });
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
  } else if (["session-canvas", "attention-0", "attention-12", "operator-thread"].includes(fixtureName)) {
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
    empty.append(element("p", "empty-title", "Pairing workspace"), element("p", "empty-hint", "The pairing dialog is open so the machine can be authorized."));
    grid.append(empty);
    main.append(grid);
  }
  shell.append(main);
  if (["attention-0", "attention-12", "operator-thread"].includes(fixtureName)) shell.append(renderContext(fixtureName === "attention-12" ? 12 : 0));
  app.replaceChildren(shell);
  renderRestingToast();
  if (fixtureName === "pairing-step-1") appendOpenPairingDialog(false);
  if (fixtureName === "pairing-cleanup") appendOpenPairingDialog(true);
  if (fixtureName === "fleet-populated" && new URLSearchParams(window.location.search).has("broken")) {
    const style = document.createElement("style");
    style.textContent = ".fixture-broken-contrast { color: var(--bg-panel); background: var(--bg-panel); }";
    document.head.append(style);
    const defect = element("p", "fixture-broken-contrast", "Deliberate contrast defect");
    document.querySelector("main")?.append(defect);
  }
}

renderShell();
