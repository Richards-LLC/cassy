import { expect, it } from "vitest";
import { statusClass, statusLabel, workerProgress } from "./progress-model";

it.each(["InProgress", "in_progress", "INPROGRESS"])("labels %s in human terms", state => {
  expect(statusClass(state)).toBe("in-progress");
});

it("shows six catalog workers with sparse status and no raw activity summary", () => {
  const names = ["a", "b", "c", "d", "e", "f"];
  const workers = workerProgress(names, [{ name: "sup", role: "supervisor" }, { name: "a", current_task: "cas-1" }, { name: "b", current_task: null }], [{ id: "cas-1", title: "Fix the question" }], "sup");
  expect(workers.map(worker => worker.name)).toEqual(names);
  expect(workers[0]).toMatchObject({ currentTask: "cas-1", work: "Fix the question" });
  expect(workers[1]?.currentTask).toBeUndefined();
  expect(workers[2]).toMatchObject({ name: "c", agent: undefined, currentTask: undefined });
});
it("keeps unknown current task and missing metadata honest and joins only exact names", () => {
  const workers = workerProgress(["a", "a", "ab"], [{ name: "a", current_task: "missing" }], [{ id: "other", title: "Unrelated", assignee: "ab" }]);
  expect(workers).toHaveLength(2); expect(workers[0]?.currentTask).toBe("missing"); expect(workers[1]?.agent).toBeUndefined();
});
it("does not turn an explicitly empty session into a project-wide worker list", () => {
  expect(workerProgress([], [{ name: "a" }], [])).toEqual([]);
  expect(workerProgress(undefined, [{ name: "sup", role: "supervisor" }, { name: "a" }], []).map(worker => worker.name)).toEqual(["a"]);
});

it("scopes server-shaped project epics to the supervisor and keeps the full board reachable", async () => {
  const { releaseProgress, taskGroups } = await import("./progress-model");
  const tasks = [
    { id: "merge", epic_id: "release", status: "AwaitingMerge" },
    { id: "blocked", epic_id: "release", status: "Blocked" },
    { id: "work", epic_id: "release", status: "InProgress" },
    { id: "ready", epic_id: "release", status: "Open" },
    { id: "other", epic_id: "other", status: "InProgress" },
    { id: "unknown", status: "Open" },
    { id: "closed-child", epic_id: "closed", status: "Open" },
    { id: "cancelled-child", epic_id: "cancelled", status: "Open" },
    { id: "verified-child", epic_id: "verified", status: "Open" },
  ];
  const epics = [{ id: "release", status: "inprogress", assignee: "supervisor-id", title: "Release 3.87", progress: { total: 8, merged: 3, merges_per_hour: 1.5, last_merged_at: "2026-10-28T14:00:00Z" } },
    { id: "other", status: "open", assignee: "another-supervisor" },
    { id: "closed", status: "closed", assignee: "supervisor-id" },
    { id: "cancelled", status: "cancelled", epic_verification_owner: "supervisor" },
    { id: "verified", status: "open", epic_verification_owner: "supervisor" },
  ];
  const supervisors = ["supervisor", "supervisor-id"];
  expect(releaseProgress(epics, "release", tasks, supervisors)).toMatchObject({ title: "Release 3.87", progress: { total: 8, merged: 3, merges_per_hour: 1.5 } });
  expect(taskGroups(tasks, epics, [], false, supervisors, "release").map(group => group.tasks.map(task => task.id))).toEqual([
    ["merge", "blocked"], ["work"], ["ready"],
  ]);
  expect(taskGroups(tasks, epics, [], true, supervisors, "release").flatMap(group => group.tasks).map(task => task.id)).toContain("other");
  expect(taskGroups(tasks, epics, [], true, supervisors, "release").flatMap(group => group.tasks).map(task => task.id)).toContain("unknown");
  const unpinned = taskGroups(tasks, epics, [], false, supervisors).flatMap(group => group.tasks).map(task => task.id);
  expect(unpinned).toEqual(["merge", "blocked", "work", "ready", "verified-child"]);
  expect(releaseProgress([...epics].reverse(), null, [], supervisors)?.id).toBe("verified");
  expect(releaseProgress(epics, "closed", tasks, supervisors)?.id).toBe("release");
  expect(releaseProgress(epics.filter(epic => ["other", "closed", "cancelled"].includes(epic.id)), null, tasks, supervisors)).toBeUndefined();
  expect(taskGroups(tasks, [], [], false, supervisors).flatMap(group => group.tasks)).toEqual([]);
});


it("keeps worker delivery states shared with the rail and never guesses milestone receipts", async () => {
  const { workerState } = await import("./progress-model");
  const task = { id: "t", status: "AwaitingMerge" };
  const blocked = { ...task, status: "Blocked" };
  expect(workerState(workerProgress(["a"], [{ name: "a", current_task: "t", liveness: "waiting_for_input" }], [blocked])[0], [blocked])).toBe("Blocked");
  for (const [agent, expected] of [
    [{ name: "a", current_task: "t", liveness: "executing" }, "Parked for merge"],
    [{ name: "a", liveness: "waiting_for_input" }, "Waiting"],
    [{ name: "a", liveness: "stalled" }, "Stalled"],
    [{ name: "a", liveness: "dead", current_task: "t" }, "Offline"],
    [{ name: "a" }, "Not reported"],
  ] as const) expect(workerState(workerProgress(["a"], [agent], [task])[0], [task])).toBe(expected);
  expect(workerState(workerProgress(["a"], [{ name: "a", current_task: "t" }], [{ ...task, delivery_state: "merged" }])[0], [{ ...task, delivery_state: "merged" }])).toBe("Merged");
});

