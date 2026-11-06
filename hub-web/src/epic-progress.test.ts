// @vitest-environment jsdom
import { expect, it } from "vitest";
import { liveEpicsSection, projectSessions } from "./epic-progress";
import type { ProjectEpic } from "./progress-model";

const epic = (key: string, overrides: Partial<ProjectEpic> = {}): ProjectEpic => ({
  key, epic: { id: key, title: `EPIC: ${key}` }, title: key, machines: ["soundwave"], workers: [], stale: false,
  rows: [{ task: { id: `${key}-task`, title: `Build ${key}` }, worker: "worker", machine: "soundwave", state: "Building", group: "In progress" }],
  building: 1, parked: 0, blocked: 0, ...overrides,
});

it("reads status only for the selected project's sessions on every machine", () => {
  type Session = { name: string; project?: string; project_dir?: string };
  const sessions = new Map<string, Session[]>([["soundwave", [{ name: "a", project_dir: "/p/cas-src" }, { name: "b", project_dir: "/p/gabber" }]], ["prowl", [{ name: "c", project: "cas-src" }]]]);
  const key = (_: string, session: Session) => session.project ?? session.project_dir!.split("/").at(-1)!;
  expect(projectSessions(sessions, "cas-src", key).map(item => `${item.machineId}:${item.session.name}`)).toEqual(["soundwave:a", "prowl:c"]);
  expect(projectSessions(sessions, undefined, key)).toEqual([]);
});

it("discloses tasks in place, shows five rows before Show more and never draws unreported progress", () => {
  const live = [epic("counted", { counts: { open: 4, in_progress: 2, merged: 6, total: 12 }, blocked: 1 }), ...["b", "c", "d", "e", "f", "g"].map(key => epic(key))];
  const render = () => liveEpicsSection(document, { project: "cas-src", live, quiet: 7, multiMachine: false, explorerHref: "https://cloud.test/explorer/tasks?project_id=p", pending: [], failed: [], retry: () => {} })!;
  let section = render(); document.body.replaceChildren(section);
  expect(section.querySelector("h3")?.textContent).toContain("7");
  const [first, second] = section.querySelectorAll<HTMLButtonElement>(".live-epic-toggle");
  expect(first!.textContent).toContain("6/12");
  expect(first!.textContent).toMatch(new RegExp("(?=[\\s\\S]*4)(?=[\\s\\S]*2)(?=[\\s\\S]*6)(?=[\\s\\S]*1)"));
  expect(first!.querySelector(".split-bar")?.getAttribute("aria-label")).toBeTruthy();
  // Missing counts: no bar, no placeholder ratio, the in-flight count still shows.
  expect(second!.querySelector(".split-bar")).toBeNull();
  expect(second!.textContent).toBeTruthy();
  expect(section.textContent).not.toMatch(/[—−-]\/[—−-]/);
  const panel = section.querySelector<HTMLElement>(`#${first!.getAttribute("aria-controls")}`)!;
  expect([first!.getAttribute("aria-expanded"), panel.hidden]).toEqual(["false", true]);
  first!.click();
  expect([first!.getAttribute("aria-expanded"), panel.hidden]).toEqual(["true", false]);
  expect(panel.textContent).toBeTruthy();
  expect(panel.textContent).not.toContain("counted-task");
  expect(panel.querySelector("details")?.open).toBe(false);
  expect(panel.querySelector("summary")?.textContent).toContain("6");
  expect([...section.querySelectorAll<HTMLElement>(".live-epic")].filter(row => !row.hidden)).toHaveLength(5);
  section.querySelector<HTMLButtonElement>(".live-epics-more")!.click();
  expect([...section.querySelectorAll<HTMLElement>(".live-epic")].filter(row => !row.hidden)).toHaveLength(7);
  expect(section.querySelector(".live-epics-footer a")?.textContent).toContain("7");
  // A status re-render keeps the disclosure and the full list.
  section = render();
  expect(section.querySelector(".live-epic-toggle")?.getAttribute("aria-expanded")).toBe("true");
  expect(section.querySelector(".live-epics-more")).toBeNull();
  expect(liveEpicsSection(document, { project: "other", live: [], quiet: 0, multiMachine: false, pending: [], failed: [], retry: () => {} })).toBeUndefined();
});

it("draws the split and the task sections from one task state, so a merged delivery leaves In progress", async () => {
  const { projectEpics } = await import("./progress-model");
  const { live } = projectEpics([{ project: "cas-src", machine: "prowl", machineLabel: "prowl", session: "s", supervisor: "sup", roster: ["worker"], stale: false, received: 1,
    // The worker still reports executing on a task whose delivery has merged.
    agents: [{ name: "worker", status: "active", current_task: "landed", liveness: "executing" }],
    tasks: [{ id: "landed", epic_id: "e", title: "Landed", status: "awaiting_merge", delivery_state: "merged", assignee: "worker" },
      { id: "work", epic_id: "e", title: "Work", status: "in_progress" }, { id: "next", epic_id: "e", title: "Next", status: "open" }],
    epics: [{ id: "e", title: "Epic", status: "in_progress", counts: { open: 1, in_progress: 2, merged: 2 } }] }], "cas-src");
  const section = liveEpicsSection(document, { project: "split", live, quiet: 0, multiMachine: false, pending: [], failed: [], retry: () => {} })!;
  expect(section.querySelector(".live-epic-split")?.textContent).toMatch(new RegExp("(?=[\\s\\S]*1)(?=[\\s\\S]*3)"));
  expect(section.querySelector(".live-epic-ratio")?.textContent).toBe("3/5");
  expect([...section.querySelectorAll(".live-epic-group-label")].map(label => label.textContent)).toEqual(["In progress · 1", "Open · 1", "Merged · 3"]);
  expect(section.querySelector(".live-epic-merged")?.textContent).toContain("Landed");
});
