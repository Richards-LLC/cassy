import type { HubSession, SessionPhase } from "./types";

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
  /** Oldest first; the last entry is where "back" leads. */
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

export function previousSelection(state: SelectionState): SessionSelection | undefined {
  return state.history.at(-1);
}

export function canGoBack(state: SelectionState): boolean {
  return state.history.length > 0;
}

export function goBackSelection(state: SelectionState): SelectionState {
  const previous = previousSelection(state);
  if (!previous) return state;
  return { current: previous, history: state.history.slice(0, -1) };
}

/**
 * A removed machine must not survive in the back stack: walking back into a
 * credential that no longer exists is a dead end, not navigation.
 */
export function forgetMachine(state: SelectionState, machineId: string): SelectionState {
  return {
    current: state.current?.machineId === machineId ? undefined : state.current,
    history: state.history.filter((entry) => entry.machineId !== machineId),
  };
}

export function backLabel(
  previous: SessionSelection | undefined,
  machineLabel: (machineId: string) => string | undefined,
): string {
  if (!previous) return "Back";
  if (previous.session) return `Back to ${previous.session}`;
  return `Back to ${machineLabel(previous.machineId) ?? "the previous machine"}`;
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

export interface SessionPickerEntry {
  readonly machineId: string;
  readonly machineLabel: string;
  readonly session: string;
  readonly role: "supervisor" | "session";
  readonly supervisor?: string;
  readonly workerCount: number;
  readonly status: string;
  readonly title?: string;
  readonly phase?: SessionPhase;
  readonly current: boolean;
}

export interface SessionPickerInput {
  readonly machines: readonly { readonly id: string; readonly label: string }[];
  readonly sessions: ReadonlyMap<string, readonly HubSession[]>;
  readonly selection?: SessionSelection;
  readonly summaries?: ReadonlyMap<string, { readonly title: string; readonly phase: SessionPhase }>;
}

/**
 * Say the roster size in words. The hub once reported an empty roster for a
 * session running five workers, so the picker suppressed the number rather than
 * state a wrong one; the roster is now the live agent registry, so a zero is a
 * fact worth printing.
 */
export function workerCountLabel(count: number): string {
  if (count === 0) return "no workers";
  return `${count} ${count === 1 ? "worker" : "workers"}`;
}

/** The one-line summary under a session name: who runs it, how many, how it is. */
export function sessionPickerMeta(entry: SessionPickerEntry): string {
  const role = entry.supervisor ? `${entry.role} ${entry.supervisor}` : entry.role;
  return [role, workerCountLabel(entry.workerCount), entry.status].join(" · ");
}

export function sessionPickerEntries(input: SessionPickerInput): SessionPickerEntry[] {
  const selectedMachineId = input.selection?.machineId;
  const ordered = [...input.machines].sort((a, b) =>
    Number(b.id === selectedMachineId) - Number(a.id === selectedMachineId));
  return ordered.flatMap((machine) => (input.sessions.get(machine.id) ?? []).map((session) => {
    const summary = input.summaries?.get(`${machine.id}:${session.name}`);
    return {
      machineId: machine.id,
      machineLabel: machine.label,
      session: session.name,
      role: session.supervisor ? "supervisor" as const : "session" as const,
      supervisor: session.supervisor || undefined,
      workerCount: session.workers.length,
      status: session.liveness.replaceAll("_", " "),
      title: summary?.title,
      phase: summary?.phase,
      current: machine.id === selectedMachineId && session.name === input.selection?.session,
    };
  }));
}
