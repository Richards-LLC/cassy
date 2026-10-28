// Opening flows replay captured real Commander rows; scroll reuses the frozen independent-QA fixture.
// No real credentials; no sends, dismissals, control claims or live service writes.
import { test, expect, type CDPSession, type Page } from "@playwright/test";
import { createServer, type Server } from "node:http";
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { resolve, extname, join } from "node:path";
import { execFileSync } from "node:child_process";
import { decode } from "@jridgewell/sourcemap-codec";
import { HubDouble, type HistoryPage, type WorkWorld } from "./hub-double";
import { ATLAS } from "./world";

type Capture = { session: string; raw_queue_rows: number; commander_turns: number; query_ms: number[]; pages: HistoryPage[]; pending_replies?: Record<string, unknown>[]; work?: WorkWorld };
const output = resolve(process.env.CONVERSATION_PERF_OUTPUT!);
const capture: Capture = JSON.parse(await readFile(process.env.CONVERSATION_PERF_CAPTURE!, "utf8"));
const scrollFlow = process.env.CONVERSATION_PERF_FLOW === "scroll";
const showWork = process.env.CONVERSATION_PERF_SHOW_WORK === "1";
if (showWork && !capture.work?.events.length) throw Error("Show work measurement requires captured work events from the same thread");
const dist = resolve(process.env.CONVERSATION_PERF_DIST ?? "dist");
let server: Server;
let origin: string;
test.beforeAll(async () => {
  server = createServer(async (req, res) => {
    const pathname = new URL(req.url!, "http://localhost").pathname;
    const file = resolve(dist, `.${pathname.replace(/^\/commander/, "") || "/index.html"}`.replace(/\/$/, "/index.html"));
    if (!file.startsWith(`${dist}/`)) { res.writeHead(403).end(); return; }
    try {
      const bytes = await readFile(file);
      res.setHeader("Content-Type", ({ ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".woff2": "font/woff2" } as Record<string, string>)[extname(file)] ?? "application/octet-stream");
      res.setHeader("Cache-Control", "no-store");
      res.end(bytes);
    } catch { res.writeHead(404).end(); }
  });
  await new Promise<void>((ok) => server.listen(0, "127.0.0.1", ok));
  const address = server.address();
  if (!address || typeof address === "string") throw Error("server did not bind");
  origin = `http://127.0.0.1:${address.port}`;
});
test.afterAll(async () => { server.closeAllConnections(); await new Promise<void>((ok) => server.close(() => ok())); });

