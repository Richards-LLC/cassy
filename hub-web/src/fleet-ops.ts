/**
 * Fleet operations from a conversation (cas-a474, fleet-operations brief S5):
 * what each row of Tasks & progress offers, the request each action sends,
 * and the state of the menu, the inline confirmation, the Undo window and the
 * ask-to-merge preview. DOM-free, so the rules are tested on their own; the
 * rail's markup lives in fleet-ops-view.ts.
 *
 * Wire: POST /v1/sessions/{s}/operations {op_id, op:{kind,...}, expected}.
 * The hub dedupes op_id, refuses a stale `expected` with 409 {error:"stale",
 * current}, and announces FleetChanged; nothing here polls.
 */

import { fleetControlGate, type FleetControlGate, type FleetOperation } from "./fleet-permissions";
import type { Scope } from "./types";

export interface FleetAgent {
  readonly name: string;
  readonly status?: string;
  readonly current_task?: string | null;
  /** The worker's spawn generation (cas-9b08), compared by the hub as-is. */
  readonly generation?: string | number | null;
  readonly started_at?: string | null;
  readonly role?: string | null;
}

export interface FleetTask {
  readonly id: string;
  readonly title?: string;
  readonly status?: string;
  readonly assignee?: string | null;
  readonly updated_at?: string | null;
  /** The delivered tip of an awaiting-merge task. */
  readonly tip?: string | null;
  readonly branch?: string | null;
}

export type OperationKind =
  | "request_merge"
  | "focus_epic"
  | "spawn_workers"
  | "set_worker_hold"
  | "recycle_worker"
  | "shutdown_workers"
  | "assign_task";

export interface OperationRequest {
  readonly op: { readonly kind: OperationKind } & Readonly<Record<string, unknown>>;
  readonly expected: Readonly<Record<string, unknown>>;
}

/** One action as the rail offers it: its words, what it sends, and how it is undone. */
export interface FleetAction {
  readonly id: string;
  readonly operation: FleetOperation;
  readonly label: string;
  /** Destructive actions confirm inline first and are never undone. */
  readonly destructive: boolean;
  readonly request: OperationRequest;
  /** What the row says while the request is in flight ("Stopping swift-lark-3…"). */
  readonly progress: string;
  /** The announcement when it is done. */
  readonly done: string;
  /** The confirmation's question, for a destructive action. */
  readonly question?: string;
  /** The inverse, for Undo; reversible actions only. */
  readonly inverse?: () => FleetAction;
  /** cas-97d58 F12: this action is itself an Undo, so it offers no Undo of its own. */
  readonly undoing?: boolean;
}

/** Status values that mean the worker is held. */
const HELD = new Set(["held", "paused", "on_hold", "hold"]);

export function agentHeld(agent: FleetAgent): boolean {
  return HELD.has(String(agent.status ?? "").toLowerCase());
}

/** A worker the rail may act on: the supervisor itself is not one. */
export function isWorker(agent: FleetAgent): boolean {
  const role = String(agent.role ?? "").toLowerCase();
  return role !== "supervisor" && role !== "director";
}

export function idleWorkers(agents: readonly FleetAgent[]): FleetAgent[] {
  return agents.filter((agent) => isWorker(agent) && !agent.current_task && !agentHeld(agent));
}

/**
 * The worker's spawn generation as the status reported it (cas-9b08). Null
 * when unreported: the hub compares it as-is, so nothing may stand in for it.
 */
function generation(agent: FleetAgent): string | number | null {
  return agent.generation ?? null;
}

export function awaitingMerge(task: FleetTask): boolean {
  return ["awaiting_merge", "awaitingmerge"].includes(String(task.status ?? "").toLowerCase());
}

export function readyTask(task: FleetTask): boolean {
  return ["open", "ready"].includes(String(task.status ?? "").toLowerCase()) && !task.assignee;
}

