import { expect, it } from "vitest";
import { statusClass, statusLabel, workerProgress } from "./progress-model";

it.each(["InProgress", "in_progress", "INPROGRESS"])("labels %s in human terms", state => {
  expect(statusLabel(state)).toBe("In progress"); expect(statusClass(state)).toBe("in-progress");
});
it.each(["AwaitingMerge", "awaiting_merge", "AWAITINGMERGE"])("labels %s in human terms", state => expect(statusLabel(state)).toBe("Awaiting merge"));
it("shows six catalog workers with sparse status and no raw activity summary", () => {
  const names = ["a", "b", "c", "d", "e", "f"];
  const workers = workerProgress(names, [{ name: "sup", role: "supervisor" }, { name: "a", current_task: "cas-1" }, { name: "b", current_task: null }], [{ id: "cas-1", title: "Fix the question" }], "sup");
  expect(workers.map(worker => worker.name)).toEqual(names);
  expect(workers[0]).toMatchObject({ currentTask: "cas-1", work: "Fix the question" });
  expect(workers[1]?.work).toBe("No current task");
  expect(workers[2]).toEqual({ name: "c", agent: undefined, currentTask: undefined, work: "Current work not reported" });
});
it("keeps unknown current task and missing metadata honest and joins only exact names", () => {
  const workers = workerProgress(["a", "a", "ab"], [{ name: "a", current_task: "missing" }], [{ id: "other", title: "Unrelated", assignee: "ab" }]);
  expect(workers).toHaveLength(2); expect(workers[0]?.work).toBe("Task details not reported"); expect(workers[1]?.work).toBe("Current work not reported");
});
it("does not turn an explicitly empty session into a project-wide worker list", () => {
  expect(workerProgress([], [{ name: "a" }], [])).toEqual([]);
  expect(workerProgress(undefined, [{ name: "sup", role: "supervisor" }, { name: "a" }], []).map(worker => worker.name)).toEqual(["a"]);
});

it("selects the focused release and groups only its supervisor's tasks, with the full board reachable", async () => {
  const { releaseProgress, taskGroups } = await import("./progress-model");
  const tasks = [
    { id: "merge", epic_id: "release", status: "AwaitingMerge" },
    { id: "blocked", epic_id: "release", status: "Blocked" },
    { id: "work", epic_id: "release", status: "InProgress" },
    { id: "ready", epic_id: "release", status: "Open" },
    { id: "other", epic_id: "other", status: "InProgress" },
    { id: "unknown", status: "Open" },
  ];
  const epics = [{ id: "release", title: "Release 3.87", progress: { total: 8, merged: 3, merges_per_hour: 1.5, last_merged_at: "2026-10-28T14:00:00Z" } }];
  expect(releaseProgress(epics, "release", tasks)).toMatchObject({ title: "Release 3.87", progress: { total: 8, merged: 3, merges_per_hour: 1.5 } });
  expect(taskGroups(tasks, epics, [], false).map(group => [group.label, group.tasks.map(task => task.id)])).toEqual([
    ["Needs you", ["merge", "blocked"]], ["In progress", ["work"]], ["Ready", ["ready"]],
  ]);
  expect(taskGroups(tasks, epics, [], true).flatMap(group => group.tasks).map(task => task.id)).toContain("other");
  expect(taskGroups(tasks, epics, [], true).flatMap(group => group.tasks).map(task => task.id)).toContain("unknown");
  expect(releaseProgress([{ id: "old" }, { id: "release" }], null, tasks)?.id).toBe("release");
  expect(releaseProgress([{ id: "unknown" }], null, [])?.progress).toBeUndefined();
});
