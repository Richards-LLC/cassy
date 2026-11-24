import type { Locator, Page } from "@playwright/test";
import { clickConversationHeaderAction, test, expect, journeyPart, type Journey } from "./journey";
import { journeyStamp } from "./clock";
import type { FleetWorld, Machine, WorkEventWire, WorkWorld } from "./hub-double";
import { ATLAS, PELICAN } from "./world";
import type { WorkTool, WorkLifecycle, WorkReasoning, WorkChangedFiles } from "../../src/work-events";

// cas-ff22 (hub-web/whole-picture.brief.md): Show work puts the supervisor's
// thinking, tool calls and task events between its replies, collapsed by
// default, and each worker's timeline is a tap away in Tasks & progress.

const WORKER = "swift-lark-3";
const event = (id: string, kind: string, label: string, offsetMs: number, extra: Partial<WorkEventWire> & { tool?: WorkTool; lifecycle?: WorkLifecycle; reasoning?: WorkReasoning; changed_files?: WorkChangedFiles } = {}): WorkEventWire =>
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

// A row is named "<project> on <machine>, …"; the row's own controls are not.
const conversationRow = (page: Page) => page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /^cas-src on / });
const workRows = (scope: Locator) => scope.getByTestId("work-row");

async function openConversation(page: Page, journey: Journey): Promise<void> {
  await journey.open();
  await conversationRow(page).click();
  await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
}