/** Pause or resume a worker (O4). Reversible: its inverse is the other. */
export function holdAction(agent: FleetAgent, hold = !agentHeld(agent)): FleetAction {
  return {
    id: `${hold ? "pause" : "resume"}:${agent.name}`,
    operation: "pause-worker",
    label: hold ? "Pause" : "Resume",
    destructive: false,
    request: { op: { kind: "set_worker_hold", worker: agent.name, hold }, expected: { worker: agent.name, generation: generation(agent) } },
    progress: `${hold ? "Pausing" : "Resuming"} ${agent.name}…`,
    done: `${agent.name} ${hold ? "paused" : "resumed"}.`,
    inverse: () => holdAction({ ...agent, status: hold ? "held" : "active" }, !hold),
  };
}

/** Restart a worker (O6). Destructive: it drops the worker's in-flight context. */
export function restartAction(agent: FleetAgent): FleetAction {
  return {
    id: `restart:${agent.name}`,
    operation: "restart-worker",
    label: "Restart…",
    destructive: true,
    request: { op: { kind: "recycle_worker", worker: agent.name }, expected: { worker: agent.name, generation: generation(agent) } },
    progress: `Restarting ${agent.name}…`,
    done: `${agent.name} restarted.`,
    question: agent.current_task
      ? `Restart ${agent.name}? Its work on ${agent.current_task} in progress is lost; the task stays with it.`
      : `Restart ${agent.name}? Anything it was doing is lost.`,
  };
}

/** Stop a worker (O7). Destructive; force only as a second step after a graceful stop fails. */
export function stopAction(agent: FleetAgent, force = false): FleetAction {
  return {
    id: `${force ? "force-stop" : "stop"}:${agent.name}`,
    operation: "stop-worker",
    label: force ? "Force stop…" : "Stop…",
    destructive: true,
    request: { op: { kind: "shutdown_workers", workers: [agent.name], ...(force ? { force: true } : {}) }, expected: { worker: agent.name, generation: generation(agent) } },
    progress: `Stopping ${agent.name}…`,
    done: `${agent.name} stopped.`,
    question: force
      ? `Force stop ${agent.name}? It stops at once, without finishing its current step.`
      : agent.current_task
        ? `Stop ${agent.name}? Its task ${agent.current_task} goes back to ready.`
        : `Stop ${agent.name}?`,
  };
}

/** Assign a ready task to a worker, or unassign it (O5). Reversible. */
export function assignAction(task: FleetTask, worker: string | null): FleetAction {
  const previous = task.assignee ?? null;
  return {
    id: `assign:${task.id}:${worker ?? ""}`,
    operation: "assign-task",
    label: worker ? `Assign to ${worker}` : "Unassign",
    destructive: false,
    request: { op: { kind: "assign_task", task_id: task.id, assignee: worker }, expected: { updated_at: task.updated_at ?? null, assignee: previous } },
    progress: worker ? `Assigning ${task.id} to ${worker}…` : `Unassigning ${task.id}…`,
    done: worker ? `${task.id} assigned to ${worker}.` : `${task.id} unassigned.`,
    // The inverse's precondition is the state this action leaves.
    inverse: () => assignAction({ ...task, assignee: worker, updated_at: null }, previous),
  };
}

/** Focus the factory on an epic (O2). Reversible. */
export function focusEpicAction(epicId: string, current: string | null): FleetAction {
  return {
    id: `focus:${epicId}`,
    operation: "focus-epic",
    label: `Focus ${epicId}`,
    destructive: false,
    request: { op: { kind: "focus_epic", epic_id: epicId }, expected: { epic_id: current } },
    progress: `Focusing ${epicId}…`,
    done: `Focused on ${epicId}.`,
    // Undo restores the previous focus, or clears it when there was none.
    inverse: () => (current ? focusEpicAction(current, epicId) : clearFocusAction(epicId)),
  };
}

