// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ConversationHistory, waitingQuestion } from "./conversation-history";
import { ProjectNavigation } from "./project-navigation";
import { isNewerRelease, machineVersionMarkup } from "./machine-version";
import type { ConversationRow } from "./conversation-list";

const row = (key: string, extra: Partial<ConversationRow> = {}): ConversationRow => ({ key, session: key, machineId: "soundwave", host: "soundwave · Linux", projectDir: "/projects/cassy", supervisor: key, connection: "Live", freshness: "", attention: 0, selected: false, ...extra });
function setup() {
  document.body.innerHTML = '<div id="conversation-filters"></div><nav id="conversation-list"></nav>';
  return document.querySelector<HTMLElement>("nav")!;
}
beforeEach(() => localStorage.clear());

describe("project-first navigation", () => {
  it("uses the first message sentence for legacy code summaries in Needs you (cas-ef6e)", () => {
    const first = "lively-stork-7 continued while ready-cobra-72 was unavailable.";
    const message = `${first} Review the merged work.`;
    for (const [summary, body, expected] of [
      ["peer_ask_override", message, first],
      ["reason_42", `${first}\nMore context`, first],
      ["", message, first],
      ["Review this merge", message, "Review this merge"],
      ["peer_ask_override", "", undefined],
      ["peer_ask_override", "peer_ask_override", undefined],
    ] as const) {
      const history = new ConversationHistory();
      history.reply({ notification_id: 1, reply_to: null, summary, message: body, device_id: "*", kind: "blocker" });
      const question = waitingQuestion(history);
      expect(question).toBe(expected);
      const nav = setup();
      new ProjectNavigation().render(nav, [row("override", { attention: history.waiting().length, operatorQuestion: question })], vi.fn());
      expect(nav.querySelector(".needs-you-question")?.textContent).toBe(expected);
      expect(nav.querySelector(".needs-you-shortcut")).not.toBeNull();
    }
  });
  it("keeps Reload Cassy on skewed cards without adding conversation Tab stops, with honest receipts", () => {
    const nav = setup();
    const navigation = new ProjectNavigation();
    const reload = vi.fn();
    const open = vi.fn();
    for (const harness of ["claude", "codex", "grok", undefined] as const) {
      const supervisor = row("reload", { harness, machine_online: true, mcp_version: "4.0.0", installed_version: "4.1.0" });
      navigation.render(nav, [supervisor], open, undefined, { reload });
      const action = nav.querySelector<HTMLButtonElement>('[data-reload-cassy="direct"]')!;
      expect(action).not.toBeNull();
      expect(action.hidden).toBe(false);
      expect(action.tabIndex, "keyboard actions stay in the conversation header menu").toBe(-1);
      expect(action.disabled).toBe(harness !== "claude");
      if (harness === "claude") {
        action.click(); expect(reload).toHaveBeenLastCalledWith(expect.objectContaining({ key: supervisor.key }));
        action.focus(); navigation.render(nav, [supervisor], open, undefined, { reload });
        expect(document.activeElement).toBe(action);
      } else expect(nav.textContent).toBeTruthy();
    }
    const current = row("reload", { harness: "claude", machine_online: true, mcp_version: "4.1.0", installed_version: "4.1.0", reloadReceipt: { client_ref: "r", command: "mcp", phase: "observed", detail: "Cassy reloaded: PID 2202, version 4.1.0", output: "", interactive: false } });
    navigation.render(nav, [current], open, undefined, { reload });
    expect(nav.querySelector<HTMLButtonElement>('[data-reload-cassy="direct"]')!.hidden).toBe(true);
    const body = nav.querySelector(".project-card-rows")!;
    const stops = [...body.querySelectorAll<HTMLElement>("button, summary, [tabindex]")]
      .filter(node => node.tabIndex >= 0 && !node.closest("[hidden]"));
    expect(stops).toEqual([...body.querySelectorAll(".conversation-row")]);
    expect(nav.querySelector(".conversation-reload-receipt")?.textContent).toContain("version 4.1.0");
    navigation.render(nav, [{ ...current, reloadReceipt: undefined }], open, undefined, { reload });
    expect(nav.querySelector<HTMLElement>(".conversation-reload")!.hidden, "no empty action line on current cards").toBe(true);
    expect(open).not.toHaveBeenCalled();
  });
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
  it("names a hub still running an older binary than the installed one, and offers only releases beyond it (cas-d786)", () => {
    const shown = (runtime: string, latest: string, installed?: string) => {
      const span = document.createElement("span");
      span.innerHTML = machineVersionMarkup(runtime, latest, undefined, installed);
      return span.textContent;
    };
    for (const [runtime, latest, installed, text] of [
      ["9.99.0", "9.99.2", "9.99.2", "Hub 9.99.0 · installed 9.99.2 Run cas hub restart"],
      ["9.99.0", "9.99.3", "9.99.2", "Hub 9.99.0 · installed 9.99.2 Run cas hub restart Update available → 9.99.3"],
      ["9.99.2", "9.99.2", "9.99.2", "Cassy 9.99.2"],
      ["9.99.2", "9.99.3", "9.99.2", "Cassy 9.99.2 Update available → 9.99.3"],
      ["9.99.0", "9.99.2", undefined, "Cassy 9.99.0 Update available → 9.99.2"],
    ] as const) expect(shown(runtime, latest, installed), `${runtime}/${installed}/${latest}`).toBe(text);
  });
  it("contains supervisors across machines in one project and renders only supplied identity", () => {
    const nav = setup();
    new ProjectNavigation().render(nav, [row("nimble-fox-66", { project: "cassy", harness: "codex", account_email: "operator@example.test", supervisorState: "working", machine_online: true, workerCount: 2, activityAt: 2, statusLine: { train: { open: 3, in_progress: 1, merged: 1, total: 5 }, blocked: 1 } }), row("mighty-eagle-86", { machineId: "prowl", host: "prowl · macOS", projectDir: "/other/cassy", machine_online: false, activityAt: 1 }), row("solo", { projectDir: "/projects/pulse-card", machine_online: true, workerCount: 0 }), row("ended", { projectDir: "/projects/pulse-card", machine_online: false, workerCount: 0, ended: { at: "2026-10-29T10:00:00Z", reason: "prompt_input_exit", workers_stopped: [], orphans: [] } })], vi.fn(), undefined, { machines: [{ id: "soundwave", label: "soundwave · Linux", online: true, runtime: "4.3.0", installed: "4.3.0" }, { id: "prowl", label: "prowl · macOS", online: false, runtime: "4.2.0", installed: "4.3.0" }] });
    const cards = nav.querySelectorAll(".project-card");
    expect(cards).toHaveLength(2);
    expect(cards[0]!.querySelectorAll(".conversation-row")).toHaveLength(2);
    expect(cards[0]!.querySelectorAll(".project-machine-chip")).toHaveLength(2);
    expect(cards[0]!.querySelectorAll(".project-machine-chip")[1]!.getAttribute("aria-label")).toContain("prowl");
    // cas-da546: one status line — the train, workers and blocked — and a version only on skew.
    expect(cards[0]!.querySelector(".project-card-summary")?.textContent).toContain("1/5");
    expect(cards[0]!.querySelector(".project-card-summary .split-bar")).not.toBeNull();
    expect(cards[0]!.querySelector(".project-card-blocked")).not.toBeNull();
    expect([...cards[0]!.querySelectorAll(".project-machine-chip")].map(chip => chip.querySelector(".machine-version") !== null)).toEqual([false, true]);
    expect(cards[0]!.classList.contains("is-quiet")).toBe(false);
    // DESIGN.md Avatars: the provider's drawn mark, never a tiny glyph.
    expect(cards[0]!.querySelector('[data-harness="codex"]')?.getAttribute("title")).toBe("OpenAI");
    expect(cards[0]!.querySelector('[data-harness="codex"] svg.provider-mark')).not.toBeNull();
    expect(cards[0]!.querySelector('[data-thread-key="nimble-fox-66"]')?.getAttribute("aria-label")).toContain("operator@example.test");
    expect(cards[1]!.textContent).not.toMatch(/undefined|Unknown harness/);
    // A quiet project without a reported train steps back.
    expect(cards[1]!.classList.contains("is-quiet")).toBe(true);
    expect(cards[1]!.querySelector(".project-machine-chip")?.getAttribute("aria-label")).toContain("soundwave");
    // Both provider marks and machine-monogram fallbacks expose the same state contract.
    for (const [key, state] of [["nimble-fox-66", "Working"], ["solo", "Idle"], ["mighty-eagle-86", "Offline"]]) {
      const dot = nav.querySelector(`[data-thread-key="${key}"] .supervisor-state`)!;
      expect(dot.classList.contains(state!.toLowerCase())).toBe(true);
      expect(dot.getAttribute("aria-label")).toBe(state);
      expect(dot.getAttribute("title")).toBe(state);
      expect(dot.getAttribute("role")).toBe("img");
      expect(dot.closest('[aria-hidden="true"]')).toBeNull();
    }
  });

  it("labels project rows with the supervisor name, falling back only when it is absent", () => {
    const nav = setup();
    new ProjectNavigation().render(nav, [
      row("named", { projectDir: "/projects/cassy", supervisor: "ready-cobra-72" }),
      row("unnamed", { projectDir: "/projects/legacy", supervisor: "supervisor" }),
      row("blank", { projectDir: "/projects/blank", supervisor: "" }),
    ], vi.fn());

    const role = (key: string) => nav.querySelector(`[data-thread-key="${key}"] .conversation-role`)?.textContent;
    expect(role("named")).toBe("ready-cobra-72");
    expect(role("unnamed")).toBe("Supervisor");
    expect(role("blank")).toBe("Supervisor");
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
    navigation.render(nav, rows(1), vi.fn(), undefined, { machines });
    const first = layout();
    expect(first).toEqual([["Atlas", "amber-fox-1"], ["cassy", "bright-lynx-4", "quiet-heron-7"], ["widget", "zesty-otter-2"]]);
    const nodes = [...nav.querySelectorAll(".conversation-row")];
    const recent = () => nav.querySelector<HTMLElement>('[data-most-recent="true"]')?.dataset.threadKey;
    expect(recent()).toBe("bright-lynx-4");
    const observer = new MutationObserver(() => {});
    observer.observe(nav, { subtree: true, attributes: true, attributeFilter: ["data-most-recent"], attributeOldValue: true });
    let badgeChanges = 0;
    for (let tick = 1; tick <= 4; tick++) {
      navigation.render(nav, tick % 2 ? [...rows(tick)].reverse() : rows(tick), vi.fn(), undefined, { machines });
      badgeChanges += observer.takeRecords().filter(record => record.oldValue !== (record.target as Element).getAttribute("data-most-recent")).length;
      expect(layout(), `update ${tick}`).toEqual(first);
      expect([...nav.querySelectorAll(".conversation-row")], `update ${tick} keeps each row's node in place`).toEqual(nodes);
    }
    observer.disconnect();
    expect(badgeChanges, "alternating activity within a minute never changes the badge (cas-b6ed)").toBe(0);
    expect(recent()).toBe("bright-lynx-4");
    navigation.render(nav, rows(4).map(item => item.key === "quiet-heron-7" ? { ...item, activityAt: 60_000 } : item), vi.fn(), undefined, { machines });
    expect(recent()).toBe("quiet-heron-7");
    expect(layout()).toEqual(first);
    expect([...nav.querySelectorAll(".conversation-row")]).toEqual(nodes);
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

  it("hides ended rows and removes the last project's card without dismissal (cas-f8ab)", () => {
    const nav = setup(); const list = new ProjectNavigation();
    const ended = row("gone", { ended: { at: "2026-11-01T10:00:00Z", reason: "exit", workers_stopped: [], orphans: [] } });
    list.render(nav, [row("running"), ended], vi.fn());
    expect(nav.querySelectorAll(".conversation-row")).toHaveLength(1);
    list.render(nav, [ended], vi.fn());
    expect(nav.querySelectorAll(".project-card")).toHaveLength(0);
    list.render(nav, [row("running")], vi.fn());
    expect(nav.querySelectorAll(".project-card")).toHaveLength(1);
  });

  it("pins operator decisions with their question and intersects the Needs you and machine filters with the supplied search results", () => {
    const nav = setup(); const list = new ProjectNavigation(); const open = vi.fn();
    const rows = [row("answer-me", { attention: 1, operatorQuestion: "**Ship now**, or hold?" }), row("merge-me", { machineId: "prowl", host: "prowl", needsYou: true, operatorReason: "Ready for your merge" }), row("only-unread", { unread: 2 })];
    list.render(nav, rows, open);
    const shortcuts = [...nav.querySelectorAll(".needs-you-shortcut")];
    expect(shortcuts.map(node => node.querySelector("q, small")?.textContent)).toEqual(["Ship now, or hold?", "Ready for your merge"]);
    (shortcuts[0] as HTMLButtonElement).click();
    expect(open.mock.calls[0]![0].session).toBe("answer-me");
    const needs = () => document.querySelector<HTMLButtonElement>(".needs-you-filter")!;
    expect(needs().textContent).toBe("Needs you 2");
    needs().click();
    expect(nav.querySelectorAll(".conversation-row")).toHaveLength(2);
    expect(needs().getAttribute("aria-pressed")).toBe("true");
    needs().click();
    expect(nav.querySelectorAll(".conversation-row")).toHaveLength(3);
    nav.querySelector<HTMLButtonElement>('.project-machine-chip[data-project-machine="prowl"]')!.click();
    expect(nav.querySelectorAll(".conversation-row")).toHaveLength(1);
    expect((nav.querySelector(".conversation-row") as HTMLElement).dataset.threadKey).toBe("merge-me");
    list.render(nav, [rows[0]!], open);
    expect(nav.querySelector(".project-nav-empty")?.textContent).toBeTruthy();
    document.querySelector<HTMLButtonElement>(".machine-filter")!.click();
    list.render(nav, [row("only-unread", { unread: 2 })], open);
    expect(nav.querySelector(".needs-you-section")).toBeNull();
    expect(document.querySelector<HTMLElement>("#conversation-filters")!.hidden).toBe(true);
  });
});
