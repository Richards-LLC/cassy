import { machineInitials } from "./machine-accent";
import { sessionPickerHeadline, sessionPickerRowMeta, type SessionPickerEntry } from "./session-selection";

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
    ...model.sessions.map((entry) => `${entry.machineId}/${entry.session}|${entry.project ?? ""}|${entry.supervisor ?? ""}|${entry.workerCount}|${entry.status}|${entry.title ?? ""}|${entry.phase ?? ""}|${entry.attentionSeverity ?? ""}|${entry.lastActivity ?? ""}`),
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
  // Project first, codename once in the line beneath (journey F1), as the
  // session picker, the list and the palette read.
  const headline = sessionPickerHeadline(entry);
  const codename = entry.project ? entry.supervisor ?? entry.session : undefined;
  button.setAttribute("aria-label", `Open ${headline}${codename ? `, ${codename}` : ""} on ${entry.machineLabel}`);
  const name = document.createElement("span");
  name.className = "session-name";
  name.textContent = headline;
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
  meta.textContent = sessionPickerRowMeta(entry);
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

/**
 * One short line under the figure (journey F2): when the board's catalog was
 * last refreshed. The per-machine details that used to run on as a paragraph
 * are the line's hover title, one machine per line.
 */
export function fleetProvenance(model: FleetBoardModel): { readonly text: string; readonly details: string } {
  const refreshed = model.machines.map((machine) => machine.catalogUpdatedAt).filter((value): value is string => Boolean(value && Number.isFinite(Date.parse(value)))).sort().at(-1);
  return {
    text: model.machines.length ? (refreshed ? `Last updated ${clockText(refreshed)}` : "Waiting for the first update") : "",
    details: model.machines.map((machine) => `${machine.label} · ${machine.phase} · Hub ${machine.hubVersion ?? "version not reported"} · updated ${clockText(machine.catalogUpdatedAt)}`).join("\n"),
  };
}

function writeProvenance(line: HTMLElement, model: FleetBoardModel): void {
  const { text, details } = fleetProvenance(model);
  line.textContent = text;
  line.title = details;
}

/**
 * A plot row's label: the project (or trimmed session), and a tag when that
 * label repeats. A twin (the same codename on several machines) also carries
 * the tag's parts, so the tag can be fitted to the column it lands in
 * (fitFleetPlotTags): `tail` gives way from the left, `mark` never does.
 */
export interface FleetPlotLabel { name: string; tag?: string; tail?: string; mark?: string }

/**
 * The ways a twin's tag can read, longest first (cas-ae5e): the whole tail
 * and machine mark, then the tail trimmed from the left one character at a
 * time ("…ican-19 · Atl"), then the mark alone. The mark is never cut.
 */
export function twinTagCandidates(tail: string, mark: string): string[] {
  const chars = Array.from(tail);
  const trimmed = chars.slice(1).map((_, index) => `…${chars.slice(index + 1).join("")} · ${mark}`);
  return [`${tail} · ${mark}`, ...trimmed, mark];
}

/** The longest twin tag that `measure` fits within `available` px; the mark alone when none does. */
export function fittingTwinTag(tail: string, mark: string, available: number, measure: (text: string) => number): string {
  const candidates = twinTagCandidates(tail, mark);
  return candidates.find((text) => measure(text) <= available) ?? mark;
}

/**
 * Fit every twin tag on the board to its measured label column (cas-ae5e).
 * The room is the label's content box less the project's minimum width
 * (2ch: it always keeps a letter) and the gap. Candidates are measured in
 * the tag's own font, including the "· " it draws before itself. A board not
 * laid out yet (no width) is left as rendered; the resize watch fits it.
 */
