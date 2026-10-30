import type { Page } from "@playwright/test";
import { test, expect, journeyPart, type Journey } from "./journey";
import { journeyStamp } from "./clock";
import type { FleetWorld, Machine, WorkEventWire, WorkWorld } from "./hub-double";
import { ATLAS, PELICAN } from "./world";
import type { WorkTool, WorkLifecycle, WorkReasoning } from "../../src/work-events";

// cas-ff22 (hub-web/whole-picture.brief.md): Show work puts the supervisor's
// thinking, tool calls and task events between its replies, collapsed by
// default, and each worker's timeline is a tap away in Tasks & progress.

const WORKER = "swift-lark-3";
const event = (id: string, kind: string, label: string, offsetMs: number, extra: Partial<WorkEventWire> & { tool?: WorkTool; lifecycle?: WorkLifecycle; reasoning?: WorkReasoning } = {}): WorkEventWire =>
  ({ id, kind, session: PELICAN, agent: PELICAN, at: journeyStamp(offsetMs), label, has_detail: kind !== "task", ...extra });

const work = (): WorkWorld => ({
  supervisor: PELICAN,
  events: [
    event("t:sup:1:0", "thinking", "Checking the lane", -170_000, { reasoning: { summary: "Checking **the lane**", source: "codex_summary" } }),
    event("t:sup:2:0", "tool", "Bash: Run the tests", -160_000, { error: true }),
    event("q:7", "task", "cas-42 parked for merge: Ship the stream", -150_000, { agent: WORKER }),
    event("t:w:1:0", "tool", "Read: src/limits.rs", -400, { agent: WORKER }),
  ],
  // Agent text is data: the markup in this detail must read as text.
  details: { "t:sup:1:0": "## Published summary\n\nCheck **the lane** before shipping.\n\n- Keep `src/limits.rs` readable.\n\n<img src=x onerror=alert(1)>", "t:sup:2:0": "Bash\n\nInput:\ncargo test <b>-p cas</b>\n\nResult (error):\n1 failed" },
});

const fleet = (): FleetWorld => ({
  agents: [{ name: WORKER, status: "active", current_task: "cas-42", generation: 1 }],
  tasks: [{ id: "cas-42", title: "Ship the stream", status: "in_progress", assignee: WORKER, updated_at: "2026-10-27T12:00:00Z" }],
  epics: [],
  focused_epic: null,
  spawnNames: [],
});

const atlas: Machine = { ...ATLAS, sessions: ATLAS.sessions.map((session) => ({ ...session, workers: [WORKER] })) };

const history = {
  [PELICAN]: [{ has_earlier: false, messages: [], replies: [
    { notification_id: 801, reply_to: null, message: "Starting the stream review.", summary: "", device_id: "journey-device", kind: "answer", attachments: [], at: journeyStamp(-180_000) },
    { notification_id: 802, reply_to: null, message: "The stream is parked for merge.", summary: "", device_id: "journey-device", kind: "answer", attachments: [], at: journeyStamp(-130_000) },
  ] }],
};

async function openConversation(page: Page, journey: Journey): Promise<void> {
  await journey.open();
  await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
  await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
}

