// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ProjectNavigation } from "./project-navigation";
import type { ConversationRow } from "./conversation-list";

const row = (key: string, extra: Partial<ConversationRow> = {}): ConversationRow => ({ key, session: key, machineId: "soundwave", host: "soundwave · Linux", projectDir: "/projects/cassy", supervisor: key, connection: "Live", freshness: "", attention: 0, selected: false, ...extra });
function setup() {
  document.body.innerHTML = '<div id="conversation-filters"></div><nav id="conversation-list"></nav>';
  return document.querySelector<HTMLElement>("nav")!;
}
beforeEach(() => localStorage.clear());

describe("project-first navigation", () => {
  it("contains supervisors across machines in one project and renders only supplied identity", () => {
    const nav = setup();
    new ProjectNavigation().render(nav, [row("nimble-fox-66", { project: "cassy", harness: "codex", account_email: "operator@example.test", machine_online: true, workerCount: 2, activityAt: 2 }), row("mighty-eagle-86", { machineId: "prowl", host: "prowl · macOS", projectDir: "/other/cassy", machine_online: false, activityAt: 1 }), row("solo", { projectDir: "/projects/pulse-card" })], vi.fn());
    const cards = nav.querySelectorAll(".project-card");
    expect(cards).toHaveLength(2);
    expect(cards[0]!.querySelectorAll(".conversation-row")).toHaveLength(2);
    expect(cards[0]!.querySelectorAll(".project-machine-chip")).toHaveLength(2);
    expect(cards[0]!.textContent).toContain("2 live workers");
    expect(cards[0]!.querySelector('[data-harness="codex"]')?.textContent).toBe(">_");
    expect(cards[0]!.querySelector(".conversation-account")?.textContent).toBe("operator@example.test");
    expect(cards[1]!.querySelector(".conversation-account")).toBeNull();
    expect(cards[1]!.textContent).not.toMatch(/undefined|Unknown harness/);
  });

  it("keeps a collapsed project on this device and retains the toggle through a catalog tick", () => {
    const nav = setup(); const list = new ProjectNavigation(); const rows = [row("one")];
    list.render(nav, rows, vi.fn());
    const toggle = nav.querySelector<HTMLButtonElement>(".project-card-toggle")!;
    toggle.focus(); toggle.click();
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(nav.querySelector<HTMLElement>(".project-card-rows")!.hidden).toBe(true);
    list.render(nav, rows, vi.fn());
    expect(document.activeElement).toBe(toggle);
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
