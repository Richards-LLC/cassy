import type { FleetAgent, FleetTask } from "./fleet-ops";

const LABELS: Readonly<Record<string, string>> = {
  inprogress: "In progress", awaitingmerge: "Awaiting merge", open: "Open", ready: "Ready",
  blocked: "Blocked", closed: "Closed", cancelled: "Cancelled", canceled: "Cancelled",
  active: "Active", idle: "Idle", held: "Paused", paused: "Paused", stopped: "Stopped",
  working: "Working", offline: "Offline", awaitingverification: "Awaiting verification",
};

/** One wire state has the same human label in CamelCase, snake_case or uppercase. */
export function statusLabel(value: unknown): string {
  if (typeof value !== "string" || !value.trim()) return "Not reported";
  const key = value.replace(/[^a-z0-9]/gi, "").toLowerCase();
  return LABELS[key] ?? value.replace(/([a-z])([A-Z])/g, "$1 $2").replace(/[_-]+/g, " ").toLowerCase().replace(/^./, letter => letter.toUpperCase());
}

export function statusClass(value: unknown): string {
  return statusLabel(value).toLowerCase().replace(/[^a-z0-9]+/g, "-");
}

export interface WorkerProgress {
  readonly name: string;
  /** Only real reported metadata may authorize a worker control. */
  readonly agent?: FleetAgent;
  readonly currentTask?: string;
  readonly work: string;
}

/** Catalog is the selected session's roster; status supplies exact-name detail.
 * Missing detail never implies idle or healthy. An absent catalog may fall back
 * to reported workers, but an explicitly empty roster stays empty.
 */
export function workerProgress(roster: readonly string[] | undefined, agents: readonly FleetAgent[], tasks: readonly FleetTask[], supervisor?: string): WorkerProgress[] {
  const names = roster ?? agents.filter(agent => agent.name !== supervisor && agent.role?.toLowerCase() !== "supervisor").map(agent => agent.name);
  return [...new Set(names)].filter(name => name && name !== supervisor).map(name => {
    const agent = agents.find(item => item.name === name && item.role?.toLowerCase() !== "supervisor");
    const currentTask = agent?.current_task || undefined;
    const task = currentTask ? tasks.find(item => item.id === currentTask) : undefined;
    const work = currentTask ? (task?.title || "Task details not reported")
      : agent?.current_task === null ? "No current task" : "Current work not reported";
    return { name, agent, currentTask, work };
  });
}

/** Additive status fields; older hubs deliberately keep progress unavailable. */
export interface ReleaseEpic {
  readonly id: string;
  readonly title?: string;
  readonly focused?: boolean;
  readonly status?: string;
  readonly assignee?: string | null;
  readonly epic_verification_owner?: string | null;
  readonly progress?: { readonly total: number; readonly merged: number; readonly merges_per_hour: number; readonly last_merged_at?: string | null };
}
export type ProgressTask = FleetTask & { readonly epic_id?: string | null; readonly delivery_state?: string | null };

/** Status epics are a project catalog, never a session ownership list. */
function supervisorEpics(epics: readonly ReleaseEpic[], focused: string | null, supervisors: readonly string[]): ReleaseEpic[] {
  const open = epics.filter(epic => ["Open", "Ready", "In progress", "Blocked", "Awaiting merge"].includes(statusLabel(epic.status)));
  const pinned = open.find(epic => epic.id === focused) ?? open.find(epic => epic.focused);
  return pinned ? [pinned] : open.filter(epic =>
    (epic.assignee != null && supervisors.includes(epic.assignee))
    || (epic.epic_verification_owner != null && supervisors.includes(epic.epic_verification_owner)));
}

export function releaseProgress(epics: readonly ReleaseEpic[], focused: string | null, tasks: readonly ProgressTask[], supervisors: readonly string[] = []): ReleaseEpic | undefined {
  epics = supervisorEpics(epics, focused, supervisors);
  return epics.find(epic => epic.id === focused || epic.focused)
    ?? [...epics].sort((a, b) => tasks.filter(task => task.epic_id === b.id && statusLabel(task.status) === "In progress").length
      - tasks.filter(task => task.epic_id === a.id && statusLabel(task.status) === "In progress").length)[0];
}

export function taskGroups(tasks: readonly ProgressTask[], epics: readonly ReleaseEpic[], workers: readonly string[], all: boolean, supervisors: readonly string[] = [], focused: string | null = null): { label: string; tasks: ProgressTask[] }[] {
  const epicIds = new Set(supervisorEpics(epics, focused, supervisors).map(epic => epic.id));
  const scoped = tasks.filter(task => all || (task.epic_id ? epicIds.has(task.epic_id) : (task.assignee != null && workers.includes(task.assignee))));
  const needs = (task: ProgressTask) => task.delivery_state !== "merged" && ["Awaiting merge", "Blocked", "Ask"].includes(statusLabel(task.status));
  const ready = (task: ProgressTask) => ["Open", "Ready"].includes(statusLabel(task.status));
  const done = (task: ProgressTask) => ["Closed", "Cancelled"].includes(statusLabel(task.status));
  return [
    { label: "Needs you", tasks: scoped.filter(needs) },
    { label: "In progress", tasks: scoped.filter(task => !needs(task) && !ready(task) && !done(task)) },
    { label: "Ready", tasks: scoped.filter(ready) },
    ...(all ? [{ label: "Finished", tasks: scoped.filter(done) }] : []),
  ];
}