// Browser-side measurements contain sizes/times only, never message bodies.
function installProbe() {
  const w = window as typeof window & { perfProbe: any };
  const resuming = sessionStorage.getItem("perf:reload") === "1";
  sessionStorage.removeItem("perf:reload");
  const probe = w.perfProbe = { t0: 0, active: resuming, paintEnabled: true, firstPaint: 0, latestPaint: 0, lastMutation: 0, parse: [] as any[], requests: [] as any[], frames: [] as any[], acks: [] as any[], longTasks: [] as any[], historyPaints: [] as any[], tailSamples: [] as any[] };
  const parse = JSON.parse;
  JSON.parse = function (text, reviver) {
    const start = performance.now();
    try { return parse(text, reviver); }
    finally { if (text.length > 1024) probe.parse.push({ at: start, ms: performance.now() - start, chars: text.length }); }
  };
  new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) probe.longTasks.push({ at: entry.startTime, ms: entry.duration });
  }).observe({ type: "longtask", buffered: true });
  const Socket = window.WebSocket;
  window.WebSocket = class extends Socket {
    constructor(url: string | URL, protocols?: string | string[]) {
      super(url, protocols);
      this.addEventListener("message", (event: MessageEvent) => {
        if (typeof event.data === "string") {
          const live = event.data.includes('"OperatorReply":');
          const reply = live ? parse(event.data).OperatorReply : undefined;
          if (event.data.includes('"ConversationHistory":')) {
            const received = performance.now();
            const thread = document.querySelector<HTMLElement>(".conversation-reading.thread");
            const box = thread?.getBoundingClientRect();
            const anchor = box && [...thread!.querySelectorAll<HTMLElement>(".msgs [data-key]")].find(node => {
              const rect = node.getBoundingClientRect(); return rect.height > 0 && rect.bottom > box.top && rect.top < box.bottom;
            });
            const key = anchor?.dataset.key;
            const top = anchor?.getBoundingClientRect().top;
            const older = probe.requests.at(-1)?.older === true;
            const hasEarlier = parse(event.data).ConversationHistory.has_earlier;
            requestAnimationFrame(() => requestAnimationFrame(() => {
              const held = key && [...(thread?.querySelectorAll<HTMLElement>(".msgs [data-key]") ?? [])].find(node => node.dataset.key === key);
              probe.historyPaints.push({ at: received, older, has_earlier: hasEarlier, paint_ms: performance.now() - received,
                anchor_key: key, anchor_drift_px: held && top !== undefined ? held.getBoundingClientRect().top - top : null });
            }));
          }
          probe.frames.push({ at: performance.now(), bytes: new TextEncoder().encode(event.data).length, history: event.data.includes('"ConversationHistory":'), live_reply: live, notification_id: reply?.notification_id });
        }
      });
    }
    send(data: string | ArrayBufferLike | Blob | ArrayBufferView) {
      if (typeof data === "string" && data.includes('"ConversationHistoryRequest":')) probe.requests.push({ at: performance.now(), bytes: new TextEncoder().encode(data).length, older: parse(data).ConversationHistoryRequest?.before != null });
      if (typeof data === "string" && data.includes('"OperatorReplyPersisted":')) {
        const frame = parse(data);
        const ack = frame.OperatorReplyPersisted ?? frame.message?.OperatorReplyPersisted;
        if (ack) probe.acks.push({ at: performance.now(), notification_id: ack.notification_id });
      }
      return super.send(data);
    }
  };
  // The click event is the tap origin, excluding Playwright's actionability wait.
  document.addEventListener("click", (event) => {
    if ((event.target as Element).closest('nav[aria-label="Choose a supervisor"] button')) { probe.t0 = performance.now(); probe.active = true; }
  }, true);
  document.addEventListener("DOMContentLoaded", () => {
    let scheduled = false;
    new MutationObserver(() => {
      probe.lastMutation = performance.now();
      if (!probe.paintEnabled || !probe.active || scheduled) return;
      scheduled = true;
      // Two rAFs: an opportunity for the changed DOM to be presented between them.
      requestAnimationFrame(() => requestAnimationFrame(() => {
        scheduled = false;
        const thread = document.querySelector<HTMLElement>(".conversation-reading.thread");
        const turns = [...document.querySelectorAll(".msgs .conversation-turn")];
        if (!thread || !turns.length) return;
        const bounds = thread.getBoundingClientRect();
        const visible = (node: Element) => { const rect = node.getBoundingClientRect(); return rect.height > 0 && rect.bottom > bounds.top && rect.top < bounds.bottom; };
        if (!probe.firstPaint && turns.some(visible)) probe.firstPaint = performance.now();
        if (!probe.latestPaint && visible(turns[turns.length - 1])) probe.latestPaint = performance.now();
        probe.tailSamples.push({ at: performance.now() - probe.t0, turns: turns.length, tail_gap_px: thread.scrollHeight - thread.clientHeight - thread.scrollTop, last_key: (turns.at(-1) as HTMLElement).dataset.key });
      }));
    }).observe(document.body, { childList: true, subtree: true, characterData: true });
  });
}

async function settle(page: Page) {
  await page.waitForFunction(() => {
    const p = (window as any).perfProbe;
    return document.querySelector(".msgs .conversation-turn") && performance.now() - p.lastMutation > 350;
  });
  await page.evaluate(() => new Promise((ok) => requestAnimationFrame(() => requestAnimationFrame(ok))));
}

