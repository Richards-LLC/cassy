import type { HubSession, PaneInfo } from "./types";

/**
 * Commander lists supervisors only (cas-6261): worker panes are never
 * requested or rendered, and the conversation reads its supervisor's pane
 * alone (cas-0546). Workers are listed in Tasks & progress.
 */

/** The catalog path, supervisors only; DPoP binds the bare path. */
export function sessionsPath(revealDormant = false): string {
  return revealDormant ? "/v1/sessions?dormant=1" : "/v1/sessions";
}

/** The panes a conversation can read: never the director, a worker or a pane that exited. */
export function readablePanes(panes: readonly PaneInfo[]): PaneInfo[] {
  return panes.filter((pane) => pane.kind !== "Director" && pane.kind !== "Worker" && !pane.exited);
}

/** A running daemon is distinct from a machine link that cannot be reached. */
export function sessionRunning(session: HubSession): boolean {
  return !session.ended && session.liveness === "live" && Boolean(session.supervisor.trim());
}

export function sessionReachable(session: HubSession, catalogFresh = true): boolean {
  return sessionRunning(session) && catalogFresh && !session.unreachable && session.dormant !== true;
}

/** An outage is shown once on the machine, rather than on every cached row. */
export function pendingSessionLabel(session: HubSession | undefined, machineConnected: boolean): string | undefined {
  return session?.unreachable && machineConnected ? "Waiting for supervisor" : undefined;
}

/** A fresh catalog is authoritative, even when a missing destination has unsent messages. */
export function visibleCatalog(sessions: readonly HubSession[], catalogFresh = true, recovery = false): HubSession[] {
  return catalogFresh ? sessions.filter(session => sessionRunning(session) && (recovery || !session.dormant)) : [];
}