for (const [width, colorScheme] of [[1280, "light"], [390, "dark"]] as const) {
  test.describe(`whole picture ${width} ${colorScheme} cas_ff22`, () => {
    test.use({ viewport: { width, height: 844 }, colorScheme, ...(width === 390 ? { hasTouch: true, isMobile: true } : {}) });

    const timeline = async ({ page, journey }: { page: Page; journey: Journey }) => {
      const hub = await journey.hub({ machines: [atlas], paired: ["atlas"], history, work: { [PELICAN]: work() }, fleet: { [PELICAN]: fleet() } });
      const dialog = page.getByRole("dialog", { name: `${WORKER} timeline` });
      const open = page.getByRole("button", { name: `${WORKER} timeline`, exact: true });

      await journey.stage("Open a worker's timeline from Tasks & progress", async () => {
        await openConversation(page, journey);
        if (width === 390) await clickConversationHeaderAction(page, "#conversation-fleet");
        const cardState = await page.getByTestId("status-agent").filter({ has: open }).getByTestId("status-chip").textContent();
        await open.click();
        await expect(dialog).toBeVisible();
        const subject = dialog.getByTestId("worker-timeline-subject");
        await expect(subject).toHaveAttribute("title", "cas-42");
        expect((await subject.textContent())?.startsWith(fleet().tasks![0]!.title)).toBe(true);
        await expect(subject.getByTestId("status-chip")).toHaveText(cardState!);
        const rows = workRows(dialog);
        await expect(rows.first()).toContainText("Read: src/limits.rs");
        await expect(dialog).not.toContainText("Run the tests");
        const box = (await dialog.boundingBox())!;
        if (width === 390) expect(Math.round(box.y + box.height), "a bottom sheet on the phone").toBe(844);
        else expect(Math.round(box.x + box.width), "a drawer on the right edge").toBe(width);
        const request = hub.frames.find((frame) => frame.kind === "WorkEventsRequest" && (frame.body as { agent?: string }).agent === WORKER);
        expect(request, "the timeline reads the worker's own stream").toBeTruthy();
        // cas-c74f: the worker's name in the terminal note reads whole, and
        // the raw-output button lines up with the note's text.
        const layout = await dialog.evaluate((node, worker) => {
          const note = [...node.querySelectorAll<HTMLElement>(".worker-timeline-note")].find((p) => p.textContent!.includes("own terminal"))!;
          const walker = document.createTreeWalker(note, NodeFilter.SHOW_TEXT);
          let text: Node | null;
          while ((text = walker.nextNode()) && !text.textContent!.includes(worker));
          const range = document.createRange();
          const at = text!.textContent!.indexOf(worker);
          range.setStart(text!, at); range.setEnd(text!, at + worker.length);
          const lines = new Set([...range.getClientRects()].map((rect) => Math.round(rect.top))).size;
          const textLeft = document.createRange(); textLeft.selectNodeContents(note);
          const button = node.querySelector<HTMLElement>(".worker-timeline-raw")!.getBoundingClientRect().left;
          return { lines, offset: Math.round(button - textLeft.getClientRects()[0].left) };
        }, WORKER);
        expect(layout, "the worker name reads whole and the button meets the text edge").toEqual({ lines: 1, offset: 0 });
      });

      await journey.stage("The timeline follows the worker live", async () => {
        // The initial Read is 400ms before fixture creation. Stamp this
        // append now so it is newer even without the recorder's stage delay.
        hub.appendWork(PELICAN, event("t:w:2:0", "tool", "Bash: Run the lane tests", 0, { agent: WORKER }));
        await expect(workRows(dialog).first()).toContainText("Bash: Run the lane tests");
      });

      await journey.stage("Close the timeline back to its worker row", async () => {
        await page.keyboard.press("Escape");
        await expect(dialog).toBeHidden();
        await expect(open).toBeFocused();
      });
    };

    test(`HUB-J25 live phase ${width} ${colorScheme}`, journeyPart, async ({ page, journey }) => {
      const world = fleet();
      world.supervisor_phase = { event_id: "phase-1", kind: "notice", lifecycle: { id: "turn-1", stage: "phase", phase: "setting_up", started_at: journeyStamp(-42_000) }, observed_at: journeyStamp(), stale: false };
      const hub = await journey.hub({ machines: [atlas], paired: ["atlas"], multiplex: true, history, fleet: { [PELICAN]: world } });
      // The thread's one status line: the supervisor's reported phase.
      const line = page.getByRole("log").getByRole("status");
      const refresh = async () => page.evaluate(session => (window as unknown as { __journeyMachineEvent: (host: string, data: string) => number }).__journeyMachineEvent("atlas.test", JSON.stringify({ kind: "fleet_changed", session })), PELICAN);
      await journey.stage("Read the reported phase without opening Show work", async () => {
        await openConversation(page, journey);
        await expect(line).toBeVisible();
        await expect(line.getByRole("timer")).toHaveText(/^\d+:\d{2}$/);
        const [minutes, seconds] = (await line.getByRole("timer").innerText()).split(":").map(Number);
        expect(minutes! * 60 + seconds!).toBeGreaterThanOrEqual(42);
        const before = await line.getByRole("timer").innerText();
        await page.clock.runFor(1000);
        await expect(line.getByRole("timer")).not.toHaveText(before);
        expect(hub.frames.filter(frame => ["WorkEventsRequest", "WorkEventDetailRequest", "ScrollbackRequest"].includes(frame.kind))).toEqual([]);
      });
      await journey.stage("Compacting changes live; waiting has no moving dots", async () => {
        world.supervisor_phase!.lifecycle.phase = "compacting";
        // Native lifecycle changes need no unrelated fleet event, and Show
        // work is still off. The existing machine heartbeat owns refresh.
        await page.clock.runFor(5_000);
        await expect(line).toContainText("Compacting", { timeout: 5_000 });
        await page.emulateMedia({ reducedMotion: "reduce", forcedColors: "active" });
        expect(await line.getByTestId("dots").evaluate(node => getComputedStyle(node.querySelector("i")!).animationName)).toBe("none");
        await page.emulateMedia({ reducedMotion: "no-preference", forcedColors: "none" });
        world.supervisor_phase!.lifecycle.phase = "waiting_on_you";
        await refresh(); await expect(line).toBeVisible();
        await expect(line).toHaveAttribute("data-active", "false");
        expect(await line.getByTestId("dots").evaluate(node => getComputedStyle(node.querySelector("i")!).animationName)).toBe("none");
        await expect(line.getByRole("timer")).toHaveCount(0);
      });
      await journey.stage("Stale data freezes the observed duration", async () => {
        world.supervisor_phase!.lifecycle.phase = "compacting";
        world.supervisor_phase!.stale = true;
        await refresh(); await expect(line).toBeVisible();
        await expect(line.getByRole("timer")).toHaveText("0:42");
        await expect(line).toHaveAttribute("data-active", "false");
      });
      // cas-c3377: one Nov 13 turn ran release.sh for minutes between short
      // tools. The daemon reports that turn, its running tool and last record;
      // the double advances its observation time as the daemon's reads would.
      const turnStart = Date.parse(journeyStamp());
      let observed = turnStart;
      const daemonReads = async (ms: number) => {
        for (let spent = 0; spent < ms; spent += 5_000) {
          observed += 5_000; world.supervisor_phase!.observed_at = new Date(observed).toISOString();
          await page.clock.runFor(5_000);
        }
      };
      await journey.stage("A two-minute command keeps the turn busy, named and timed", async () => {
        world.session = { is_running: true };
        world.supervisor_phase = { event_id: "turn-2", kind: "notice", lifecycle: { id: "turn-2", stage: "start", started_at: new Date(turnStart).toISOString() },
          step: "Bash", last_activity_at: new Date(turnStart).toISOString(), observed_at: new Date(turnStart).toISOString(), stale: false };
        await daemonReads(5_000);
        await expect(line).toHaveAttribute("data-state", "busy");
        await expect(line.getByTestId("phase-step")).toHaveText("Bash");
        await expect(line.locator("time")).toHaveAttribute("datetime", world.supervisor_phase.last_activity_at!);
        await daemonReads(120_000);
        await expect(line).toHaveAttribute("data-state", "busy");
        await expect(line.getByRole("timer")).toHaveText(/^2:\d{2}$/);
      });
      await journey.stage("The turn's end reads idle within five seconds", async () => {
        world.supervisor_phase!.lifecycle = { ...world.supervisor_phase!.lifecycle, stage: "end", ended_at: new Date(observed).toISOString() };
        delete world.supervisor_phase!.step;
        await daemonReads(5_000);
        await expect(line).toHaveAttribute("data-state", "idle");
        await expect(line.getByTestId("phase-step")).toHaveCount(0);
      });
      await journey.stage("A stopped daemon reads unknown, never idle", async () => {
        world.session = { is_running: false };
        await daemonReads(5_000);
        await expect(line).toHaveAttribute("data-state", "unknown");
        world.session = { is_running: true };
      });
      await journey.stage("A completed boundary and a disconnected machine never look active", async () => {
        world.supervisor_phase!.stale = false;
        world.supervisor_phase!.lifecycle = { id: "boundary", stage: "phase", phase: "compacted", outcome: "success" };
        await refresh(); await expect(line).toBeVisible();
        await expect(line).not.toContainText("Compacting");
        await expect(line.getByRole("timer")).toHaveCount(0);
        // The machine hub can stay live while this supervisor's stream is gone.
        hub.hold(PELICAN); hub.drop(PELICAN);

        await expect(line).toHaveAttribute("data-active", "false");
        await hub.down("atlas", { sockets: "close" });

        await expect(line).toHaveAttribute("data-active", "false");
      });
    });

    // One catalog journey (HUB-J25), run as a part per surface and width.
    // Both show-work cases were release-path flakes (390 dark: v4.40, cas-94a8;
    // 1280 light: cas-929df; family cas-d755) and are removed.
    test(`HUB-J25 worker timeline ${width} ${colorScheme} cas_ff22`, journeyPart, timeline);
  });
}
