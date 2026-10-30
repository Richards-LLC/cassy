// @vitest-environment jsdom
import { expect, it, vi } from "vitest";
import { renderTeam, teamFinding, updateTeamElapsed, type TeamSnapshot } from "./team-view";

it("joins the selected session to real receipts, keeps focus on live updates and exposes no raw activity", () => {
  const root = document.createElement("section"); root.tabIndex = 0; document.body.append(root);
  const snapshot: TeamSnapshot = {
    roster: ["lark", "owl", "robin"], supervisor: "supervisor", supervisors: ["supervisor"], focused: "release", asks: 1,
    agents: [{ name: "lark", current_task: "work", liveness: "executing", harness: "codex", model: "gpt-6.1", task_started_at: "2026-10-28T12:00:00Z", recent_trail: [{ step: "started", reference: "event:12" }], latest_activity: { summary: "SECRET RAW TOOL OUTPUT", created_at_rfc3339: "2026-10-28T12:00:00Z" } }, { name: "outsider" }],
    tasks: [{ id: "work", title: "Ship the staffing view", status: "InProgress", epic_id: "release" }, { id: "merge", title: "Review", status: "AwaitingMerge", epic_id: "release" }],
    epics: [{ id: "release", title: "Release staffing", status: "InProgress", progress: { total: 8, merged: 3, merges_per_hour: 1.5 } }],
  };
  const questions = vi.fn(), progress = vi.fn();
  renderTeam(root, snapshot, questions, progress);
  expect(root.querySelectorAll(".team-lane")).toHaveLength(3);
  expect(root.textContent).toContain("3 of 8 merged");
  expect(root.textContent).toContain("Ship the staffing view");
  expect(root.textContent).toContain("codex · gpt-6.1");
  expect(root.textContent).not.toMatch(/SECRET RAW|outsider/);
  expect(root.querySelectorAll('[data-reported="true"]')).toHaveLength(1);
  expect(root.querySelector('.team-lane[data-worker="lark"] .supervisor-state')?.classList.contains("working")).toBe(true);
  // Receipt names say when it was reported; an internal event id means nothing to a reader.
  const started = root.querySelector('.team-lane[data-worker="lark"] .team-trail li');
  expect(started?.getAttribute("aria-label")).toMatch(/^Started: Reported/);
  expect(started?.getAttribute("aria-label")).not.toContain("event:");
  updateTeamElapsed(root, Date.parse("2026-10-28T12:42:00Z"));
  expect(root.querySelector(".team-elapsed")?.textContent).toBe("42m on task");
  const initialLane = root.querySelector(".team-lane");
  renderTeam(root, { ...snapshot, agents: [{ ...snapshot.agents[0], latest_activity: { summary: "UNRELATED RAW UPDATE", created_at_rfc3339: "2026-10-28T12:42:00Z" } }, snapshot.agents[1]] }, questions, progress);
  expect(root.querySelector(".team-lane")).toBe(initialLane);
  const merge = root.querySelector<HTMLButtonElement>('[data-team-action="merges"]')!;
  merge.click(); expect(progress).toHaveBeenCalledOnce(); merge.focus();
  renderTeam(root, { ...snapshot, agents: [{ ...snapshot.agents[0], liveness: "waiting_for_input" }] }, questions, progress);
  expect(root.querySelector(".team-state")?.textContent).toBe("Waiting");
  expect((document.activeElement as HTMLElement).dataset.teamAction).toBe("merges");
  root.querySelector<HTMLButtonElement>('[data-team-action="asks"]')!.click(); expect(questions).toHaveBeenCalledOnce();
  renderTeam(root, { ...snapshot, askLabel: "Ship it today?" }, questions, progress);
  expect(root.querySelector('[data-team-action="asks"]')?.textContent).toBe("Ship it today?");
  renderTeam(root, { ...snapshot, asks: 0, tasks: [snapshot.tasks[0]] }, questions, progress);
  expect(document.activeElement).toBe(root);
  root.remove();
});

it("keeps unavailable, empty and stale staffing explicit without inventing worker health", () => {
  const root = document.createElement("section");
  const snapshot: TeamSnapshot = { roster: [], agents: [{ name: "unrelated" }], tasks: [], epics: [], focused: null, supervisors: [], asks: 0 };
  renderTeam(root, { ...snapshot, unavailable: "Waiting for project status…" }, vi.fn(), vi.fn());
  expect(root.textContent).toContain("Waiting for project status");
  expect(root.querySelectorAll(".team-lane")).toHaveLength(0);
  renderTeam(root, { ...snapshot, stale: "Not live — reconnecting." }, vi.fn(), vi.fn());
  expect(root.textContent).toContain("No workers in this supervisor’s session.");
  expect(root.textContent).toContain("Not live — reconnecting.");
  expect(root.textContent).toContain("Nothing needs you.");
  expect(root.querySelector("h2")?.textContent).toBe("Last reported: No workers reporting.");
  expect(root.textContent).not.toContain("unrelated");
});

it("leads with what the session reports and names what it cannot read", () => {
  expect(teamFinding(["Executing", "Executing", "Executing", "Waiting"], 0)).toBe("3 working, 1 waiting.");
  expect(teamFinding(["Executing", "Stalled", "Parked for merge"], 1)).toBe("1 working, 1 parked, 1 stalled. 1\u00a0needs\u00a0you.");
  expect(teamFinding(["Executing", "Unknown"], 2)).toBe("1 working, 1 not reported. 2\u00a0need\u00a0you.");
});
