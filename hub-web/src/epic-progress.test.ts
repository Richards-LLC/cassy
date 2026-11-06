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
  const sessions = new Map([["soundwave", [{ name: "a", project_dir: "/p/cas-src" }, { name: "b", project_dir: "/p/gabber" }]], ["prowl", [{ name: "c", project: "cas-src" }]]]);
  const key = (_: string, session: { project?: string; project_dir?: string }) => session.project ?? session.project_dir!.split("/").at(-1)!;
  expect(projectSessions(sessions, "cas-src", key).map(item => `${item.machineId}:${item.session.name}`)).toEqual(["soundwave:a", "prowl:c"]);
  expect(projectSessions(sessions, undefined, key)).toEqual([]);
});

it("discloses tasks in place, shows five rows before Show more and never draws unreported progress", () => {
  const live = [epic("counted", { counts: { open: 4, in_progress: 2, merged: 6, total: 12 }, blocked: 1 }), ...["b", "c", "d", "e", "f", "g"].map(key => epic(key))];
  const render = () => liveEpicsSection(document, { project: "cas-src", live, quiet: 7, multiMachine: false, explorerHref: "https://cloud.test/explorer/tasks?project_id=p", pending: [], failed: [], retry: () => {} })!;
  let section = render(); document.body.replaceChildren(section);
  expect(section.querySelector("h3")?.textContent).toBe("Live epics · 7");
  const [first, second] = section.querySelectorAll<HTMLButtonElement>(".live-epic-toggle");
  expect(first!.textContent).toContain("6/12");
  expect(first!.textContent).toContain("4 open · 2 in progress · 6 merged · 1 blocked");
  expect(first!.querySelector(".split-bar")?.getAttribute("aria-label")).toBe("counted: 4 open · 2 in progress · 6 merged");
  // Missing counts: no bar, no placeholder ratio, the in-flight count still shows.
  expect(second!.querySelector(".split-bar")).toBeNull();
  expect(second!.textContent).toBe("b1 building");
  expect(section.textContent).not.toMatch(/[—−-]\/[—−-]/);
  const panel = section.querySelector<HTMLElement>(`#${first!.getAttribute("aria-controls")}`)!;
  expect([first!.getAttribute("aria-expanded"), panel.hidden]).toEqual(["false", true]);
  first!.click();
  expect([first!.getAttribute("aria-expanded"), panel.hidden]).toEqual(["true", false]);
  expect(panel.textContent).toContain("Build counted");
  expect(panel.textContent).not.toContain("counted-task");
  expect(panel.querySelector("details")?.open).toBe(false);
  expect(panel.querySelector("summary")?.textContent).toBe("Merged · 6");
  expect([...section.querySelectorAll<HTMLElement>(".live-epic")].filter(row => !row.hidden)).toHaveLength(5);
  section.querySelector<HTMLButtonElement>(".live-epics-more")!.click();
  expect([...section.querySelectorAll<HTMLElement>(".live-epic")].filter(row => !row.hidden)).toHaveLength(7);
  expect(section.querySelector(".live-epics-footer a")?.textContent).toBe("7 quiet open epics in Explorer");
  // A status re-render keeps the disclosure and the full list.
  section = render();
  expect(section.querySelector(".live-epic-toggle")?.getAttribute("aria-expanded")).toBe("true");
  expect(section.querySelector(".live-epics-more")).toBeNull();
  expect(liveEpicsSection(document, { project: "other", live: [], quiet: 0, multiMachine: false, pending: [], failed: [], retry: () => {} })).toBeUndefined();
});
