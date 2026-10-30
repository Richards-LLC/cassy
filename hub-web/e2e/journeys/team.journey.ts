import { test, expect, journeyPart } from "./journey";
import { ATLAS, PELICAN } from "./world";
import { journeyNow } from "./clock";
import type { FleetWorld } from "./hub-double";

for (const [width, colorScheme] of [[1280, "light"], [390, "dark"]] as const) {
  test.describe(`Team staffing ${width}`, () => {
    test.use({ viewport: { width, height: width === 1280 ? 800 : 844 }, colorScheme });
    test(`HUB-J26 see live staffing without opening output ${width}`, journeyPart, async ({ page, journey }) => {
      const names = ["swift-lark-3", "quiet-owl-7", "brisk-wren-9", "calm-heron-5"];
      const fleet: FleetWorld = {
        agents: names.map((name, i) => ({ name, generation: 1, status: "active", current_task: `task-${i}`, liveness: i === 0 ? "executing" : "waiting_for_input",
          harness: "codex", model: i === 2 ? undefined : "gpt-6.1", task_started_at: new Date(journeyNow() - 42 * 60_000).toISOString(),
          recent_trail: [{ step: "started", reference: "event:1" }], latest_activity: { summary: "RAW TOOL OUTPUT MUST STAY PRIVATE" } })),
        tasks: names.map((name, i) => ({ id: `task-${i}`, title: ["Build the Team view", "Review the delivery", "Write the notes", "Check the release"][i], status: i === 1 ? "awaiting_merge" : "in_progress", epic_id: "release", assignee: name, updated_at: new Date(journeyNow()).toISOString() })),
        epics: [{ id: "release", title: "Commander staffing release", status: "inprogress", progress: { total: 8, merged: 3, merges_per_hour: 1.5, last_merged_at: new Date(journeyNow()).toISOString() } }], focused_epic: "release", spawnNames: [],
      };
      const machine = { ...ATLAS, sessions: ATLAS.sessions.map(session => ({ ...session, workers: names })) };
      const hub = await journey.hub({ machines: [machine], paired: [ATLAS.id], fleet: { [PELICAN]: fleet } });
      await journey.stage("Open Team beside the supervisor conversation", async () => {
        await journey.open();
        await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src on Atlas/ }).click();
        await expect(page.locator("#team-panel")).toBeHidden();
        await expect(page.locator(".team-lane")).toHaveCount(0);
        await page.getByRole("textbox", { name: "Your message" }).fill("Keep my draft while checking the team");
        const tab = page.getByRole("tab", { name: "Conversation", exact: true });
        await tab.focus(); await page.keyboard.press("ArrowRight");
        await expect(page.getByRole("tab", { name: "Team", exact: true })).toBeFocused();
        const panel = page.getByRole("tabpanel", { name: "Team", exact: true });
        await expect(panel.getByRole("heading", { name: "Your team, at a glance." })).toBeVisible();
        await expect(panel.locator(".team-lane")).toHaveCount(4);
        // The whole team fits above the fold, and the release block has one home.
        const lastLane = await panel.locator(".team-lane").last().boundingBox();
        expect(lastLane!.y + lastLane!.height).toBeLessThanOrEqual(page.viewportSize()!.height);
        await expect(page.locator(".conversation-context .release-progress")).toBeHidden();
        await expect(panel).toContainText("3 of 8 merged");
        await expect(panel).toContainText("Build the Team view");
        await expect(panel.locator('.team-lane[data-worker="swift-lark-3"] .team-model')).toHaveText("codex · gpt-6.1");
        await expect(panel.locator('.team-lane[data-worker="brisk-wren-9"] .team-model')).toHaveText("codex · Model not reported");
        await expect(panel).not.toContainText("RAW TOOL OUTPUT");
        await expect(page.locator("#conversation-composer-slot")).toBeHidden();
        expect(hub.frames.filter(frame => ["ScrollbackRequest", "WorkEventsRequest", "WorkEventDetailRequest"].includes(frame.kind))).toEqual([]);
      });
      await journey.stage("A lifecycle event updates staffing without reloading", async () => {
        fleet.agents[0].liveness = "stalled";
        const started = performance.now();
        await page.evaluate(([host, session]) => (window as unknown as { __journeyMachineEvent: (host: string, data: string) => void }).__journeyMachineEvent(host, JSON.stringify({ kind: "fleet_changed", session })), ["atlas.test", PELICAN]);
        await expect(page.locator('.team-lane[data-worker="swift-lark-3"] .team-state')).toHaveText("Stalled", { timeout: 5_000 });
        expect(performance.now() - started).toBeLessThan(5_000);
        await expect(page.getByRole("tab", { name: "Team", exact: true })).toHaveAttribute("aria-selected", "true");
      });
      await journey.stage("Read Team with forced colors and reduced motion", async () => {
        await page.emulateMedia({ forcedColors: "active", reducedMotion: "reduce" });
        await expect(page.locator("#team-panel")).toBeVisible();
        expect(await page.locator(".team-state").first().evaluate(node => getComputedStyle(node).animationName)).toBe("none");
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
        await page.getByRole("tab", { name: "Conversation", exact: true }).click();
        await expect(page.locator("#team-panel")).toBeHidden();
        if (width === 1280) await expect(page.locator(".conversation-context .release-progress")).toBeVisible();
        await expect(page.locator("#conversation-composer-slot")).toBeVisible();
        await expect(page.getByRole("textbox", { name: "Your message" })).toHaveValue("Keep my draft while checking the team");
      });
    });
  });
}
