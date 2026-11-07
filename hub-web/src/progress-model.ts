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
  /** The factory TUI's child split: open (open or blocked), in progress (in progress or parked), merged (delivered closures). */
  readonly counts?: EpicCounts;
  /** cas-2f4d: a frozen train's measured line, "Frozen 2:05 PM ET · ETA 2:50 PM ET". */
  readonly freeze?: { readonly frozen_at: string; readonly eta?: string | null; readonly summary: string } | null;
}
export interface EpicCounts { readonly open: number; readonly in_progress: number; readonly merged: number }

/** "EPIC: Release train" reads "Release train"; a title that is only a prefix is no title. */
export function epicTitle(epic: Pick<ReleaseEpic, "title"> | undefined): string {
  return (epic?.title ?? "").replace(/^\s*epic\s*(?::|—|–|-)\s*/i, "").trim();
}

/** Only a complete, non-negative split is drawn; anything else is unreported. */
export function epicCounts(epic: ReleaseEpic | undefined): (EpicCounts & { total: number }) | undefined {
  const counts = epic?.counts;
  if (!counts || ![counts.open, counts.in_progress, counts.merged].every(value => Number.isSafeInteger(value) && value >= 0)) return undefined;
  return { open: counts.open, in_progress: counts.in_progress, merged: counts.merged, total: counts.open + counts.in_progress + counts.merged };
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
  /** The conversation list's project key (sessionProjectKey). */
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
export type EpicTaskGroup = "In progress" | "Open" | "Merged";

/** One task-state source for an epic's sections and its split: a merged
 * delivery is Merged whatever its parked status or worker activity says. */
export function epicTaskGroup(task: ProgressTask): EpicTaskGroup | undefined {
  if (task.delivery_state === "merged") return "Merged";
  const label = statusLabel(task.status);
  if (["In progress", "Awaiting merge", "Awaiting verification"].includes(label)) return "In progress";
  return ["Open", "Ready", "Blocked"].includes(label) ? "Open" : undefined;
}

/** The split as drawn: open and in progress counted from the very task rows
 * the sections list, merged as the producer's delivered closures plus any
 * listed task whose delivery has merged. Undefined when unreported. */
export function displayCounts(epic: ReleaseEpic | undefined, tasks: readonly ProgressTask[]): (EpicCounts & { total: number }) | undefined {
  const base = epicCounts(epic);
  if (!base) return undefined;
  const count = (group: EpicTaskGroup) => tasks.filter(task => epicTaskGroup(task) === group).length;
  const split = { open: count("Open"), in_progress: count("In progress"), merged: base.merged + count("Merged") };
  return { ...split, total: split.open + split.in_progress + split.merged };
}
export interface EpicTaskRow { readonly task: ProgressTask; readonly worker?: string; readonly machine: string; readonly state: string; readonly group: EpicTaskGroup }
export interface ProjectEpic {
  readonly key: string;
  readonly epic: ReleaseEpic;
  readonly title: string;
  readonly counts?: EpicCounts & { total: number };
  readonly machines: readonly string[];
  readonly workers: readonly { name: string; machine: string; state: string }[];
  readonly rows: readonly EpicTaskRow[];
  readonly building: number;
  readonly parked: number;
  readonly blocked: number;
  readonly stale: boolean;
}

/** Display the task's delivery truth before worker liveness; never infer a merge from closure. */
function epicTaskState(task: ProgressTask, worker: WorkerProgress | undefined): string {
  if (task.delivery_state === "merged") return "Merged";
  if (statusLabel(task.status) === "Awaiting merge") return "Parked";
  if (statusLabel(task.status) === "Blocked") return "Blocked";
  if (["Open", "Ready"].includes(statusLabel(task.status))) return "Ready";
  const state = worker ? workerState(worker, [task]) : statusLabel(task.status);
  return state === "Executing" ? "Building" : state;
}

const LIVE_EPIC = ["Open", "Ready", "In progress", "Blocked", "Awaiting merge", "Awaiting verification"];

/** The selected session's project only, joined across machines by epic id.
 * The release train has its own header; open epics without in-flight work or
 * a live worker only add to the quiet count. Untitled epics never render. */
export function projectEpics(sources: readonly EpicSessionProgress[], project: string, train?: string): { live: ProjectEpic[]; quiet: number } {
  const epics = new Map<string, { epic: ReleaseEpic; machines: Set<string>; workers: Map<string, ProjectEpic["workers"][number]>; rows: Map<string, EpicTaskRow>; stale: boolean }>();
  const latest = new Map<string, ReleaseEpic>();
  // Prefer the newest authoritative task/progress snapshot when sessions overlap.
  for (const source of [...sources].filter(source => source.project === project).sort((a, b) => b.received - a.received)) {
    const workers = workerProgress(source.roster, source.agents, source.tasks, source.supervisor);
    for (const epic of source.epics) {
      if (!latest.has(epic.id)) latest.set(epic.id, epic);
      if (epic.id === train || !epicTitle(latest.get(epic.id)) || !LIVE_EPIC.includes(statusLabel(latest.get(epic.id)?.status))) continue;
      let entry = epics.get(epic.id);
      if (!entry) epics.set(epic.id, entry = { epic, machines: new Set(), workers: new Map(), rows: new Map(), stale: true });
      else if (!epicCounts(entry.epic) && epicCounts(epic)) entry.epic = { ...entry.epic, counts: epic.counts };
      entry.machines.add(source.machineLabel);
      entry.stale &&= source.stale;
      const tasks = source.tasks.filter(task => task.epic_id === epic.id);
      for (const task of tasks) {
        const group = epicTaskGroup(task);
        if (!group || entry.rows.has(task.id)) continue;
        const worker = workers.find(item => item.currentTask === task.id)
          ?? (task.assignee ? workers.find(item => item.name === task.assignee || item.agent?.id === task.assignee) : undefined);
        entry.rows.set(task.id, { task, worker: worker?.name ?? task.assignee ?? undefined, machine: source.machineLabel, state: epicTaskState(task, worker), group });
      }
      for (const worker of workers.filter(item => tasks.some(task => task.id === item.currentTask))) {
        const agent = worker.agent;
        const live = agent && !["dead", "offline"].includes(agent.liveness ?? "")
          && !["Stopped", "Offline"].includes(statusLabel(agent.status))
          && (agent.liveness !== undefined || ["Active", "Idle", "Paused"].includes(statusLabel(agent.status)));
        if (!source.stale && live) entry.workers.set(`${source.machine}:${worker.name}`, { name: worker.name, machine: source.machineLabel, state: workerState(worker, tasks) });
      }
    }
  }
  const all = [...epics].map(([key, entry]): ProjectEpic => {
    const rows = [...entry.rows.values()];
    const count = (state: string) => rows.filter(row => row.state === state).length;
    const blocked = count("Blocked");
    const parked = count("Parked");
    return { key, epic: entry.epic, title: epicTitle(entry.epic), counts: displayCounts(entry.epic, rows.map(row => row.task)), machines: [...entry.machines], workers: [...entry.workers.values()], rows, stale: entry.stale,
      blocked, parked, building: rows.filter(row => row.group === "In progress").length - parked };
  });
  const live = all.filter(item => item.building + item.parked + item.blocked > 0 || item.workers.length > 0);
  return {
    live: live.sort((a, b) => Number(b.blocked > 0) - Number(a.blocked > 0)
      || (b.building + b.parked + b.blocked) - (a.building + a.parked + a.blocked)
      || b.workers.length - a.workers.length || a.title.localeCompare(b.title) || a.key.localeCompare(b.key)),
    quiet: all.length - live.length,
  };
}
