// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { WorkEventStream, workEventSupport, workEventsPage } from "./work-events";
import { WorkPicture, type WorkEventTransport } from "./work-picture";
import { renderWork } from "./work-rows-view";

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

  it("sets agent text as text and loads a row's detail only when it opens", () => {
    const open = new Set<string>();
    const requested: string[] = [];
    const context = { detail: () => undefined, requestDetail: (id: string) => void requested.push(id), isOpen: (key: string) => open.has(key), setOpen: (key: string, on: boolean) => void (on ? open.add(key) : open.delete(key)) };
    const node = document.createElement("div");
    const label = '<img src=x onerror="alert(1)">';
    const event = workEventsPage({ request_id: "x", events: [wire("t:1", 1, { label, error: true })], has_earlier: false })!.events[0]!;
    open.add("work:t:1");
    renderWork(node, { type: "work", key: "work:t:1", events: [event] }, context, "cas-src supervisor");
    expect(node.querySelector("img")).toBeNull();
    expect(node.querySelector(".work-row-text")!.textContent).toBe(label);
    expect(requested).toEqual([]);
    node.querySelector<HTMLButtonElement>(".work-row-toggle")!.click();
    expect(requested).toEqual(["t:1"]);
    expect(node.querySelector(".work-detail")!.textContent).toBe("Loading…");
  });
});
