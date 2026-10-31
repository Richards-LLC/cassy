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
        await expect(panel.getByRole("heading", { level: 2 })).toHaveText("1 working, 2 waiting, 1 parked. 1 needs you.");
        expect(await page.evaluate(() => document.documentElement.scrollHeight - innerHeight)).toBeLessThanOrEqual(0);
        await expect(panel.locator(".team-lane")).toHaveCount(4);
        // The whole team fits above the fold, and the release block has one home.
        const lastLane = await panel.locator(".team-lane").last().boundingBox();
        expect(lastLane!.y + lastLane!.height).toBeLessThanOrEqual(page.viewportSize()!.height);
        await expect(page.locator(".conversation-context .release-progress")).toBeHidden();
        if (width === 1280) {
          // The rail stays a working control list, and the lanes out-weigh it.
          const card = page.locator(".conversation-context .status-agent").first();
          await expect(card.getByText("swift-lark-3", { exact: true })).toBeVisible();
          await expect(card.getByRole("button", { name: /timeline/ })).toBeVisible();
          expect((await card.boundingBox())!.height).toBeLessThan(lastLane!.height);
          // Name and both controls share one line.
          const centers = await card.evaluate(row => [row.querySelector(".status-identifier")!, row.querySelector(".status-timeline")!, row.querySelector(".fleet-ops-trigger")!].map(node => { const box = node.getBoundingClientRect(); return box.top + box.height / 2; }));
          expect(Math.max(...centers) - Math.min(...centers)).toBeLessThanOrEqual(2);
        }
        await expect(panel).toContainText("3 of 8 merged");
        await expect(panel).toContainText("Build the Team view");
        await expect(panel.locator('.team-lane[data-worker="swift-lark-3"] .team-model')).toHaveText("codex · gpt-6.1");
        await expect(panel.locator('.team-lane[data-worker="brisk-wren-9"] .team-model')).toHaveText("codex · Model not reported");
        await expect(panel).not.toContainText("RAW TOOL OUTPUT");
        // Working and waiting differ by color and mark, not just by the word.
        const look = (worker: string) => panel.locator(`.team-lane[data-worker="${worker}"]`).evaluate(lane => [lane.querySelector(".team-state")!, lane.querySelector(".supervisor-state")!].map(node => getComputedStyle(node).color + getComputedStyle(node).backgroundColor));
        const [working, waiting] = [await look("swift-lark-3"), await look("brisk-wren-9")];
        expect(working[0]).not.toBe(waiting[0]);
        expect(working[1]).not.toBe(waiting[1]);
        // The track is an outline, so a release at zero cannot read as a full bar.
        expect(await panel.locator(".release-progress progress").evaluate(bar => { const style = getComputedStyle(bar); return [style.borderTopStyle, style.backgroundColor]; })).toEqual(["solid", "rgba(0, 0, 0, 0)"]);
        // The track's connector is a graphic that carries meaning, so it holds 3:1 against its lane.
        const connector = await panel.locator(".team-lane").first().evaluate(lane => {
          const channels = (value: string) => value.match(/[\d.]+/g)!.slice(0, 3).map(Number);
          const luminance = (value: string) => { const [r, g, b] = channels(value).map(part => { const unit = part / 255; return unit <= 0.03928 ? unit / 12.92 : ((unit + 0.055) / 1.055) ** 2.4; }); return 0.2126 * r + 0.7152 * g + 0.0722 * b; };
          const [line, card] = [luminance(getComputedStyle(lane.querySelectorAll(".team-trail li")[1], "::after").backgroundColor), luminance(getComputedStyle(lane).backgroundColor)];
          return (Math.max(line, card) + 0.05) / (Math.min(line, card) + 0.05);
        });
        expect(connector).toBeGreaterThanOrEqual(3);
        const merges = panel.locator('[data-team-action="merges"]');
        expect(await merges.evaluate(node => getComputedStyle(node).backgroundColor)).not.toBe("rgba(0, 0, 0, 0)");
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
        const forced = await page.evaluate(() => {
          const color = (selector: string, property: "color" | "borderBottomColor" = "color") => getComputedStyle(document.querySelector(selector)!)[property];
          return { selected: color("#team-tab", "borderBottomColor"), unselected: color("#conversation-tab", "borderBottomColor"), page: getComputedStyle(document.body).backgroundColor, row: color('.conversation-row[aria-current="true"]'), name: color('.conversation-row[aria-current="true"] .conversation-supervisor') };
        });
        expect(forced.unselected).toBe(forced.page);
        expect(forced.selected).not.toBe(forced.unselected);
        expect(forced.name).toBe(forced.row);
        if (width === 1280) expect(await page.locator(".conversation-context .status-agent").first().evaluate(row => getComputedStyle(row).borderTopWidth)).toBe("0px");
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
