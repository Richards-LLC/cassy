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
    new ProjectNavigation().render(nav, [row("nimble-fox-66", { project: "cassy", harness: "codex", account_email: "operator@example.test", machine_online: true, workerCount: 2, activityAt: 2 }), row("mighty-eagle-86", { machineId: "prowl", host: "prowl · macOS", projectDir: "/other/cassy", machine_online: false, activityAt: 1 }), row("solo", { projectDir: "/projects/pulse-card" })], vi.fn());
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
  });

  it("keeps two active supervisors of one project in place and the selection pinned while they trade turns (cas-4f42)", () => {
    const nav = setup();
    const navigation = new ProjectNavigation();
    const tick = (calm: number, kestrel: number) => navigation.render(nav, [
      row("calm-swan-15", { project: "cas-src", startedAt: 1, activityAt: calm, selected: true }),
      row("ready-kestrel-84", { project: "cas-src", machineId: "prowl", host: "prowl · macOS", startedAt: 2, activityAt: kestrel }),
    ], vi.fn());
    tick(10, 5);
    const nodes = [...nav.querySelectorAll<HTMLElement>(".conversation-row")];
    const marked = () => [...nav.querySelectorAll(".conversation-row")].map((node) => node.querySelector(".conversation-session-mark") !== null);
    expect(marked()).toEqual([true, false]);
    for (const [calm, kestrel] of [[10, 20], [30, 20], [30, 40]]) {
      tick(calm, kestrel);
      // Same nodes, same order: nothing moves, so nothing flashes.
      expect([...nav.querySelectorAll<HTMLElement>(".conversation-row")]).toEqual(nodes);
      expect(nodes[0]!.getAttribute("aria-current")).toBe("true");
      expect(marked()).toEqual([calm > kestrel, kestrel > calm]);
    }
  });
  it("keeps a collapsed project on this device and retains its keyed toggle through a catalog tick and search", () => {
    const nav = setup(); const list = new ProjectNavigation(); const rows = [row("one")];
    list.render(nav, rows, vi.fn());
    const toggle = nav.querySelector<HTMLButtonElement>(".project-card-toggle")!;
    toggle.focus(); toggle.click();
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(nav.querySelector<HTMLElement>(".project-card-rows")!.hidden).toBe(true);
    list.render(nav, rows, vi.fn());
    expect(document.activeElement).toBe(toggle);
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