/** Clear the factory's epic focus: focus_epic with clear=true (cas-566b). */
export function clearFocusAction(current: string): FleetAction {
  return {
    id: "focus:clear",
    operation: "focus-epic",
    label: "Clear focus",
    destructive: false,
    request: { op: { kind: "focus_epic", clear: true }, expected: { epic_id: current } },
    progress: "Clearing the epic focus…",
    done: "Epic focus cleared.",
    inverse: () => focusEpicAction(current, null),
  };
}

/**
 * The hub's own inverse for Undo (cas-31f0): `outcome.inverse` is {op,
 * expected} preconditioned on the state the operation left. It replaces the
 * locally derived one when present; a late Undo comes back stale.
 */
export function hubInverse(action: FleetAction, outcome: Readonly<Record<string, unknown>> | undefined): FleetAction | undefined {
  const inverse = outcome?.inverse as { op?: Record<string, unknown>; expected?: Record<string, unknown> } | undefined;
  const local = action.inverse?.();
  if (!inverse?.op || typeof inverse.op.kind !== "string") return local;
  return {
    id: local?.id ?? `undo:${action.id}`,
    operation: action.operation,
    label: local?.label ?? "Undo",
    destructive: false,
    request: { op: inverse.op as OperationRequest["op"], expected: inverse.expected ?? {} },
    progress: local?.progress ?? "Undoing…",
    done: local?.done ?? "Undone.",
  };
}

/** Add workers (O3). Additive: no Undo; the result names the new workers. */
export function spawnAction(count: number, taskId?: string): FleetAction {
  const n = Math.max(1, Math.min(4, Math.round(count)));
  return {
    id: `spawn:${n}:${taskId ?? ""}`,
    operation: "add-workers",
    label: n === 1 ? "Add 1 worker" : `Add ${n} workers`,
    destructive: false,
    request: { op: { kind: "spawn_workers", count: n, ...(taskId ? { task_id: taskId } : {}) }, expected: {} },
    progress: `Adding ${n === 1 ? "a worker" : `${n} workers`}…`,
    done: n === 1 ? "Added a worker." : `Added ${n} workers.`,
  };
}

/**
 * The exact message O1 sends the supervisor, shown before it is sent: the hub
 * generates it with the same template (cas-cli ops::fleet::request_merge_text,
 * cas-566b), so the preview is what the supervisor receives.
 */
export function mergeRequestMessage(task: FleetTask): string {
  return `Operator request from Commander: please merge ${task.id} (${task.title ?? ""}).\nBranch: ${task.branch ?? "<not recorded>"}\nTip: ${task.tip ?? "<not recorded>"}\nIt is awaiting merge. Merge it into its epic, or reply with what blocks it.`;
}

/** Ask the supervisor to merge (O1). A message, not a mutation: no Undo, no confirm. */
export function requestMergeAction(task: FleetTask): FleetAction {
  return {
    id: `merge:${task.id}`,
    operation: "ask-merge",
    label: "Ask supervisor to merge",
    destructive: false,
    request: { op: { kind: "request_merge", task_id: task.id }, expected: { status: "awaiting_merge", tip: task.tip ?? null } },
    progress: `Asking the supervisor to merge ${task.id}…`,
    done: `Asked the supervisor to merge ${task.id}.`,
  };
}

/** The actions an agent row's ⋯ menu lists, in the brief's order: Pause or Resume, Restart…, Stop…. */
export function agentMenu(agent: FleetAgent): FleetAction[] {
  return [holdAction(agent), restartAction(agent), stopAction(agent)];
}

/** One menu item with its gate: allowed, or disabled with the reason and command. */
export interface FleetMenuItem {
  readonly action: FleetAction;
  readonly gate: FleetControlGate;
}

export function gatedItems(actions: readonly FleetAction[], scopes: readonly Scope[], origin: string): FleetMenuItem[] {
  return actions.map((action) => ({ action, gate: fleetControlGate(scopes, action.operation, origin) }));
}

/**
 * What the row says when the fleet moved on (409 stale): what changed, in
 * the operator's words, from the hub's `current`.
 */
