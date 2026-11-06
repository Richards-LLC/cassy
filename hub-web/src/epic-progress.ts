import { splitBar, splitText } from "./progress-view";
import type { ProjectEpic } from "./progress-model";

/** Rows shown before "Show N more". */
export const LIVE_EPIC_ROWS = 5;

interface ProjectSession { readonly name: string; readonly project?: string; readonly project_dir?: string }

/** The running sessions whose status feeds Live epics: the selected project's
 * only, on every machine. Other projects' sessions are never read for it. */
export function projectSessions<T extends ProjectSession>(sessions: ReadonlyMap<string, readonly T[]>, project: string | undefined,
  key: (machineId: string, session: T) => string): { machineId: string; session: T }[] {
  if (!project) return [];
  return [...sessions].flatMap(([machineId, roster]) => roster.filter(session => key(machineId, session) === project).map(session => ({ machineId, session })));
}

export interface LiveEpicsOptions {
  readonly project: string;
  readonly live: readonly ProjectEpic[];
  readonly quiet: number;
  /** Only a project on two machines names the machine on each task. */
  readonly multiMachine: boolean;
  readonly explorerHref?: string | null;
  /** "Checking prowl…" while a peer's first status is on its way. */
  readonly pending: readonly string[];
  /** Peers whose status failed, with their last-known time. */
  readonly failed: readonly { label: string; at?: number }[];
  readonly retry: () => void;
}

// Disclosure state belongs to the browser and survives every status re-render.
const expanded = new Set<string>();
const showingAll = new Set<string>();
const stateKey = (project: string, epic: string) => JSON.stringify([project, epic]);

const time = (at: number) => new Date(at).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit", hour12: true });

