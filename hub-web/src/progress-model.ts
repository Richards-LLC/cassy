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
