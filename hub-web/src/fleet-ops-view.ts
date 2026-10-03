/**
 * The rail's fleet controls (cas-a474, fleet-operations brief S5, 1280):
 * a ⋯ menu on each agent row, Assign… on a ready task, Ask supervisor to
 * merge on an awaiting-merge task, Add worker… and Focus epic… in the
 * section header, the inline destructive confirmation, the Undo offer and
 * one live region for every result. State lives in FleetOpsState; this only
 * draws it and reports what the operator chose.
 */

import {
  agentMenu,
  assignAction,
  awaitingMerge,
  focusEpicAction,
  gatedItems,
  idleWorkers,
  mergeRequestMessage,
  readyTask,
  requestMergeAction,
  spawnAction,
  type FleetAction,
  type FleetAgent,
  type FleetOpsState,
  type FleetTask,
} from "./fleet-ops";
import { fleetControlGate, type FleetControlGate } from "./fleet-permissions";
import type { Scope } from "./types";

export interface FleetOpsViewContext {
  readonly state: FleetOpsState;
  readonly scopes: readonly Scope[];
  readonly origin: string;
  readonly now: number;
  readonly agents: readonly FleetAgent[];
  readonly tasks: readonly FleetTask[];
  readonly epics: readonly string[];
  readonly currentEpic: string | null;
  /** When each awaiting-merge task was last asked about, by task id. */
  readonly asked: ReadonlyMap<string, number>;
  readonly relative: (at: number) => string;
  readonly on: FleetOpsHandlers;
}

export interface FleetOpsHandlers {
  toggleMenu(rowKey: string): void;
  choose(rowKey: string, action: FleetAction): void;
  confirm(): void;
  cancelConfirm(): void;
  openPreview(rowKey: string, task: FleetTask): void;
  sendMerge(rowKey: string, task: FleetTask): void;
  closePanels(): void;
  toggleAssign(rowKey: string): void;
  toggleHeader(panel: "add" | "focus"): void;
  undo(): void;
}

export const agentRowKey = (agent: FleetAgent): string => `agent:${agent.name}`;
export const taskRowKey = (task: FleetTask): string => `task:${task.id}`;

function button(document: Document, label: string, className: string, focusKey: string, onclick: (event: MouseEvent) => void): HTMLButtonElement {
  const node = document.createElement("button");
  node.type = "button";
  node.className = className;
  node.textContent = label;
  node.dataset.fleetFocus = focusKey;
  node.onclick = onclick;
  return node;
}

/** A control the pairing cannot use: still focusable, saying why, never acting. */
function gateDisabled(node: HTMLButtonElement, gate: FleetControlGate, reasonId: string): void {
  if (gate.allowed) return;
  node.setAttribute("aria-disabled", "true");
  node.setAttribute("aria-describedby", reasonId);
  node.title = gate.reason;
  node.onclick = (event) => event.preventDefault();
}

function reasonLine(document: Document, gate: FleetControlGate, id: string): HTMLElement | undefined {
  if (gate.allowed) return undefined;
  const line = document.createElement("small");
  line.className = "fleet-ops-reason";
  line.id = id;
  line.textContent = `${gate.state}. ${gate.reason}${gate.grantable ? " Allow managing workers in Paired machines." : ` Run: ${gate.command}`}`;
  return line;
}

const safeId = (value: string): string => value.replace(/[^a-z0-9_-]/gi, "_");

function noteLine(document: Document, context: FleetOpsViewContext, rowKey: string): HTMLElement | undefined {
  const pending = context.state.pending.get(rowKey);
  if (pending) {
    const line = document.createElement("p");
    line.className = "fleet-ops-progress";
    line.setAttribute("role", "status");
    line.tabIndex = -1;
    line.dataset.fleetFocus = `${rowKey}:progress`;
    line.textContent = pending.progress;
    return line;
  }
  const note = context.state.notes.get(rowKey);
  if (!note) return undefined;
  const line = document.createElement("p");
  line.className = `fleet-ops-note fleet-ops-note--${note.tone}`;
  line.tabIndex = -1;
  line.dataset.fleetFocus = `${rowKey}:note`;
  line.textContent = note.text;
  return line;
}

/** The inline destructive confirmation (End session's pattern): the question, Cancel first, then the danger button. */
function confirmation(document: Document, context: FleetOpsViewContext, rowKey: string, action: FleetAction): HTMLElement {
  const box = document.createElement("div");
  box.className = "fleet-ops-confirm";
  box.setAttribute("role", "group");
  box.setAttribute("aria-label", action.question ?? action.label);
  const question = document.createElement("p");
  question.className = "fleet-ops-question";
  question.textContent = action.question ?? action.label;
  const cancel = button(document, "Cancel", "fleet-ops-cancel", `${rowKey}:cancel`, (event) => { if (event.detail > 1) return; context.on.cancelConfirm(); });
  // A double-click's second press lands here; only a deliberate press confirms.
  const confirm = button(document, action.label.replace(/…$/, ""), "fleet-ops-confirm-action danger", `${rowKey}:confirm`, (event) => { if (event.detail > 1) return; context.on.confirm(); });
  const actions = document.createElement("div");
  actions.className = "fleet-ops-confirm-actions";
  actions.append(cancel, confirm);
  box.append(question, actions);
  return box;
}

