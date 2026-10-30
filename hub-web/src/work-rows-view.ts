/**
 * Work rows (cas-ff22): how a stretch of the supervisor's work paints in the
 * thread, and how a worker's timeline lists its rows. Brief:
 * `hub-web/whole-picture.brief.md`.
 *
 * Explicitly published v2 reasoning uses the safe Markdown dialect. Legacy
 * thinking, labels and tool records remain literal text.
 */
import { clockLabel, workTurnKey, type ThreadWork } from "./thread-model";
import { reasoningLabel, rowText, taskChip, turnNotice, workFailed, workSummary, type DetailState, type WorkEvent } from "./work-events";
import { plainTextMarkdown, renderMarkdown } from "./markdown-renderer";
import { unifiedDiff } from "./rich-diff";
import { createCodeFrame, setCodeFramePreview } from "./code-frame";
import "./rich-content.css";

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
    item.changedFiles?.map(event => [event.changed_files, context.isOpen(`changes:${workTurnKey(event)}`),
      event.changed_files!.files.map(file => [context.isOpen(`change:${workTurnKey(event)}:${file.path}`), context.detail(file.source_event_id)])]),
    item.events.map((event) => [
      event.id, event.kind, event.label, event.error, event.hasDetail, event.tool, event.lifecycle, event.reasoning,
      context.isOpen(`row:${event.id}`),
      open && context.isOpen(`row:${event.id}`) ? context.detail(event.id) ?? null : null,
    ]),
  ]);
}

/** Only the contract's published sources opt into formatted reasoning. */
function publishedReasoning(event: WorkEvent): string | undefined {
  return event.kind === "thinking" && event.reasoning
    && ["codex_summary", "claude_thinking"].includes(event.reasoning.source)
    && event.reasoning.summary.trim() ? event.reasoning.summary : undefined;
}

function reasoningDetail(document: Document, detail: HTMLElement, event: WorkEvent, text: string, context: WorkRowsContext): void {
  const content = document.createElement("div"); content.className = "work-reasoning-content";
  content.append(...renderMarkdown(document, text));
  const raw = document.createElement("details"); raw.className = "work-reasoning-raw";
  const key = `raw:${event.id}`;
  raw.open = context.isOpen(key);
  raw.ontoggle = () => { if (raw.isConnected) context.setOpen(key, raw.open); };
  const toggle = document.createElement("summary"); toggle.textContent = "Raw summary"; toggle.dataset.focusKey = key;
  const record = document.createElement("pre"); record.className = "work-record"; record.textContent = text;
  raw.append(toggle, record);
  detail.append(content, raw);
}

