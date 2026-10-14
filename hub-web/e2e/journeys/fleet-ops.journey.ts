import { test, expect, journeyPart } from "./journey";
import type { Route } from "@playwright/test";
import { SCOPES, type FleetWorld, type Machine } from "./hub-double";
import { ATLAS, STUDIO, PELICAN, OTTER } from "./world";

// cas-a474 (fleet-operations brief S5): the desktop rail runs the fleet. The
// hub double serves the brief's operations endpoint (op_id dedupe, 409 stale,
// FleetChanged), and every result is announced in one live region.
const fleet = (): FleetWorld => ({
  agents: [
    { name: "swift-lark-3", status: "active", current_task: "cas-1234", generation: 2, latest_activity: { summary: "Writing the docs lane" } },
    { name: "quiet-owl-7", status: "idle", current_task: null, generation: 1 },
    { name: "brisk-wren-9", status: "active", current_task: "cas-1500", generation: 4 },
  ],
  tasks: [
    { id: "cas-1234", title: "Docs lane", status: "in_progress", assignee: "swift-lark-3", updated_at: "2026-10-03T11:00:00Z" },
    { id: "cas-1500", title: "Release notes", status: "in_progress", assignee: "brisk-wren-9", updated_at: "2026-10-03T11:10:00Z" },
    { id: "cas-2001", title: "Footer copy", status: "open", assignee: null, updated_at: "2026-10-03T11:20:00Z" },
    { id: "cas-1999", title: "Pairing wording", status: "awaiting_merge", assignee: "brisk-wren-9", updated_at: "2026-10-03T11:30:00Z", tip: "9ffb3897", branch: "factory/wren-cas-1999" },
  ],
  epics: [{ id: "cas-f29b" }, { id: "cas-c4d3" }],
  focused_epic: "cas-f29b",
  spawnNames: ["amber-heron-11", "copper-fox-2"],
});

// The catalog is the roster; fleet status supplies each worker's details.
// Clone the shared worlds so later spawn/stop operations can update the roster
// without leaking one journey's workers into another machine or test.
const fleetMachine = (machine: Machine): Machine => ({
  ...machine,
  sessions: machine.sessions.map(session => ({
    ...session, workers: fleet().agents.map(agent => agent.name),
  })),
});