it("keeps Live epics to the selected project across machines, titled, deduplicated and ranked", async () => {
  const { projectEpics, epicTitle } = await import("./progress-model");
  type Source = import("./progress-model").EpicSessionProgress;
  const source = (machine: string, overrides: Partial<Source> = {}): Source => ({ project: "cas-src", machine, machineLabel: machine, session: machine, supervisor: "supervisor", roster: ["worker"],
    agents: [{ name: "worker", status: "active", current_task: "build", liveness: "executing" }],
    tasks: [{ id: "build", epic_id: "shared", title: "Ship the rail", status: "in_progress", assignee: "worker" }],
    epics: [{ id: "shared", title: "EPIC: Commander rail", status: "in_progress", counts: { open: 4, in_progress: 2, merged: 6 } }], stale: false, received: 1, ...overrides });
  const { live, quiet } = projectEpics([
    source("soundwave", { tasks: [
      { id: "build", epic_id: "shared", title: "Ship the rail", status: "in_progress", assignee: "worker" },
      { id: "stuck", epic_id: "blocked", title: "Wait on review", status: "blocked" },
      { id: "busy-1", epic_id: "busy", status: "in_progress" }, { id: "busy-2", epic_id: "busy", status: "awaiting_merge" },
      { id: "nameless-work", epic_id: "nameless", status: "in_progress" }, { id: "train-work", epic_id: "train", status: "in_progress" },
    ], epics: [
      { id: "shared", title: "EPIC: Commander rail", status: "in_progress", counts: { open: 4, in_progress: 2, merged: 6 } },
      { id: "blocked", title: "Epic — Blocked work", status: "open" }, { id: "busy", title: "Busy", status: "open" },
      { id: "nameless", title: "EPIC:", status: "open" }, { id: "untitled", status: "open" }, { id: "train", title: "Release train", status: "in_progress" },
      { id: "quiet", title: "Someday", status: "open" }, { id: "done", title: "Done", status: "closed" },
    ] }),
    source("prowl", { received: 2 }),
    source("prowl", { project: "gabber", session: "gabber", epics: [{ id: "foreign", title: "Gabber epic", status: "in_progress" }], tasks: [{ id: "g", epic_id: "foreign", status: "in_progress" }] }),
  ], "cas-src", "train");
  expect(epicTitle({ title: "EPIC: Conversational search" })).toBe("Conversational search");
  // Blocked first, then most in flight; the train, untitled, closed and foreign epics never appear.
  expect(live.map(epic => [epic.key, epic.title])).toEqual([["blocked", "Blocked work"], ["busy", "Busy"], ["shared", "Commander rail"]]);
  const shared = live[2]!;
  expect(shared.machines).toEqual(["prowl", "soundwave"]);
  expect(shared.counts).toEqual({ open: 0, in_progress: 1, merged: 6, total: 7 });
  expect(shared.workers.map(worker => worker.machine)).toEqual(["prowl", "soundwave"]);
  expect(shared.rows.map(row => [row.task.title, row.group, row.state])).toEqual([["Ship the rail", "In progress", "Building"]]);
  expect(live[1]).toMatchObject({ building: 1, parked: 1, blocked: 0, counts: undefined });
  expect(live[0]).toMatchObject({ blocked: 1, rows: [{ group: "Open", state: "Blocked" }] });
  expect(quiet).toBe(1);
});

it("links a parked task to its running preview only, never this page or a non-loopback origin", async () => {
  const { deliveryPreview } = await import("./progress-model");
  const now = Date.parse("2026-11-06T21:00:00Z");
  const parked = { id: "cas-1", status: "AwaitingMerge" };
  const preview = (origin: string, until = "2026-11-06T22:00:00Z", task = "cas-1") => [{ origin, task, until }];
  expect(deliveryPreview(parked, preview("http://127.0.0.1:4999"), now, "https://commander.example")).toBe("http://127.0.0.1:4999/");
  expect(deliveryPreview({ ...parked, status: "InProgress" }, preview("http://127.0.0.1:4999"), now, "https://commander.example")).toBeUndefined();
  expect(deliveryPreview(parked, preview("http://127.0.0.1:4999", "2026-11-06T20:59:59Z"), now, "https://commander.example")).toBeUndefined();
  expect(deliveryPreview(parked, preview("http://127.0.0.1:4999", undefined, "cas-2"), now, "https://commander.example")).toBeUndefined();
  expect(deliveryPreview(parked, preview("http://127.0.0.1:4999"), now, "http://127.0.0.1:4999")).toBeUndefined();
  expect(deliveryPreview(parked, preview("javascript:alert(1)"), now, "https://commander.example")).toBeUndefined();
  expect(deliveryPreview(parked, preview("https://evil.example"), now, "https://commander.example")).toBeUndefined();
});
