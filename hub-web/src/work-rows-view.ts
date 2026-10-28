/**
 * Work rows (cas-ff22): how a stretch of the supervisor's work paints in the
 * thread, and how a worker's timeline lists its rows. Brief:
 * `hub-web/whole-picture.brief.md`.
 *
 * Every string here that came from an agent (labels, thinking, tool input
 * and output) is set as `textContent`. Nothing is parsed as markup.
 */
import { clockLabel, type ThreadWork } from "./thread-model";
import { rowText, taskChip, workSummary, type DetailState, type WorkEvent } from "./work-events";

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
    item.events.map((event) => [
      event.id, event.label, event.error,
      open && context.isOpen(`row:${event.id}`) ? context.detail(event.id) ?? null : null,
    ]),
  ]);
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
  row.classList.toggle("is-failed", event.error);
  row.dataset.eventId = event.id;
  const key = `row:${event.id}`;
  const open = event.hasDetail && context.isOpen(key);
  const text = document.createElement("span"); text.className = "work-row-text"; text.textContent = rowText(event);
  const parts: HTMLElement[] = [text];
  if (event.error) {
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
    const detail = document.createElement("pre");
    detail.className = "work-detail";
    detail.id = `work-detail-${domId(event.id)}`;
    toggle.setAttribute("aria-controls", detail.id);
    detail.tabIndex = 0;
    const state = context.detail(event.id);
    if (!state || state.state === "loading") { detail.textContent = "Loading…"; detail.setAttribute("aria-busy", "true"); }
    else if (state.state === "ready") detail.textContent = state.truncated ? `${state.text}\n…cut at 16 KB` : state.text;
    else if (state.state === "gone") detail.textContent = "This detail is no longer on the machine.";
    else detail.textContent = `Could not load: ${state.reason}`;
    row.append(detail);
  }
  return row;
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
    const rows = document.createElement("ol");
    rows.className = "work-rows";
    rows.id = `work-rows-${domId(item.key)}`;
    summary.setAttribute("aria-controls", rows.id);
    for (const event of item.events) rows.append(workRow(document, event, context, repaint));
    children.push(rows);
  }
  node.replaceChildren(...children);
  if (focused) refocus(node, focused);
}