for (const width of [390, 1280]) for (const colorScheme of ["light", "dark"] as const) {
  test.describe(`fleet failure copy ${width} ${colorScheme} cas_a348`, () => {
    test.use({ viewport: { width, height: 844 }, colorScheme });
    test(`HUB-J17 fleet failure copy ${width} ${colorScheme} cas_a348`, journeyPart, async ({ page, journey }) => {
      await journey.hub({ machines: [fleetMachine(ATLAS)], paired: ["atlas"], scopes: { atlas: [...SCOPES, "factory-operate", "factory-manage"] }, fleet: { [PELICAN]: fleet() } });
      const row = page.locator("#status-view .status-agent", { hasText: "swift-lark-3" });
      const panel = () => width === 390 ? page.locator("dialog.fleet-action-sheet") : row;
      const result = page.locator("#fleet-ops-announcer");
      await journey.stage("Open the fleet", async () => {
        await journey.open();
        await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
        if (width === 390) await page.getByRole("button", { name: "Tasks & progress", exact: true }).click();
        await expect(row).toBeVisible();
      });
      await journey.stage("A server failure names the worker and offers a next step", async () => {
        await page.route("**/v1/sessions/*/operations", (route) => route.fulfill({ status: 500, contentType: "text/html", body: "Internal Server Error" }));
        await row.getByRole("button", { name: "Actions for swift-lark-3" }).click();
        await panel().getByRole("menuitem", { name: "Stop…" }).click();
        await panel().getByRole("button", { name: "Stop", exact: true }).click();
        const text = "Could not stop swift-lark-3: the machine returned an error. Try again.";
        await expect(result).toHaveText(text);
        await expect(row.locator(".fleet-ops-note")).toHaveText(text);
        expect(await row.evaluate((element) => {
          const trigger = element.querySelector(".fleet-ops-trigger")!.getBoundingClientRect();
          const range = document.createRange(); range.selectNodeContents(element.querySelector(".fleet-ops-note")!);
          return Array.from(range.getClientRects()).every((rect) => rect.bottom <= trigger.top || rect.top >= trigger.bottom || rect.right <= trigger.left);
        }), "the action button does not cover error text").toBe(true);
        // cas-d2df: the note's text uses the row's full content width, not a
        // column narrowed by the trigger's width on every line.
        expect(await row.evaluate((element) => {
          const style = getComputedStyle(element);
          const content = element.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
          const note = element.querySelector<HTMLElement>(".fleet-ops-note")!;
          return Math.round(note.clientWidth - parseFloat(getComputedStyle(note).paddingRight) - content);
        }), "the failure note spans the row").toBe(0);
        // cas-d2df: the note names its row in the row header's mono face.
        await expect(row.locator(".fleet-ops-note .fleet-ops-note-subject")).toHaveText("swift-lark-3");
        expect(await row.evaluate((element) => getComputedStyle(element.querySelector(".fleet-ops-note-subject")!).fontFamily
          === getComputedStyle(element.querySelector(".status-line .status-identifier")!).fontFamily), "same face as the row header").toBe(true);
        if (width === 390) await expect(page.locator("#fleet-phone-undo")).toContainText(text);
        await expect(row.locator(".status-chip")).toHaveText("Active");
      });
      await journey.stage("A connection failure offers connection recovery without diagnostics", async () => {
        await page.unroute("**/v1/sessions/*/operations");
        await page.route("**/v1/sessions/*/operations", (route) => route.abort("failed"));
        await row.getByRole("button", { name: "Actions for swift-lark-3" }).click();
        await panel().getByRole("menuitem", { name: "Pause" }).click();
        await expect(result).toHaveText("Could not pause swift-lark-3: the machine could not be reached. Check its connection and try again.");
        await expect(row.locator(".fleet-ops-note")).not.toContainText(/POST|\/v1\/|Failed to fetch|500/);
        await expect(row.locator(".status-chip")).toHaveText("Active");
      });
      for (const [status, reason, detail] of [
        [403, "scope_denied", "this pairing does not allow the action. Check its permissions in Paired machines."],
        [401, "revoked", "the pairing is no longer accepted. Pair the machine again."],
      ] as const) {
        await journey.stage(`A ${status} refusal names the pairing problem and recovery`, async () => {
          await page.unroute("**/v1/sessions/*/operations");
          await page.route("**/v1/sessions/*/operations", (route) => route.fulfill({ status, json: { error: "unauthorized", reason, retryable: false } }));
          await row.getByRole("button", { name: "Actions for swift-lark-3" }).click();
          await panel().getByRole("menuitem", { name: "Pause" }).click();
          const text = `Could not pause swift-lark-3: ${detail}`;
          await expect(result).toHaveText(text);
          await expect(row.locator(".fleet-ops-note")).toHaveText(text);
          await expect(row.locator(".status-chip")).toHaveText("Active");
          if (width === 390) await expect(page.locator("#fleet-phone-undo")).toContainText(text);
        });
      }
      await journey.stage("A failed assignment names the task before its assignee", async () => {
        await page.unroute("**/v1/sessions/*/operations");
        await page.route("**/v1/sessions/*/operations", (route) => route.fulfill({ status: 500, contentType: "text/html", body: "Internal Server Error" }));
        const task = page.locator("#status-view .status-task", { hasText: "cas-2001" });
        if (width === 390) {
          await task.getByRole("button", { name: "Actions for cas-2001" }).click();
          await page.locator("dialog.fleet-action-sheet").getByRole("menuitem", { name: "Assign…" }).click();
        } else await task.getByRole("button", { name: "Assign cas-2001" }).click();
        const menu = width === 390 ? page.locator("dialog.fleet-action-sheet") : task;
        await menu.getByRole("menuitem", { name: "Assign to quiet-owl-7" }).click();
        const text = "Could not assign cas-2001 to quiet-owl-7: the machine returned an error. Try again.";
        await expect(result).toHaveText(text);
        await expect(task.locator(".fleet-ops-note")).toHaveText(text);
        if (width === 390) await expect(page.locator("#fleet-phone-undo")).toContainText(text);
      });
      await journey.stage("A successful retry clears failure feedback and offers Undo", async () => {
        await page.unroute("**/v1/sessions/*/operations");
        await row.getByRole("button", { name: "Actions for swift-lark-3" }).click();
        await panel().getByRole("menuitem", { name: "Pause" }).click();
        await expect(result).toHaveText("swift-lark-3 paused.");
        await expect(row.locator(".fleet-ops-note")).toHaveCount(0);
        await expect(row.locator(".status-chip")).toHaveText("Paused");
        await expect(page.getByRole("button", { name: "Undo", exact: true })).toBeVisible();
      });
    });
  });
}