export function staleMessage(action: FleetAction, current: Readonly<Record<string, unknown>> | undefined): string {
  const op = action.request.op;
  const worker = typeof op.worker === "string" ? op.worker : Array.isArray(op.workers) ? String(op.workers[0]) : undefined;
  if (op.kind === "assign_task") {
    const assignee = typeof current?.assignee === "string" ? current.assignee : null;
    return assignee ? `${String(op.task_id)} is already assigned to ${assignee}.` : `${String(op.task_id)} changed since you looked.`;
  }
  if (op.kind === "focus_epic") {
    const epic = typeof current?.epic_id === "string" ? current.epic_id : null;
    return epic ? `The factory is already focused on ${epic}.` : "The factory's focus changed since you looked.";
  }
  if (op.kind === "request_merge") {
    const status = typeof current?.status === "string" ? current.status.replaceAll("_", " ") : null;
    return `${String(op.task_id)} is ${status ?? "no longer awaiting merge"}.`;
  }
  if (worker) {
    // The hub answers {worker, generation}; a null generation means no live worker by that name.
    if (current?.exists === false || (current && "generation" in current && current.generation === null)) return `${worker} is already gone.`;
    if (op.kind === "set_worker_hold" && typeof current?.held === "boolean") return `${worker} is already ${current.held ? "paused" : "running"}.`;
    return `${worker} already restarted.`;
  }
  return "The fleet changed since you looked.";
}

/** How long a reversible action's Undo stays offered. */
export const UNDO_WINDOW_MS = 8_000;

export interface PendingOperation {
  readonly rowKey: string;
  readonly action: FleetAction;
}

export interface UndoOffer {
  readonly action: FleetAction;
  readonly label: string;
  readonly expiresAt: number;
}

export interface RowNote {
  readonly text: string;
  readonly tone: "error" | "stale";
}

/**
 * The rail's operation state: one open menu, one inline confirmation, the
 * requests in flight, an Undo offer and per-row notes. Pure: every change
 * returns nothing but mutates this object; time comes in as an argument.
 */
export interface FleetRequestOwner {
  readonly selection: number;
  readonly rowKey: string;
  readonly request: symbol;
}

export class FleetOpsState {
  private selectionKey: string | undefined;
  private selectionVersion = 0;
  private readonly requests = new Map<string, symbol>();
  private readonly dismissedPhoneNotices = new WeakSet<FleetAction | RowNote | UndoOffer>();

  /** Dismiss only this presentation; requests, row notes and Undo remain intact. */
  dismissPhoneNotice(notice: FleetAction | RowNote | UndoOffer): void { this.dismissedPhoneNotices.add(notice); }
  phoneNoticeDismissed(notice: FleetAction | RowNote | UndoOffer): boolean { return this.dismissedPhoneNotices.has(notice); }

  get selectionEpoch(): number { return this.selectionVersion; }

  /** Selection lifetime belongs to state, never to a replaceable DOM container. */
  select(key: string): boolean {
    if (this.selectionKey === key) return false;
    this.selectionKey = key;
    this.selectionVersion += 1;
    this.closeMenus(); this.cancelConfirm();
    this.pending.clear(); this.requests.clear(); this.notes.clear();
    this.undo = undefined; this.result = undefined; this.announcement = "";
    return true;
  }

  owns(owner: FleetRequestOwner): boolean {
    return owner.selection === this.selectionVersion && this.requests.get(owner.rowKey) === owner.request;
  }

  menuFor: string | undefined;
  confirm: { rowKey: string; action: FleetAction } | undefined;
  preview: { rowKey: string; task: FleetTask } | undefined;
  assignFor: string | undefined;
  /** The phone picker's form value survives a status redraw, never a new picker. */
  pickerQuery = "";
  readonly pending = new Map<string, FleetAction>();
  readonly notes = new Map<string, RowNote>();
  undo: UndoOffer | undefined;
  /**
   * cas-97d58 F12: a destructive action's outcome (Stop, Restart, End) has no
   * Undo, and its row may leave the panel; this keeps the result on screen
   * for the same window an Undo would have.
   */
  result: { label: string; expiresAt: number } | undefined;
  announcement = "";