async function startProfile(cdp: CDPSession) {
  await cdp.send("Profiler.start");
  await cdp.send("Tracing.start", { categories: "devtools.timeline,v8,blink.user_timing", transferMode: "ReturnAsStream" });
  return cdp.send("Performance.getMetrics");
}

async function endProfile(cdp: CDPSession, directory: string, phase: string) {
  const metrics = await cdp.send("Performance.getMetrics");
  const profile = await cdp.send("Profiler.stop");
  const completed = new Promise<any>((ok) => cdp.once("Tracing.tracingComplete", ok));
  await cdp.send("Tracing.end");
  const { stream } = await completed;
  let trace = "";
  while (true) {
    const chunk = await cdp.send("IO.read", { handle: stream });
    trace += chunk.base64Encoded ? Buffer.from(chunk.data, "base64").toString("utf8") : chunk.data;
    if (chunk.eof) break;
  }
  await cdp.send("IO.close", { handle: stream });
  await writeFile(join(directory, `${phase}.trace.json`), trace);
  await writeFile(join(directory, `${phase}.cpuprofile`), JSON.stringify(profile.profile));
  const sourceSamples = await attributeSamples(profile.profile);
  return { metrics, sourceSamples };
}

// Exclusive sampled leaf cost. Parent-inclusive values are separately labelled.
// Decode the production bundle's source map rather than inventing function timings.
async function attributeSamples(profile: any) {
  const parents = new Map<number, number>();
  for (const node of profile.nodes) for (const child of node.children ?? []) parents.set(child, node.id);
  const maps = new Map<string, any>();
  async function source(frame: any): Promise<string> {
    if (!frame.url.startsWith(origin) || frame.lineNumber < 0) return frame.functionName || "native/idle";
    const name = new URL(frame.url).pathname.replace(/^\/commander\//, "");
    if (!maps.has(name)) {
      try { const map = JSON.parse(await readFile(join(dist, `${name}.map`), "utf8")); maps.set(name, { ...map, lines: decode(map.mappings) }); }
      catch { maps.set(name, null); }
    }
    const map = maps.get(name);
    const segments = map?.lines[frame.lineNumber] ?? [];
    let mapped: number[] | undefined;
    for (const segment of segments) { if (segment[0] > frame.columnNumber) break; if (segment.length > 1) mapped = segment; }
    return mapped ? map.sources[mapped[1]].replace(/^.*src\//, "src/") : name;
  }
  const leaf: Record<string, number> = {}, inclusive: Record<string, number> = {};
  const labels = new Map<number, string>();
  for (const node of profile.nodes) labels.set(node.id, await source(node.callFrame));
  for (const [index, id] of (profile.samples ?? []).entries()) {
    const ms = (profile.timeDeltas[index] ?? 0) / 1000;
    const label = labels.get(id) ?? "unknown";
    leaf[label] = (leaf[label] ?? 0) + ms;
    const seen = new Set<string>();
    let cursor: number | undefined = id;
    while (cursor !== undefined) { seen.add(labels.get(cursor) ?? "unknown"); cursor = parents.get(cursor); }
    for (const parent of seen) inclusive[parent] = (inclusive[parent] ?? 0) + ms;
  }
  const sorted = (values: Record<string, number>) => Object.fromEntries(Object.entries(values).sort((a, b) => b[1] - a[1]));
  return { leaf_ms: sorted(leaf), inclusive_ms_do_not_sum: sorted(inclusive) };
}

const profiles = scrollFlow
  ? ["light", "dark"].flatMap(colorScheme => [
    { name: "desktop", viewport: { width: 1280, height: 844 }, cpu: 1, colorScheme: colorScheme as "light" | "dark" },
    { name: "phone", viewport: { width: 390, height: 844 }, cpu: 1, colorScheme: colorScheme as "light" | "dark" },
  ])
  : [
    { name: "desktop", viewport: { width: 1440, height: 900 }, cpu: 1, colorScheme: "light" as const },
    { name: "phone", viewport: { width: 390, height: 844 }, cpu: 4, colorScheme: "light" as const },
  ];
for (const profile of profiles) test(`${scrollFlow ? "HUB-J4 " : ""}conversation ${scrollFlow ? "scroll paging" : "opening baseline"} ${profile.name}${scrollFlow ? ` ${profile.colorScheme}` : ""}`, async ({ browser }, info) => {
  const directory = join(output, `${profile.name}${scrollFlow ? `-${profile.colorScheme}` : ""}-${info.repeatEachIndex}`);
  await mkdir(directory, { recursive: true });
  const context = await browser.newContext({ viewport: profile.viewport, isMobile: profile.name === "phone", hasTouch: profile.name === "phone", colorScheme: profile.colorScheme });
  if (scrollFlow) await context.tracing.start({ snapshots: { dom: true, aria: true, screen: false }, screenshots: false, sources: true });
  const page = await context.newPage();
  const cdp = await context.newCDPSession(page);
  await cdp.send("Emulation.setCPUThrottlingRate", { rate: profile.cpu });
  await cdp.send("Performance.enable");
  await cdp.send("Profiler.enable");
  await cdp.send("Profiler.setSamplingInterval", { interval: 1000 });
  const hub = new HubDouble(page, {
    time: { now: () => performance.timeOrigin + performance.now(), delay: (callback, ms) => { setTimeout(callback, ms); } },
    ...(scrollFlow ? { paired: [ATLAS.id], machines: [ATLAS] } : {
      paired: ["baseline"], machines: [{ id: "baseline", label: "Soundwave baseline", sessions: [{ name: capture.session, supervisor: "supervisor", project_dir: "/projects/cas-src", workers: [], liveness: "live" as const }] }],
    }),
    history: { [capture.session]: capture.pages },
    ...(capture.work ? { work: { [capture.session]: capture.work } } : {}),
  });
  await hub.install();
  await page.addInitScript(installProbe);
  await page.addInitScript((on) => localStorage.setItem("commander.showWork", on ? "1" : "0"), showWork);
  const results: Record<string, any> = {};
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const drainPending = async () => {
    for (let index = 0; index < capture.pending_replies!.length; index += 10) {
      // Real authenticated persistence ACKs remove pending daemon rows.
      const acked = new Set(hub.persistedReplies.map((reply) => reply.notification_id));
      for (const reply of capture.pending_replies!.slice(index, index + 10)) if (!acked.has(reply.notification_id as number)) hub.send(capture.session, { OperatorReply: reply });
      await page.evaluate(() => new Promise((ok) => requestAnimationFrame(() => requestAnimationFrame(ok))));
    }
  };
  try {
    await page.goto(`${origin}/commander/`);
    await hub.seedPaired();
    await page.reload();
    const row = page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ });
    await expect(row).toBeVisible();
    const measure = async (phase: string, action: () => Promise<void>) => {
      const base = await startProfile(cdp);
      const since = phase === "reload_resume" ? 0 : await page.evaluate((phase) => { const p = (window as any).perfProbe; p.paintEnabled = phase !== "accumulated"; p.active = true; p.t0 = performance.now(); p.firstPaint = 0; p.latestPaint = 0; p.tailSamples = []; return p.t0; }, phase);
      const requestStart = hub.historyRequests.length;
      const actionComplete = action();
      const composer = page.getByRole("textbox", { name: "Your message", exact: true });
      await expect(composer).toBeVisible();
      const acknowledgeInput = () => page.evaluate(async () => {
        const start = performance.now();
        const input = document.querySelector<HTMLTextAreaElement>("#message-text")!;
        input.focus();
        input.value = "Performance probe (never sent)";
        input.dispatchEvent(new Event("input", { bubbles: true }));
        await new Promise((ok) => requestAnimationFrame(() => requestAnimationFrame(ok)));
        const result = { at: performance.now(), input_ack_ms: performance.now() - start, value_retained: input.value === "Performance probe (never sent)" };
        input.value = ""; input.dispatchEvent(new Event("input", { bubbles: true }));
        return result;
      });
      // First accepted input runs while pending replies are still draining.
      // Keep the historical end-of-drain metric as a separate measurement.
      const firstInput = await acknowledgeInput();
      expect(firstInput.value_retained).toBe(true);
      await actionComplete;
      if (phase === "initial" && process.env.CONVERSATION_PERF_REQUIRE_ALL_ACKED === "1") {
        await expect.poll(() => {
          const acked = new Set(hub.persistedReplies.map((reply) => reply.notification_id));
          return capture.pending_replies!.filter((reply) => !acked.has(reply.notification_id as number)).length;
        }, { timeout: 30_000, message: "every frozen pending notification must be durably ACKed before reconnect" }).toBe(0);
      }
      const interactive = await acknowledgeInput();
      expect(interactive.value_retained).toBe(true);
      await settle(page);
      const probe = await page.evaluate((since) => {
        const p = (window as any).perfProbe;
        return { tap_at: p.t0, first_turn_painted_ms: p.firstPaint ? p.firstPaint - p.t0 : null, latest_turn_painted_ms: p.latestPaint ? p.latestPaint - p.t0 : null,
          parses: p.parse.filter((entry: any) => entry.at >= since), requests: p.requests.filter((entry: any) => entry.at >= since),
          frames: p.frames.filter((entry: any) => entry.at >= since), long_tasks: p.longTasks.filter((entry: any) => entry.at >= since),
          acknowledgement_timeline: p.acks.filter((entry: any) => entry.at >= since).map((entry: any) => ({ notification_id: entry.notification_id, ms: entry.at - p.t0 })),
          history_paints: p.historyPaints.filter((entry: any) => entry.at >= since),
          tail_samples: p.tailSamples, dom_elements: document.querySelectorAll("*").length, rendered_turns: document.querySelectorAll(".msgs .conversation-turn").length };
      }, since);
      const costs = await endProfile(cdp, directory, phase);
      const metrics = Object.fromEntries(costs.metrics.metrics.map((m: any) => [m.name, m.value]));
      const before = Object.fromEntries(base.metrics.map((m: any) => [m.name, m.value]));
      const pendingIds = [...new Set((capture.pending_replies ?? []).map((reply) => reply.notification_id as number))];
      const ackedIds = new Set(hub.persistedReplies.filter((reply) => reply.session === capture.session).map((reply) => reply.notification_id));
      results[phase] = { ...probe, first_input_acknowledged_ms: firstInput.at - probe.tap_at, first_input_ack_ms: firstInput.input_ack_ms, interactive_upper_bound_ms: interactive.at - probe.tap_at, input_ack_ms: interactive.input_ack_ms,
        history_requests: hub.historyRequests.length - requestStart,
        work_requests_total: hub.frames.filter(frame => frame.kind === "WorkEventsRequest").length,
        mounted_work_blocks: await page.locator(".msgs .work-turn").count(),
        websocket_payload_bytes: probe.frames.reduce((sum: number, f: any) => sum + f.bytes, 0),
        history_payload_bytes: probe.frames.filter((f: any) => f.history).reduce((sum: number, f: any) => sum + f.bytes, 0),
        live_reply_frames: probe.frames.filter((f: any) => f.live_reply).length,
        last_live_reply_ms: probe.frames.filter((f: any) => f.live_reply).at(-1)?.at - probe.tap_at || null,
        final_paint_opportunity_ms: probe.tail_samples.at(-1)?.at ?? null,
        final_tail_gap_px: probe.tail_samples.at(-1)?.tail_gap_px ?? null,
        replay_request_to_response_ms: probe.frames.filter((f: any) => f.history).map((frame: any, index: number) => frame.at - probe.requests[index]?.at),
        json_parse_ms: probe.parses.reduce((sum: number, entry: any) => sum + entry.ms, 0),
        script_ms: (metrics.ScriptDuration - (phase === "reload_resume" ? 0 : before.ScriptDuration)) * 1000,
        layout_ms: (metrics.LayoutDuration - (phase === "reload_resume" ? 0 : before.LayoutDuration)) * 1000,
        style_ms: (metrics.RecalcStyleDuration - (phase === "reload_resume" ? 0 : before.RecalcStyleDuration)) * 1000,
        heap_used_bytes: metrics.JSHeapUsedSize, dom_nodes: metrics.Nodes,
        persisted_reply_acks_total: hub.persistedReplies.length,
        pending_acknowledgements: { expected_ids: pendingIds, acknowledged_ids: pendingIds.filter((id) => ackedIds.has(id)), remaining_ids: pendingIds.filter((id) => !ackedIds.has(id)) },
        replayed_pending_ids: [...new Set(probe.frames.filter((frame: any) => pendingIds.includes(frame.notification_id)).map((frame: any) => frame.notification_id))],
        heap: await cdp.send("Runtime.getHeapUsage"), cpu: costs.sourceSamples,
        long_task_total_ms: probe.long_tasks.reduce((sum: number, entry: any) => sum + entry.ms, 0),
        long_task_max_ms: Math.max(0, ...probe.long_tasks.map((entry: any) => entry.ms)) };
      await writeFile(join(directory, "receipt.json"), JSON.stringify({ status: "PASS", profile, browser: browser.version(), source_head: execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim(),
        bundle_revision: process.env.CONVERSATION_PERF_BUNDLE_REVISION ?? execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim(),
        source_session: capture.session, raw_queue_rows: capture.raw_queue_rows, commander_turns: capture.commander_turns, query_ms: capture.query_ms, pending_replies: capture.pending_replies?.length,
        show_work: showWork, work_events: capture.work?.events.length ?? 0,
        flow: process.env.CONVERSATION_PERF_FLOW ?? "history", replay_cadence: "history response plus batches of ten pending OperatorReply frames, paced by browser rAF; fastest-case replay, daemon scheduling/network excluded",
        label: scrollFlow ? "real-bundle, frozen three-page independent-QA fixture, protocol-double" : "real-bundle, real-supervisor-data, protocol-replay; transport is a double, not real hub latency", results, page_errors: errors }, null, 2));
    };
    const backlog = process.env.CONVERSATION_PERF_FLOW === "backlog";
    if (backlog) expect(capture.pending_replies?.length, "capture real pending rows, not fabricated history").toBeGreaterThan(50);
    await measure("initial", async () => {
      await row.click();
      await expect(page.locator(".msgs .conversation-turn").first()).toBeAttached();
      if (backlog) await drainPending();
    });
    expect(results.initial.history_requests).toBe(1);
    if (scrollFlow) {
      // Frozen independent-QA probe: unthrottled, wheel(-400), 60ms cadence,
      // 1280x844 / 390x844, three pages of forty exchanges supplied by capture.
      const thread = page.locator(".conversation-reading.thread");
      const box = (await thread.boundingBox())!;
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
      const requests = hub.historyRequests.length;
      const sampleStart = await page.evaluate(() => performance.now());
      const wheelMs: number[] = [], stepMs: number[] = [];
      await startProfile(cdp);
      for (let step = 0; step < 80 && hub.historyRequests.length === requests; step++) {
        const start = performance.now();
        await page.mouse.wheel(0, -400);
        wheelMs.push(performance.now() - start);
        await page.waitForTimeout(60);
        stepMs.push(performance.now() - start);
      }
      const requested = hub.historyRequests.length > requests;
      await settle(page);
      const paints = await page.evaluate(since => (window as any).perfProbe.historyPaints.filter((paint: any) => paint.older && paint.at >= since), sampleStart);
      const costs = await endProfile(cdp, directory, "scroll");
      const median = (values: number[]) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)];
      const receipt = { profile, source_head: execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim(),
        bundle_revision: process.env.CONVERSATION_PERF_BUNDLE_REVISION, browser: browser.version(), requested_older: requested,
        wheel_ms: wheelMs, step_ms: stepMs, wheel_median_ms: median(wheelMs), step_median_ms: median(stepMs), history_paints: paints,
        cpu: costs.sourceSamples, page_errors: errors };
      await writeFile(join(directory, "scroll.json"), JSON.stringify(receipt, null, 2));
      await page.screenshot({ path: join(directory, "paged.png") });
      // Baselines retain their failing paging/latency evidence for comparison.
      if (process.env.CONVERSATION_PERF_BASELINE !== "1") {
        expect(requested, "scroll reaches the next history page").toBe(true);
        expect(paints.length, "measure the page's actual anchor").toBeGreaterThan(0);
        for (const paint of paints) expect(Math.abs(paint.anchor_drift_px), "fractional reading anchor stays fixed").toBeLessThanOrEqual(0.5);
        expect(median(wheelMs), "same independent-QA wheel probe, base 358ms").toBeLessThanOrEqual(358);
      }
      expect(errors).toEqual([]);
      return;
    }

    if (showWork) {
      await expect(page.locator("#conversation-show-work")).toHaveAttribute("aria-pressed", "true");
      expect(results.initial.work_requests_total).toBeGreaterThan(0);
      // Real work may precede the latest exchange. Exercise its reading window
      // without moving captured timestamps or inserting synthetic tail events.
      await measure("work_window", async () => {
        for (let step = 0; step < 12 && await page.locator(".msgs .work-turn").count() === 0; step++) {
          await page.getByRole("button", { name: "Load earlier", exact: true }).click();
          await settle(page);
        }
      });
      expect(results.work_window.mounted_work_blocks, "captured work must actually enter a reading window").toBeGreaterThan(0);
      const jump = page.getByRole("button", { name: "Jump to latest" });
      if (await jump.isVisible()) await jump.click();
    } else expect(results.initial.work_requests_total).toBe(0);
    if (!backlog) await measure("accumulated", async () => {
      for (let index = 1; index < capture.pages.length; index++) {
        const count = hub.historyRequests.length;
        while (hub.historyRequests.length === count) {
          const firstItem = () => page.evaluate(() => {
            const node = document.querySelector<HTMLElement>(".msgs [data-key], .msgs .coalesce");
            return node?.dataset.key ?? node?.id;
          });
          const first = await firstItem();
          await page.getByRole("button", { name: "Load earlier", exact: true }).click();
          // A window first reveals cached turns; only its oldest edge asks
          // the daemon for another page. Both are progress toward the start.
          await expect.poll(async () => hub.historyRequests.length > count || await firstItem() !== first).toBe(true);
          await settle(page);
        }
        await expect.poll(() => hub.historyRequests.length).toBe(count + 1);
        await settle(page);
      }
    });
    if (!backlog) expect(results.accumulated.history_requests).toBe(capture.pages.length - 1);
    if (backlog && profile.name === "phone") {
      await page.locator("#conversation-back").click();
      await expect(row).toBeVisible();
      await measure("warm_reopen", async () => { await row.click(); });
      const attaches = hub.attaches.length;
      await measure("reconnect", async () => {
        hub.drop(capture.session);
        await expect.poll(() => hub.attaches.length, { timeout: 20_000 }).toBeGreaterThan(attaches);
        await expect(page.locator("#conversation-connection")).toContainText("Live");
        await drainPending();
      });
      await page.evaluate(() => sessionStorage.setItem("perf:reload", "1"));
      await measure("reload_resume", async () => {
        await page.reload();
        await expect(page.locator(".msgs .conversation-turn").first()).toBeAttached();
        await drainPending();
      });
      // Record DOM size without requiring the baseline's bad behavior: the
      // fix must be able to rerun this exact command with a bounded window.
    }
    expect(errors).toEqual([]);
  } finally {
    if (scrollFlow) await context.tracing.stop({ path: join(directory, "playwright.trace.zip") });
    await context.close();
  }
});