for (const colorScheme of ["light", "dark"] as const) {
  test.describe(`phone fleet feedback ${colorScheme} cas_c2e7`, () => {
    test.use({ viewport: { width: 390, height: 844 }, colorScheme });
    test("HUB-J17 phone feedback clears the composer and task rows and can be dismissed", journeyPart, async ({ page, journey }) => {
      const atlas = fleetMachine(ATLAS);
      const world = fleet();
      const hub = await journey.hub({ machines: [atlas], paired: ["atlas"], scopes: { atlas: [...SCOPES, "factory-operate", "factory-manage"] }, fleet: { [PELICAN]: world } });
      await page.route("**/v1/sessions/*/operations", (route) => route.fulfill({ status: 500, json: { error: "unavailable" } }));
      const rail = page.locator("#status-view");
      const agent = rail.locator(".status-agent", { hasText: "swift-lark-3" });
      const notice = page.locator("#fleet-phone-undo");
      const open = () => page.getByRole("button", { name: "Tasks & progress", exact: true }).click();
      await journey.stage("A refused Stop leaves the final task readable at rest", async () => {
        await journey.open();
        await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
        await open();
        await agent.getByRole("button", { name: "Actions for swift-lark-3" }).click();
        await page.locator("dialog.fleet-action-sheet").getByRole("menuitem", { name: "Stop…" }).click();
        await page.locator("dialog.fleet-action-sheet").getByRole("button", { name: "Stop", exact: true }).click();
        await expect(notice).toContainText("Could not stop");
        const task = rail.locator(".status-task").last();
        // A status refresh redraws the notice; measure once both are laid out.
        await expect.poll(async () => (await Promise.all([task.boundingBox(), notice.boundingBox()])).every(Boolean)).toBe(true);
        const [row, feedback] = await Promise.all([task.boundingBox(), notice.boundingBox()]);
        expect(row!.y + row!.height, "feedback reserves space after the last task").toBeLessThanOrEqual(feedback!.y);
        await task.scrollIntoViewIfNeeded();
        expect(await task.evaluate((row) => {
          const r = row.getBoundingClientRect();
          return row.contains(document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2));
        }), "the final task receives pointer input").toBe(true);
      });
      await journey.stage("Close the sheet; dismiss feedback by keyboard without losing the row's error", async () => {
        await page.getByRole("button", { name: "Close tasks & progress" }).click();
        const [feedback, composer] = await Promise.all([notice.boundingBox(), page.getByRole("textbox", { name: "Your message" }).boundingBox()]);
        expect(feedback!.y + feedback!.height, "feedback stays above the composer pill").toBeLessThanOrEqual(composer!.y);
        const dismiss = notice.getByRole("button", { name: "Dismiss fleet notice", exact: true });
        await dismiss.focus(); await expect(dismiss).toBeFocused();
        // Real status and shell replacements must retain this keyboard stop.
        world.tasks[0]!.title = "Docs lane refreshed";
        await page.evaluate((session) => (window as unknown as { __journeyMachineEvent: (host: string, event: string) => void }).__journeyMachineEvent("atlas.test", JSON.stringify({ kind: "fleet_changed", session })), PELICAN);
        await expect(rail.locator(".status-task", { hasText: "cas-1234" })).toContainText("Docs lane refreshed");
        await expect(dismiss).toBeFocused();
        atlas.sessions[0]!.project_dir = "/projects/cas-src-refreshed";
        await hub.announceCatalog("atlas");
        await expect(page.locator(".conversation-identity h1")).toHaveText("cas-src-refreshed");
        await expect(dismiss).toBeFocused(); await page.keyboard.press("Enter");
        await expect(notice).toHaveCount(0);
        await expect(page.getByRole("button", { name: "Tasks & progress", exact: true })).toBeFocused();
        await open(); await expect(agent.locator(".fleet-ops-note")).toContainText("Could not stop");
        await page.getByRole("button", { name: "Close tasks & progress" }).click();
        await expect(notice, "redrawing the same result does not undo Dismiss").toHaveCount(0);
      });
    });
    test("HUB-J17 phone pickers visibly identify the task and current epic", journeyPart, async ({ page, journey }) => {
      await journey.hub({ machines: [fleetMachine(ATLAS)], paired: ["atlas"], scopes: { atlas: [...SCOPES, "factory-operate", "factory-manage"] }, fleet: { [PELICAN]: fleet() } });
      const sheet = page.locator("dialog.fleet-action-sheet");
      await journey.stage("Choose an assignee while seeing the task being assigned", async () => {
        await journey.open();
        await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
        await page.getByRole("button", { name: "Tasks & progress", exact: true }).click();
        await page.locator(".status-task", { hasText: "cas-2001" }).getByRole("button", { name: "Actions for cas-2001" }).click();
        await sheet.getByRole("menuitem", { name: "Assign…" }).click();
        await expect(sheet.getByRole("heading", { name: "Assign cas-2001 to", exact: true })).toBeVisible();
        await expect(sheet.getByRole("searchbox")).toBeFocused();
        await sheet.getByRole("searchbox").fill("absent");
        await expect(sheet.getByRole("status")).toContainText("No matches");
        await expect(sheet.getByRole("heading")).toBeVisible();
        await page.keyboard.press("Escape");
      });
      await journey.stage("Choose a new focus while seeing the current epic", async () => {
        await page.locator("#status-view").getByRole("button", { name: "Focus epic…" }).click();
        await expect(sheet.getByRole("heading", { name: "Focus epic", exact: true })).toBeVisible();
        await expect(sheet.getByText("Current focus: cas-f29b", { exact: true })).toBeVisible();
        await expect(sheet.getByRole("searchbox")).toBeFocused();
        await expect(sheet.getByRole("menuitem", { name: "Focus cas-c4d3" })).toBeVisible();
        await page.keyboard.press("Escape");
        await expect(page.locator("#status-view").getByRole("button", { name: "Focus epic…" })).toBeFocused();
      });
    });
  });
}

