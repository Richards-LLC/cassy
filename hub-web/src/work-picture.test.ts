// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { WorkEventStream, workEventSupport, workEventsPage } from "./work-events";
import { WorkPicture, type WorkEventTransport } from "./work-picture";
import { renderWork, workSignature } from "./work-rows-view";
import { ConversationHistory } from "./conversation-history";
import { threadModel } from "./thread-model";

const wire = (id: string, second: number, extra: Record<string, unknown> = {}) => ({ id, kind: "tool", session: "s", at: `2026-10-27T12:00:${String(second).padStart(2, "0")}Z`, label: id, has_detail: true, ...extra });

function livePicture(agent?: string) {
  let request = 0;
  const transport: WorkEventTransport = { workEvents: () => "live", requestWorkEvents: vi.fn(() => `r${++request}`), requestWorkEventDetail: () => undefined };
  const picture = new WorkPicture({ transport: () => transport, repaintThread: vi.fn(), repaintTimeline: vi.fn(), storage: { getItem: () => "1", setItem: () => {} } });
  const open = () => agent ? picture.watchTimeline({ machineId: "m", session: "s", agent }) : picture.watchThread("m", "s");
  open();
  return { picture, transport, open };
}

describe("work events (cas-ff22)", () => {
  it("shows each turn's newest tool-reported edits once and persists lazy file evidence", () => {
    const storage = new Map<string, string>([["commander.showWork", "1"]]);
    const transport: WorkEventTransport = { workEvents: () => "live", requestWorkEvents: () => "page", requestWorkEventDetail: vi.fn(() => "detail") };
    const makePicture = () => new WorkPicture({ transport: () => transport, repaintThread: vi.fn(), repaintTimeline: vi.fn(), storage: { getItem: key => storage.get(key) ?? null, setItem: (key, value) => void storage.set(key, value) } });
    const files = [
      { path: "src/a.ts", operation: "edit", source_event_id: "edit", line_counts: { added: 12, removed: 4 } },
      { path: "src/<b>.ts", operation: "edit", source_event_id: "other" },
      { path: "src/empty.ts", operation: "edit", source_event_id: "other", line_counts: { added: 0, removed: 0 } },
    ];
    const events = [
      wire("edit", 1, { turn: "one", changed_files: { files: files.slice(0, 1), provenance: "successful_tool_calls", complete: false } }),
      wire("other", 3, { turn: "one", changed_files: { files, provenance: "successful_tool_calls", complete: false } }),
      wire("next", 4, { turn: "two", changed_files: { files: [files[0]], provenance: "successful_tool_calls", complete: false } }),
    ];
    const history = new ConversationHistory();
    for (const [id, second] of [[1, 0], [2, 2], [3, 5]]) history.reply({ notification_id: id!, reply_to: null, message: "Updated", summary: "", device_id: "d", kind: "answer" }, Date.parse(wire("", second!).at));
    let picture = makePicture();
    const load = () => { picture.watchThread("m", "s"); picture.page("m", "s", workEventsPage({ request_id: "page", events, has_earlier: false })!); };
    load();
    const node = document.createElement("div"); document.body.append(node);
    const items = threadModel(history.events, { work: picture.threadWork("m", "s"), historyEnd: true }).filter(item => item.type === "work");
    const paint = () => renderWork(node, items[1]!, picture.rows("m", "s"), "Supervisor");
    renderWork(node, items[0]!, picture.rows("m", "s"), "Supervisor");
    expect(node.querySelector(".changed-files")).toBeNull();
    paint();
    expect(node.querySelectorAll(".changed-files")).toHaveLength(2);
    expect(node.textContent).toContain("Tool-reported edits · 3 files");
    expect(node.textContent).not.toMatch(/\+\d|−\d/);
    expect(transport.requestWorkEventDetail).not.toHaveBeenCalled();
    const card = () => node.querySelector<HTMLElement>(".changed-files")!;
    card().querySelector<HTMLButtonElement>(".changed-files-toggle")!.click();
    const rows = card().querySelectorAll(".changed-file");
    expect(rows[0]!.querySelector(".changed-file-count-visual")?.textContent).toBe("+12/−4");
    expect(rows[0]!.querySelector(".changed-file-count-label")?.textContent).toBe("12 lines added, 4 lines removed in the latest successful edit");
    expect(rows[1]!.querySelector(".changed-file-counts")).toBeNull();
    expect(rows[2]!.querySelector(".changed-file-count-visual")?.textContent).toBe("+0/−0");
    const file = () => card().querySelector<HTMLButtonElement>(".changed-file-toggle")!;
    file().focus(); file().click();
    expect(document.activeElement).toBe(file());
    expect(card().textContent).toContain("Loading");
    expect(transport.requestWorkEventDetail).toHaveBeenCalledWith("s", "edit");
    const signature = workSignature(items[1]!, picture.rows("m", "s"));
    const patch = "--- a/src/a.ts\n+++ b/src/a.ts\n@@ -1 +1 @@\n-old\n+<script>new</script>\n--- a/unrelated.ts\n+++ b/unrelated.ts\n@@ -1 +1 @@\n-no\n+unrelated";
    picture.detail("m", "s", { request_id: "detail", event_id: "edit", detail: patch, truncated: false });
    expect(workSignature(items[1]!, picture.rows("m", "s"))).not.toBe(signature);
    paint();
    expect(card().querySelector('.diff-line[data-change="add"]')?.textContent).toContain("<script>new</script>");
    expect(card().textContent).not.toContain("unrelated");
    expect(node.querySelectorAll("b, script")).toHaveLength(0);
    card().querySelectorAll<HTMLButtonElement>(".changed-file-toggle")[1]!.click();
    picture.detail("m", "s", { request_id: "detail", event_id: "other", detail: "Exact edit input and output", truncated: true });
    paint();
    expect(card().textContent).toContain("Exact edit input and output");
    expect(card().textContent).toContain("cut");
    picture = makePicture(); load(); paint();
    expect(card().querySelector(".changed-files-toggle")?.getAttribute("aria-expanded")).toBe("true");
    expect(file().getAttribute("aria-expanded")).toBe("true");
    expect(card().textContent).toContain("Loading");
    node.remove();
  });
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
      changed_files: { files: [{ path: "a.ts", operation: "edit", source_event_id: "a", line_counts: { added: 3, removed: 0 } }], provenance: "successful_tool_calls", complete: false },
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
    // A bad optional extension cannot discard the file's existing provenance.
    for (const line_counts of [undefined, null, {}, { added: 1 }, { added: -1, removed: 0 }, { added: 0.5, removed: 0 }, { added: 0, removed: "1" }, { added: Number.MAX_SAFE_INTEGER + 1, removed: 0 }]) {
      const file = { path: "a.ts", operation: "edit", source_event_id: "a" };
      const decoded = workEventsPage({ request_id: "counts", events: [wire("a", 1, {
        changed_files: { files: [{ ...file, line_counts }], provenance: "successful_tool_calls", complete: false },
      })] })!.events[0]!.changed_files!.files[0];
      expect(decoded).toEqual(file);
    }
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

  it.each([undefined, "worker"])("retains coalesced live notices across newest, gap and refused reads (agent: %s)", (agent) => {
    const { picture, transport } = livePicture(agent);
    const page = (request: string, events: ReturnType<typeof wire>[], gap?: string) => picture.page("m", "s", workEventsPage({ request_id: request, events, has_earlier: gap !== undefined, next_before: gap })!);
    const row = (id: string, second: number) => wire(id, second, { agent });
    picture.appended("m", "s"); picture.appended("m", "s");
    expect(transport.requestWorkEvents, "notices cannot start concurrent newest reads").toHaveBeenCalledTimes(1);
    page("r1", [row("read", 1)]);
    expect(transport.requestWorkEvents, "an old answer cannot consume newer notices").toHaveBeenCalledTimes(2);
    page("r2", [row("read", 1), row("bash", 2)]);
    expect(picture.stream("m", "s", agent).events.map(event => event.id)).toEqual(["read", "bash"]);
    expect(transport.requestWorkEvents).toHaveBeenCalledTimes(2);

    picture.appended("m", "s"); picture.appended("m", "s");
    page("r3", [row("later", 5)], "gap-cursor");
    expect(transport.requestWorkEvents).toHaveBeenCalledTimes(4);
    expect(transport.requestWorkEvents).toHaveBeenLastCalledWith("s", { before: "gap-cursor", ...(agent ? { agent } : {}) });
    picture.appended("m", "s"); picture.appended("m", "s");
    expect(transport.requestWorkEvents, "gap completion comes before the coalesced refresh").toHaveBeenCalledTimes(4);
    page("r4", [row("bash", 2), row("middle", 4)]);
    expect(transport.requestWorkEvents).toHaveBeenCalledTimes(5);
    picture.failed("m", "s", "r5", "temporarily unavailable");
    expect(transport.requestWorkEvents, "a failed follow-up without another notice cannot retry forever").toHaveBeenCalledTimes(5);

    picture.appended("m", "s"); picture.appended("m", "s");
    picture.failed("m", "s", "r6", "temporarily unavailable");
    expect(transport.requestWorkEvents, "a refusal still owes the notice received during that request").toHaveBeenCalledTimes(7);
    picture.failed("m", "s", "r7", "temporarily unavailable");
    expect(transport.requestWorkEvents).toHaveBeenCalledTimes(7);
  });

  it.each([undefined, "worker"])("does not refresh deferred notices after closing or disconnecting (agent: %s)", (agent) => {
    for (const stop of ["close", "disconnect"] as const) {
      const { picture, transport, open } = livePicture(agent);
      if (stop === "close") {
        picture.page("m", "s", workEventsPage({ request_id: "r1", events: [wire("old", 1, { agent })], has_earlier: false })!);
        picture.appended("m", "s");
      }
      picture.appended("m", "s");
      if (stop === "disconnect") picture.disconnected("m", "s");
      else if (agent) picture.watchTimeline(undefined);
      else picture.setShowWork(false);
      const pending = stop === "close" ? 2 : 1;
      picture.page("m", "s", workEventsPage({ request_id: `r${pending}`, events: [wire("newer", 3, { agent })], has_earlier: true, next_before: "gap-cursor" })!);
      picture.appended("other", "s");
      expect(transport.requestWorkEvents, "closed views do not continue a gap or deferred refresh").toHaveBeenCalledTimes(pending);
      if (stop === "disconnect") picture.reattached("m", "s");
      else if (agent) open();
      else picture.setShowWork(true);
      expect(transport.requestWorkEvents).toHaveBeenCalledTimes(pending + 1);
      picture.page("m", "s", workEventsPage({ request_id: `r${pending + 1}`, events: [wire("fresh", 4, { agent })], has_earlier: false })!);
      expect(transport.requestWorkEvents, "reopening does not replay an old deferred notice").toHaveBeenCalledTimes(pending + 1);
    }
  });

  it("folds only published reasoning into readable detail and retains the reader's state", () => {
    const open = new Set(["work:published"]);
    const requested = vi.fn();
    const detail = { state: "ready" as const, text: "## Published detail\n\nKeep **the reader** here.\n\n- Check `src/a.ts`.\n\n<img src=x onerror=alert(1)> [unsafe](javascript:alert)", truncated: true };
    let loaded = false;
    const context = { detail: () => loaded ? detail : undefined, requestDetail: requested, isOpen: (key: string) => open.has(key), setOpen: (key: string, on: boolean) => void (on ? open.add(key) : open.delete(key)) };
    const events = workEventsPage({ request_id: "x", events: [
      wire("published", 1, { kind: "thinking", label: "Legacy label", reasoning: { summary: "Check **the lane**", source: "codex_summary" } }),
      wire("summary-only", 2, { kind: "thinking", has_detail: false, lifecycle: { id: "active", stage: "start" }, reasoning: { summary: "Only **published text**", source: "claude_thinking" } }),
      wire("legacy", 3, { kind: "thinking", label: "Legacy **literal**", reasoning: { summary: "Unknown source", source: "future_source" } }),
    ] })!.events;
    const item = { type: "work" as const, key: "work:published", events };
    const node = document.createElement("div"); document.body.append(node);
    renderWork(node, item, context, "supervisor");
    const row = () => node.querySelector<HTMLElement>('[data-event-id="published"]')!;
    const toggle = () => row().querySelector<HTMLButtonElement>(".work-row-toggle")!;
    expect(row().textContent).toContain("Reasoning summaryCheck the lane");
    expect(toggle().getAttribute("aria-expanded")).toBe("false");
    expect(requested).not.toHaveBeenCalled();
    toggle().focus(); toggle().click();
    expect(requested).toHaveBeenCalledExactlyOnceWith("published");
    loaded = true;
    renderWork(node, item, context, "supervisor");
    expect(row().querySelector(".work-reasoning-content p strong")!.textContent).toBe("the reader");
    expect(row().querySelectorAll("img, a[href^=javascript]")).toHaveLength(0);
    expect(row().querySelector(".work-detail-limit")!.textContent).toContain("16 KB");
    const raw = row().querySelector<HTMLDetailsElement>(".work-reasoning-raw")!;
    raw.open = true; raw.dispatchEvent(new Event("toggle"));
    expect(raw.querySelector("pre")!.textContent).toBe(detail.text);
    row().querySelector<HTMLElement>(".work-detail")!.scrollTop = 42;
    const revised = { ...item, events: events.map(event => event.id === "published" ? { ...event, reasoning: { ...event.reasoning!, summary: "Updated **summary**" } } : event) };
    expect(workSignature(revised, context)).not.toBe(workSignature(item, context));
    const updated = { ...revised, events: revised.events.map(event => event.id === "published" ? { ...event, lifecycle: { id: "turn", stage: "end" } } : event) };
    expect(workSignature(updated, context)).not.toBe(workSignature(item, context));
    renderWork(node, updated, context, "supervisor");
    expect(row().querySelector(".work-reasoning-label")!.textContent).toBe("Thought");
    expect(row().querySelector<HTMLDetailsElement>(".work-reasoning-raw")!.open).toBe(true);
    expect(row().querySelector<HTMLElement>(".work-detail")!.scrollTop).toBe(42);
    expect(document.activeElement?.getAttribute("data-focus-key")).toBe("row:published");
    expect(node.querySelector('[data-event-id="summary-only"] .work-reasoning-label')!.textContent).toBe("Thinking");
    node.querySelector<HTMLButtonElement>('[data-event-id="summary-only"] button')!.click();
    expect(node.querySelector('[data-event-id="summary-only"] .work-reasoning-content p strong')!.textContent).toBe("published text");
    expect(requested).toHaveBeenCalledTimes(1);
    node.querySelector<HTMLButtonElement>('[data-event-id="legacy"] button')!.click();
    expect(node.querySelector('[data-event-id="legacy"] .work-record')!.textContent).toBe(detail.text);
    expect(node.querySelector('[data-event-id="legacy"] strong')).toBeNull();
    node.remove();
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
    expect(node.querySelectorAll(".work-row-gutter")).toHaveLength(node.querySelectorAll(".work-row").length);
    const live = { ...item, events: events.map(event => event.id === "command" ? { ...event, error: false, tool: { ...event.tool!, exit_code: undefined }, lifecycle: { id: "cmd", stage: "start" } } : event) };
    renderWork(node, live, context, "cas-src supervisor");
    expect(node.querySelectorAll(".work-row.is-live")).toHaveLength(1);
    expect(node.querySelector(".work-row.is-live")!.getAttribute("data-event-id")).toBe("command");
    renderWork(node, item, context, "cas-src supervisor");
    expect(node.querySelector(".work-row.is-live")).toBeNull();
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
