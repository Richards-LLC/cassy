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

/** Only positive process evidence can retire a destination. Catalog omission is unknown. */
export function sessionStopped(session: HubSession): boolean {
  return Boolean(session.ended) || session.liveness === "stale_metadata";
}

export function reconcileCatalog(previous: readonly HubSession[], listed: readonly HubSession[], stopped: readonly HubSession[] = []): HubSession[] {
  const known = new Map<string, HubSession>(previous.map(session => [session.name, { ...session, stale: true }]));
  for (const session of stopped) if (sessionStopped(session)) known.set(session.name, session);
  for (const session of listed) {
    if (session.liveness === "missing_endpoint" && known.has(session.name)) continue;
    known.set(session.name, session);
  }
  return [...known.values()];
}

/** Freshness changes the label, never whether a last-known running daemon exists. */
export function visibleCatalog(sessions: readonly HubSession[], catalogFresh = true, recovery = false): HubSession[] {
  return sessions.filter(sessionRunning).map(session => catalogFresh ? session : { ...session, stale: true });
}