for (const [width, colorScheme] of [[1280, "light"], [390, "dark"]] as const) {
  test.describe(`whole picture ${width} ${colorScheme} cas_ff22`, () => {
    test.use({ viewport: { width, height: 844 }, colorScheme, ...(width === 390 ? { hasTouch: true, isMobile: true } : {}) });

    const showWork = async ({ page, journey }: { page: Page; journey: Journey }) => {
      const hub = await journey.hub({ machines: [atlas], paired: ["atlas"], history, work: { [PELICAN]: work() } });
      const thread = page.getByRole("log");
      const toggle = page.locator("#conversation-show-work");
      const blocks = page.locator(".work-turn");
      const workRequests = () => hub.frames.filter((frame) => frame.kind === "WorkEventsRequest").length;

      await journey.stage("Show work is off and the thread is replies only", async () => {
        await openConversation(page, journey);
        await expect(thread).toContainText("The stream is parked for merge.");
        await expect(toggle).toHaveAttribute("aria-pressed", "false");
        await expect(blocks).toHaveCount(0);
        expect(workRequests(), "a thread with Show work off reads no work").toBe(0);
      });

      await journey.stage("Turn Show work on", async () => {
        await toggle.click();
        await expect(toggle).toHaveAttribute("aria-pressed", "true");
        // The work between the two replies is one collapsed block, in order,
        // with its task chip showing and its rows not.
        await expect(blocks).toHaveCount(1);
        const order = await thread.locator(":scope > *").evaluateAll((nodes) => nodes.map((node) => node.classList.contains("work-turn") ? "work" : node.textContent?.includes("parked for merge.") ? "after" : node.textContent?.includes("Starting") ? "before" : "other").filter((kind) => kind !== "other"));
        expect(order).toEqual(["before", "work", "after"]);
        const summary = blocks.getByRole("button", { name: /Reasoning, ran 1 tool \(1 failed\)/ });
        await expect(summary).toHaveAttribute("aria-expanded", "false");
        await expect(blocks.locator(".work-chip")).toHaveText("cas-42 parked");
        await expect(blocks.locator(".work-row")).toHaveCount(0);
        // The worker's own tool call belongs to its timeline, not the thread.
        await expect(thread).not.toContainText("limits.rs");
      });

      await journey.stage("Open the work and one tool call's detail", async () => {
        const summary = blocks.locator(".work-summary");
        await summary.focus();
        await page.keyboard.press("Enter");
        await expect(summary).toBeFocused();
        await expect(summary).toHaveAttribute("aria-expanded", "true");
        await expect(blocks.locator(".work-row")).toHaveCount(3);
        const tool = blocks.getByRole("button", { name: /Bash: Run the tests failed/ });
        await tool.focus();
        await page.keyboard.press("Enter");
        await expect(tool).toBeFocused();
        const detail = blocks.locator(".work-detail");
        await expect(detail).toContainText("cargo test <b>-p cas</b>");
        await expect(detail.locator("b"), "agent text is never parsed as markup").toHaveCount(0);
        expect(hub.frames.filter((frame) => frame.kind === "WorkEventDetailRequest").map((frame) => (frame.body as { event_id: string }).event_id)).toEqual(["t:sup:2:0"]);
      });

      await journey.stage("Read the published reasoning summary in place", async () => {
        const row = blocks.locator('[data-event-id="t:sup:1:0"]');
        const toggle = row.getByRole("button", { name: /Reasoning summary/ });
        await expect(toggle).toHaveAttribute("aria-expanded", "false");
        await expect(toggle).toContainText("Checking the lane");
        await toggle.focus();
        const before = (await toggle.boundingBox())!.y;
        await page.keyboard.press("Enter");
        await expect(row.locator(".work-reasoning-content p strong")).toHaveText("the lane");
        await expect(row.locator("img")).toHaveCount(0);
        await expect(toggle).toBeFocused();
        expect(Math.abs((await toggle.boundingBox())!.y - before)).toBeLessThanOrEqual(1);
        await row.getByText("Raw summary", { exact: true }).click();
        await expect(row.locator(".work-reasoning-raw pre")).toContainText("**the lane**");
        await toggle.click(); await toggle.click();
        await expect(row.locator(".work-reasoning-raw")).toHaveAttribute("open", "");
        expect(hub.frames.filter(frame => frame.kind === "WorkEventDetailRequest" && (frame.body as { event_id: string }).event_id === "t:sup:1:0")).toHaveLength(1);
      });

      await journey.stage("New work arrives live without redrawing the thread", async () => {
        const reply = thread.locator(".conversation-turn", { hasText: "Starting the stream review." });
        await reply.evaluate((node) => { (node as HTMLElement & { journeyMark?: boolean }).journeyMark = true; });
        hub.appendWork(PELICAN, event("t:sup:3:0", "tool", "Bash: Merge the lane", -1_000));
        await expect(page.locator(".work-turn").last()).toContainText("Ran 1 tool");
        expect(await reply.evaluate((node) => (node as HTMLElement & { journeyMark?: boolean }).journeyMark === true), "the earlier reply is the same node").toBe(true);
      });

      await journey.stage("Read typed work scope", async () => {
        const typed = [
          event("typed-start", "notice", "Turn started", -900, { has_detail: false, lifecycle: { id: "typed-turn", stage: "start" } }),
          event("typed-read", "tool", "Read files", -800, { tool: { name: "Read", operation: "read", targets: ["src/a.ts", "src/b.ts", "src/c.ts"], counts: { files: 3 } } }),
          event("typed-search", "tool", "Find the renderer", -700, { tool: { name: "Grep", operation: "search", targets: ["src"], counts: { matches: 7 } } }),
          event("typed-command", "tool", "Check the build", -600, { tool: { name: "Bash", operation: "command", targets: [], command: "npm run check", cwd: "/project", exit_code: 2 }, lifecycle: { id: "check", stage: "end", outcome: "error" } }),
          event("typed-edit", "tool", "Update the renderer", -500, { tool: { name: "Edit", operation: "edit", targets: ["src/a.ts", "src/b.ts"], counts: { files: 2 } }, lifecycle: { id: "edit", stage: "end", outcome: "success" } }),
          event("typed-other", "tool", "functions.exec: inspect the work", -400, { tool: { name: "functions.exec", operation: "other", targets: [] } }),
          event("typed-end", "notice", "Turn completed", -300, { has_detail: false, lifecycle: { id: "typed-turn", stage: "end", outcome: "unknown" } }),
        ];
        const exact = "Read\n\nInput:\n{\"file\":\"src/a.ts\"}\n\nResult:\nexport const literal = '<script>neverRun()</script>';";
        for (const row of typed) hub.appendWork(PELICAN, row, row.id === "typed-read" ? exact : `${row.label}\n\nInput:\nraw arguments\n\nResult:\nexact ${row.id} output`);
        const block = blocks.last();
        await block.locator(".work-summary").click();
        await expect(block.getByRole("button", { name: "Read · 3 files", exact: true })).toBeVisible();
        await expect(block.getByRole("button", { name: "Search · 7 matches", exact: true })).toBeVisible();
        await expect(block.getByRole("button", { name: "Ran command · exit 2 failed", exact: true })).toBeVisible();
        await expect(block.getByRole("button", { name: "Edited 2 files", exact: true })).toBeVisible();
        await expect(block.getByRole("button", { name: "functions.exec: inspect the work", exact: true })).toBeVisible();
        await expect(block).not.toContainText("Turn started");
        await expect(block).not.toContainText("Turn completed");
        const read = block.getByRole("button", { name: "Read · 3 files", exact: true });
        await read.focus(); await page.keyboard.press("Enter");
        await expect(read).toBeFocused();
        const detail = block.locator('[data-event-id="typed-read"] .work-detail');
        await expect(detail.locator(".work-targets li")).toHaveText(["src/a.ts", "src/b.ts", "src/c.ts"]);
        await expect(detail.locator(".work-record")).toHaveText(exact);
        await expect(detail.locator("script")).toHaveCount(0);
        // The same event ID receives new metadata while its detail is open.
        await detail.focus();
        hub.appendWork(PELICAN, { ...typed[1]!, tool: { name: "Read", operation: "read", targets: ["src/a.ts"], counts: { files: 1 } } } as WorkEventWire, exact);
        await expect(block.getByRole("button", { name: "Read · 1 file", exact: true })).toHaveAttribute("aria-expanded", "true");
        await expect(detail).toBeFocused();
        await expect(detail.locator(".work-record")).toHaveText(exact);
      });

      await journey.stage("Show work stays as set on this device", async () => {
        await page.reload();
        // A phone reopens the conversation it had open; the desktop lists it.
        const row = page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row"));
        const send = page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true });
        await expect(send.or(row).first()).toBeVisible();
        if (!(await send.isVisible())) await row.click();
        await expect(send).toBeVisible();
        await expect(toggle).toHaveAttribute("aria-pressed", "true");
        await expect(blocks.first()).toBeVisible();
        await toggle.click();
        await expect(toggle).toHaveAttribute("aria-pressed", "false");
        await expect(blocks).toHaveCount(0);
      });
    };

    const timeline = async ({ page, journey }: { page: Page; journey: Journey }) => {
      const hub = await journey.hub({ machines: [atlas], paired: ["atlas"], history, work: { [PELICAN]: work() }, fleet: { [PELICAN]: fleet() } });
      const dialog = page.getByRole("dialog", { name: `${WORKER} timeline` });
      const open = page.getByRole("button", { name: `${WORKER} timeline`, exact: true });

      await journey.stage("Open a worker's timeline from Tasks & progress", async () => {
        await openConversation(page, journey);
        if (width === 390) await page.getByRole("button", { name: "Tasks & progress", exact: true }).click();
        await open.click();
        await expect(dialog).toBeVisible();
        await expect(dialog).toContainText("Working");
        await expect(dialog).toContainText("cas-42 · Ship the stream");
        const rows = dialog.locator(".work-row");
        await expect(rows.first()).toContainText("Read: src/limits.rs");
        await expect(dialog).not.toContainText("Run the tests");
        const box = (await dialog.boundingBox())!;
        if (width === 390) expect(Math.round(box.y + box.height), "a bottom sheet on the phone").toBe(844);
        else expect(Math.round(box.x + box.width), "a drawer on the right edge").toBe(width);
        const request = hub.frames.find((frame) => frame.kind === "WorkEventsRequest" && (frame.body as { agent?: string }).agent === WORKER);
        expect(request, "the timeline reads the worker's own stream").toBeTruthy();
      });

      await journey.stage("The timeline follows the worker live", async () => {
        // The initial Read is 400ms before fixture creation. Stamp this
        // append now so it is newer even without the recorder's stage delay.
        hub.appendWork(PELICAN, event("t:w:2:0", "tool", "Bash: Run the lane tests", 0, { agent: WORKER }));
        await expect(dialog.locator(".work-row").first()).toContainText("Bash: Run the lane tests");
      });

      await journey.stage("Close the timeline back to its worker row", async () => {
        await page.keyboard.press("Escape");
        await expect(dialog).toBeHidden();
        await expect(page.locator(`.status-timeline[data-worker="${WORKER}"]`).first()).toBeFocused();
      });
    };

    test(`HUB-J25 live phase ${width} ${colorScheme}`, journeyPart, async ({ page, journey }) => {
      const world = fleet();
      world.supervisor_phase = { event_id: "phase-1", kind: "notice", lifecycle: { id: "turn-1", stage: "phase", phase: "setting_up", started_at: journeyStamp(-42_000) }, observed_at: journeyStamp(), stale: false };
      const hub = await journey.hub({ machines: [atlas], paired: ["atlas"], multiplex: true, history, fleet: { [PELICAN]: world } });
      const line = page.getByRole("log").locator(".phase-line");
      const refresh = async () => page.evaluate(session => (window as unknown as { __journeyMachineEvent: (host: string, data: string) => number }).__journeyMachineEvent("atlas.test", JSON.stringify({ kind: "fleet_changed", session })), PELICAN);
      await journey.stage("Read the reported phase without opening Show work", async () => {
        await openConversation(page, journey);
        await expect(line).toContainText("Setting up");
        await expect(line.locator(".phase-clock")).toHaveText(/^\d+:\d{2}$/);
        const [minutes, seconds] = (await line.locator(".phase-clock").innerText()).split(":").map(Number);
        expect(minutes! * 60 + seconds!).toBeGreaterThanOrEqual(42);
        const before = await line.locator(".phase-clock").innerText();
        await page.clock.runFor(1000);
        await expect(line.locator(".phase-clock")).not.toHaveText(before);
        expect(hub.frames.filter(frame => ["WorkEventsRequest", "WorkEventDetailRequest", "ScrollbackRequest"].includes(frame.kind))).toEqual([]);
      });
      await journey.stage("Compacting changes live; waiting has no moving dots", async () => {
        world.supervisor_phase!.lifecycle.phase = "compacting";
        // Native lifecycle changes need no unrelated fleet event, and Show
        // work is still off. The existing machine heartbeat owns refresh.
        await page.clock.runFor(5_000);
        await expect(line).toContainText("Compacting", { timeout: 5_000 });
        await page.emulateMedia({ reducedMotion: "reduce", forcedColors: "active" });
        expect(await line.locator(".dots i").first().evaluate(node => getComputedStyle(node).animationName)).toBe("none");
        await page.emulateMedia({ reducedMotion: "no-preference", forcedColors: "none" });
        world.supervisor_phase!.lifecycle.phase = "waiting_on_you";
        await refresh(); await expect(line).toContainText("Waiting on you");
        await expect(line).toHaveAttribute("data-active", "false");
        expect(await line.locator(".dots i").first().evaluate(node => getComputedStyle(node).animationName)).toBe("none");
        await expect(line.locator(".phase-clock")).toHaveCount(0);
      });
      await journey.stage("Stale data freezes the observed duration", async () => {
        world.supervisor_phase!.lifecycle.phase = "compacting";
        world.supervisor_phase!.stale = true;
        await refresh(); await expect(line).toContainText("Update stale");
        await expect(line.locator(".phase-clock")).toHaveText("0:42");
        await expect(line).toHaveAttribute("data-active", "false");
      });
      await journey.stage("A completed boundary and a disconnected machine never look active", async () => {
        world.supervisor_phase!.stale = false;
        world.supervisor_phase!.lifecycle = { id: "boundary", stage: "phase", phase: "compacted", outcome: "success" };
        await refresh(); await expect(line).toContainText("Compacted");
        await expect(line).not.toContainText("Compacting");
        await expect(line.locator(".phase-clock")).toHaveCount(0);
        // The machine hub can stay live while this supervisor's stream is gone.
        hub.hold(PELICAN); hub.drop(PELICAN);
        await expect(line).toContainText("Disconnected");
        await expect(line).toHaveAttribute("data-active", "false");
        await hub.down("atlas", { sockets: "close" });
        await expect(line).toContainText("Disconnected");
        await expect(line).toHaveAttribute("data-active", "false");
      });
    });

    // One catalog journey (HUB-J25), run as a part per surface and width.
    test(`HUB-J25 show work ${width} ${colorScheme} cas_ff22`, journeyPart, showWork);
    test(`HUB-J25 worker timeline ${width} ${colorScheme} cas_ff22`, journeyPart, timeline);
  });
}