/** Only a recorded unified section naming this file is a file diff. */
function fileDiff(text: string, path: string): string | undefined {
  const headers = [...text.matchAll(/^--- (.+)\r?\n\+\+\+ (.+)\r?\n(?=@@ -\d)/gm)];
  const names = (header: string): string[] => {
    const name = header.replace(/\r$/, "").split("\t")[0]!;
    return [name, name.replace(/^[ab]\//, "")];
  };
  for (let index = 0; index < headers.length; index++) {
    const match = headers[index]!;
    if (![...names(match[1]!), ...names(match[2]!)].includes(path)) continue;
    return text.slice(match.index, headers[index + 1]?.index).split("\ndiff --git ")[0];
  }
  return undefined;
}

/** The provenance is deliberately visible: these facts never claim Git state. */
function changedFiles(document: Document, event: WorkEvent, context: WorkRowsContext, repaint: (key: string) => void): HTMLElement {
  const facts = event.changed_files!;
  const key = `changes:${workTurnKey(event)}`;
  const card = document.createElement("section"); card.className = "changed-files";
  card.setAttribute("aria-label", "Tool-reported edits");
  const toggle = document.createElement("button"); toggle.type = "button"; toggle.className = "changed-files-toggle work-row-toggle";
  toggle.dataset.focusKey = key; toggle.setAttribute("aria-expanded", String(context.isOpen(key)));
  toggle.insertAdjacentHTML("afterbegin", CHEVRON);
  toggle.append(`Tool-reported edits · ${facts.files.length} file${facts.files.length === 1 ? "" : "s"}`);
  toggle.onclick = () => { context.setOpen(key, !context.isOpen(key)); repaint(key); };
  card.append(toggle);
  if (!context.isOpen(key)) return card;
  const note = document.createElement("p"); note.className = "changed-files-note";
  note.textContent = facts.provenance === "successful_tool_calls" ? "Successful edit calls. Other changes may be unreported." : `Reported by ${facts.provenance}.`;
  const list = document.createElement("ul"); list.className = "changed-files-list";
  list.id = `changed-files-${domId(event.id)}`; toggle.setAttribute("aria-controls", list.id);
  for (const file of facts.files) {
    const item = document.createElement("li"); item.className = "changed-file";
    const fileKey = `change:${workTurnKey(event)}:${file.path}`;
    const open = context.isOpen(fileKey);
    const button = document.createElement("button"); button.type = "button"; button.className = "changed-file-toggle work-row-toggle";
    button.dataset.focusKey = fileKey; button.setAttribute("aria-expanded", String(open));
    button.insertAdjacentHTML("afterbegin", CHEVRON);
    const path = document.createElement("span"); path.className = "changed-file-path"; path.textContent = file.path;
    const operation = document.createElement("span"); operation.className = "changed-file-operation"; operation.textContent = file.operation;
    button.append(path, operation);
    button.onclick = () => { context.setOpen(fileKey, !context.isOpen(fileKey)); repaint(fileKey); };
    item.append(button);
    if (open) {
      const detail = document.createElement("div"); detail.className = "work-detail changed-file-detail";
      detail.id = `changed-file-${domId(event.id)}-${facts.files.indexOf(file)}`; button.setAttribute("aria-controls", detail.id);
      detail.tabIndex = 0; detail.dataset.focusKey = `detail:${fileKey}`;
      const state = context.detail(file.source_event_id);
      if (!state) context.requestDetail(file.source_event_id);
      if (!state || state.state === "loading") { detail.textContent = "Loading source edit…"; detail.setAttribute("aria-busy", "true"); }
      else if (state.state === "gone") detail.textContent = "This source edit is no longer on the machine.";
      else if (state.state === "failed") {
        const message = document.createElement("p"); message.textContent = `Could not load source edit: ${state.reason}`;
        const retry = document.createElement("button"); retry.type = "button"; retry.textContent = "Retry source edit";
        retry.dataset.focusKey = `retry:${fileKey}`;
        retry.onclick = () => { context.requestDetail(file.source_event_id); repaint(retry.dataset.focusKey!); };
        detail.append(message, retry);
      } else {
        const patch = fileDiff(state.text, file.path);
        const diff = patch ? unifiedDiff(document, patch) : undefined;
        if (diff) {
          const disclosure = diff.querySelector("details");
          if (disclosure) disclosure.open = true;
          const { frame, pre } = createCodeFrame(document, { source: patch!, language: "diff", filename: file.path });
          setCodeFramePreview(pre, diff);
          detail.append(frame);
        }
        else {
          const label = document.createElement("p"); label.className = "work-detail-label"; label.textContent = "Source edit · no recorded unified diff";
          const source = document.createElement("pre"); source.className = "work-record"; source.textContent = state.text;
          detail.append(label, source);
        }
        if (state.truncated) { const cut = document.createElement("p"); cut.className = "work-detail-limit"; cut.textContent = "…source detail cut at 16 KB"; detail.append(cut); }
      }
      item.append(detail);
    }
    list.append(item);
  }
  card.append(note, list);
  return card;
}

/** Typed scope comes from protocol facts; the original detail is never reparsed. */
function readyDetail(document: Document, detail: HTMLElement, event: WorkEvent, state: Extract<DetailState, { state: "ready" }>, context: WorkRowsContext): void {
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
  if (publishedReasoning(event) !== undefined) reasoningDetail(document, detail, event, state.text, context);
  else {
    const record = document.createElement("pre"); record.className = "work-record"; record.textContent = state.text;
    detail.append(record);
  }
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
  row.classList.toggle("is-live", event.kind === "tool" && !workFailed(event) && event.tool?.exit_code === undefined && !event.lifecycle?.outcome && !event.lifecycle?.ended_at
    && (event.lifecycle?.stage === "start" || event.lifecycle?.stage === "phase"));
  const key = `row:${event.id}`;
  const published = publishedReasoning(event);
  const expandable = event.hasDetail || published !== undefined;
  const open = expandable && context.isOpen(key);
  const text = document.createElement("span"); text.className = "work-row-text"; text.textContent = rowText(event);
  if (published !== undefined) {
    text.classList.add("work-reasoning-heading");
    const label = document.createElement("span"); label.className = "work-reasoning-label";
    label.textContent = reasoningLabel(event);
    const preview = document.createElement("span"); preview.className = "work-reasoning-preview";
    preview.textContent = plainTextMarkdown(published);
    text.replaceChildren(label, preview);
  }
  const gutter = document.createElement("span"); gutter.className = "work-row-gutter"; gutter.setAttribute("aria-hidden", "true");
  if (expandable) gutter.insertAdjacentHTML("afterbegin", CHEVRON);
  const parts: HTMLElement[] = [gutter, text];
  if (workFailed(event)) {
    const failed = document.createElement("span"); failed.className = "work-row-failed"; failed.textContent = "failed";
    parts.push(failed);
  }
  if (options.time) {
    const time = document.createElement("time"); time.className = "work-row-time"; time.dateTime = new Date(event.at).toISOString();
    time.textContent = clockLabel(event.at) ?? "";
    parts.push(time);
  }
  if (!expandable) {
    const line = document.createElement("span"); line.className = "work-row-line"; line.append(...parts);
    row.append(line);
    return row;
  }
  const toggle = document.createElement("button");
  toggle.type = "button"; toggle.className = "work-row-toggle"; toggle.dataset.focusKey = key;
  toggle.setAttribute("aria-expanded", String(open));
  toggle.append(...parts);
  toggle.onclick = () => {
    const opening = !context.isOpen(key);
    context.setOpen(key, opening);
    if (opening && event.hasDetail && context.detail(event.id) === undefined) context.requestDetail(event.id);
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
    const state = event.hasDetail ? context.detail(event.id) : { state: "ready" as const, text: published!, truncated: false };
    if (!state || state.state === "loading") { detail.textContent = "Loading…"; detail.setAttribute("aria-busy", "true"); }
    else if (state.state === "ready") readyDetail(document, detail, event, state, context);
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
  const scroll = new Map([...node.querySelectorAll<HTMLElement>(".work-detail[data-focus-key]")].map(detail => [detail.dataset.focusKey, detail.scrollTop]));
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
  for (const event of item.changedFiles ?? []) children.push(changedFiles(document, event, context, repaint));
  if (open) {
    const rows = workRows(document, item.events, context, repaint, { noticesKey: `turns:${item.key}` });
    rows.id = `work-rows-${domId(item.key)}`;
    summary.setAttribute("aria-controls", rows.id);
    children.push(rows);
  }
  node.replaceChildren(...children);
  for (const detail of node.querySelectorAll<HTMLElement>(".work-detail[data-focus-key]")) detail.scrollTop = scroll.get(detail.dataset.focusKey) ?? 0;
  if (focused) refocus(node, focused);
}
