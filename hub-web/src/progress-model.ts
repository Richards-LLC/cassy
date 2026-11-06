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


/** The rail and Team use the same lifecycle/liveness precedence. */
export function workerState(worker: WorkerProgress, tasks: readonly FleetTask[]): string {
  const agent = worker.agent;
  if (["dead", "offline"].includes(agent?.liveness ?? "")) return "Offline";
  const task = tasks.find(item => item.id === worker.currentTask);
  if (task?.delivery_state === "merged") return "Merged";
  if (statusLabel(task?.status) === "Blocked") return "Blocked";
  if (statusLabel(task?.status) === "Awaiting merge") return "Parked for merge";
  const live: Record<string, string> = { executing: "Executing", waiting_for_input: "Waiting", awaiting_first_prompt: "Waiting", budget_aborted: "Budget stopped", stalled: "Stalled" };
  return live[agent?.liveness ?? ""] ?? statusLabel(agent?.status);
}

/** Existing status snapshots from the running sessions on paired machines. */
export interface EpicSessionProgress {
  readonly project: string;
  readonly machine: string;
  readonly machineLabel: string;
  readonly session: string;
  readonly supervisor: string;
  readonly roster: readonly string[];
  readonly agents: readonly FleetAgent[];
  readonly tasks: readonly ProgressTask[];
  readonly epics: readonly ReleaseEpic[];
  readonly stale: boolean;
  readonly received: number;
}
export interface EpicTab {
  readonly key: string;
  readonly epic: ReleaseEpic;
  readonly machines: readonly string[];
  readonly workers: readonly { name: string; machine: string; state: string }[];
  readonly rows: readonly { task: ProgressTask; worker: string; machine: string; state: string }[];
  readonly inFlight: number;
  readonly stale: boolean;
}

/** Display the task's delivery truth before worker liveness; never infer a merge from closure. */
function epicTaskState(task: ProgressTask, worker: WorkerProgress | undefined): string {
  if (task.delivery_state === "merged") return "Merged";
  if (statusLabel(task.status) === "Awaiting merge") return "Parking";
  if (["Open", "Ready"].includes(statusLabel(task.status))) return "Ready";
  const state = worker ? workerState(worker, [task]) : statusLabel(task.status);
  return state === "Executing" ? "Building" : state;
}

/** Project identity groups a shared epic across machines; same IDs in other projects stay separate. */
export function epicTabs(sources: readonly EpicSessionProgress[]): EpicTab[] {
  const tabs = new Map<string, { epic: ReleaseEpic; machines: Set<string>; workers: Map<string, EpicTab["workers"][number]>; rows: Map<string, EpicTab["rows"][number]>; stale: boolean }>();
  const latest = new Map<string, ReleaseEpic>();
  // Prefer the newest authoritative task/progress snapshot when sessions overlap.
  for (const source of [...sources].sort((a, b) => b.received - a.received)) {
    const workers = workerProgress(source.roster, source.agents, source.tasks, source.supervisor);
    for (const epic of source.epics) {
      const key = JSON.stringify([source.project, epic.id]);
      if (!latest.has(key)) latest.set(key, epic);
      if (!["Open", "Ready", "In progress", "Blocked", "Awaiting merge", "Awaiting verification"].includes(statusLabel(latest.get(key)?.status))) continue;
      let tab = tabs.get(key);
      if (!tab) {
        tab = { epic, machines: new Set(), workers: new Map(), rows: new Map(), stale: true };
        tabs.set(key, tab);
      } else if (!tab.epic.progress && epic.progress) tab.epic = { ...tab.epic, progress: epic.progress };
      tab.machines.add(source.machineLabel);
      tab.stale &&= source.stale;
      const tasks = source.tasks.filter(task => task.epic_id === epic.id);
      const groups = taskGroups(tasks, [epic], [], true);
      for (const task of groups.filter(group => group.label !== "Finished").flatMap(group => group.tasks)) {
        const worker = workers.find(item => item.currentTask === task.id)
          ?? (task.assignee ? workers.find(item => item.name === task.assignee || item.agent?.id === task.assignee) : undefined);
        if (!tab.rows.has(task.id)) tab.rows.set(task.id, { task, worker: worker?.name ?? task.assignee ?? "Unassigned", machine: source.machineLabel, state: epicTaskState(task, worker) });
      }
      for (const worker of workers.filter(item => tasks.some(task => task.id === item.currentTask))) {
        const agent = worker.agent;
        const live = agent && !["dead", "offline"].includes(agent.liveness ?? "")
          && !["Stopped", "Offline"].includes(statusLabel(agent.status))
          && (agent.liveness !== undefined || ["Active", "Idle", "Paused"].includes(statusLabel(agent.status)));
        if (!source.stale && live) tab.workers.set(`${source.machine}:${worker.name}`, { name: worker.name, machine: source.machineLabel, state: workerState(worker, tasks) });
      }
    }
  }
  return [...tabs].map(([key, tab]) => ({ key, epic: tab.epic, machines: [...tab.machines], workers: [...tab.workers.values()], rows: [...tab.rows.values()], stale: tab.stale,
    inFlight: [...tab.rows.values()].filter(({ task }) => task.delivery_state !== "merged" && ["In progress", "Awaiting merge", "Awaiting verification"].includes(statusLabel(task.status))).length,
  })).sort((a, b) => b.inFlight - a.inFlight || b.workers.length - a.workers.length || a.key.localeCompare(b.key));
}

/** A manual choice survives changing activity, but cannot pin an epic after it closes. */
export function selectedEpicTab(tabs: readonly EpicTab[], choice?: string): string | undefined {
  return tabs.find(tab => tab.key === choice)?.key ?? tabs[0]?.key;
}
