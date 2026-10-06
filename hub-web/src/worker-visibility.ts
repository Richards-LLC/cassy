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

/**
 * Require a fresh, live supervisor; recovery is explicit and pending work
 * stays visible. A supervisor that has not spawned workers yet is still one
 * the operator can talk to: the hub lists it (cas-7103), so every surface
 * does (cas-645e). This is the one rule for the list, the palette, the
 * session picker and its "N available" count.
 */
export function sessionReachable(session: HubSession, catalogFresh = true): boolean {
  return catalogFresh && !session.unreachable && session.dormant !== true
    && session.liveness === "live" && Boolean(session.supervisor.trim());
}

export function visibleCatalog(sessions: readonly HubSession[], pending: (name: string) => boolean, catalogFresh = true, recovery = false): HubSession[] {
  return sessions.flatMap(session => {
    if (sessionReachable(session, catalogFresh)) return [session];
    if (pending(session.name)) return [{ ...session, unreachable: true }];
    // The recovery view ("Show dormant sessions") lists every supervisor,
    // whatever its state, but not a row with no supervisor: there is no one
    // to talk to, so the conversation list cannot show it either
    // (cas-645e QA F02).
    return recovery && Boolean(session.supervisor.trim()) ? [session] : [];
  });
}

/** A filtered server response cannot erase a conversation awaiting its outcome. */
export function retainPendingSessions(previous: readonly HubSession[], incoming: readonly HubSession[], pending: (name: string) => boolean): HubSession[] {
  const names = new Set(incoming.map(session => session.name));
  return [...incoming, ...previous.filter(session => !names.has(session.name) && pending(session.name)).map(session => ({ ...session, unreachable: true }))];
}