test("HUB-J17 run the fleet from a conversation", async ({ page, journey }) => {
  const hub = await journey.hub({
    machines: [fleetMachine(ATLAS), fleetMachine(STUDIO)],
    paired: ["atlas", "studio"],
    scopes: { atlas: [...SCOPES, "factory-operate", "factory-manage"], studio: SCOPES },
    fleet: { [PELICAN]: fleet(), [OTTER]: fleet() },
  });
  const rail = page.locator("#status-view");
  const announcer = page.locator("#fleet-ops-announcer");
  const row = (name: string) => rail.locator(".status-agent", { hasText: name });
  const task = (id: string) => rail.locator(".status-task", { hasText: id });
  /** Operations the double received for a session, by kind and status. */
  const sent = (session: string) => hub.operations.filter((call) => call.session === session).map((call) => `${String((call.body.op as Record<string, unknown>).kind)}:${call.status}`);

  await journey.stage("Open the conversation; the rail lists the fleet", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
    await expect(row("swift-lark-3")).toBeVisible();
    await expect(rail.getByRole("button", { name: "Add worker…" })).toBeVisible();
    await expect(rail.getByRole("button", { name: "Focus epic…" })).toBeVisible();
  });

  await journey.stage("Pause a worker and undo it", async () => {
    const trigger = row("swift-lark-3").getByRole("button", { name: "Actions for swift-lark-3" });
    await trigger.click();
    const menu = row("swift-lark-3").getByRole("menu", { name: "Actions for swift-lark-3" });
    await expect(menu.getByRole("menuitem")).toHaveText(["Pause", "Restart…", "Stop…"]);
    await expect(menu.getByRole("menuitem", { name: "Pause" })).toBeFocused();
    await menu.getByRole("menuitem", { name: "Pause" }).click();
    await expect(announcer).toHaveText("swift-lark-3 paused.");
    await expect(row("swift-lark-3").locator(".status-chip")).toHaveText("Paused");
    await expect(rail.locator(".fleet-ops-undo")).toContainText("swift-lark-3 paused.");
    await rail.getByRole("button", { name: "Undo" }).click();
    await expect(announcer).toHaveText("swift-lark-3 resumed.");
    await expect(row("swift-lark-3").locator(".status-chip")).toHaveText("Active");
    expect(sent(PELICAN)).toEqual(["set_worker_hold:200", "set_worker_hold:200"]);
  });

  await journey.stage("Assign a ready task to an idle worker", async () => {
    await task("cas-2001").getByRole("button", { name: "Assign cas-2001" }).click();
    const menu = task("cas-2001").getByRole("menu", { name: "Assign cas-2001 to" });
    await expect(menu.getByRole("menuitem")).toHaveText(["Assign to quiet-owl-7"]);
    await menu.getByRole("menuitem", { name: "Assign to quiet-owl-7" }).click();
    await expect(announcer).toHaveText("cas-2001 assigned to quiet-owl-7.");
    await expect(row("quiet-owl-7")).toContainText("cas-2001");
    expect(hub.operations.at(-1)?.body).toMatchObject({ op: { kind: "assign_task", task_id: "cas-2001", assignee: "quiet-owl-7" }, expected: { assignee: null, updated_at: "2026-10-03T11:20:00Z" } });
  });

  await journey.stage("Stop another worker after confirming", async () => {
    await row("brisk-wren-9").getByRole("button", { name: "Actions for brisk-wren-9" }).click();
    await row("brisk-wren-9").getByRole("menuitem", { name: "Stop…" }).click();
    const confirm = row("brisk-wren-9").locator(".fleet-ops-confirm");
    await expect(confirm.locator(".fleet-ops-question")).toHaveText("Stop brisk-wren-9? Its task cas-1500 goes back to Open, for any worker to pick up.");
    await expect(confirm.getByRole("button", { name: "Cancel" })).toBeFocused();
    // Escape closes it, back to the opener; nothing was sent.
    await page.keyboard.press("Escape");
    await expect(confirm).toHaveCount(0);
    await expect(row("brisk-wren-9").getByRole("button", { name: "Actions for brisk-wren-9" })).toBeFocused();
    const before = hub.operations.length;
    await page.keyboard.press("Enter");
    await row("brisk-wren-9").getByRole("menuitem", { name: "Stop…" }).click();
    await row("brisk-wren-9").locator(".fleet-ops-confirm").getByRole("button", { name: "Stop", exact: true }).click();
    await expect(announcer).toHaveText("brisk-wren-9 stopped.");
    await expect(row("brisk-wren-9")).toHaveCount(0);
    // cas-97d58 F12: the row is gone, so its result shows where an Undo would,
    // in view and focused (never <body>, QA N1), and without an Undo.
    const result = rail.locator(".fleet-ops-result");
    await expect(result).toHaveText("brisk-wren-9 stopped.");
    await expect(result).toBeFocused();
    await expect(result).toBeInViewport();
    expect(hub.operations.length).toBe(before + 1);
    expect(hub.operations.at(-1)?.body).toMatchObject({ op: { kind: "shutdown_workers", workers: ["brisk-wren-9"] }, expected: { worker: "brisk-wren-9", generation: 4 } });
    // No Undo for a destructive action.
    await expect(rail.locator(".fleet-ops-undo")).toHaveCount(0);
    // cas-d043 G15: the task's chip says the word the confirmation promised.
    await expect(task("cas-1500").locator(".status-chip")).toHaveText("Open");
  });

  await journey.stage("A stale stop says what changed", async () => {
    // Another device restarted swift-lark-3 since this rail drew it.
    const world = (hub as unknown as { options: { fleet: Record<string, FleetWorld> } }).options.fleet[PELICAN]!;
    world.agents.find((agent) => agent.name === "swift-lark-3")!.generation = 3;
    await row("swift-lark-3").getByRole("button", { name: "Actions for swift-lark-3" }).click();
    await row("swift-lark-3").getByRole("menuitem", { name: "Stop…" }).click();
    await row("swift-lark-3").locator(".fleet-ops-confirm").getByRole("button", { name: "Stop", exact: true }).click();
    await expect(row("swift-lark-3").locator(".fleet-ops-note")).toHaveText("swift-lark-3 already restarted.");
    await expect(announcer).toHaveText("swift-lark-3 already restarted.");
    expect(sent(PELICAN).at(-1)).toBe("shutdown_workers:409");
    await expect(row("swift-lark-3")).toBeVisible();
  });

  await journey.stage("Ask the supervisor to merge, after seeing the exact message", async () => {
    await task("cas-1999").getByRole("button", { name: "Ask supervisor to merge" }).click();
    const preview = task("cas-1999").locator(".fleet-ops-preview");
    await expect(preview.locator(".fleet-ops-preview-text")).toHaveText("Operator request from Commander: please merge cas-1999 (Pairing wording).\nBranch: factory/wren-cas-1999\nTip: 9ffb3897\nIt is awaiting merge. Merge it into its epic, or reply with what blocks it.");
    await expect(preview.getByRole("button", { name: "Cancel" })).toBeFocused();
    await preview.getByRole("button", { name: "Send" }).click();
    await expect(announcer).toHaveText("Asked the supervisor to merge cas-1999.");
    await expect(task("cas-1999").locator(".fleet-ops-asked")).toHaveText(/^Asked (just now|\d+[smh] ago)$/);
    expect(hub.operations.filter((call) => (call.body.op as Record<string, unknown>).kind === "request_merge")).toHaveLength(1);
  });

  await journey.stage("Stopping a row hands focus to the next row", async () => {
    await row("swift-lark-3").getByRole("button", { name: "Actions for swift-lark-3" }).click();
    await row("swift-lark-3").getByRole("menuitem", { name: "Stop…" }).click();
    await row("swift-lark-3").locator(".fleet-ops-confirm").getByRole("button", { name: "Stop", exact: true }).click();
    await expect(announcer).toHaveText("swift-lark-3 stopped.");
    await expect(row("swift-lark-3")).toHaveCount(0);
    await expect(row("quiet-owl-7").getByRole("button", { name: "Actions for quiet-owl-7" })).toBeFocused();
    // cas-d043 G06: with a row following, the result line is in view as well.
    await expect(page.locator(".conversation-context .fleet-ops-result")).toHaveText("swift-lark-3 stopped.");
    await expect(page.locator(".conversation-context .fleet-ops-result")).toBeInViewport();
    await expect(row("quiet-owl-7").getByRole("button", { name: "Actions for quiet-owl-7" })).toBeInViewport();
  });

  await journey.stage("A pairing without factory:manage sees Stop disabled and where to add it", async () => {
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /gabber-studio/ }).click();
    await expect(row("swift-lark-3")).toBeVisible();
    // cas-d043 G13: the merge was asked on cas-src; the same task id here is another task.
    await expect(task("cas-1999")).toBeVisible();
    await expect(task("cas-1999").locator(".fleet-ops-asked")).toHaveCount(0);
    await row("swift-lark-3").getByRole("button", { name: "Actions for swift-lark-3" }).click();
    const stop = row("swift-lark-3").getByRole("menuitem", { name: "Stop…" });
    await expect(stop).toHaveAttribute("aria-disabled", "true");
    const reason = row("swift-lark-3").locator(`#${await stop.getAttribute("aria-describedby")}`);
    await expect(reason).toContainText("Not allowed on this pairing.");
    // cas-97d58 F06: by what it allows, with one remedy; the command and its
    // scope ids live in Paired machines (with Copy), never in a menu a 720px
    // viewport cuts off.
    await expect(reason).toContainText("Needs the Stop and restart workers and sessions permission. Add it in Paired machines.");
    await expect(reason).not.toContainText(/factory:|--scopes/);
    // Pause needs factory:operate, which this control pairing may allow itself.
    await expect(row("swift-lark-3").getByRole("menuitem", { name: "Pause" })).toHaveAttribute("aria-disabled", "true");
    await expect(row("swift-lark-3").locator(".fleet-ops-reason").first()).toContainText("Allow managing workers in Paired machines.");
    // cas-a217: the header names the permission each control needs; Write
    // access needs factory:manage, not the factory:operate the others need.
    const header = page.locator("#status-view .fleet-ops-header");
    await expect(header.locator(":scope > .fleet-ops-reason")).toHaveText([
      "Add worker and Focus epic: Not allowed on this pairing. Needs the Manage workers and tasks permission. Allow managing workers in Paired machines.",
      "Write access: Not allowed on this pairing. Needs the Stop and restart workers and sessions permission. Add it in Paired machines.",
    ]);
    for (const name of ["Add worker…", "Focus epic…", "Write access…"]) {
      const control = header.getByRole("button", { name });
      await expect(header.locator(`#${await control.getAttribute("aria-describedby")}`), name).toBeVisible();
    }
    const before = hub.operations.length;
    await stop.click({ force: true });
    expect(hub.operations.length, "a disabled item sends nothing").toBe(before);
    await page.keyboard.press("Escape");
  });
});


