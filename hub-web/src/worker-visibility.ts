import type { PaneInfo } from "./types";

/**
 * Worker visibility is off by default (cas-6261). The operator asked for a Hub
 * that lists supervisors only: on a phone the worker panes crowd the list, and
 * every hidden worker is also one terminal the hub does not stream. Workers
 * come back only through an explicit control — `?workers=1` on the route, or
 * the "Workers" command in the palette — and that choice is remembered per
 * device so a debugging session survives a reload.
 */
export const WORKERS_STORAGE_KEY = "cas-commander:workers";
export const WORKERS_QUERY_PARAM = "workers";

export interface WorkerVisibilityStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

const ENABLED = new Set(["1", "true", "yes", "on"]);

function enabledFlag(value: string | null | undefined): boolean | undefined {
  if (value === null || value === undefined) return undefined;
  return ENABLED.has(value.trim().toLowerCase());
}

/** True only when the route or the stored device preference asks for workers. */
export function workersRevealed(search: string, storage: WorkerVisibilityStorage | undefined): boolean {
  const fromRoute = enabledFlag(new URLSearchParams(search).get(WORKERS_QUERY_PARAM));
  if (fromRoute !== undefined) return fromRoute;
  return enabledFlag(storage?.getItem(WORKERS_STORAGE_KEY)) ?? false;
}

/** Persist the choice; the default (hidden) is stored as an absence, not a value. */
export function saveWorkersRevealed(storage: WorkerVisibilityStorage | undefined, revealed: boolean): void {
  if (!storage) return;
  if (revealed) storage.setItem(WORKERS_STORAGE_KEY, "1");
  else storage.removeItem(WORKERS_STORAGE_KEY);
}

/** The route a reload lands on with the given visibility. */
export function workersRoute(search: string, revealed: boolean): string {
  const params = new URLSearchParams(search);
  if (revealed) params.set(WORKERS_QUERY_PARAM, "1");
  else params.delete(WORKERS_QUERY_PARAM);
  const query = params.toString();
  return query ? `?${query}` : "";
}

/** The catalog path for these visibilities; DPoP binds the bare path. */
export function sessionsPath(revealWorkers: boolean, revealDormant = false): string {
  const params = new URLSearchParams();
  if (revealWorkers) params.set(WORKERS_QUERY_PARAM, "1");
  if (revealDormant) params.set("dormant", "1");
  const query = params.toString();
  return query ? `/v1/sessions?${query}` : "/v1/sessions";
}

export interface PaneVisibility {
  readonly visible: PaneInfo[];
  /** Worker panes the default view keeps off screen. */
  readonly hiddenWorkers: PaneInfo[];
}

/** Director panes never render; worker panes render only when revealed. */
export function splitVisiblePanes(panes: readonly PaneInfo[], revealed: boolean): PaneVisibility {
  const visible: PaneInfo[] = [];
  const hiddenWorkers: PaneInfo[] = [];
  for (const pane of panes) {
    if (pane.kind === "Director") continue;
    if (pane.kind === "Worker" && !revealed) hiddenWorkers.push(pane);
    else visible.push(pane);
  }
  return { visible, hiddenWorkers };
}

/** The one-line note that stands where the worker strip would be. */
export function hiddenWorkersLabel(count: number): string {
  if (count === 0) return "";
  return count === 1 ? "1 worker hidden" : `${count} workers hidden`;
}

export function workersCommandLabel(revealed: boolean): { title: string; hint: string } {
  return revealed
    ? { title: "Workers · Shown", hint: "Hide worker panes; supervisors only" }
    : { title: "Workers · Hidden", hint: "Show worker panes for debugging" };
}