export function fitFleetPlotTags(root: ParentNode): void {
  const tags = [...root.querySelectorAll<HTMLElement>(".fleet-plot-name.tagged > .fleet-plot-tag[data-mark]")];
  if (!tags.length) return;
  const document = tags[0]!.ownerDocument;
  const view = document.defaultView;
  if (!view) return;
  const probe = document.createElement("span");
  probe.setAttribute("aria-hidden", "true");
  probe.style.cssText = "position:absolute;left:-10000px;top:0;visibility:hidden;white-space:pre;";
  document.body.append(probe);
  try {
    for (const tag of tags) {
      const name = tag.parentElement!;
      if (!name.isConnected || name.clientWidth === 0) continue;
      const project = name.querySelector<HTMLElement>(".fleet-plot-project");
      const style = view.getComputedStyle(name);
      const room = name.clientWidth - parseFloat(style.paddingLeft || "0") - parseFloat(style.paddingRight || "0")
        - (project ? parseFloat(view.getComputedStyle(project).minWidth) || 0 : 0)
        - (parseFloat(style.columnGap) || 0);
      const font = view.getComputedStyle(tag);
      probe.style.font = font.font;
      probe.style.letterSpacing = font.letterSpacing;
      const measure = (text: string) => { probe.textContent = `· ${text}`; return probe.getBoundingClientRect().width; };
      const tail = tag.dataset.tail ?? "";
      const mark = tag.dataset.mark ?? "";
      const fitted = fittingTwinTag(tail, mark, room, measure);
      if (tag.textContent !== fitted) tag.textContent = fitted;
    }
  } finally {
    probe.remove();
  }
}


/** A machine's own name, letters and digits only: "Atlas" from "Atlas · Linux". */
function machineName(label: string): string {
  const name = label.split(/\s+[·•|—–]\s+/u)[0] ?? label;
  return (name.match(/[\p{L}\p{N}]+/gu) ?? []).join("") || label;
}

/**
 * The short machine mark that tells rows of one codename apart (cas-ae5e):
 * the rail initials ("AT"). When two of those machines share initials (Atlas
 * and Attic), the shortest prefix of the machine's own name that differs
 * ("Atl" / "Att"), at most four letters. Failing that, the initials and an
 * ordinal in label order ("BS1" / "BS2"). It is never the full machine label,
 * which the narrow label column cannot hold (cas-ae5e QA F01).
 */
function machineMarks(entries: readonly FleetSessionView[]): Map<FleetSessionView, string> {
  const marks = new Map<FleetSessionView, string>();
  const byInitials = new Map<string, FleetSessionView[]>();
  for (const entry of entries) {
    const initials = machineInitials(entry.machineLabel);
    byInitials.set(initials, [...(byInitials.get(initials) ?? []), entry]);
  }
  for (const [initials, clash] of byInitials) {
    if (clash.length === 1) { marks.set(clash[0]!, initials); continue; }
    let settled = false;
    for (let length = 3; length <= 4 && !settled; length += 1) {
      const prefixes = clash.map((entry) => Array.from(machineName(entry.machineLabel)).slice(0, length).join(""));
      if (new Set(prefixes.map((prefix) => prefix.toLocaleLowerCase())).size === clash.length) {
        clash.forEach((entry, index) => marks.set(entry, prefixes[index]!));
        settled = true;
      }
    }
    if (!settled) {
      [...clash].sort((a, b) => a.machineLabel.localeCompare(b.machineLabel) || a.machineId.localeCompare(b.machineId))
        .forEach((entry, index) => marks.set(entry, `${initials}${index + 1}`));
    }
  }
  return marks;
}

/**
 * Plot row labels lead with the project (journey F1). When several rows share
 * a project, each also carries the shortest tail of its codename that tells
 * the codenames apart ("pelican-9", "otter-5"): at least its last two words
 * (cas-598e QA F01). The same codename running on two machines adds a short
 * machine mark (machineMarks: "otter-5 · AT", or "otter-5 · Atl" when initials
 * collide). Such a twin tag is fitted to its column once it is laid out
 * (fitFleetPlotTags), trimming the codename from the left and never the mark
 * (cas-ae5e). Keyed by machine/session.
 */