for (const viewport of [{ width: 390, height: 844 }, { width: 844, height: 390 }]) {
  test.describe(`phone ${viewport.width}×${viewport.height}`, () => {
  test.use({ viewport });
  test(`HUB-J17 phone ${viewport.width}×${viewport.height}: operate each machine from its own conversation`, journeyPart, async ({ page, journey }) => {
    const hub = await journey.hub({ machines: [fleetMachine(ATLAS), fleetMachine(STUDIO)], paired: ["atlas", "studio"], scopes: { atlas: [...SCOPES, "factory-operate", "factory-manage"], studio: [...SCOPES, "factory-operate", "factory-manage"] }, fleet: { [PELICAN]: fleet(), [OTTER]: fleet() } });
    const rail = page.locator("#status-view");
    const agent = (name: string) => rail.locator(".status-agent", { hasText: name });
    const task = (id: string) => rail.locator(".status-task", { hasText: id });
    const actions = () => page.locator("dialog.fleet-action-sheet");
    const result = page.locator("#fleet-ops-announcer");
    const openFleet = () => page.getByRole("button", { name: "Tasks & progress", exact: true }).click();
    const target = async (control: ReturnType<typeof page.getByRole>) => {
      await expect(control).toBeVisible(); const box = await control.boundingBox();
      expect(box!.width).toBeGreaterThanOrEqual(44); expect(box!.height).toBeGreaterThanOrEqual(44);
    };
    await journey.stage("Open the first machine's Tasks & progress sheet", async () => {
      await journey.open();
      await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
      await target(page.getByRole("button", { name: "Tasks & progress", exact: true })); await openFleet();
      await expect(page.getByRole("dialog", { name: "Tasks & progress", exact: true })).toBeVisible();
      // cas-d043 G01: the sheet covers the header, so it names whose tasks these are.
      await expect(page.locator(".context-sheet-where")).toHaveText("cas-src on Atlas");
      await expect(page.locator(".context-sheet-where")).toBeVisible();
      await expect(page.getByRole("dialog", { name: "Tasks & progress", exact: true })).toHaveAccessibleDescription("cas-src on Atlas");
      await expect(agent("swift-lark-3")).toBeVisible();
    });
    await journey.stage("Add a worker on the first machine", async () => {
      await rail.getByRole("button", { name: "Add worker…" }).click();
      await expect(actions()).toBeVisible();
      const add = actions().getByRole("button", { name: "Add", exact: true });
      await target(add); await expect(add).toBeFocused(); await page.keyboard.press("Enter");
      await expect(agent("amber-heron-11")).toBeVisible();
      expect(hub.operations.at(-1)).toMatchObject({ machine: "atlas", session: PELICAN, body: { op: { kind: "spawn_workers", count: 1 } }, status: 200 });
    });
    await journey.stage("Pause then use Undo by keyboard above the composer", async () => {
      const trigger = agent("swift-lark-3").getByRole("button", { name: "Actions for swift-lark-3" });
      await target(trigger); await trigger.click(); await target(actions().getByRole("menuitem", { name: "Pause" }));
      await actions().getByRole("menuitem", { name: "Pause" }).click(); await expect(result).toHaveText("swift-lark-3 paused.");
      await page.getByRole("button", { name: "Close tasks & progress" }).click();
      const undo = page.getByRole("button", { name: "Undo", exact: true }); await target(undo);
      // cas-2796a F03: the × says what dismissing does to Undo.
      await expect(page.locator("#fleet-phone-undo").getByRole("button", { name: "Dismiss; Undo stays in Tasks & progress", exact: true })).toBeVisible();
      await undo.focus(); await page.keyboard.press("Enter"); await expect(result).toHaveText("swift-lark-3 resumed.");
      await openFleet();
    });
    await journey.stage("Dismissing the Undo offer keeps Undo in Tasks & progress (cas-2796a F03)", async () => {
      await agent("swift-lark-3").getByRole("button", { name: "Actions for swift-lark-3" }).click();
      await actions().getByRole("menuitem", { name: "Pause" }).click(); await expect(result).toHaveText("swift-lark-3 paused.");
      await page.getByRole("button", { name: "Close tasks & progress" }).click();
      await page.locator("#fleet-phone-undo").getByRole("button", { name: "Dismiss; Undo stays in Tasks & progress", exact: true }).click();
      await expect(page.locator("#fleet-phone-undo")).toHaveCount(0);
      await openFleet();
      const kept = rail.getByRole("button", { name: "Undo", exact: true });
      await expect(kept).toBeVisible();
      await kept.click(); await expect(result).toHaveText("swift-lark-3 resumed.");
    });
    await journey.stage("Search the full-height Assign and Focus pickers", async () => {
      await task("cas-2001").getByRole("button", { name: "Actions for cas-2001" }).click();
      await actions().getByRole("menuitem", { name: "Assign…" }).click();
      const search = actions().getByRole("searchbox", { name: "Search assign cas-2001 to" });
      // cas-2796a F01: the picker names the task by its title, not its id alone.
      await expect(actions().getByText("Task: Footer copy", { exact: true })).toBeVisible();
      await expect(search).toBeFocused(); await search.fill("absent"); await expect(actions().getByRole("status")).toContainText("No matches");
      await search.fill("quiet"); await expect(actions().getByRole("menuitem")).toHaveCount(1);
      await actions().getByRole("menuitem", { name: "Assign to quiet-owl-7" }).click(); await expect(result).toHaveText("cas-2001 assigned to quiet-owl-7.");
      await rail.getByRole("button", { name: "Focus epic…" }).click();
      await expect(actions().getByRole("searchbox", { name: "Search focus epic" })).toBeFocused();
      await page.keyboard.press("Escape"); await expect(rail.getByRole("button", { name: "Focus epic…" })).toBeFocused();
    });
    await journey.stage("Switch conversation and confirm Stop on the second machine", async () => {
      await page.getByRole("button", { name: "Close tasks & progress" }).click();
      await page.getByRole("button", { name: "‹ Conversations" }).click();
      await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /gabber-studio/ }).click(); await openFleet();
      await expect(page.locator(".context-sheet-where")).toHaveText("gabber-studio on Studio Mac");
      await agent("brisk-wren-9").getByRole("button", { name: "Actions for brisk-wren-9" }).click();
      await actions().getByRole("menuitem", { name: "Stop…" }).click();
      await expect(actions().getByRole("button", { name: "Cancel", exact: true })).toBeFocused();
      await page.keyboard.press("Tab");
      await expect(actions().getByRole("button", { name: "Stop", exact: true })).toBeFocused();
      await page.keyboard.press("Tab");
      await expect(actions().getByRole("button", { name: "Close actions", exact: true })).toBeFocused();
      await page.keyboard.press("Shift+Tab");
      await expect(actions().getByRole("button", { name: "Stop", exact: true })).toBeFocused();
      await target(actions().getByRole("button", { name: "Stop", exact: true }));
      await actions().getByRole("button", { name: "Stop", exact: true }).click(); await expect(result).toHaveText("brisk-wren-9 stopped.");
      await expect(agent("brisk-wren-9")).toHaveCount(0);
      expect(hub.operations.at(-1)).toMatchObject({ machine: "studio", session: OTTER, body: { op: { kind: "shutdown_workers", workers: ["brisk-wren-9"] } }, status: 200 });
    });
    await journey.stage("A stale Stop explains the second machine's changed generation", async () => {
      await agent("swift-lark-3").getByRole("button", { name: "Actions for swift-lark-3" }).click();
      await actions().getByRole("menuitem", { name: "Stop…" }).click();
      // Double-only: change backend generation AFTER the operator read this row.
      (hub as any).options.fleet[OTTER].agents.find((worker: any) => worker.name === "swift-lark-3").generation = 3;
      await actions().getByRole("button", { name: "Stop", exact: true }).click();
      await expect(result).toHaveText("swift-lark-3 already restarted."); await expect(agent("swift-lark-3")).toBeVisible();
      expect(hub.operations.at(-1)).toMatchObject({ machine: "studio", session: OTTER, status: 409 });
      // cas-2796a F02: the refusal is in view in the open sheet, landscape included.
      await expect(page.locator("#fleet-phone-undo")).toContainText("already restarted");
      await expect(page.locator("#fleet-phone-undo")).toBeInViewport();
      await page.keyboard.press("Escape"); await expect(page.getByRole("button", { name: "Tasks & progress", exact: true })).toBeFocused();
    });
  });
  });
}

