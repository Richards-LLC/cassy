import { sessionPickerMeta, type SessionPickerEntry } from "./session-selection";

/**
 * The fleet board: the canvas with machines paired and no session open. It is
 * a region — rebuilt only when what it shows actually changed — so a heartbeat
 * never pulls a session card out from under a thumb or a focus ring.
 */
export interface FleetMachineView {
  readonly id: string;
  readonly label: string;
  /** The connection lifecycle class (idle, live, backoff, failed, …). */
  readonly state: string;
  /** Phase in words, without latency: "Live", "Reconnecting", "Unreachable". */
  readonly phase: string;
  readonly selected: boolean;
  readonly hubVersion?: string;
  readonly catalogUpdatedAt?: string;
}

export interface FleetSessionView extends SessionPickerEntry {
  readonly attentionSeverity?: "critical" | "warning" | "info";
  readonly lastActivity?: string;
}

export interface FleetBoardModel {
  readonly machines: readonly FleetMachineView[];
  readonly sessions: readonly FleetSessionView[];
}

export interface FleetBoardCallbacks {
  open(machineId: string, session: string): void;
}

/**
 * What the board renders, and nothing that changes every heartbeat: no latency,
 * no counts, no stale age. One heartbeat-driven field here would rebuild the
 * board every five seconds and blur whatever the operator was on.
 */
export function fleetBoardSignature(model: FleetBoardModel): string {
  return [
    ...model.machines.map((machine) => `${machine.id}|${machine.label}|${machine.state}|${machine.phase}|${machine.selected ? 1 : 0}|${machine.hubVersion ?? ""}`),
    ...model.sessions.map((entry) => `${entry.machineId}/${entry.session}|${entry.supervisor ?? ""}|${entry.workerCount}|${entry.status}|${entry.title ?? ""}|${entry.phase ?? ""}|${entry.attentionSeverity ?? ""}|${entry.lastActivity ?? ""}`),
  ].join("~");
}

function summaryText(model: FleetBoardModel): string {
  const machineCount = model.machines.length;
  const sessionCount = model.sessions.length;
  const notLive = model.machines.filter((machine) => machine.state !== "live").length;
  return [
    `${machineCount} ${machineCount === 1 ? "machine" : "machines"}`,
    `${sessionCount} ${sessionCount === 1 ? "session" : "sessions"}`,
    ...(notLive > 0 ? [`${notLive} not live`] : []),
  ].join(" · ");
}

function sessionCard(entry: FleetSessionView, callbacks: FleetBoardCallbacks): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = `fleet-session${fleetSessionState(entry) === "needs-you" ? " needs-you" : ""}`;
  button.dataset.fleetMachine = entry.machineId;
  button.dataset.fleetSession = entry.session;
  button.setAttribute("aria-label", `Open ${entry.session} on ${entry.machineLabel}`);
  const name = document.createElement("span");
  name.className = "session-name";
  name.textContent = entry.session;
  button.append(name);
  if (entry.phase) {
    const chip = document.createElement("span");
    chip.className = `phase-chip phase-${entry.phase}`;
    chip.textContent = entry.phase;
    button.append(chip);
  }
  if (entry.title) {
    const title = document.createElement("span");
    title.className = "session-summary-title";
    title.textContent = entry.title;
    button.append(title);
  }
  const meta = document.createElement("small");
  meta.className = "session-meta";
  meta.textContent = sessionPickerMeta(entry);
  button.append(meta);
  // A region re-creates this node, so it carries its own handler.
  button.onclick = () => callbacks.open(entry.machineId, entry.session);
  return button;
}

function machineSection(machine: FleetMachineView, sessions: readonly SessionPickerEntry[], callbacks: FleetBoardCallbacks): HTMLElement {
  const section = document.createElement("section");
  section.className = `fleet-machine${machine.selected ? " active" : ""}`;
  section.dataset.fleetMachine = machine.id;
  const header = document.createElement("header");
  header.className = "fleet-machine-header";
  const dot = document.createElement("span");
  dot.className = `machine-state ${machine.state}`;
  const label = document.createElement("strong");
  label.textContent = machine.label;
  const phase = document.createElement("small");
  phase.className = `fleet-machine-phase ${machine.state}`;
  phase.textContent = machine.phase;
  header.append(dot, label, phase);
  section.append(header);
  const list = document.createElement("div");
  list.className = "fleet-ledger";
  list.setAttribute("role", "list");
  for (const entry of sessions) {
    const row = document.createElement("div");
    row.setAttribute("role", "listitem");
    row.append(sessionCard(entry, callbacks));
    list.append(row);
  }
  if (!list.childElementCount) {
    const empty = document.createElement("p");
    empty.className = "fleet-empty-sessions";
    empty.textContent = machine.state === "live" ? "No live sessions." : "Sessions appear once the machine is reachable.";
    list.append(empty);
  }
  section.append(list);
  return section;
}

