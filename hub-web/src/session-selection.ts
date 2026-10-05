import type { HubSession } from "./types";

/**
 * One machine runs several Cassy sessions, so "where am I" is a pair, not a
 * session name: the machine plus the session open on it. Machine-only
 * selections are legitimate — that is the state right after a machine is
 * picked and before one of its sessions is opened.
 */
export interface SessionSelection {
  readonly machineId: string;
  readonly session?: string;
}

export interface SelectionState {
  readonly current?: SessionSelection;
  /** Oldest first; the last entry is the selection before `current`. */
  readonly history: readonly SessionSelection[];
}

export interface SelectionStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

export const SELECTION_HISTORY_LIMIT = 20;
const STORAGE_KEY = "cas-commander:selection";
const SELECTION_SCHEMA_VERSION = 1;

export function sameSelection(a: SessionSelection | undefined, b: SessionSelection | undefined): boolean {
  if (!a || !b) return a === b;
  return a.machineId === b.machineId && (a.session ?? "") === (b.session ?? "");
}

export function selectSelection(state: SelectionState, next: SessionSelection): SelectionState {
  if (sameSelection(state.current, next)) return state;
  const history = state.current ? [...state.history, state.current].slice(-SELECTION_HISTORY_LIMIT) : state.history;
  return { current: next, history };
}

/**
 * Where a pairing lands (cas-b452, journey F39). A first pairing lands on the
 * new machine. A re-pair started from an open conversation returns to that
 * conversation, on whichever machine it is, instead of the landing page.
 */
export function selectionAfterPairing(machineId: string, repairing: boolean, current: SessionSelection | undefined): SessionSelection {
  return repairing && current?.session !== undefined ? { machineId: current.machineId, session: current.session } : { machineId };
}

/**
 * A removed machine must not survive in the selection or its history: a
 * credential that no longer exists is a dead end, not a place to return to.
 */
export function forgetMachine(state: SelectionState, machineId: string): SelectionState {
  return {
    current: state.current?.machineId === machineId ? undefined : state.current,
    history: state.history.filter((entry) => entry.machineId !== machineId),
  };
}

export function loadStoredSelection(storage: SelectionStorage | undefined): SessionSelection | undefined {
  if (!storage) return undefined;
  try {
    const raw = storage.getItem(STORAGE_KEY);
    if (!raw) return undefined;
    const candidate = JSON.parse(raw) as Record<string, unknown>;
    if (candidate?.version !== SELECTION_SCHEMA_VERSION) return undefined;
    if (typeof candidate.machineId !== "string" || candidate.machineId.length === 0) return undefined;
    if (candidate.session !== undefined && typeof candidate.session !== "string") return undefined;
    return candidate.session === undefined
      ? { machineId: candidate.machineId }
      : { machineId: candidate.machineId, session: candidate.session };
  } catch {
    return undefined;
  }
}

export function saveStoredSelection(storage: SelectionStorage | undefined, selection: SessionSelection): void {
  if (!storage) return;
  try {
    storage.setItem(STORAGE_KEY, JSON.stringify({ version: SELECTION_SCHEMA_VERSION, ...selection }));
  } catch {
    // A private or full browser store must not block session switching.
  }
}

export function clearStoredSelection(storage: SelectionStorage | undefined): void {
  if (!storage) return;
  try {
    storage.removeItem(STORAGE_KEY);
  } catch {
    // Same contract as saving: storage is a convenience, never a gate.
  }
}

/**
 * Restore is claimed against the hub's own session list rather than assumed:
 * a session that ended between visits must land on the empty canvas, not on a
 * name that cannot be attached.
 */
export function restorableSession(
  stored: SessionSelection | undefined,
  machineId: string,
  sessions: readonly HubSession[],
): string | undefined {
  if (!stored?.session || stored.machineId !== machineId) return undefined;
  return sessions.some((session) => session.name === stored.session) ? stored.session : undefined;
}

/**
 * The conversation a phone opens right after pairing a machine (journey F8):
 * the machine's first session that can actually be attached. Pairing then
 * lands on that conversation instead of a list the operator must tap through.
 * A dormant, unreachable or not-yet-live session is never opened on the
 * operator's behalf; with none live, the list stays as it was.
 */
export function pairedSessionToOpen(sessions: readonly HubSession[]): string | undefined {
  return sessions.find((session) => session.liveness === "live" && !session.dormant && !session.unreachable)?.name;
}
