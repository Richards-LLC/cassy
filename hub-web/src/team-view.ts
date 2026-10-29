import type { FleetAgent } from "./fleet-ops";
import { workerProgress, workerState, releaseProgress, taskGroups, statusClass, type ProgressTask, type ReleaseEpic } from "./progress-model";
import { releaseHeader } from "./progress-view";

export interface TeamSnapshot {
  readonly roster?: readonly string[];
  readonly supervisor?: string;
  readonly agents: readonly FleetAgent[];
  readonly tasks: readonly ProgressTask[];
  readonly epics: readonly ReleaseEpic[];
  readonly focused: string | null;
  readonly supervisors: readonly string[];
  readonly asks: number;
  readonly stale?: string;
  readonly unavailable?: string;
}

const STEPS = [["started", "Started"], ["pushed", "Pushed"], ["ci_green", "CI green"], ["merged", "Merged"], ["verified", "Verified"]] as const;

/** Display clock only; this component has no connection or polling authority. */
export function updateTeamElapsed(root: HTMLElement, now = Date.now()): void {
  for (const node of root.querySelectorAll<HTMLElement>("[data-task-start]")) {
    const at = Date.parse(node.dataset.taskStart ?? "");
    if (!Number.isFinite(at) || at > now) { node.textContent = "Task time not reported"; continue; }
    const minutes = Math.floor((now - at) / 60_000);
    node.textContent = minutes < 60 ? `${minutes}m on task` : `${Math.floor(minutes / 60)}h ${minutes % 60}m on task`;
  }
}

/** Pure rendering of the same session-scoped model used by Tasks & progress. */
export function renderTeam(root: HTMLElement, snapshot: TeamSnapshot, openAttention: () => void, openProgress: () => void): void {
  const document = root.ownerDocument;
  const priorFocus = (document.activeElement as HTMLElement | null)?.dataset.teamAction;
  const workers = workerProgress(snapshot.roster, snapshot.agents, snapshot.tasks, snapshot.supervisor);
  const release = releaseProgress(snapshot.epics, snapshot.focused, snapshot.tasks, snapshot.supervisors);
  const needs = taskGroups(snapshot.tasks, snapshot.epics, workers.map(worker => worker.name), false, snapshot.supervisors, snapshot.focused)[0].tasks;
  // Heartbeats and raw activity are not rendered. They cannot remount lanes,
  // restart their animation, or replace a control under the pointer.
  const signature = JSON.stringify([snapshot.asks, snapshot.stale, snapshot.unavailable, release, needs.map(task => [task.id, task.status]),
    workers.map(worker => [worker.name, worker.work, workerState(worker, snapshot.tasks), worker.agent?.harness, worker.agent?.model, worker.agent?.task_started_at, worker.agent?.recent_trail])]);
  if (root.dataset.signature === signature) { updateTeamElapsed(root); return; }
  root.dataset.signature = signature;
  const text = (tag: string, value: string, className?: string): HTMLElement => {
    const node = document.createElement(tag); node.textContent = value; if (className) node.className = className; return node;
  };
  const priorStates = new Map([...root.querySelectorAll<HTMLElement>(".team-lane")].map(lane => [lane.dataset.worker, lane.querySelector(".team-state")?.textContent]));
  const priorButtons = new Map([...root.querySelectorAll<HTMLButtonElement>("[data-team-action]")].map(button => [button.dataset.teamAction, button]));
  const heading = text("h2", "Your team, at a glance.");
  const notice = text("p", snapshot.stale ?? "Live staffing from this supervisor’s session", "team-freshness");
  notice.setAttribute("role", "status");
  root.replaceChildren(heading, notice);
  if (snapshot.unavailable) { root.append(text("p", snapshot.unavailable, "team-empty")); return; }
  root.append(releaseHeader(document, release));
  const strip = document.createElement("section"); strip.className = "team-needs"; strip.setAttribute("aria-label", "Needs you");
  strip.append(text("h3", "Needs you"));
  const action = (label: string, key: string, run: () => void): void => {
    const button = priorButtons.get(key) ?? document.createElement("button"); button.type = "button"; button.textContent = label; button.dataset.teamAction = key; button.onclick = run; strip.append(button);
  };
  if (snapshot.asks) action(`${snapshot.asks} supervisor ask${snapshot.asks === 1 ? " or blocker" : "s or blockers"}`, "asks", openAttention);
  const blockers = needs.filter(task => statusClass(task.status) === "blocked").length;
  const merges = needs.filter(task => statusClass(task.status) === "awaiting-merge").length;
  if (blockers) action(`${blockers} blocker${blockers === 1 ? "" : "s"}`, "blockers", openProgress);
  if (merges) action(`${merges} merge${merges === 1 ? "" : "s"} waiting`, "merges", openProgress);
  if (!snapshot.asks && !needs.length) strip.append(text("p", "No decisions waiting."));
  root.append(strip);
  const list = document.createElement("ul"); list.className = "team-lanes"; list.setAttribute("aria-label", "Worker lanes");
  for (const worker of workers) {
    const lane = document.createElement("li"); lane.className = "team-lane"; lane.dataset.worker = worker.name;
    const identity = document.createElement("div"); identity.className = "team-identity";
    identity.append(text("h3", worker.name), text("span", [worker.agent?.harness ?? "Harness not reported", worker.agent?.model ?? "Model not reported"].join(" · "), "team-model"));
    const state = workerState(worker, snapshot.tasks);
    const badge = text("span", state, `team-state status-chip status-chip--${statusClass(state)}`); badge.setAttribute("aria-label", `${worker.name}: ${state}`);
    if (priorStates.has(worker.name) && priorStates.get(worker.name) !== state) badge.classList.add("team-state--changed");
    const work = text("p", worker.work, "team-task");
    const elapsed = text("p", "Task time not reported", "team-elapsed");
    if (worker.agent?.task_started_at) elapsed.dataset.taskStart = worker.agent.task_started_at;
    const trail = document.createElement("ol"); trail.className = "team-trail"; trail.setAttribute("aria-label", `${worker.name} delivery evidence`);
    for (const [step, label] of STEPS) {
      const evidence = worker.agent?.recent_trail?.find(item => item.step === step && item.reference);
      const item = text("li", label); item.dataset.reported = String(Boolean(evidence));
      const time = evidence?.at ? new Date(evidence.at) : undefined;
      const detail = evidence ? `Reported${time && Number.isFinite(time.getTime()) ? ` at ${time.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit", hour12: true })}` : ""}: ${evidence.reference}` : "Not reported";
      item.title = detail; item.setAttribute("aria-label", `${label}: ${detail}`); trail.append(item);
    }
    lane.append(identity, badge, work, elapsed, trail); list.append(lane);
  }
  root.append(list);
  if (!workers.length) root.append(text("p", "No workers in this supervisor’s session.", "team-empty"));
  root.append(text("p", "Delivery trail: solid marks have a recorded receipt; open marks are not reported.", "team-evidence-key"));
  updateTeamElapsed(root);
  if (priorFocus) (root.querySelector<HTMLElement>(`[data-team-action="${priorFocus}"]`) ?? root).focus({ preventScroll: true });
}