const TRACK = ["needs-you", "working", "idle", "stale", "unreachable"] as const;
type FleetState = typeof TRACK[number];
const STATE_LABEL: Record<FleetState, string> = {
  "needs-you": "Needs you", working: "Working", idle: "Idle", stale: "Stale", unreachable: "Unreachable",
};

/** Critical work wins over liveness; unknown catalog values stay visible as stale. */
export function fleetSessionState(entry: FleetSessionView): FleetState {
  if (entry.attentionSeverity === "critical" || entry.phase === "blocked") return "needs-you";
  if (entry.status === "missing_endpoint") return "unreachable";
  if (entry.status !== "live") return "stale";
  if (["planning", "editing", "building", "testing", "reviewing"].includes(entry.phase ?? "")) return "working";
  return "idle";
}

function orderedSessions(model: FleetBoardModel): FleetSessionView[] {
  const priority: Record<FleetState, number> = { "needs-you": 0, unreachable: 1, stale: 2, idle: 3, working: 4 };
  return [...model.sessions].sort((a, b) => priority[fleetSessionState(a)] - priority[fleetSessionState(b)]
    || (Date.parse(b.lastActivity ?? "") || 0) - (Date.parse(a.lastActivity ?? "") || 0));
}

export function fleetVerdict(model: FleetBoardModel): string {
  const total = model.sessions.length;
  if (!model.machines.length) return "Your fleet starts with one machine.";
  if (!total) return "Your machines are paired; no sessions are running yet.";
  const counts = Object.fromEntries(TRACK.map((state) => [state, model.sessions.filter((entry) => fleetSessionState(entry) === state).length])) as Record<FleetState, number>;
  if (counts.working === total) return total === 1 ? "Your session is working." : `All ${total} sessions are working.`;
  const evidence = (["working", "idle", "stale", "unreachable"] as const)
    .filter((state) => counts[state] > 0)
    .map((state) => `${counts[state]} ${state}`).join(", ");
  const lead = counts["needs-you"]
    ? `${counts["needs-you"]} of ${total} sessions ${counts["needs-you"] === 1 ? "needs" : "need"} you`
    : "No sessions need you";
  return `${lead}${evidence ? `; ${evidence}` : ""}.`;

}

function textNode(tag: string, className: string, text: string): HTMLElement {
  const node = document.createElement(tag);
  node.className = className;
  node.textContent = text;
  return node;
}

function clockText(timestamp: string | undefined): string {
  if (!timestamp || !Number.isFinite(Date.parse(timestamp))) return "not reported";
  return new Date(timestamp).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
}

function provenance(model: FleetBoardModel): string {
  return model.machines.map((machine) => `${machine.label} · ${machine.phase} · Hub ${machine.hubVersion ?? "version not reported"} · catalog ${clockText(machine.catalogUpdatedAt)}`).join(" / ");
}