// M12: retaining async ownership through a real shell replacement must also
// retain usable navigation. Every render replaces the shell, and Raw output
// opens a sheet over the conversation: a pending operation keeps its notice
// through both, and a completion that lands while Raw output is open keeps its
// inline Undo or refusal, reachable by keyboard once the sheet closes.
for (const viewport of [{ width: 390, height: 844 }, { width: 844, height: 390 }]) {
  test.describe(`phone Raw output return ${viewport.width}×${viewport.height}`, () => {
    test.use({ viewport });
    for (const disposition of ["success", "stale"] as const) {
      test(`HUB-J17 phone Raw output return ${viewport.width}×${viewport.height}: pending and ${disposition} remain usable`, journeyPart, async ({ page, journey }) => {
        const hub = await journey.hub({ machines: [fleetMachine(ATLAS)], paired: ["atlas"], scopes: { atlas: [...SCOPES, "factory-operate", "factory-manage"] }, fleet: { [PELICAN]: fleet() } });
        let request: Route | undefined;
        await page.route("**/v1/sessions/*/operations", async (route) => { request = route; });
        const agent = () => page.locator(".status-agent", { hasText: "swift-lark-3" });
        const raw = () => page.getByRole("button", { name: "Raw output", exact: true });
        const drawer = () => page.getByRole("dialog", { name: "Raw output" });
        await journey.open();
        await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
        await journey.stage("M12 a pending notice survives a shell rebuild and Raw output", async () => {
          await page.getByRole("button", { name: "Tasks & progress", exact: true }).click();
          await agent().getByRole("button", { name: "Actions for swift-lark-3" }).click();
          await page.locator("dialog.fleet-action-sheet").getByRole("menuitem", { name: "Pause" }).click();
          await expect.poll(() => Boolean(request)).toBe(true);
          await page.getByRole("button", { name: "Close tasks & progress" }).click();
          await expect(page.locator("#fleet-phone-undo")).toContainText("Pausing swift-lark-3…");
          // A catalog change re-renders, which replaces the shell.
          await hub.announceCatalog("atlas");
          await expect(page.locator("#fleet-phone-undo")).toContainText("Pausing swift-lark-3…");
          await expect(agent().locator(".fleet-ops-progress")).toHaveText("Pausing swift-lark-3…");
          await raw().click();
          await expect(drawer()).toBeVisible();
          await page.keyboard.press("Escape");
          await expect(drawer()).toBeHidden();
          await expect(raw()).toBeFocused();
          await expect(page.locator("#fleet-phone-undo")).toContainText("Pausing swift-lark-3…");
        });
        await journey.stage("M12 a completion under Raw output keeps its feedback and the keyboard return", async () => {
          await raw().click();
          await expect(drawer()).toBeVisible();
          const body = request!.request().postDataJSON();
          await request!.fulfill(disposition === "success"
            ? { json: { op_id: body.op_id, outcome: { kind: "set_worker_hold" } } }
            : { status: 409, json: { error: "stale", current: { worker: "swift-lark-3", generation: 3 } } });
          const text = disposition === "success" ? "swift-lark-3 paused." : "swift-lark-3 already restarted.";
          await expect(page.locator("#fleet-ops-announcer")).toHaveText(text);
          await expect(agent().locator(".fleet-ops-progress")).toHaveCount(0);
          // On a phone the Undo sits above the composer, not in the sheet.
          if (disposition === "stale") await expect(agent().locator(".fleet-ops-note")).toHaveText(text);
          await page.keyboard.press("Escape");
          await expect(drawer()).toBeHidden();
          await expect(raw()).toBeFocused();
          await expect(page.locator("#fleet-phone-undo")).toContainText(text);
          if (disposition === "success") {
            const undo = page.locator("#fleet-phone-undo").getByRole("button", { name: "Undo", exact: true });
            await expect(undo).toBeVisible();
            // The real hit target, not force-clicking through a fixed overlay.
            expect(await undo.evaluate((button) => {
              const box = button.getBoundingClientRect();
              return button.contains(document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2));
            })).toBe(true);
            await undo.focus();
            await expect(undo).toBeFocused();
          }
        });
      });
    }
  });
}
