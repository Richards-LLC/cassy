import type { FleetAgent } from "./fleet-ops";
import { workerProgress, workerState, releaseProgress, statusClass, statusLabel, type OperatorAction, type ProgressTask, type ReleaseEpic } from "./progress-model";
import { releaseHeader } from "./progress-view";
import { avatarLetters, providerAvatarMarkup, type AvatarState } from "./provider-mark";

export interface TeamSnapshot {
  readonly roster?: readonly string[];
  readonly supervisor?: string;
  readonly agents: readonly FleetAgent[];
  readonly tasks: readonly ProgressTask[];
  readonly epics: readonly ReleaseEpic[];
  readonly focused: string | null;
  readonly supervisors: readonly string[];
  readonly actions: readonly OperatorAction[];
  readonly stale?: string;
  readonly unavailable?: string;
}

/** The avatar dot repeats the pill's state as a mark; the pill still says it in words. */
const LANE_DOTS: Readonly<Record<string, AvatarState>> = { Executing: "working", Merged: "working", Waiting: "idle", Blocked: "needs-you", Stalled: "needs-you", "Budget stopped": "needs-you", "Parked for merge": "needs-you", Offline: "offline" };

const STEPS = [["started", "Started"], ["pushed", "Pushed"], ["ci_green", "CI green"], ["merged", "Merged"], ["verified", "Verified"]] as const;

const FINDING_GROUPS: ReadonlyArray<readonly [string, string]> = [["Executing", "working"], ["Blocked", "blocked"], ["Waiting", "waiting"], ["Parked for merge", "parked"], ["Merged", "merged"], ["Stalled", "stalled"], ["Budget stopped", "stopped"], ["Offline", "offline"]];

