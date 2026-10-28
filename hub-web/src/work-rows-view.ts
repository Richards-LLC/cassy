/**
 * Work rows (cas-ff22): how a stretch of the supervisor's work paints in the
 * thread, and how a worker's timeline lists its rows. Brief:
 * `hub-web/whole-picture.brief.md`.
 *
 * Every string here that came from an agent (labels, thinking, tool input
 * and output) is set as `textContent`. Nothing is parsed as markup.
 */
import { clockLabel, type ThreadWork } from "./thread-model";
import { rowText, taskChip, turnNotice, workFailed, workSummary, type DetailState, type WorkEvent } from "./work-events";

export interface WorkRowsContext {
  /** The loaded detail of one event, if it was asked for. */
  detail(eventId: string): DetailState | undefined;
  /** Ask the daemon for one event's detail. */
  requestDetail(eventId: string): void;
  /** Disclosure state, kept by the owner so a repaint keeps it. */
  isOpen(key: string): boolean;
  setOpen(key: string, open: boolean): void;
}

const CHEVRON = '<svg class="work-chevron" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M6 4l4 4-4 4"/></svg>';

function domId(key: string): string {
  return key.replace(/[^\w-]/g, "-");
}

/** What a work block paints, so an unchanged block keeps its node untouched. */
export function workSignature(item: ThreadWork, context: WorkRowsContext): string {
  const open = context.isOpen(item.key);
  return JSON.stringify([
    open,
    context.isOpen(`turns:${item.key}`),
    item.events.map((event) => [
      event.id, event.label, event.error, event.hasDetail, event.tool, event.lifecycle,
      open && context.isOpen(`row:${event.id}`) ? context.detail(event.id) ?? null : null,
    ]),
  ]);
}

/** Typed scope comes from protocol facts; the original detail is never reparsed. */
function readyDetail(document: Document, detail: HTMLElement, event: WorkEvent, state: Extract<DetailState, { state: "ready" }>): void {
  const tool = event.kind === "tool" ? event.tool : undefined;
  const operation = tool && ["read", "search", "command", "edit"].includes(tool.operation) ? tool.operation : undefined;
  const label = (text: string): void => {
    const name = document.createElement("p"); name.className = "work-detail-label"; name.textContent = text;
    detail.append(name);
  };
  if (tool && operation) {
    detail.dataset.operation = operation;
    if (operation === "command") {
      if (tool.command !== undefined) {
        label("Command");
        const command = document.createElement("pre"); command.className = "work-command"; command.textContent = tool.command;
        detail.append(command);
      }
      if (tool.cwd !== undefined) {
        label("Working directory");
        const cwd = document.createElement("p"); cwd.className = "work-path"; cwd.textContent = tool.cwd;
        detail.append(cwd);
      }
      if (tool.exit_code !== undefined) label(`Exit ${tool.exit_code}`);
    } else {
      label(operation === "read" ? "Read targets" : operation === "edit" ? "Edit targets" : "Search scope");
      if (tool.targets.length > 0) {
        const targets = document.createElement("ul"); targets.className = "work-targets";
        for (const path of tool.targets) { const file = document.createElement("li"); file.textContent = path; targets.append(file); }
        detail.append(targets);
      } else {
        const unknown = document.createElement("p"); unknown.textContent = "No scope reported."; detail.append(unknown);
      }
      if (operation === "search" && tool.counts?.matches !== undefined) label(`${tool.counts.matches} match${tool.counts.matches === 1 ? "" : "es"}`);
    }
    label("Tool input and output");
  }
  const record = document.createElement("pre"); record.className = "work-record"; record.textContent = state.text;
  detail.append(record);
  if (state.truncated) { const cut = document.createElement("p"); cut.className = "work-detail-limit"; cut.textContent = "…cut at 16 KB"; detail.append(cut); }
}

/** The chips a block shows while collapsed: its task transitions. */
function chips(document: Document, events: readonly WorkEvent[]): HTMLElement | undefined {
  const found = events.map(taskChip).filter((chip) => chip !== undefined);
  if (found.length === 0) return undefined;
  const list = document.createElement("ul");
  list.className = "work-chips";
  list.setAttribute("aria-label", "Task events");
  for (const chip of found) {
    const item = document.createElement("li");
    item.className = `work-chip work-chip--${chip.tone}`;
    item.title = chip.label;
    const dot = document.createElement("span"); dot.className = "work-chip-dot"; dot.setAttribute("aria-hidden", "true");
    const subject = document.createElement("span"); subject.className = "work-chip-id"; subject.textContent = chip.subject;
    item.append(dot, subject, document.createTextNode(` ${chip.verb}`));
    list.append(item);
  }
  return list;
}