function fleetFigure(model: FleetBoardModel): HTMLElement {
  const figure = document.createElement("figure");
  figure.className = "fleet-figure";
  figure.setAttribute("aria-label", "Sessions on the work-state track");
  const table = document.createElement("table");
  table.className = "fleet-plot";
  const head = table.createTHead().insertRow();
  for (const label of ["Session", ...TRACK.map((state) => STATE_LABEL[state])]) {
    const cell = document.createElement("th");
    cell.scope = "col";
    if (label === "Unreachable") cell.append("Unreach", document.createElement("wbr"), "able");
    else cell.textContent = label;
    head.append(cell);
  }
  const body = table.createTBody();
  for (const entry of orderedSessions(model)) {
    const state = fleetSessionState(entry);
    const row = body.insertRow();
    row.className = `fleet-plot-row ${state}`;
    row.dataset.fleetSession = entry.session;
    row.dataset.state = state;
    const name = document.createElement("th");
    name.scope = "row";
    name.title = entry.session;
    name.setAttribute("aria-label", `${entry.session} on ${entry.machineLabel}`);
    name.textContent = entry.session.split("-").slice(-3).join("-");
    row.append(name);
    for (const position of TRACK) {
      const cell = row.insertCell();
      cell.className = `fleet-track-cell track-${position}`;
      if (position !== state) continue;
      cell.setAttribute("aria-label", `${STATE_LABEL[state]}${state === "working" ? `: ${entry.phase}` : ""}`);
      const dot = textNode("span", "fleet-dot", "");
      dot.setAttribute("aria-hidden", "true");
      cell.append(dot);
      if (state === "working") cell.append(textNode("span", "fleet-dot-phase", entry.phase ?? ""));
      cell.append(textNode("span", "sr-only", STATE_LABEL[state]));
    }
  }
  const scroll = document.createElement("div");
  scroll.className = "fleet-plot-scroll";
  scroll.tabIndex = 0;
  scroll.setAttribute("role", "region");
  scroll.setAttribute("aria-label", "Fleet state plot; scroll for more sessions");
  scroll.append(table);
  figure.append(scroll, textNode("figcaption", "fleet-figure-caption", "One dot per session. Ringed: needs you. Shaded: working."));
  return figure;
}

/** Fills `board` from scratch; the figure and ledger use the same derived state. */
export function renderFleetBoardInto(board: HTMLElement, model: FleetBoardModel, callbacks: FleetBoardCallbacks): void {
  board.replaceChildren();
  const hero = textNode("section", "fleet-hero", "");
  const header = textNode("header", "fleet-board-header", "");
  header.append(textNode("p", "fleet-eyebrow", "Fleet"), textNode("p", "fleet-board-summary", summaryText(model)));
  const verdict = textNode("h2", "fleet-verdict", fleetVerdict(model));
  verdict.setAttribute("role", "status");
  verdict.setAttribute("aria-live", "polite");
  const refreshed = model.machines.map((machine) => machine.catalogUpdatedAt).filter((value): value is string => Boolean(value)).sort().at(-1);
  const time = textNode("time", "fleet-catalog-time", refreshed ? clockText(refreshed) : "Catalog awaiting refresh");
  if (refreshed) time.setAttribute("datetime", refreshed);
  header.append(time, verdict);
  hero.append(header);
  if (model.sessions.length) hero.append(fleetFigure(model));
  else hero.append(textNode("p", "fleet-empty-sessions", model.machines.length ? "Start a Cassy session on a paired machine. It will appear here." : "Pair the machine your sessions run on to see their state here."));
  board.append(hero, textNode("p", "fleet-provenance", provenance(model)));
  const ordered = [...model.machines].sort((a, b) => Number(b.selected) - Number(a.selected));
  const ledger = textNode("section", "fleet-evidence", "");
  if (ordered.length) ledger.append(textNode("h3", "fleet-eyebrow", "Session ledger"));
  for (const machine of ordered) ledger.append(machineSection(machine, orderedSessions(model).filter((entry) => entry.machineId === machine.id), callbacks));
  board.append(ledger);
}

/**
 * Owns the "is this board already showing this?" decision. The answer is keyed
 * on the board *element* as well as the signature: a shell rebuild hands the
 * updater a brand-new empty container, and an unchanged signature must not
 * leave it empty.
 */
export class FleetBoardRenderer {
  private board: HTMLElement | undefined;
  private signature: string | undefined;

  /** Returns true when the board was (re)built, false when left untouched. */
  render(board: HTMLElement | null | undefined, model: FleetBoardModel, callbacks: FleetBoardCallbacks): boolean {
    if (!board) {
      this.board = undefined;
      this.signature = undefined;
      return false;
    }
    const signature = fleetBoardSignature(model);
    if (board === this.board && board.isConnected && signature === this.signature && board.childElementCount > 0) {
      const source = board.querySelector(".fleet-provenance");
      if (source) source.textContent = provenance(model);
      const refreshed = model.machines.map((machine) => machine.catalogUpdatedAt).filter((value): value is string => Boolean(value)).sort().at(-1);
      const time = board.querySelector(".fleet-catalog-time");
      if (time && refreshed) { time.textContent = clockText(refreshed); time.setAttribute("datetime", refreshed); }
      return false;
    }
    renderFleetBoardInto(board, model, callbacks);
    this.board = board;
    this.signature = signature;
    return true;
  }
}
