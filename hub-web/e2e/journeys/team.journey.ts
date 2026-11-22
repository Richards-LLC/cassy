import type { Locator, Page } from "@playwright/test";
import { clickConversationHeaderAction, test, expect, journeyPart } from "./journey";
import { ATLAS, PELICAN } from "./world";
import { journeyNow } from "./clock";
import { mkdirSync, writeFileSync } from "node:fs";
import type { FleetWorld } from "./hub-double";

// Counts are asserted as the numbers a panel shows, in order, so a copy edit cannot pass or fail them.
const numbers = async (where: Locator) => ((await where.innerText()).match(/\d+/g) ?? []).map(Number);
const expectNumbers = (where: Locator, exact: number[]) => expect.poll(() => numbers(where)).toEqual(exact);
const expectRun = (where: Locator, run: number[]) => expect.poll(async () => { const all = await numbers(where); return all.some((_, at) => run.every((n, i) => all[at + i] === n)); }).toBe(true);

const teamPanel = (page: Page) => page.getByRole("tabpanel", { name: "Team", exact: true });
// A lane is a "Worker lanes" item headed by its worker; its delivery trail's items have no heading.
const teamLanes = (page: Page) => page.getByRole("list", { name: "Worker lanes", exact: true }).getByRole("listitem").filter({ has: page.getByRole("heading", { level: 3 }) });
const teamLane = (page: Page, worker: string) => teamLanes(page).filter({ has: page.getByRole("heading", { level: 3, name: worker, exact: true }) });
const laneState = (page: Page, worker: string) => teamLane(page, worker).getByTestId("team-state");
const statusView = (page: Page) => page.getByTestId("status-view");
const railRelease = (page: Page) => statusView(page).getByTestId("release-progress");
const workerCard = (page: Page, worker: string) => statusView(page).getByRole("article").filter({ has: page.getByRole("button", { name: `${worker} timeline`, exact: true }) });
const composerSlot = (page: Page) => page.getByTestId("conversation-composer-slot");
// A Live epics row is the item holding its toggle; its task rows do not.
const epicRow = (page: Page, epics: Locator, name: RegExp) => epics.getByRole("listitem").filter({ has: page.getByRole("button", { name }) });
const groupLabels = (tasks: Locator) => tasks.getByTestId("live-epic-group-label");

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
        await expect(teamPanel(page)).toBeHidden();
        await expect(teamLanes(page)).toHaveCount(0);
        await page.getByRole("textbox", { name: "Your message" }).fill("Keep my draft while checking the team");
        const tab = page.getByRole("tab", { name: "Conversation", exact: true });
        await tab.focus(); await page.keyboard.press("ArrowRight");
        await expect(page.getByRole("tab", { name: "Team", exact: true })).toBeFocused();
        const panel = page.getByRole("tabpanel", { name: "Team", exact: true });
        await expectNumbers(panel.getByRole("heading", { level: 2 }), [1, 2, 1]);
        expect(await page.evaluate(() => document.documentElement.scrollHeight - innerHeight)).toBeLessThanOrEqual(0);
        await expect(teamLanes(page)).toHaveCount(4);
        await teamLanes(page).last().scrollIntoViewIfNeeded();
        const lastLane = await teamLanes(page).last().boundingBox();
        expect(lastLane!.y + lastLane!.height).toBeLessThanOrEqual(page.viewportSize()!.height);
        await expect(railRelease(page)).toBeHidden();
        await panel.evaluate(element => { element.scrollTop = 0; });
        if (width === 1280) {
          // The rail stays a working control list, and the lanes out-weigh it.
          const card = workerCard(page, "swift-lark-3");
          await expect(card.getByText("swift-lark-3", { exact: true })).toBeVisible();
          await expect(card.getByRole("button", { name: /timeline/ })).toBeVisible();
          expect((await card.boundingBox())!.height).toBeLessThan(lastLane!.height);
          // Name and both controls share one line.
          const centers = await card.evaluate(row => [row.querySelector(".status-identifier")!, row.querySelector(".status-timeline")!, row.querySelector(".fleet-ops-trigger")!].map(node => { const box = node.getBoundingClientRect(); return box.top + box.height / 2; }));
          expect(Math.max(...centers) - Math.min(...centers)).toBeLessThanOrEqual(2);
        }
        await expectRun(panel, [3, 8]);
        await expect(panel).toContainText("Build the Team view");
        const model = (worker: string) => teamLane(page, worker).getByTestId("team-model");
        await expect(model("swift-lark-3")).toHaveAttribute("data-model-reported", "true");
        await expect(model("swift-lark-3")).toHaveAttribute("data-model", "gpt-6.1");
        await expect(model("brisk-wren-9")).toHaveAttribute("data-model-reported", "false");
        await expect(model("brisk-wren-9")).not.toHaveAttribute("data-model", /.*/);
        await expect(panel).not.toContainText("RAW TOOL OUTPUT");
        // Working and waiting differ by color and mark, not just by the word.
        const look = (worker: string) => teamLane(page, worker).evaluate(lane => [lane.querySelector(".team-state")!, lane.querySelector(".supervisor-state")!].map(node => { const s = getComputedStyle(node); return [s.color, s.backgroundColor, s.outlineColor, s.outlineWidth, s.borderRadius].join(" "); }));
        const [working, waiting] = [await look("swift-lark-3"), await look("brisk-wren-9")];
        expect(working[1]).not.toBe(waiting[1]);
        for (const [worker, label] of [["swift-lark-3", "Working"], ["brisk-wren-9", "Idle"]]) {
          await expect(teamLane(page, worker).getByRole("img", { name: label, exact: true })).toHaveAttribute("title", label);
        }
        // The track is an outline, so a release at zero cannot read as a full bar.
        expect(await panel.getByRole("progressbar").evaluate(bar => { const style = getComputedStyle(bar); return [style.borderTopStyle, style.backgroundColor]; })).toEqual(["solid", "rgba(0, 0, 0, 0)"]);
        // The track's connector is a graphic that carries meaning, so it holds 3:1 against its lane.
        const connector = await teamLanes(page).first().evaluate(lane => {
          const channels = (value: string) => value.match(/[\d.]+/g)!.slice(0, 3).map(Number);
          const luminance = (value: string) => { const [r, g, b] = channels(value).map(part => { const unit = part / 255; return unit <= 0.03928 ? unit / 12.92 : ((unit + 0.055) / 1.055) ** 2.4; }); return 0.2126 * r + 0.7152 * g + 0.0722 * b; };
          const [line, card] = [luminance(getComputedStyle(lane.querySelectorAll(".team-trail li")[1], "::after").backgroundColor), luminance(getComputedStyle(lane).backgroundColor)];
          return (Math.max(line, card) + 0.05) / (Math.min(line, card) + 0.05);
        });
        expect(connector).toBeGreaterThanOrEqual(3);
        await expect(panel.getByRole("region", { name: "Needs you", exact: true }).getByRole("button")).toHaveCount(0);
        await expect(composerSlot(page)).toBeHidden();
        expect(hub.frames.filter(frame => ["ScrollbackRequest", "WorkEventsRequest", "WorkEventDetailRequest"].includes(frame.kind))).toEqual([]);
      });
      await journey.stage("Same-state workers expose the blocked task before healthy peers", async () => {
        for (const agent of fleet.agents) agent.liveness = "waiting_for_input";
        fleet.tasks[2].status = "blocked";
        await page.evaluate(([host, session]) => (window as unknown as { __journeyMachineEvent: (host: string, data: string) => void }).__journeyMachineEvent(host, JSON.stringify({ kind: "fleet_changed", session })), ["atlas.test", PELICAN]);
        const panel = page.getByRole("tabpanel", { name: "Team", exact: true });
        await expect(teamLanes(page).first()).toHaveAttribute("data-worker", "brisk-wren-9");
        await expect(laneState(page, "brisk-wren-9")).toHaveText("Blocked");
        await expect(panel.getByRole("region", { name: "Needs you", exact: true }).getByRole("button")).toHaveCount(0);
        await expect(panel.getByRole("region", { name: "Needs you", exact: true })).toHaveAttribute("data-active", "false");
        await expectNumbers(panel.getByRole("heading", { level: 2 }), [1, 2, 1]);
      });
      await journey.stage("A lifecycle event updates staffing without reloading", async () => {
        fleet.tasks[2].status = "in_progress";
        fleet.agents[0].liveness = "stalled";
        fleet.agents[3].liveness = "offline";
        const started = performance.now();
        await page.evaluate(([host, session]) => (window as unknown as { __journeyMachineEvent: (host: string, data: string) => void }).__journeyMachineEvent(host, JSON.stringify({ kind: "fleet_changed", session })), ["atlas.test", PELICAN]);
        await expect(laneState(page, "swift-lark-3")).toHaveText("Stalled", { timeout: 5_000 });
        const waiting = teamLane(page, "brisk-wren-9");
        const offline = teamLane(page, "calm-heron-5");
        await expect(offline.getByRole("img", { name: "Offline", exact: true })).toHaveAttribute("title", "Offline");
        const look = (lane: typeof waiting) => lane.evaluate(node => { const pill = getComputedStyle(node.querySelector(".team-state")!); const dot = getComputedStyle(node.querySelector(".supervisor-state")!); return [pill.color, `${dot.backgroundColor} ${dot.outlineColor} ${dot.outlineWidth}`]; });
        const [waitingLook, offlineLook] = [await look(waiting), await look(offline)];
        expect(waitingLook[0]).not.toBe(offlineLook[0]);
        expect(waitingLook[1]).not.toBe(offlineLook[1]);
        expect(performance.now() - started).toBeLessThan(5_000);
        await expect(page.getByRole("tab", { name: "Team", exact: true })).toHaveAttribute("aria-selected", "true");
      });
      await journey.stage("Read Team with forced colors and reduced motion", async () => {
        await page.emulateMedia({ forcedColors: "active", reducedMotion: "reduce" });
        await expect(teamPanel(page)).toBeVisible();
        expect(await laneState(page, "swift-lark-3").evaluate(node => getComputedStyle(node).animationName)).toBe("none");
        const waiting = teamLane(page, "brisk-wren-9");
        const offline = teamLane(page, "calm-heron-5");
        const treatment = (lane: typeof waiting) => lane.evaluate(node => { const pill = getComputedStyle(node.querySelector(".team-state")!); const dot = getComputedStyle(node.querySelector(".supervisor-state")!); return [pill.textDecorationLine, dot.borderRadius]; });
        const [waitingLook, offlineLook] = [await treatment(waiting), await treatment(offline)];
        expect(waitingLook[0]).not.toBe(offlineLook[0]);
        expect(waitingLook[1]).not.toBe(offlineLook[1]);
        const forced = await page.evaluate(() => {
          const color = (selector: string, property: "color" | "borderBottomColor" = "color") => getComputedStyle(document.querySelector(selector)!)[property];
          return { selected: color("#team-tab", "borderBottomColor"), unselected: color("#conversation-tab", "borderBottomColor"), page: getComputedStyle(document.body).backgroundColor, row: color('.conversation-row[aria-current="true"]'), name: color('.conversation-row[aria-current="true"] .conversation-role') };
        });
        expect(forced.unselected).toBe(forced.page);
        expect(forced.selected).not.toBe(forced.unselected);
        expect(forced.name).toBe(forced.row);
        if (width === 1280) expect(await workerCard(page, "swift-lark-3").evaluate(row => getComputedStyle(row).borderTopWidth)).toBe("0px");
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
        await page.getByRole("tab", { name: "Conversation", exact: true }).click();
        await expect(teamPanel(page)).toBeHidden();
        if (width === 1280) await expect(railRelease(page)).toBeVisible();
        await expect(composerSlot(page)).toBeVisible();
        await expect(page.getByRole("textbox", { name: "Your message" })).toHaveValue("Keep my draft while checking the team");
      });
    });
  });
}