  toggleMenu(rowKey: string): void {
    this.menuFor = this.menuFor === rowKey ? undefined : rowKey;
    this.confirm = undefined;
    this.assignFor = undefined;
  }

  closeMenus(): void {
    this.menuFor = undefined;
    this.assignFor = undefined;
    this.preview = undefined;
    this.pickerQuery = "";
  }

  /** Choose a menu item: a destructive one opens its confirmation; anything else runs. Returns what should run now. */
  choose(rowKey: string, action: FleetAction): FleetAction | undefined {
    this.menuFor = undefined;
    this.assignFor = undefined;
    if (action.destructive) { this.confirm = { rowKey, action }; return undefined; }
    return action;
  }

  cancelConfirm(): void { this.confirm = undefined; }

  /** The confirmation's own button: what to run, once. */
  confirmed(): FleetAction | undefined {
    const action = this.confirm?.action;
    this.confirm = undefined;
    return action;
  }

  openPreview(rowKey: string, task: FleetTask): void { this.preview = { rowKey, task }; this.menuFor = undefined; }

  started(rowKey: string, action: FleetAction): FleetRequestOwner {
    const request = Symbol("fleet operation");
    this.requests.set(rowKey, request);
    this.pending.set(rowKey, action);
    this.notes.delete(rowKey);
    this.result = undefined;
    this.announcement = action.progress;
    return { selection: this.selectionVersion, rowKey, request };
  }

  succeeded(rowKey: string, action: FleetAction, now: number, outcome?: Readonly<Record<string, unknown>>): void {
    this.pending.delete(rowKey);
    this.requests.delete(rowKey);
    this.notes.delete(rowKey);
    this.announcement = action.done;
    const inverse = !action.undoing && (action.inverse || outcome?.inverse) ? hubInverse(action, outcome) : undefined;
    this.undo = inverse ? { action: inverse, label: action.done, expiresAt: now + UNDO_WINDOW_MS } : undefined;
    this.result = !inverse && action.destructive ? { label: action.done, expiresAt: now + UNDO_WINDOW_MS } : undefined;
  }

  failed(rowKey: string, action: FleetAction, failure: { stale?: boolean; current?: Readonly<Record<string, unknown>>; detail?: string; subject?: string }): void {
    this.pending.delete(rowKey);
    this.requests.delete(rowKey);
    const detail = failure.detail ?? "the machine refused it";
    const suffix = /[.!?…]\s*$/.test(detail) ? "" : ".";
    const subject = failure.subject ? ` ${failure.subject}` : "";
    const label = action.operation === "assign-task"
      ? action.request.op.assignee ? `assign ${action.request.op.task_id} to ${action.request.op.assignee}` : `unassign ${action.request.op.task_id}`
      : `${action.label.replace(/…$/, "").toLowerCase()}${subject}`;
    const text = failure.stale ? staleMessage(action, failure.current) : `Could not ${label}: ${detail}${suffix}`;
    this.notes.set(rowKey, { text, tone: failure.stale ? "stale" : "error" });
    this.announcement = text;
  }

  /** A destructive action's visible result while it lasts (F12). */
  currentResult(now: number): { label: string; expiresAt: number } | undefined {
    if (this.result && now >= this.result.expiresAt) this.result = undefined;
    return this.result;
  }

  /** The Undo offer while it lasts; undefined once it has expired. */
  currentUndo(now: number): UndoOffer | undefined {
    if (this.undo && now >= this.undo.expiresAt) this.undo = undefined;
    return this.undo;
  }

  takeUndo(now: number): FleetAction | undefined {
    const offer = this.currentUndo(now);
    this.undo = undefined;
    return offer ? { ...offer.action, undoing: true } : undefined;
  }
}

/** A client UUID for op_id; a retry of the same action reuses it. */
export function newOperationId(random: () => string = () => crypto.randomUUID()): string {
  return random();
}
