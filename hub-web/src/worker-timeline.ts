/**
 * Worker timeline (cas-ff22): one worker's task, state and recent work rows,
 * live, in a sheet opened from its row in Tasks & progress. It replaces
 * watching the worker's terminal. A right sheet on desktop and a bottom
 * sheet on a phone, the same frame as Raw output. Agent text is set only as
 * `textContent`, except published reasoning rendered through safe Markdown.
 */
import type { WorkEventStream } from "./work-events";
import { statusClass, statusLabel } from "./progress-model";
import { refocus, workRows, type WorkRowsContext } from "./work-rows-view";

export interface WorkerTimelineSubject {
  readonly worker: string;
  readonly status?: string;
  /** The same lifecycle/liveness projection painted on the worker card. */
  readonly stateLabel?: string;
  readonly task?: string;
  readonly taskTitle?: string;
  /** Why the timeline cannot be read, when it cannot. */
  readonly unavailable?: string;
}

export interface WorkerTimelineHandlers {
  readOlder(): void;
  closed(): void;
  /** The terminal fallback. */
  rawOutput?(): void;
}

/** The one dialog; it lives on document.body so a shell rebuild never closes it. */
export function workerTimelineDialog(document: Document, handlers: WorkerTimelineHandlers): HTMLDialogElement {
  let dialog = document.querySelector<HTMLDialogElement>("#worker-timeline");
  if (!dialog) {
    dialog = document.createElement("dialog");
    dialog.id = "worker-timeline";
    dialog.className = "raw-output-drawer worker-timeline";
    dialog.setAttribute("aria-labelledby", "worker-timeline-title");
    dialog.setAttribute("aria-describedby", "worker-timeline-subject");
    const header = document.createElement("header"); header.className = "raw-output-header";
    const heading = document.createElement("div"); heading.className = "raw-output-heading";
    const title = document.createElement("h2"); title.id = "worker-timeline-title";
    const subject = document.createElement("p"); subject.id = "worker-timeline-subject"; subject.className = "raw-output-subject";
    heading.append(title, subject);
    const close = document.createElement("button"); close.type = "button"; close.className = "raw-output-close"; close.setAttribute("aria-label", "Close timeline"); close.textContent = "×";
    close.onclick = () => dialog!.close();
    header.append(heading, close);
    const body = document.createElement("div"); body.className = "worker-timeline-body";
    dialog.append(header, body);
    document.body.append(dialog);
  }
  const opened = dialog;
  opened.onclose = () => handlers.closed();
  return opened;
}

/** Paint the timeline: the worker's state and task, then its rows, newest first. */
export function renderWorkerTimeline(dialog: HTMLDialogElement, subject: WorkerTimelineSubject, stream: WorkEventStream | undefined, rows: WorkRowsContext | undefined, handlers: WorkerTimelineHandlers): void {
  const document = dialog.ownerDocument;
  const active = document.activeElement instanceof HTMLElement && dialog.contains(document.activeElement) ? document.activeElement.dataset.focusKey : undefined;
  dialog.querySelector<HTMLElement>("#worker-timeline-title")!.textContent = `${subject.worker} timeline`;
  const state = subject.stateLabel ?? statusLabel(subject.status);
  const line = dialog.querySelector<HTMLElement>("#worker-timeline-subject")!;
  const chip = document.createElement("span"); chip.className = `status-chip status-chip--${statusClass(state)}`; chip.textContent = state;
  line.replaceChildren(chip);
  if (subject.task) {
    const id = document.createElement("span"); id.className = "status-identifier"; id.textContent = subject.task;
    line.append(" ", id);
    if (subject.taskTitle) line.append(` ${subject.taskTitle}`);
  } else line.append(" No task assigned");
  const body = dialog.querySelector<HTMLElement>(".worker-timeline-body")!;
  const scroll = new Map([...body.querySelectorAll<HTMLElement>(".work-detail[data-focus-key]")].map(detail => [detail.dataset.focusKey, detail.scrollTop]));
  const children: HTMLElement[] = [];
  const note = (text: string): HTMLElement => { const p = document.createElement("p"); p.className = "worker-timeline-note"; p.setAttribute("role", "status"); p.textContent = text; return p; };
  if (subject.unavailable) children.push(note(subject.unavailable));
  else if (!stream || !rows || (!stream.loaded && stream.loading)) children.push(note("Loading this worker's activity…"));
  else if (stream.events.length === 0) children.push(note("No activity recorded for this worker yet."));
  else {
    const repaint = (focusKey: string): void => { renderWorkerTimeline(dialog, subject, stream, rows, handlers); refocus(dialog, focusKey); };
    const list = workRows(document, [...stream.events].reverse(), rows, repaint, { time: true, noticesKey: "timeline:turns" });
    list.classList.add("worker-timeline-rows");
    list.setAttribute("aria-label", `${subject.worker}'s recent activity, newest first`);
    children.push(list);
    if (stream.hasEarlier) {
      const older = document.createElement("button"); older.type = "button"; older.className = "worker-timeline-older"; older.dataset.focusKey = "older";
      older.textContent = stream.pending("older") ? "Loading older…" : "Load older";
      older.disabled = stream.pending("older");
      older.onclick = () => handlers.readOlder();
      children.push(older);
    }
  }
  if (handlers.rawOutput) {
    const fallback = document.createElement("button"); fallback.type = "button"; fallback.className = "worker-timeline-raw"; fallback.dataset.focusKey = "raw";
    // Commander attaches only the supervisor's pane (DESIGN.md D5); a worker's
    // own terminal is on its machine. Say so instead of implying otherwise.
    fallback.textContent = "Open the supervisor's raw output";
    const limit = document.createElement("p"); limit.className = "worker-timeline-note";
    // The worker's name is one identifier: it never breaks at its hyphen.
    const name = document.createElement("span"); name.className = "worker-timeline-name"; name.textContent = `${subject.worker}'s`;
    limit.append("Commander shows the supervisor's terminal only. ", name, " own terminal is on its machine.");
    children.push(limit);
    fallback.onclick = () => { dialog.close(); handlers.rawOutput?.(); };
    children.push(fallback);
  }
  body.replaceChildren(...children);
  for (const detail of body.querySelectorAll<HTMLElement>(".work-detail[data-focus-key]")) detail.scrollTop = scroll.get(detail.dataset.focusKey) ?? 0;
  if (active) refocus(dialog, active);
}

/** A worker's timeline opener, shared with its rail row. */
export function workerTimelineButton(document: Document, worker: string, open: () => void): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button"; button.className = "status-identifier status-timeline"; button.dataset.worker = worker;
  button.setAttribute("aria-haspopup", "dialog");
  button.setAttribute("aria-label", `${worker} timeline`);
  button.textContent = worker;
  button.onclick = open;
  return button;
}