for (const width of [1280, 390] as const) for (const colorScheme of ["light", "dark"] as const) {
  test.describe(`Project epics ${width} ${colorScheme}`, () => {
    test.use({ viewport: { width, height: width === 1280 ? 800 : 844 }, colorScheme });
    test(`HUB-J26 project epics in Tasks & progress ${width} ${colorScheme}`, journeyPart, async ({ page, journey }) => {
      const at = new Date(journeyNow()).toISOString();
      const local: FleetWorld = {
        agents: ["builder", "reviewer"].map((name, i) => ({ name, generation: 1, status: "active", current_task: i ? "rail-1" : "train-0", liveness: "executing" })),
        tasks: [{ id: "train-0", title: "Check the release", status: "in_progress", epic_id: "train", assignee: "builder", updated_at: at },
          { id: "rail-1", title: "Build the progress rail", status: "in_progress", epic_id: "rail", assignee: "reviewer", updated_at: at },
          { id: "rail-2", title: "Wait on the design review", status: "blocked", epic_id: "rail", updated_at: at },
          { id: "rail-3", title: "Park the token pass", status: "awaiting_merge", epic_id: "rail", updated_at: at },
          // The producer's split counts exactly these rows (session.rs epic_counts).
          ...[1, 2, 3].flatMap(n => [{ id: `train-open-${n}`, title: `Queue release step ${n}`, status: "open", epic_id: "train", updated_at: at },
            { id: `rail-open-${n}`, title: `Queue rail step ${n}`, status: "open", epic_id: "rail", updated_at: at }])],
        epics: [{ id: "train", title: "Release train", status: "inprogress", counts: { open: 3, in_progress: 1, merged: 1 }, progress: { merged: 1, total: 5, merges_per_hour: 1, last_merged_at: at } },
          { id: "rail", title: "EPIC: Commander progress rail", status: "inprogress", counts: { open: 4, in_progress: 2, merged: 6 } },
          { id: "untitled", title: "EPIC:", status: "inprogress" }, { id: "quiet", title: "Someday", status: "open" }],
        focused_epic: "train", spawnNames: [],
      };
      const follow: FleetWorld = {
        agents: [{ name: "remote-worker", generation: 1, status: "active", current_task: "follow-1", liveness: "waiting_for_input" }],
        tasks: [{ id: "follow-1", title: "Follow up on the release", epic_id: "follow-ups", status: "awaiting_merge", assignee: "remote-worker", updated_at: at },
          ...[1, 2].map(n => ({ id: `follow-open-${n}`, title: `Queue follow-up ${n}`, status: "open", epic_id: "follow-ups", updated_at: at }))],
        epics: [{ id: "follow-ups", title: "Follow-ups", status: "open", counts: { open: 2, in_progress: 1, merged: 2 } },
          { id: "rail", title: "EPIC: Commander progress rail", status: "inprogress", counts: { open: 4, in_progress: 2, merged: 6 } }],
        focused_epic: null, spawnNames: [],
      };
      const foreign: FleetWorld = {
        agents: [{ name: "voice-worker", generation: 1, status: "active", current_task: "voice-1", liveness: "executing" }],
        tasks: [{ id: "voice-1", title: "Tune the voice model", epic_id: "voice", status: "in_progress", assignee: "voice-worker", updated_at: at }],
        epics: [{ id: "voice", title: "Gabber voice", status: "inprogress", counts: { open: 1, in_progress: 1, merged: 0 } }], focused_epic: null, spawnNames: [],
      };
      const remote = { id: "prowl", label: "prowl", sessions: [
        { name: "remote-session", supervisor: "remote-supervisor", project_dir: "/projects/cas-src", workers: ["remote-worker"], liveness: "live" as const },
        { name: "gabber-session", supervisor: "gabber-supervisor", project_dir: "/projects/gabber", workers: ["voice-worker"], liveness: "live" as const },
      ] };
      const machine = { ...ATLAS, sessions: ATLAS.sessions.map(session => ({ ...session, workers: ["builder", "reviewer"] })) };
      const statusReads: string[] = [];
      page.on("request", request => { const match = /\/v1\/sessions\/([^/]+)\/status$/.exec(new URL(request.url()).pathname); if (match) statusReads.push(decodeURIComponent(match[1]!)); });
      await journey.hub({ machines: [machine, remote], paired: [ATLAS.id, remote.id], fleet: { [PELICAN]: local, "remote-session": follow, "gabber-session": foreign } });
      const status = statusView(page);
      const epics = status.getByTestId("live-epics");
      await journey.stage("Read the project's live epics in Tasks & progress, with nothing above the conversation", async () => {
        await journey.open();
        await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src on Atlas/ }).click();
        // Only the hidden Team panel sits between the view tabs and the conversation.
        expect(await page.evaluate(() => {
          const between: string[] = [];
          for (let node = document.querySelector(".conversation-tabs")?.nextElementSibling; node && node.id !== "conversation-pane-slot"; node = node.nextElementSibling) if (!(node as HTMLElement).hidden) between.push(node.id || node.className);
          return between;
        })).toEqual([]);
        const tabs = (await page.getByRole("tablist", { name: "Conversation views", exact: true }).boundingBox())!;
        expect(tabs.y + tabs.height).toBeLessThanOrEqual((await page.getByRole("tabpanel", { name: "Conversation", exact: true }).boundingBox())!.y);
        if (width === 390) await clickConversationHeaderAction(page, "#conversation-fleet");
        await expectNumbers(epics.getByRole("heading", { level: 3 }).first(), [3]);
        expect(await status.evaluate(root => Boolean(root.querySelector(".release-progress")!.compareDocumentPosition(root.querySelector(".live-epics")!) & Node.DOCUMENT_POSITION_FOLLOWING))).toBe(true);
        await expectRun(railRelease(page), [1, 5]);
        await expectRun(railRelease(page), [3, 1, 1]);
        await expectRun(railRelease(page), [4]);
        await expectRun(railRelease(page), [1, 1]);
        const rail = epics.getByRole("button", { name: /Commander progress rail/ });
        await expect(rail).toContainText("6/12");
        await expectRun(rail, [4, 2, 6, 1]);
        await expect(rail.getByRole("img", { name: "Commander progress rail: 4 open · 2 in progress · 6 merged" })).toBeVisible();
        // Blocked work leads; the train, untitled and foreign epics never appear.
        expect(await epics.getByRole("button").evaluateAll(nodes => nodes.map(node => (node as HTMLElement).dataset.fleetFocus))).toEqual(["epic:rail", "epic:follow-ups", "epic:quiet"]);
        await expect(status).not.toContainText("Gabber voice");
        await expect(status).not.toContainText("EPIC");
        await expect(status).not.toContainText("—/—");
        await expect(epics.getByTestId("live-epics-footer")).toHaveCount(0);
        // cas-da546: each project card reads its own project's status, so the foreign project is read for its card only.
        expect(statusReads).toContain("gabber-session");
        expect(statusReads).toContain("remote-session");
        expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(0);
        if (process.env.EPIC_PROGRESS_QA) {
          mkdirSync(process.env.EPIC_PROGRESS_QA, { recursive: true });
          await page.screenshot({ path: `${process.env.EPIC_PROGRESS_QA}/epic-progress-${width}-${colorScheme}.png` });
          writeFileSync(`${process.env.EPIC_PROGRESS_QA}/epic-progress-${width}-${colorScheme}.html`, await page.evaluate(async () => {
            const css = await Promise.all([...document.querySelectorAll<HTMLLinkElement>('link[rel="stylesheet"]')].map(async link => {
              let text = await (await fetch(link.href)).text();
              for (const asset of [...text.matchAll(/url\(["']?([^)'"\s]+\.woff2)["']?\)/g)]) {
                const bytes = new Uint8Array(await (await fetch(new URL(asset[1]!, link.href))).arrayBuffer());
                let binary = ""; for (const byte of bytes) binary += String.fromCharCode(byte);
                text = text.replace(asset[0], `url(data:font/woff2;base64,${btoa(binary)})`);
              }
              return text;
            }));
            const root = document.documentElement.cloneNode(true) as HTMLElement;
            root.querySelectorAll("script, link").forEach(node => node.remove());
            return `<!doctype html>${root.outerHTML}`.replace("</head>", `<style>${css.join("\n")}</style></head>`)
              .replace("</body>", '<script>document.querySelectorAll("dialog[open]").forEach(dialog => { dialog.removeAttribute("open"); dialog.showModal(); });</script></body>');
          }));
        }
      });
      await journey.stage("Open the peer epic in place and watch a merge update its split", async () => {
        const peer = epics.getByRole("button", { name: /Follow-ups/ });
        const tasks = epicRow(page, epics, /Follow-ups/).getByTestId("live-epic-tasks");
        await expect(tasks).toBeHidden();
        await peer.focus(); await page.keyboard.press("Enter");
        await expect(peer).toHaveAttribute("aria-expanded", "true");
        await expect(tasks).toContainText("Follow up on the release");
        await expect(tasks).not.toContainText("follow-1");
        const parked = tasks.getByRole("listitem").filter({ hasText: "Follow up on the release" });
        await expect(parked.getByTestId("live-epic-task-machine")).toHaveText("prowl");
        await expect(parked.getByTestId("live-epic-chip")).toHaveClass(/live-epic-chip--parked/);
        await expect.poll(async () => (await groupLabels(tasks).allTextContents()).map(text => Number(text.match(/\d+/)?.[0]))).toEqual([1, 2, 2]);
        await expectRun(peer, [2, 1, 2]);
        // The delivery merges while its task is still parked and its worker still live:
        // the row moves to Merged and the split moves with it.
        follow.tasks[0].delivery_state = "merged";
        await page.evaluate(([host, session]) => (window as unknown as { __journeyMachineEvent: (host: string, data: string) => void }).__journeyMachineEvent(host, JSON.stringify({ kind: "fleet_changed", session })), ["prowl.test", "remote-session"]);
        await expectRun(peer, [2, 0, 3]);
        await expect(peer).toContainText("3/5");
        await expect(groupLabels(tasks)).toHaveText(["Open · 2", "Merged · 3"]);
        await tasks.getByRole("group").getByTestId("live-epic-group-label").click();
        await expect(tasks.getByRole("group")).toContainText("Follow up on the release");
        await expect(peer).toHaveAttribute("aria-expanded", "true");
        // cas-da546: each project card reads its own project's status, so the foreign project is read for its card only.
        expect(statusReads).toContain("gabber-session");
        if (process.env.EPIC_PROGRESS_QA) await page.screenshot({ path: `${process.env.EPIC_PROGRESS_QA}/epic-progress-${width}-${colorScheme}-expanded.png` });
        if (width === 390) {
          await page.keyboard.press("Escape");
          await expect(page.getByRole("button", { name: "More conversation actions", exact: true })).toBeFocused();
        }
      });
      await journey.stage("Read Live epics with forced colors and reduced motion", async () => {
        await page.emulateMedia({ forcedColors: "active", reducedMotion: "reduce" });
        if (width === 390) await clickConversationHeaderAction(page, "#conversation-fleet");
        await expect(epics.getByRole("button", { name: /Follow-ups/ })).toBeVisible();
        const opening = epicRow(page, epics, /Follow-ups/);
        await opening.evaluate(node => node.classList.add("is-opening"));
        expect(await opening.getByTestId("live-epic-tasks").evaluate(node => getComputedStyle(node).animationName)).toBe("none");
        expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(0);
      });
    });
  });
}