export function fleetPlotLabels(sessions: readonly FleetSessionView[]): Map<string, FleetPlotLabel> {
  const key = (entry: FleetSessionView) => `${entry.machineId}/${entry.session}`;
  const name = (entry: FleetSessionView) => entry.project ?? entry.session.split("-").slice(-3).join("-");
  const codename = (entry: FleetSessionView) => entry.supervisor ?? entry.session;
  const labels = new Map<string, FleetPlotLabel>();
  const groups = new Map<string, FleetSessionView[]>();
  for (const entry of sessions) groups.set(name(entry), [...(groups.get(name(entry)) ?? []), entry]);
  for (const [label, group] of groups) {
    if (group.length === 1) { labels.set(key(group[0]!), { name: label }); continue; }
    // Tails are chosen over the distinct codenames, so a twin on another
    // machine does not lengthen every other row's tag.
    const distinct = [...new Set(group.map(codename))].map((value) => value.split("-"));
    const longest = Math.max(...distinct.map((words) => words.length));
    let length = longest;
    for (let candidate = 2; candidate < longest; candidate += 1) {
      if (new Set(distinct.map((words) => words.slice(-candidate).join("-"))).size === distinct.length) { length = candidate; break; }
    }
    const tail = (entry: FleetSessionView) => codename(entry).split("-").slice(-length).join("-");
    const twinSets = new Map<string, FleetSessionView[]>();
    for (const entry of group) twinSets.set(codename(entry), [...(twinSets.get(codename(entry)) ?? []), entry]);
    const tags = new Map<FleetSessionView, { tag: string; tail?: string; mark?: string }>();
    for (const twins of twinSets.values()) {
      if (twins.length === 1) { tags.set(twins[0]!, { tag: tail(twins[0]!) }); continue; }
      const marks = machineMarks(twins);
      for (const entry of twins) tags.set(entry, { tag: `${tail(entry)} · ${marks.get(entry)!}`, tail: tail(entry), mark: marks.get(entry)! });
    }
    for (const entry of group) labels.set(key(entry), { name: label, ...tags.get(entry)! });
  }
  return labels;
}