function element<K extends keyof HTMLElementTagNameMap>(document: Document, tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function activityText(epic: ProjectEpic): string {
  return ([[epic.building, "building"], [epic.parked, "parked"], [epic.blocked, "blocked"]] as const)
    .filter(([value]) => value > 0).map(([value, label]) => `${value} ${label}`).join(" · ");
}

function taskGroup(document: Document, epic: ProjectEpic, label: "In progress" | "Open", multiMachine: boolean): HTMLElement | undefined {
  const rows = epic.rows.filter(row => row.group === label);
  if (!rows.length) return undefined;
  const group = element(document, "section", "live-epic-group");
  group.append(element(document, "h4", "live-epic-group-label", `${label} · ${rows.length}`));
  const list = element(document, "ul", "live-epic-task-list");
  for (const row of rows) {
    const item = element(document, "li", `live-epic-task${row.state === "Blocked" ? " is-blocked" : ""}`);
    // The task's plain title, never its id.
    item.append(element(document, "span", "live-epic-task-title", row.task.title?.trim() || "Untitled task"));
    const meta = element(document, "span", "live-epic-task-meta");
    meta.append(element(document, "span", "live-epic-task-worker", row.worker ?? "Unassigned"));
    if (multiMachine) meta.append(element(document, "span", "live-epic-task-machine", row.machine));
    meta.append(element(document, "span", `live-epic-chip live-epic-chip--${row.state.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`, row.state));
    item.append(meta);
    list.append(item);
  }
  group.append(list);
  return group;
}

function epicRow(document: Document, epic: ProjectEpic, index: number, options: LiveEpicsOptions): HTMLLIElement {
  const key = stateKey(options.project, epic.key);
  const open = expanded.has(key);
  const item = element(document, "li", `live-epic${epic.blocked ? " is-blocked" : ""}`);
  const panelId = `live-epic-tasks-${index}`;
  const toggle = element(document, "button", "live-epic-toggle");
  toggle.type = "button"; toggle.title = epic.epic.title ?? epic.title;
  toggle.dataset.fleetFocus = `epic:${epic.key}`;
  toggle.setAttribute("aria-expanded", String(open)); toggle.setAttribute("aria-controls", panelId);
  const title = element(document, "span", "live-epic-title", epic.title);
  const ratio = element(document, "span", "live-epic-ratio");
  toggle.append(title, ratio);
  if (epic.counts) {
    ratio.textContent = `${epic.counts.merged}/${epic.counts.total}`;
    ratio.setAttribute("aria-label", `${epic.counts.merged} of ${epic.counts.total} merged`);
    toggle.append(splitBar(document, epic.counts, epic.title));
  }
  const line = element(document, "span", "live-epic-line");
  const split = element(document, "span", "live-epic-split", epic.counts ? splitText(epic.counts) : activityText(epic));
  if (epic.counts && epic.blocked) split.append(" ", element(document, "span", "live-epic-blocked", `· ${epic.blocked} blocked`));
  line.append(split);
  if (epic.workers.length) {
    const dots = element(document, "span", "live-epic-dots");
    dots.setAttribute("role", "img");
    dots.setAttribute("aria-label", `${epic.workers.length} live ${epic.workers.length === 1 ? "worker" : "workers"}`);
    for (const worker of epic.workers) {
      const dot = element(document, "span", `live-epic-dot${worker.state === "Waiting" ? " is-waiting" : ""}`);
      dot.title = `${worker.name} · ${worker.machine} · ${worker.state}`;
      dots.append(dot);
    }
    line.append(dots);
  }
  toggle.append(line);
  const panel = element(document, "div", "live-epic-tasks"); panel.id = panelId; panel.hidden = !open;
  for (const label of ["In progress", "Open"] as const) {
    const group = taskGroup(document, epic, label, options.multiMachine);
    if (group) panel.append(group);
  }
  if (epic.counts?.merged) {
    // Closed tasks are not part of the status snapshot; Merged states the
    // authoritative count and points at the full board.
    const merged = element(document, "details", "live-epic-group live-epic-merged");
    merged.append(element(document, "summary", "live-epic-group-label", `Merged · ${epic.counts.merged}`));
    const note = element(document, "p", "live-epic-merged-note");
    if (options.explorerHref) {
      const link = element(document, "a", undefined, "Open the merged tasks in Explorer");
      link.href = options.explorerHref; link.target = "cassy-explorer"; link.rel = "noopener noreferrer";
      note.append(link);
    } else note.textContent = "Merged tasks are listed in Explorer.";
    merged.append(note);
    panel.append(merged);
  }
  toggle.onclick = () => {
    const next = !expanded.has(key);
    if (next) expanded.add(key); else expanded.delete(key);
    toggle.setAttribute("aria-expanded", String(next));
    panel.hidden = !next;
    item.classList.toggle("is-opening", next);
  };
  item.append(toggle, panel);
  return item;
}

/** Live epics for the selected project. Absent when there are none. */
export function liveEpicsSection(document: Document, options: LiveEpicsOptions): HTMLElement | undefined {
  if (!options.live.length && !options.pending.length && !options.failed.length && !options.quiet) return undefined;
  const section = element(document, "section", "live-epics");
  section.setAttribute("aria-labelledby", "live-epics-heading");
  const heading = element(document, "h3", "status-section-label", `Live epics · ${options.live.length}`); heading.id = "live-epics-heading";
  section.append(heading);
  for (const label of options.pending) {
    const notice = element(document, "p", "live-epics-notice", `Checking ${label}…`); notice.setAttribute("role", "status");
    section.append(notice);
  }
  for (const peer of options.failed) {
    const notice = element(document, "p", "live-epics-notice", peer.at ? `${peer.label} last known ${time(peer.at)}` : `${peer.label} status unavailable`);
    notice.setAttribute("role", "status");
    const retry = element(document, "button", "live-epics-retry", "Retry"); retry.type = "button"; retry.onclick = options.retry;
    retry.setAttribute("aria-label", `Retry ${peer.label} status`);
    notice.append(" ", retry);
    section.append(notice);
  }
  if (options.live.length) {
    const list = element(document, "ul", "live-epic-list");
    const all = showingAll.has(options.project);
    const rows = options.live.map((epic, index) => epicRow(document, epic, index, options));
    rows.forEach((row, index) => { row.hidden = !all && index >= LIVE_EPIC_ROWS; list.append(row); });
    section.append(list);
    if (!all && rows.length > LIVE_EPIC_ROWS) {
      const more = element(document, "button", "live-epics-more", `Show ${rows.length - LIVE_EPIC_ROWS} more`); more.type = "button";
      more.dataset.fleetFocus = "epics:more";
      more.onclick = () => {
        showingAll.add(options.project);
        for (const row of rows) row.hidden = false;
        rows[LIVE_EPIC_ROWS]?.querySelector<HTMLElement>(".live-epic-toggle")?.focus();
        more.remove();
      };
      section.append(more);
    }
  }
  if (options.quiet) {
    const text = `${options.quiet} quiet open ${options.quiet === 1 ? "epic" : "epics"} in Explorer`;
    const footer = element(document, "p", "live-epics-footer");
    if (options.explorerHref) {
      const link = element(document, "a", undefined, text); link.href = options.explorerHref; link.target = "cassy-explorer"; link.rel = "noopener noreferrer";
      footer.append(link);
    } else footer.textContent = text;
    section.append(footer);
  }
  return section;
}
