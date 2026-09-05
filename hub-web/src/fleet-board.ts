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
}

export interface FleetBoardModel {
  readonly machines: readonly FleetMachineView[];
  readonly sessions: readonly SessionPickerEntry[];
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
    ...model.machines.map((machine) => `${machine.id}|${machine.label}|${machine.state}|${machine.phase}|${machine.selected ? 1 : 0}`),
    ...model.sessions.map((entry) => `${entry.machineId}/${entry.session}|${entry.supervisor ?? ""}|${entry.workerCount}|${entry.status}|${entry.title ?? ""}|${entry.phase ?? ""}`),
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

function sessionCard(entry: SessionPickerEntry, callbacks: FleetBoardCallbacks): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "fleet-session";
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
  list.className = "fleet-sessions";
  for (const entry of sessions) list.append(sessionCard(entry, callbacks));
  if (!list.childElementCount) {
    const empty = document.createElement("p");
    empty.className = "fleet-empty-sessions";
    empty.textContent = machine.state === "live" ? "No live sessions." : "Sessions appear once the machine is reachable.";
    list.append(empty);
  }
  section.append(list);
  return section;
}

/** Fills `board` from scratch. */
export function renderFleetBoardInto(board: HTMLElement, model: FleetBoardModel, callbacks: FleetBoardCallbacks): void {
  board.replaceChildren();
  const header = document.createElement("header");
  header.className = "fleet-board-header";
  const heading = document.createElement("h2");
  heading.textContent = "Fleet";
  const summary = document.createElement("p");
  summary.className = "fleet-board-summary";
  summary.textContent = summaryText(model);
  header.append(heading, summary);
  board.append(header);
  const ordered = [...model.machines].sort((a, b) => Number(b.selected) - Number(a.selected));
  for (const machine of ordered) {
    board.append(machineSection(machine, model.sessions.filter((entry) => entry.machineId === machine.id), callbacks));
  }
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
    if (board === this.board && board.isConnected && signature === this.signature && board.childElementCount > 0) return false;
    renderFleetBoardInto(board, model, callbacks);
    this.board = board;
    this.signature = signature;
    return true;
  }
}