function fleetFigure(model: FleetBoardModel): HTMLElement {
  const figure = document.createElement("figure");
  figure.className = "fleet-figure";
  figure.setAttribute("aria-label", "Sessions on the work-state track");
  const table = document.createElement("table");
  // Journey F2: the Working column is shaded only when a session is in it.
  const anyWorking = model.sessions.some((entry) => fleetSessionState(entry) === "working");
  table.className = `fleet-plot${anyWorking ? " has-working" : ""}`;
  const head = table.createTHead().insertRow();
  for (const [index, label] of ["Session", ...TRACK.map((state) => STATE_LABEL[state])].entries()) {
    const cell = document.createElement("th");
    cell.scope = "col";
    cell.setAttribute("aria-label", label);
    cell.append(textNode("span", "fleet-track-label", label));
    if (index > 0) {
      const key = textNode("span", "fleet-track-key", String(index));
      key.setAttribute("aria-hidden", "true");
      cell.append(key);
    }
    head.append(cell);
  }
  const body = table.createTBody();
  const labels = fleetPlotLabels(model.sessions);
  for (const entry of orderedSessions(model)) {
    const state = fleetSessionState(entry);
    const row = body.insertRow();
    row.className = `fleet-plot-row ${state}`;
    row.dataset.fleetSession = entry.session;
    row.dataset.state = state;
    const name = document.createElement("th");
    name.scope = "row";
    // The plot's row labels lead with the project too (journey F1); without
    // one, the session name, trimmed to its last three words as before.
    const codename = entry.project ? entry.supervisor ?? entry.session : undefined;
    name.title = codename ? `${entry.project} · ${codename}` : entry.session;
    name.setAttribute("aria-label", `${sessionPickerHeadline(entry)}${codename ? `, ${codename}` : ""} on ${entry.machineLabel}`);
    const label = labels.get(`${entry.machineId}/${entry.session}`)!;
    const plotName = textNode("span", "fleet-plot-name", "");
    if (label.tag) {
      // The project gives way before the tag does, so repeated rows stay apart at any width.
      plotName.classList.add("tagged");
      const tag = textNode("span", "fleet-plot-tag", label.tag);
      if (label.mark !== undefined) {
        // Fitted to the column once laid out (fitFleetPlotTags); the full tag stays in the title.
        tag.dataset.tail = label.tail ?? "";
        tag.dataset.mark = label.mark;
        tag.title = label.tag;
      }
      plotName.append(textNode("span", "fleet-plot-project", label.name), tag);
    } else {
      plotName.textContent = label.name;
    }
    name.append(plotName);
    row.append(name);
    for (const position of TRACK) {
      const cell = row.insertCell();
      cell.className = `fleet-track-cell track-${position}`;
      if (position !== state) continue;
      cell.setAttribute("aria-label", `${STATE_LABEL[state]}${state === "working" ? `: ${entry.phase}` : ""}`);
      const dot = textNode("span", "fleet-dot", "");
      dot.setAttribute("aria-hidden", "true");
      const mark = textNode("span", "fleet-plot-mark", "");
      mark.append(dot);
      if (state === "working") mark.append(textNode("span", "fleet-dot-phase", entry.phase ?? ""));
      cell.append(mark);
      cell.append(textNode("span", "sr-only", STATE_LABEL[state]));
    }
  }
  const scroll = document.createElement("div");
  scroll.className = "fleet-plot-scroll";
  scroll.tabIndex = 0;
  scroll.setAttribute("role", "region");
  scroll.setAttribute("aria-label", "Fleet state plot; scroll for more sessions");
  scroll.append(table);
  const legend = textNode("div", "fleet-track-legend", "");
  legend.setAttribute("aria-hidden", "true");
  TRACK.forEach((state, index) => legend.append(textNode("span", "", `${index + 1} ${STATE_LABEL[state]}`)));
  figure.append(scroll, legend, textNode("figcaption", "fleet-figure-caption", `One dot per session. Ringed: needs you.${anyWorking ? " Shaded: working." : ""}`));
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
  // The refresh time is said once, on the line under the figure (journey F2).
  header.append(verdict);
  hero.append(header);
  if (model.sessions.length) hero.append(fleetFigure(model));
  else hero.append(textNode("p", "fleet-empty-sessions", model.machines.length ? "Start a Cassy session on a paired machine. It will appear here." : "Pair the machine your sessions run on to see their state here."));
  const provenanceLine = textNode("p", "fleet-provenance", "");
  writeProvenance(provenanceLine, model);
  board.append(hero, provenanceLine);
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
  /** Refits twin tags when the board's width changes (cas-ae5e). */
  private resize?: ResizeObserver;
  private observed?: HTMLElement;

  private watch(board: HTMLElement): void {
    fitFleetPlotTags(board);
    if (this.observed === board || typeof ResizeObserver === "undefined") return;
    this.resize?.disconnect();
    this.resize = new ResizeObserver(() => { if (this.observed) fitFleetPlotTags(this.observed); });
    this.resize.observe(board);
    this.observed = board;
  }

  /** Returns true when the board was (re)built, false when left untouched. */
  render(board: HTMLElement | null | undefined, model: FleetBoardModel, callbacks: FleetBoardCallbacks): boolean {
    if (!board) {
      this.board = undefined;
      this.signature = undefined;
      return false;
    }
    const signature = fleetBoardSignature(model);
    if (board === this.board && board.isConnected && signature === this.signature && board.childElementCount > 0) {
      const source = board.querySelector<HTMLElement>(".fleet-provenance");
      if (source) writeProvenance(source, model);
      return false;
    }
    renderFleetBoardInto(board, model, callbacks);
    this.board = board;
    this.signature = signature;
    this.watch(board);
    return true;
  }
}