function menu(document: Document, context: FleetOpsViewContext, rowKey: string, actions: readonly FleetAction[], label: string): HTMLElement {
  const list = document.createElement("div");
  list.className = "fleet-ops-menu";
  list.setAttribute("role", "menu");
  list.setAttribute("aria-label", label);
  const items = gatedItems(actions, context.scopes, context.origin);
  // Destructive items come last, after a separator.
  let separated = false;
  for (const [index, { action, gate }] of items.entries()) {
    if (action.destructive && !separated && index > 0) {
      const rule = document.createElement("div"); rule.setAttribute("role", "separator"); rule.className = "fleet-ops-separator"; list.append(rule); separated = true;
    }
    const item = button(document, action.label, `fleet-ops-item${action.destructive ? " danger" : ""}`, `${rowKey}:item:${action.id}`, () => context.on.choose(rowKey, action));
    item.setAttribute("role", "menuitem");
    const reasonId = `fleet-reason-${safeId(rowKey)}-${safeId(action.id)}`;
    gateDisabled(item, gate, reasonId);
    list.append(item);
    const reason = reasonLine(document, gate, reasonId);
    if (reason) list.append(reason);
  }
  list.onkeydown = (event) => {
    const entries = [...list.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
    const at = entries.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const next = entries[(at + (event.key === "ArrowDown" ? 1 : entries.length - 1)) % entries.length];
      next?.focus();
    }
  };
  return list;
}

/** Trailing controls for one agent row. */
export function agentControls(document: Document, context: FleetOpsViewContext, agent: FleetAgent): HTMLElement {
  const rowKey = agentRowKey(agent);
  const wrap = document.createElement("div");
  wrap.className = "fleet-ops";
  const open = context.state.menuFor === rowKey;
  const trigger = button(document, "⋯", "fleet-ops-trigger", `${rowKey}:trigger`, () => context.on.toggleMenu(rowKey));
  trigger.setAttribute("aria-haspopup", "menu");
  trigger.setAttribute("aria-expanded", String(open));
  trigger.setAttribute("aria-label", `Actions for ${agent.name}`);
  wrap.append(trigger);
  if (open) wrap.append(menu(document, context, rowKey, agentMenu(agent), `Actions for ${agent.name}`));
  if (context.state.confirm?.rowKey === rowKey) wrap.append(confirmation(document, context, rowKey, context.state.confirm.action));
  const note = noteLine(document, context, rowKey);
  if (note) wrap.append(note);
  return wrap;
}

/** Trailing controls for one task row: Assign… on a ready task, Ask supervisor to merge on an awaiting-merge one. */
export function taskControls(document: Document, context: FleetOpsViewContext, task: FleetTask): HTMLElement | undefined {
  const rowKey = taskRowKey(task);
  const wrap = document.createElement("div");
  wrap.className = "fleet-ops";
  if (awaitingMerge(task)) {
    const gate = fleetControlGate(context.scopes, "ask-merge", context.origin);
    const asked = context.asked.get(task.id);
    if (asked !== undefined && context.state.preview?.rowKey !== rowKey) {
      const line = document.createElement("p");
      line.className = "fleet-ops-asked";
      line.textContent = `Asked ${context.relative(asked)}`;
      wrap.append(line);
    }
    const ask = button(document, asked !== undefined ? "Ask again" : "Ask supervisor to merge", "fleet-ops-ask", `${rowKey}:ask`, () => context.on.openPreview(rowKey, task));
    const reasonId = `fleet-reason-${safeId(rowKey)}-ask`;
    gateDisabled(ask, gate, reasonId);
    wrap.append(ask);
    const reason = reasonLine(document, gate, reasonId);
    if (reason) wrap.append(reason);
    if (context.state.preview?.rowKey === rowKey) {
      // The exact message, before it is sent.
      const preview = document.createElement("div");
      preview.className = "fleet-ops-preview";
      preview.setAttribute("role", "group");
      preview.setAttribute("aria-label", "Message to the supervisor");
      const lead = document.createElement("p"); lead.className = "fleet-ops-preview-lead"; lead.textContent = "This message goes to the supervisor:";
      const quote = document.createElement("blockquote"); quote.className = "fleet-ops-preview-text"; quote.textContent = mergeRequestMessage(task);
      const actions = document.createElement("div"); actions.className = "fleet-ops-confirm-actions";
      actions.append(
        button(document, "Cancel", "fleet-ops-cancel", `${rowKey}:preview-cancel`, () => context.on.closePanels()),
        button(document, "Send", "fleet-ops-send primary", `${rowKey}:send`, (event) => { if (event.detail > 1) return; context.on.sendMerge(rowKey, task); }),
      );
      preview.append(lead, quote, actions);
      wrap.append(preview);
    }
  } else if (readyTask(task)) {
    const gate = fleetControlGate(context.scopes, "assign-task", context.origin);
    const open = context.state.assignFor === rowKey;
    const assign = button(document, "Assign…", "fleet-ops-assign", `${rowKey}:assign`, () => context.on.toggleAssign(rowKey));
    assign.setAttribute("aria-haspopup", "menu");
    assign.setAttribute("aria-expanded", String(open));
    assign.setAttribute("aria-label", `Assign ${task.id}`);
    const reasonId = `fleet-reason-${safeId(rowKey)}-assign`;
    gateDisabled(assign, gate, reasonId);
    wrap.append(assign);
    const reason = reasonLine(document, gate, reasonId);
    if (reason) wrap.append(reason);
    if (open && gate.allowed) {
      const idle = idleWorkers(context.agents);
      const actions = idle.map((agent) => assignAction(task, agent.name));
      if (actions.length) wrap.append(menu(document, context, rowKey, actions, `Assign ${task.id} to`));
      else {
        const none = document.createElement("p"); none.className = "fleet-ops-note"; none.textContent = "No idle worker. Add worker… starts one on this task.";
        wrap.append(none);
      }
    }
  } else {
    return undefined;
  }
  const note = noteLine(document, context, rowKey);
  if (note) wrap.append(note);
  return wrap;
}

