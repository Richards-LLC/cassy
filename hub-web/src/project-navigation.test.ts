// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ProjectNavigation } from "./project-navigation";
import { isNewerRelease } from "./machine-version";
import type { ConversationRow } from "./conversation-list";

const row = (key: string, extra: Partial<ConversationRow> = {}): ConversationRow => ({ key, session: key, machineId: "soundwave", host: "soundwave · Linux", projectDir: "/projects/cassy", supervisor: key, connection: "Live", freshness: "", attention: 0, selected: false, ...extra });
function setup() {
  document.body.innerHTML = '<div id="conversation-filters"></div><nav id="conversation-list"></nav>';
  return document.querySelector<HTMLElement>("nav")!;
}
beforeEach(() => localStorage.clear());

describe("project-first navigation", () => {
  it("compares published versions numerically without inventing updates for unknown runtimes", () => {
    for (const [current, latest, newer] of [
      ["3.9.0", "v3.10.0", true], ["3.87.0", "3.87.1", true],
      ["3.87.0", "4.0.0", true], ["3.88.0", "3.88.0", false],
      ["3.89.0", "3.88.0", false], ["3.88.0-rc.1", "3.88.0", true],
      ["3.88.0+debug", "3.88.0", false], ["unknown", "3.88.0", false],
      [undefined, "3.88.0", false], ["3.87.0", undefined, false],
      ["3.87.0", "invalid", false], ["3.87.0", "3.88.0-rc.1", false],
    ] as const) expect(isNewerRelease(current, latest), `${current} → ${latest}`).toBe(newer);
  });
  it("contains supervisors across machines in one project and renders only supplied identity", () => {
    const nav = setup();
    new ProjectNavigation().render(nav, [row("nimble-fox-66", { project: "cassy", harness: "codex", account_email: "operator@example.test", machine_online: true, workerCount: 2, activityAt: 2 }), row("mighty-eagle-86", { machineId: "prowl", host: "prowl · macOS", projectDir: "/other/cassy", machine_online: false, activityAt: 1 }), row("solo", { projectDir: "/projects/pulse-card", machine_online: true, workerCount: 0 }), row("ended", { projectDir: "/projects/pulse-card", machine_online: false, workerCount: 0, ended: { at: "2026-10-29T10:00:00Z", reason: "prompt_input_exit", workers_stopped: [], orphans: [] } })], vi.fn(), undefined, { machines: [{ id: "soundwave", label: "soundwave · Linux", online: true }, { id: "prowl", label: "prowl · macOS", online: false }] });
    const cards = nav.querySelectorAll(".project-card");
    expect(cards).toHaveLength(2);
    expect(cards[0]!.querySelectorAll(".conversation-row")).toHaveLength(2);
    expect(cards[0]!.querySelectorAll(".project-machine-chip")).toHaveLength(2);
    expect(cards[0]!.querySelectorAll(".project-machine-chip")[1]!.getAttribute("aria-label")).toBe("Filter conversations on prowl, offline");
    expect(cards[0]!.textContent).toContain("2 live workers");
    // DESIGN.md Avatars: the provider's drawn mark, never a tiny glyph.
    expect(cards[0]!.querySelector('[data-harness="codex"]')?.getAttribute("title")).toBe("OpenAI");
    expect(cards[0]!.querySelector('[data-harness="codex"] svg.provider-mark')).not.toBeNull();
    expect(cards[0]!.querySelector(".conversation-account")?.textContent).toBe("operator@example.test");
    expect(cards[1]!.querySelector(".conversation-account")).toBeNull();
    expect(cards[1]!.textContent).not.toMatch(/undefined|Unknown harness/);
    expect(cards[1]!.querySelector<HTMLElement>(".project-card-summary")?.hidden).toBe(true);
    expect(cards[1]!.querySelector(".project-machine-chip")?.getAttribute("aria-label")).toBe("Filter conversations on soundwave, online");
  });

  it("orders cards by project name and rows by supervisor, and activity never moves either (cas-ade2)", () => {
    const nav = setup();
    const navigation = new ProjectNavigation();
    const machines = [{ id: "soundwave", label: "soundwave · Linux", online: true }, { id: "prowl", label: "prowl · macOS", online: true }];
    const rows = (tick: number): ConversationRow[] => [
      row("zesty-otter-2", { projectDir: "/projects/widget", activityAt: tick % 2 ? 9 : 1 }),
      row("quiet-heron-7", { project: "cas-src", projectName: "cassy", machineId: "prowl", host: "prowl · macOS", activityAt: tick % 2 ? 1 : 9 }),
      row("bright-lynx-4", { project: "cas-src", projectName: "cassy", activityAt: tick % 2 ? 9 : 1 }),
      row("amber-fox-1", { projectDir: "/projects/Atlas", activityAt: tick }),
    ];
    const layout = () => [...nav.querySelectorAll<HTMLElement>(".project-card")].map((card) => [
      card.querySelector(".project-card-name")!.textContent,
      ...[...card.querySelectorAll<HTMLElement>(".conversation-row")].map((node) => node.dataset.threadKey),
    ]);
    navigation.render(nav, rows(0), vi.fn(), undefined, { machines });
    const first = layout();
    expect(first).toEqual([["Atlas", "amber-fox-1"], ["cassy", "bright-lynx-4", "quiet-heron-7"], ["widget", "zesty-otter-2"]]);
    const nodes = [...nav.querySelectorAll(".conversation-row")];
    for (let tick = 1; tick <= 4; tick++) {
      navigation.render(nav, tick % 2 ? [...rows(tick)].reverse() : rows(tick), vi.fn(), undefined, { machines });
      expect(layout(), `update ${tick}`).toEqual(first);
      expect([...nav.querySelectorAll(".conversation-row")], `update ${tick} keeps each row's node in place`).toEqual(nodes);
    }
  });

  it("names a project card by its display name, else its folder, never its identity (cas-6f79)", () => {
    const nav = setup();
    new ProjectNavigation().render(nav, [row("named", { project: "cas-src", projectName: "Cassy", projectDir: "/home/me/cassy" }), row("unnamed", { project: "github.com/acme/widget", projectDir: "/projects/widget" })], vi.fn());
    const names = [...nav.querySelectorAll<HTMLElement>(".project-card-name")];
    expect(names.map((name) => name.textContent)).toEqual(["Cassy", "widget"]);
    expect(names.map((name) => name.title)).toEqual(["cas-src", "github.com/acme/widget"]);
  });

  it("keeps a collapsed project on this device and retains focused controls through a catalog tick and search", () => {
    const nav = setup(); const list = new ProjectNavigation(); const rows = [row("one")];
    list.render(nav, rows, vi.fn());
    const toggle = nav.querySelector<HTMLButtonElement>(".project-card-toggle")!;
    toggle.focus(); toggle.click();
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(nav.querySelector<HTMLElement>(".project-card-rows")!.hidden).toBe(true);
    list.render(nav, rows, vi.fn());
    expect(document.activeElement).toBe(toggle);
    const chip = nav.querySelector<HTMLButtonElement>(".project-machine-chip")!;
    chip.focus();
    list.render(nav, rows, vi.fn());
    expect(document.activeElement).toBe(chip);
    list.render(nav, [], vi.fn(), undefined, { searchActive: true });
    list.render(nav, rows, vi.fn());
    expect(nav.querySelector(".project-card-toggle")).toBe(toggle);
    new ProjectNavigation().render(nav, rows, vi.fn());
    expect(nav.querySelector(".project-card-toggle")?.getAttribute("aria-expanded")).toBe("false");
  });

  it("pins operator decisions and intersects the Needs you and machine filters with the supplied search results", () => {
    const nav = setup(); const list = new ProjectNavigation(); const open = vi.fn();
    const rows = [row("answer-me", { attention: 1 }), row("merge-me", { machineId: "prowl", host: "prowl", needsYou: true }), row("only-unread", { unread: 2 })];
    list.render(nav, rows, open);
    expect(nav.querySelectorAll(".needs-you-shortcut")).toHaveLength(2);
    nav.querySelector<HTMLButtonElement>(".needs-you-shortcut")!.click();
    expect(open.mock.calls[0]![0].session).toBe("answer-me");
    document.querySelector<HTMLButtonElement>('[data-nav-filter="needs-you"]')!.click();
    expect(nav.querySelectorAll(".conversation-row")).toHaveLength(2);
    document.querySelector<HTMLButtonElement>('[data-nav-filter="machine:prowl"]')!.click();
    expect(nav.querySelectorAll(".conversation-row")).toHaveLength(1);
    expect(nav.querySelector(".conversation-row")?.textContent).toContain("merge-me");
    list.render(nav, [rows[0]!], open);
    expect(nav.querySelector(".project-nav-empty")?.textContent).toContain("All");
    document.querySelector<HTMLButtonElement>('[data-nav-filter="all"]')!.click();
    list.render(nav, [row("only-unread", { unread: 2 })], open);
    expect(nav.querySelector(".needs-you-section")).toBeNull();
  });
});