/** One work row; a row with detail is a disclosure whose text loads on first open. */
export function workRow(document: Document, event: WorkEvent, context: WorkRowsContext, repaint: (focusKey: string) => void, options: { time?: boolean } = {}): HTMLElement {
  const row = document.createElement("li");
  row.className = `work-row work-row--${event.kind}`;
  row.classList.toggle("is-failed", workFailed(event));
  row.dataset.eventId = event.id;
  const key = `row:${event.id}`;
  const open = event.hasDetail && context.isOpen(key);
  const text = document.createElement("span"); text.className = "work-row-text"; text.textContent = rowText(event);
  const parts: HTMLElement[] = [text];
  if (workFailed(event)) {
    const failed = document.createElement("span"); failed.className = "work-row-failed"; failed.textContent = "failed";
    parts.push(failed);
  }
  if (options.time) {
    const time = document.createElement("time"); time.className = "work-row-time"; time.dateTime = new Date(event.at).toISOString();
    time.textContent = clockLabel(event.at) ?? "";
    parts.push(time);
  }
  if (!event.hasDetail) {
    const line = document.createElement("span"); line.className = "work-row-line"; line.append(...parts);
    row.append(line);
    return row;
  }
  const toggle = document.createElement("button");
  toggle.type = "button"; toggle.className = "work-row-toggle"; toggle.dataset.focusKey = key;
  toggle.setAttribute("aria-expanded", String(open));
  toggle.insertAdjacentHTML("afterbegin", CHEVRON);
  toggle.append(...parts);
  toggle.onclick = () => {
    const opening = !context.isOpen(key);
    context.setOpen(key, opening);
    if (opening && context.detail(event.id) === undefined) context.requestDetail(event.id);
    repaint(key);
  };
  row.append(toggle);
  if (open) {
    const detail = document.createElement("div");
    detail.className = "work-detail";
    detail.id = `work-detail-${domId(event.id)}`;
    toggle.setAttribute("aria-controls", detail.id);
    detail.tabIndex = 0;
    detail.dataset.focusKey = `detail:${event.id}`;
    const state = context.detail(event.id);
    if (!state || state.state === "loading") { detail.textContent = "Loading…"; detail.setAttribute("aria-busy", "true"); }
    else if (state.state === "ready") readyDetail(document, detail, event, state);
    else if (state.state === "gone") detail.textContent = "This detail is no longer on the machine.";
    else detail.textContent = `Could not load: ${state.reason}`;
    row.append(detail);
  }
  return row;
}

/** Both surfaces fold routine turn bookends into one optional disclosure. */
export function workRows(document: Document, events: readonly WorkEvent[], context: WorkRowsContext, repaint: (focusKey: string) => void, options: { time?: boolean; noticesKey: string }): HTMLOListElement {
  const rows = document.createElement("ol"); rows.className = "work-rows";
  const notices = events.filter(turnNotice);
  let placed = false;
  for (const event of events) {
    if (!turnNotice(event)) { rows.append(workRow(document, event, context, repaint, options)); continue; }
    if (placed) continue;
    placed = true;
    const group = document.createElement("li"); group.className = "work-turn-notices";
    const toggle = document.createElement("button"); toggle.type = "button"; toggle.className = "work-row-toggle";
    toggle.dataset.focusKey = options.noticesKey;
    const open = context.isOpen(options.noticesKey);
    toggle.setAttribute("aria-expanded", String(open));
    toggle.insertAdjacentHTML("afterbegin", CHEVRON);
    toggle.append(`Turn activity · ${notices.length} event${notices.length === 1 ? "" : "s"}`);
    toggle.onclick = () => { context.setOpen(options.noticesKey, !context.isOpen(options.noticesKey)); repaint(options.noticesKey); };
    group.append(toggle);
    if (open) {
      const list = document.createElement("ol"); list.className = "work-rows"; list.id = `turn-notices-${domId(options.noticesKey)}`;
      toggle.setAttribute("aria-controls", list.id);
      for (const notice of notices) list.append(workRow(document, notice, context, repaint, options));
      group.append(list);
    }
    rows.append(group);
  }
  return rows;
}

/** Hand focus back to the control a repaint replaced. */
export function refocus(container: HTMLElement, focusKey: string): void {
  [...container.querySelectorAll<HTMLElement>("[data-focus-key]")].find((node) => node.dataset.focusKey === focusKey)?.focus({ preventScroll: true });
}

/**
 * Paint one work block into its keyed thread node: a quiet summary
 * disclosure, the block's task chips, and (open) one row per event.
 */
export function renderWork(node: HTMLElement, item: ThreadWork, context: WorkRowsContext, speaker: string, onRepaint?: (topBefore: number) => void): void {
  const document = node.ownerDocument;
  // A repaint (a detail landing, an event appended) keeps the reader's focus.
  const focused = document.activeElement instanceof HTMLElement && node.contains(document.activeElement) ? document.activeElement.dataset.focusKey : undefined;
  const open = context.isOpen(item.key);
  const last = item.events.at(-1);
  const time = clockLabel(last?.at);
  node.className = "turn work-turn";
  node.setAttribute("role", "group");
  node.setAttribute("aria-label", `${speaker}, work${time ? `, ${time}` : ""}`);
  node.removeAttribute("aria-description");
  const repaint = (focusKey: string): void => {
    const top = node.getBoundingClientRect().top;
    renderWork(node, item, context, speaker, onRepaint);
    node.dataset.signature = workSignature(item, context);
    refocus(node, focusKey);
    onRepaint?.(top);
  };
  const summary = document.createElement("button");
  summary.type = "button"; summary.className = "work-summary"; summary.dataset.focusKey = item.key;
  summary.setAttribute("aria-expanded", String(open));
  summary.insertAdjacentHTML("afterbegin", CHEVRON);
  const words = document.createElement("span"); words.textContent = workSummary(item.events);
  summary.append(words);
  if (time) { const at = document.createElement("time"); at.className = "work-time"; at.textContent = time; summary.append(at); }
  summary.onclick = () => { context.setOpen(item.key, !context.isOpen(item.key)); repaint(item.key); };
  const children: HTMLElement[] = [summary];
  const chipList = chips(document, item.events);
  if (chipList) children.push(chipList);
  if (open) {
    const rows = workRows(document, item.events, context, repaint, { noticesKey: `turns:${item.key}` });
    rows.id = `work-rows-${domId(item.key)}`;
    summary.setAttribute("aria-controls", rows.id);
    children.push(rows);
  }
  node.replaceChildren(...children);
  if (focused) refocus(node, focused);
}