/** The section header's Add worker… and Focus epic…, with their small panels. */
export function headerControls(document: Document, context: FleetOpsViewContext, panel: "add" | "focus" | undefined): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "fleet-ops-header";
  const add = button(document, "Add worker…", "fleet-ops-add", "header:add", () => context.on.toggleHeader("add"));
  add.setAttribute("aria-expanded", String(panel === "add"));
  const addGate = fleetControlGate(context.scopes, "add-workers", context.origin);
  gateDisabled(add, addGate, "fleet-reason-header-add");
  const focus = button(document, "Focus epic…", "fleet-ops-focus", "header:focus", () => context.on.toggleHeader("focus"));
  focus.setAttribute("aria-expanded", String(panel === "focus"));
  const focusGate = fleetControlGate(context.scopes, "focus-epic", context.origin);
  gateDisabled(focus, focusGate, "fleet-reason-header-focus");
  wrap.append(add, focus);
  for (const [gate, id] of [[addGate, "fleet-reason-header-add"], [focusGate, "fleet-reason-header-focus"]] as const) {
    const reason = reasonLine(document, gate, id);
    if (reason) { wrap.append(reason); break; }
  }
  if (panel === "add" && addGate.allowed) {
    const form = document.createElement("div");
    form.className = "fleet-ops-panel";
    form.setAttribute("role", "group");
    form.setAttribute("aria-label", "Add worker");
    const count = document.createElement("select"); count.className = "fleet-ops-count"; count.setAttribute("aria-label", "How many workers");
    for (let n = 1; n <= 4; n += 1) count.append(new Option(String(n), String(n)));
    const start = document.createElement("select"); start.className = "fleet-ops-start-on"; start.setAttribute("aria-label", "Start on");
    start.append(new Option("No task yet", ""));
    for (const task of context.tasks.filter(readyTask)) start.append(new Option(`${task.id}${task.title ? ` · ${task.title}` : ""}`, task.id));
    const go = button(document, "Add", "fleet-ops-add-go primary", "header:add-go", () => context.on.choose("header", spawnAction(Number(count.value), start.value || undefined)));
    form.append(count, start, go);
    wrap.append(form);
  }
  if (panel === "focus" && focusGate.allowed) {
    const actions = context.epics.filter((epic) => epic !== context.currentEpic).map((epic) => focusEpicAction(epic, context.currentEpic));
    if (actions.length) wrap.append(menu(document, context, "header", actions, "Focus epic"));
    else { const none = document.createElement("p"); none.className = "fleet-ops-note"; none.textContent = "No other epic to focus."; wrap.append(none); }
  }
  const note = noteLine(document, context, "header");
  if (note) wrap.append(note);
  return wrap;
}

/** The Undo offer for the last reversible action, while it lasts. */
export function undoBar(document: Document, context: FleetOpsViewContext): HTMLElement | undefined {
  const offer = context.state.currentUndo(context.now);
  if (!offer) return undefined;
  const bar = document.createElement("div");
  bar.className = "fleet-ops-undo";
  const text = document.createElement("span"); text.textContent = offer.label;
  bar.append(text, button(document, "Undo", "fleet-ops-undo-action", "undo", () => context.on.undo()));
  return bar;
}
