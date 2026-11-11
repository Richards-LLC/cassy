// @vitest-environment jsdom
import { expect, it, vi } from "vitest";
import { renderTeam, updateTeamElapsed, type TeamSnapshot } from "./team-view";

const request = { owner: "operator" as const, request_id: 21, task_id: "work", request: { notification_id: 21, device_id: "*", reply_to: null, kind: "blocker" as const, summary: "Choose release", message: "Choose release" } };

it("joins the selected session to real receipts, keeps focus on live updates and exposes no raw activity", () => {
  const root = document.createElement("section"); root.tabIndex = 0; document.body.append(root);
  const snapshot: TeamSnapshot = {
    roster: ["lark", "owl", "robin"], supervisor: "supervisor", supervisors: ["supervisor"], focused: "release", actions: [request],
    agents: [{ name: "lark", current_task: "work", liveness: "executing", harness: "codex", model: "gpt-6.1", task_started_at: "2026-10-28T12:00:00Z", recent_trail: [{ step: "started", reference: "event:12" }], latest_activity: { summary: "SECRET RAW TOOL OUTPUT", created_at_rfc3339: "2026-10-28T12:00:00Z" } }, { name: "outsider" }],
    tasks: [{ id: "work", title: "Ship the staffing view", status: "InProgress", epic_id: "release" }, { id: "merge", title: "Review", status: "AwaitingMerge", epic_id: "release" }],
    epics: [{ id: "release", title: "Release staffing", status: "InProgress", progress: { total: 8, merged: 3, merges_per_hour: 1.5 } }],
  };
  const questions = vi.fn();
  renderTeam(root, snapshot, questions);
  expect(root.querySelectorAll(".team-lane")).toHaveLength(3);
  expect(root.textContent).toMatch(new RegExp("(?=[\\s\\S]*3)(?=[\\s\\S]*8)"));
  expect(root.textContent).toContain("Ship the staffing view");
  expect(root.textContent).toContain("6.1");
  expect(root.textContent).not.toMatch(/SECRET RAW|outsider/);
  expect(root.querySelectorAll('[data-reported="true"]')).toHaveLength(1);
  expect(root.querySelector('.team-lane[data-worker="lark"] .supervisor-state')?.classList.contains("working")).toBe(true);
  // Receipt names say when it was reported; an internal event id means nothing to a reader.
  const started = root.querySelector('.team-lane[data-worker="lark"] .team-trail li');
  expect(started?.getAttribute("aria-label")).toBeTruthy();
  expect(started?.getAttribute("aria-label")).not.toContain("event:");
  updateTeamElapsed(root, Date.parse("2026-10-28T12:42:00Z"));
  expect(root.querySelector(".team-elapsed")?.textContent).toContain("42m");
  const initialLane = root.querySelector(".team-lane");
  renderTeam(root, { ...snapshot, agents: [{ ...snapshot.agents[0], latest_activity: { summary: "UNRELATED RAW UPDATE", created_at_rfc3339: "2026-10-28T12:42:00Z" } }, snapshot.agents[1]] }, questions);
  expect(root.querySelector(".team-lane")).toBe(initialLane);
  const ask = root.querySelector<HTMLButtonElement>('[data-team-action="request:21"]')!;
  ask.click(); expect(questions).toHaveBeenCalledWith(21); ask.focus();
  expect(root.querySelector('[data-team-action="merges"]')).toBeNull();
  renderTeam(root, { ...snapshot, agents: [{ ...snapshot.agents[0], liveness: "waiting_for_input" }] }, questions);
  expect(root.querySelector(".team-state")?.textContent).toBeTruthy();
  expect((document.activeElement as HTMLElement).dataset.teamAction).toBe("request:21");
  renderTeam(root, { ...snapshot, actions: [{ ...request, request: { ...request.request, summary: "Ship it today?" } }] }, questions);
  expect(root.querySelector('[data-team-action="request:21"]')?.getAttribute("data-task-id")).toBe("work");
  renderTeam(root, { ...snapshot, actions: [], tasks: [snapshot.tasks[0]] }, questions);
  expect(document.activeElement).toBe(root);
  root.remove();
});

it("keeps unavailable, empty and stale staffing explicit without inventing worker health", () => {
  const root = document.createElement("section");
  const snapshot: TeamSnapshot = { roster: [], agents: [{ name: "unrelated" }], tasks: [], epics: [], focused: null, supervisors: [], actions: [] };
  renderTeam(root, { ...snapshot, unavailable: "Waiting for project status…" }, vi.fn());
  expect(root.textContent).toContain("Waiting for project status");
  expect(root.querySelectorAll(".team-lane")).toHaveLength(0);
  renderTeam(root, { ...snapshot, stale: "Not live — reconnecting." }, vi.fn());
  expect(root.textContent).toBeTruthy();
  expect(root.textContent).toContain("Not live — reconnecting.");

  expect(root.querySelector("h2")?.textContent).toBeTruthy();
  expect(root.textContent).not.toContain("unrelated");
});


it("puts reported trouble ahead of same-state peers and names the worker in attention navigation", () => {
  const root = document.createElement("section"); document.body.append(root);
  const snapshot: TeamSnapshot = {
    roster: ["lark", "owl", "wren"], supervisor: "lead", supervisors: ["lead"], focused: "release", actions: [],
    agents: ["lark", "owl", "wren"].map(name => ({ name, current_task: name, liveness: "waiting_for_input" })),
    tasks: [
      { id: "lark", title: "Write release notes", status: "InProgress", epic_id: "release" },
      { id: "owl", title: "Fix login", status: "Blocked", epic_id: "release" },
      { id: "wren", title: "Review receipts", status: "InProgress", epic_id: "release" },
    ],
    epics: [{ id: "release", title: "Release", status: "InProgress" }],
  };
  renderTeam(root, snapshot, vi.fn());
  expect([...root.querySelectorAll<HTMLElement>(".team-lane")].map(node => node.dataset.worker)).toEqual(["owl", "lark", "wren"]);
  expect([...root.querySelectorAll<HTMLElement>(".team-lane")].map(node => node.dataset.urgency)).toEqual(["critical", undefined, undefined]);
  expect(root.querySelector('.team-lane[data-worker="owl"][data-urgency="critical"]')).not.toBeNull();
  expect(root.querySelectorAll("[data-team-action]")).toHaveLength(0);
  expect(root.querySelector(".team-needs")?.getAttribute("data-active")).toBe("false");
  renderTeam(root, { ...snapshot, agents: snapshot.agents.map(agent => agent.name === "wren" ? { ...agent, liveness: "stalled" } : agent) }, vi.fn());
  expect(root.querySelectorAll("[data-team-action]")).toHaveLength(0);
  expect([...root.querySelectorAll("button")].map(button => button.textContent)).toBeTruthy();
  renderTeam(root, { ...snapshot, tasks: snapshot.tasks.map(task => ({ ...task, status: "InProgress" })) }, vi.fn());
  expect([...root.querySelectorAll<HTMLElement>(".team-lane")].map(node => node.dataset.worker)).toEqual(snapshot.roster);
  expect(root.textContent).toBeTruthy();
  root.remove();
});