/** The sentence Team leads with: only what this session reports, never a guess at health. */
export function teamFinding(states: readonly string[], decisions: number): string {
  const parts = FINDING_GROUPS.map(([state, word]) => [states.filter(item => item === state).length, word] as const).filter(([count]) => count > 0).map(([count, word]) => `${count} ${word}`);
  const other = states.filter(state => !FINDING_GROUPS.some(([known]) => known === state)).length;
  if (other) parts.push(`${other} not reported`);
  const staffing = parts.length ? `${parts.join(", ")}.` : "No workers reporting.";
  return decisions ? `${staffing} ${decisions}\u00a0${decisions === 1 ? "needs" : "need"}\u00a0you.` : staffing;
}

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
export function renderTeam(root: HTMLElement, snapshot: TeamSnapshot, openAttention: (requestId: number) => void): void {
  const document = root.ownerDocument;
  const priorWorkerFocus = (document.activeElement as HTMLElement | null)?.dataset.worker;
  const priorFocus = (document.activeElement as HTMLElement | null)?.dataset.teamAction;
  const workers = workerProgress(snapshot.roster, snapshot.agents, snapshot.tasks, snapshot.supervisor);
  // Trouble is reported evidence, never inferred from task age or a quiet heartbeat.
  const lanes = workers.map(worker => {
    const state = workerState(worker, snapshot.tasks);
    const task = snapshot.tasks.find(task => task.id === worker.currentTask);
    const blocked = task?.delivery_state !== "merged" && statusLabel(task?.status) === "Blocked";
    const trouble = ["Stalled", "Budget stopped"].includes(state);
    const reason = trouble ? state : blocked ? "Blocked" : state === "Parked for merge" ? "Merge waiting" : undefined;
    return { worker, state, reason, priority: trouble || blocked ? 2 : reason ? 1 : 0 };
  }).sort((a, b) => b.priority - a.priority);
  const release = releaseProgress(snapshot.epics, snapshot.focused, snapshot.tasks, snapshot.supervisors);
  const decisions = snapshot.actions.length;
  // Heartbeats and raw activity are not rendered. They cannot remount lanes,
  // restart their animation, or replace a control under the pointer.
  const signature = JSON.stringify([snapshot.actions, snapshot.stale, snapshot.unavailable, release,
    workers.map(worker => [worker.name, worker.work, workerState(worker, snapshot.tasks), lanes.find(lane => lane.worker === worker)?.reason, worker.agent?.harness, worker.agent?.model, worker.agent?.task_started_at, worker.agent?.recent_trail])]);
  if (root.dataset.signature === signature) { updateTeamElapsed(root); return; }
  root.dataset.signature = signature;
  const text = (tag: string, value: string, className?: string): HTMLElement => {
    const node = document.createElement(tag); node.textContent = value; if (className) node.className = className; return node;
  };
  const priorStates = new Map([...root.querySelectorAll<HTMLElement>(".team-lane")].map(lane => [lane.dataset.worker, lane.querySelector(".team-state")?.textContent]));
  const priorButtons = new Map([...root.querySelectorAll<HTMLButtonElement>("[data-team-action]")].map(button => [button.dataset.teamAction, button]));
  const finding = teamFinding(workers.map(worker => workerState(worker, snapshot.tasks)), decisions);
  const heading = text("h2", snapshot.unavailable ? "Team status unavailable." : snapshot.stale ? `Last reported: ${finding}` : finding);
  if (decisions && !snapshot.unavailable) {
    const suffix = finding.slice(finding.lastIndexOf(". ") + 2);
    heading.textContent = heading.textContent!.slice(0, -suffix.length);
    heading.append(text("strong", suffix, "team-attention-count"));
  }
  const notice = text("p", snapshot.stale ?? "Live staffing from this supervisor’s session", "team-freshness");
  notice.setAttribute("role", "status");
  const head = document.createElement("div"); head.className = "team-head"; head.append(heading, notice);
  root.replaceChildren(head);
  if (snapshot.unavailable) { root.append(text("p", snapshot.unavailable, "team-empty")); return; }
  root.append(releaseHeader(document, release));
  const strip = document.createElement("section"); strip.className = "team-needs"; strip.setAttribute("aria-label", "Needs you");
  strip.append(text("h3", "Needs you"));
  const action = (label: string, key: string, run: () => void): HTMLButtonElement => {
    const button = priorButtons.get(key) ?? document.createElement("button"); button.type = "button"; button.textContent = label; button.dataset.teamAction = key; button.onclick = run; strip.append(button); return button;
  };
  for (const item of snapshot.actions) {
    const button = action(item.request.summary || item.request.message, `request:${item.request_id}`, () => openAttention(item.request_id));
    button.dataset.owner = item.owner;
    button.dataset.requestId = String(item.request_id);
    if (item.task_id) button.dataset.taskId = item.task_id;
  }
  strip.dataset.active = String(decisions > 0);
  if (!decisions) strip.append(text("p", "Nothing needs you."));
  root.append(strip);
  const list = document.createElement("ul"); list.className = "team-lanes"; list.setAttribute("aria-label", "Worker lanes");
  for (const { worker, state, reason, priority } of lanes) {
    const lane = document.createElement("li"); lane.className = "team-lane"; lane.dataset.worker = worker.name; lane.tabIndex = -1;
    if (reason) lane.dataset.urgency = priority === 2 ? "critical" : "waiting";
    const identity = document.createElement("div"); identity.className = "team-identity";
    const model = text("span", [worker.agent?.harness ?? "Harness not reported", worker.agent?.model ?? "Model not reported"].join(" · "), "team-model");
    model.dataset.modelReported = String(Boolean(worker.agent?.model)); if (worker.agent?.model) model.dataset.model = worker.agent.model;
    identity.append(text("h3", worker.name), model);
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
      const detail = evidence ? `Reported${time && Number.isFinite(time.getTime()) ? ` at ${time.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit", hour12: true })}` : ""}${evidence.reference.startsWith("event:") ? "" : `: ${evidence.reference}`}` : "Not reported";
      item.title = detail; item.setAttribute("aria-label", `${label}: ${detail}`); trail.append(item);
    }
    const avatar = document.createElement("template");
    avatar.innerHTML = providerAvatarMarkup({ harness: worker.agent?.harness, fallback: avatarLetters(worker.name), size: "lane", state: LANE_DOTS[state] });
    lane.append(avatar.content, identity, badge, work, elapsed, trail); list.append(lane);
  }
  root.append(list);
  if (!workers.length) root.append(text("p", "No workers in this supervisor’s session.", "team-empty"));
  root.append(text("p", "Delivery trail: solid marks have a recorded receipt; open marks are not reported.", "team-evidence-key"));
  updateTeamElapsed(root);
  if (priorFocus) ([...root.querySelectorAll<HTMLElement>("[data-team-action]")].find(node => node.dataset.teamAction === priorFocus) ?? root).focus({ preventScroll: true });
  else if (priorWorkerFocus) ([...root.querySelectorAll<HTMLElement>(".team-lane")].find(node => node.dataset.worker === priorWorkerFocus) ?? root).focus({ preventScroll: true });
}
