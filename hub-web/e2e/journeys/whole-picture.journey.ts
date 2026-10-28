import type { Page } from "@playwright/test";
import { test, expect, journeyPart, type Journey } from "./journey";
import { journeyStamp } from "./clock";
import type { FleetWorld, Machine, WorkEventWire, WorkWorld } from "./hub-double";
import { ATLAS, PELICAN } from "./world";

// cas-ff22 (hub-web/whole-picture.brief.md): Show work puts the supervisor's
// thinking, tool calls and task events between its replies, collapsed by
// default, and each worker's timeline is a tap away in Tasks & progress.

const WORKER = "swift-lark-3";
const event = (id: string, kind: string, label: string, offsetMs: number, extra: Partial<WorkEventWire> = {}): WorkEventWire =>
  ({ id, kind, session: PELICAN, agent: PELICAN, at: journeyStamp(offsetMs), label, has_detail: kind !== "task", ...extra });

const work = (): WorkWorld => ({
  supervisor: PELICAN,
  events: [
    event("t:sup:1:0", "thinking", "Checking the lane", -170_000),
    event("t:sup:2:0", "tool", "Bash: Run the tests", -160_000, { error: true }),
    event("q:7", "task", "cas-42 parked for merge: Ship the stream", -150_000, { agent: WORKER }),
    event("t:w:1:0", "tool", "Read: src/limits.rs", -140_000, { agent: WORKER }),
  ],
  // Agent text is data: the markup in this detail must read as text.
  details: { "t:sup:2:0": "Bash\n\nInput:\ncargo test <b>-p cas</b>\n\nResult (error):\n1 failed" },
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
        const summary = blocks.getByRole("button", { name: /Thought, ran 1 tool \(1 failed\)/ });
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

      await journey.stage("New work arrives live without redrawing the thread", async () => {
        const reply = thread.locator(".conversation-turn", { hasText: "Starting the stream review." });
        await reply.evaluate((node) => { (node as HTMLElement & { journeyMark?: boolean }).journeyMark = true; });
        hub.appendWork(PELICAN, event("t:sup:3:0", "tool", "Bash: Merge the lane", -1_000));
        await expect(page.locator(".work-turn").last()).toContainText("Ran 1 tool");
        expect(await reply.evaluate((node) => (node as HTMLElement & { journeyMark?: boolean }).journeyMark === true), "the earlier reply is the same node").toBe(true);
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
        hub.appendWork(PELICAN, event("t:w:2:0", "tool", "Bash: Run the lane tests", -1_000, { agent: WORKER }));
        await expect(dialog.locator(".work-row").first()).toContainText("Bash: Run the lane tests");
      });

      await journey.stage("Close the timeline back to its worker row", async () => {
        await page.keyboard.press("Escape");
        await expect(dialog).toBeHidden();
        await expect(page.locator(`.status-timeline[data-worker="${WORKER}"]`).first()).toBeFocused();
      });
    };

    // One catalog journey (HUB-J25), run as a part per surface and width.
    test(`HUB-J25 show work ${width} ${colorScheme} cas_ff22`, journeyPart, showWork);
    test(`HUB-J25 worker timeline ${width} ${colorScheme} cas_ff22`, journeyPart, timeline);
  });
}
