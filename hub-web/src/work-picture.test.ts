// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { WorkEventStream, workEventSupport, workEventsPage } from "./work-events";
import { WorkPicture, type WorkEventTransport } from "./work-picture";
import { renderWork, workSignature } from "./work-rows-view";

const wire = (id: string, second: number, extra: Record<string, unknown> = {}) => ({ id, kind: "tool", session: "s", at: `2026-10-27T12:00:${String(second).padStart(2, "0")}Z`, label: id, has_detail: true, ...extra });

describe("work events (cas-ff22)", () => {
  it("merges pages by id and reads back from a newest page that missed the loaded events", () => {
    const stream = new WorkEventStream();
    stream.requested("first", "newest");
    stream.accept(workEventsPage({ request_id: "first", events: [wire("a", 1), wire("b", 2)], has_earlier: true, next_before: "c1" })!);
    expect(stream.cursor).toBe("c1");
    stream.requested("next", "newest");
    // Two events arrived after the loaded ones, and more before them: a gap.
    const gap = stream.accept(workEventsPage({ request_id: "next", events: [wire("e", 5), wire("f", 6)], has_earlier: true, next_before: "c2" })!);
    expect(gap).toBe("c2");
    stream.requested("gap", "gap");
    expect(stream.accept(workEventsPage({ request_id: "gap", events: [wire("b", 2), wire("d", 4)], has_earlier: true, next_before: "c3" })!)).toBeUndefined();
    expect(stream.events.map((event) => event.id)).toEqual(["a", "b", "d", "e", "f"]);
    expect(stream.cursor, "the oldest page's cursor is kept").toBe("c1");
    expect(workEventsPage({ request_id: "x", events: [wire("odd", 1, { kind: "telepathy" })], has_earlier: false })!.events[0]!.kind).toBe("notice");
    expect([workEventSupport(["work_events"]), workEventSupport(["work_events", "work_events_live"]), workEventSupport([])]).toEqual(["pull", "live", "none"]);
  });

  it("preserves optional v2 families while keeping legacy and malformed rows readable", () => {
    const extensions = {
      tool: { name: "Read", operation: "read", targets: ["a.ts"], counts: { files: 1 } },
      lifecycle: { id: "call-1", stage: "end", outcome: "success" },
      reasoning: { summary: "Published", source: "codex_summary" },
      changed_files: { files: [{ path: "a.ts", operation: "edit", source_event_id: "a" }], provenance: "successful_tool_calls", complete: false },
    };
    const page = workEventsPage({ request_id: "v2", schema_version: 2, events: [
      wire("a", 1, extensions), wire("legacy", 2),
      wire("bad", 3, { tool: { name: 4 }, lifecycle: null, reasoning: [], changed_files: { files: "bad" } }),
      wire("future", 4, { tool: { name: "future", operation: "teleport", targets: [] }, lifecycle: { id: "f", stage: "future" } }),
    ] })!;
    expect(page.events[0]).toMatchObject(extensions);
    expect(page.events.map((event) => event.label)).toEqual(["a", "legacy", "bad", "future"]);
    expect(page.events[2]).not.toHaveProperty("tool");
    expect(page.events[3]).toMatchObject({ tool: { operation: "teleport" }, lifecycle: { stage: "future" } });
  });

  it("reads nothing while Show work is off, polls only a daemon without change notices, and re-reads on a notice", () => {
    let support: "pull" | "live" = "live";
    const transport: WorkEventTransport = {
      workEvents: () => support,
      requestWorkEvents: vi.fn(() => `r${Math.random()}`),
      requestWorkEventDetail: vi.fn(() => "d1"),
    };
    const storage = new Map<string, string>();
    const setInterval = vi.fn(() => 7);
    const picture = new WorkPicture({ transport: () => transport, repaintThread: vi.fn(), repaintTimeline: vi.fn(), storage: { getItem: (key) => storage.get(key) ?? null, setItem: (key, value) => void storage.set(key, value) }, setInterval, clearInterval: vi.fn() });
    picture.watchThread("m", "s");
    expect(transport.requestWorkEvents).not.toHaveBeenCalled();
    expect(picture.threadWork("m", "s")).toBeUndefined();
    picture.setShowWork(true);
    expect(storage.get("commander.showWork")).toBe("1");
    expect(transport.requestWorkEvents).toHaveBeenCalledTimes(1);
    picture.appended("m", "s");
    expect(transport.requestWorkEvents, "a page already in flight is not asked for twice").toHaveBeenCalledTimes(1);
    expect(setInterval, "a live daemon pushes; nothing polls").not.toHaveBeenCalled();
    support = "pull";
    picture.watchThread("m", "s");
    expect(setInterval).toHaveBeenCalledTimes(1);
  });

  it("re-reads a thread's newest work when the operator comes back to it", () => {
    const transport: WorkEventTransport = { workEvents: () => "live", requestWorkEvents: vi.fn((session: string) => `${session}-${Math.random()}`), requestWorkEventDetail: vi.fn(() => undefined) };
    const picture = new WorkPicture({ transport: () => transport, repaintThread: vi.fn(), repaintTimeline: vi.fn(), storage: { getItem: () => "1", setItem: () => {} }, setInterval: vi.fn(() => 1), clearInterval: vi.fn() });
    picture.watchThread("m", "a");
    const [first] = (transport.requestWorkEvents as ReturnType<typeof vi.fn>).mock.results.map((result) => result.value as string);
    picture.page("m", "a", workEventsPage({ request_id: first!, events: [wire("a1", 1)], has_earlier: false })!);
    picture.watchThread("m", "b");
    // A's supervisor works while the operator reads B; its notice is not acted on there.
    picture.appended("m", "a");
    const forA = () => (transport.requestWorkEvents as ReturnType<typeof vi.fn>).mock.calls.filter(([session]) => session === "a").length;
    expect(forA()).toBe(1);
    picture.watchThread("m", "a");
    expect(forA(), "coming back reads A's newest page").toBe(2);
    picture.watchThread("m", "a");
    expect(forA(), "a repaint of the same thread reads nothing").toBe(2);
  });

  it("renders typed work evidence safely, lazily and without losing disclosure or focus", () => {
    const open = new Set<string>(["work:read"]);
    const requested: string[] = [];
    const details = new Map<string, { state: "ready"; text: string; truncated: boolean }>();
    const context = { detail: (id: string) => details.get(id), requestDetail: (id: string) => void requested.push(id), isOpen: (key: string) => open.has(key), setOpen: (key: string, on: boolean) => void (on ? open.add(key) : open.delete(key)) };
    const node = document.createElement("div"); document.body.append(node);
    const raw = '<img src=x onerror="alert(1)">\n\nResult:\nexact output';
    const events = workEventsPage({ request_id: "x", schema_version: 2, events: [
      wire("start", 0, { kind: "notice", label: "Turn started", has_detail: false, lifecycle: { id: "turn", stage: "start" } }),
      wire("read", 1, { tool: { name: "Read", operation: "read", targets: ["a.ts", "b.ts", "c.ts"], counts: { files: 3 } } }),
      wire("search", 2, { tool: { name: "Grep", operation: "search", targets: ["src"], counts: { matches: 7 } } }),
      wire("command", 3, { tool: { name: "Bash", operation: "command", targets: [], command: "npm test <b>", cwd: "/project", exit_code: 2 }, lifecycle: { id: "cmd", stage: "end", outcome: "error" } }),
      wire("edit", 4, { tool: { name: "Edit", operation: "edit", targets: ["a.ts", "b.ts"], counts: { files: 2 } }, lifecycle: { id: "edit", stage: "end", outcome: "success" } }),
      wire("other", 5, { label: raw, tool: { name: "functions.exec", operation: "other", targets: [] } }),
      wire("end", 6, { kind: "notice", label: "Turn completed", has_detail: false, lifecycle: { id: "turn", stage: "end", outcome: "unknown" } }),
      wire("failed-turn", 7, { kind: "notice", label: "Turn failed", has_detail: false, lifecycle: { id: "failed", stage: "end", outcome: "error" } }),
    ], has_earlier: false })!.events;
    const item = { type: "work" as const, key: "work:read", events };
    renderWork(node, item, context, "cas-src supervisor");
    expect([...node.querySelectorAll(".work-row-text")].map(row => row.textContent)).toEqual(["Read · 3 files", "Search · 7 matches", "Ran command · exit 2", "Edited 2 files", raw, "Turn failed"]);
    expect(node.querySelector("img")).toBeNull();
    expect(node.querySelector('[data-event-id="command"]')!.textContent).toContain("failed");
    expect(node.textContent).not.toContain("Turn started");
    expect(requested).toEqual([]);
    for (const id of ["read", "search", "command", "edit", "other"]) {
      const toggle = node.querySelector<HTMLButtonElement>(`[data-event-id="${id}"] .work-row-toggle`)!;
      toggle.focus(); toggle.click();
      expect(requested.at(-1)).toBe(id);
      expect(node.querySelector(`[data-event-id="${id}"] .work-detail`)!.textContent).toBe("Loading…");
      details.set(id, { state: "ready", text: raw, truncated: false });
      renderWork(node, item, context, "cas-src supervisor");
      expect(document.activeElement?.getAttribute("data-focus-key")).toBe(`row:${id}`);
      expect(node.querySelector(`[data-event-id="${id}"] [aria-expanded="true"]`)).not.toBeNull();
    }
    expect(node.querySelector('[data-event-id="read"] .work-targets')!.textContent).toBe("a.tsb.tsc.ts");
    expect(node.querySelector('[data-event-id="search"] .work-detail')!.textContent).toContain("7 matches");
    expect(node.querySelector('[data-event-id="command"] .work-command')!.textContent).toBe("npm test <b>");
    expect(node.querySelector('[data-event-id="command"] .work-detail')!.textContent).toContain("/project");
    expect(node.querySelector('[data-event-id="edit"] .work-targets')!.textContent).toBe("a.tsb.ts");
    expect(node.querySelector('[data-event-id="other"] pre')!.textContent).toBe(raw);
    expect(node.querySelectorAll("img, b")).toHaveLength(0);
    const before = workSignature(item, context);
    const updated = { ...item, events: events.map(event => event.id === "command" ? { ...event, tool: { ...event.tool!, exit_code: 0 }, lifecycle: { id: "cmd", stage: "end", outcome: "success" } } : event) };
    expect(workSignature(updated, context)).not.toBe(before);
    node.querySelector<HTMLElement>('[data-event-id="command"] .work-detail')!.focus();
    renderWork(node, updated, context, "cas-src supervisor");
    expect(document.activeElement?.getAttribute("data-focus-key")).toBe("detail:command");
    expect(node.querySelector('[data-event-id="command"]')!.textContent).toContain("Ran command · exit 0");
    expect(node.querySelector('[data-event-id="command"] .work-row-failed')).toBeNull();
    node.querySelector<HTMLButtonElement>(".work-turn-notices > button")!.click();
    expect(node.textContent).toContain("Turn started"); expect(node.textContent).toContain("Turn completed");
    node.remove();
  });
});
