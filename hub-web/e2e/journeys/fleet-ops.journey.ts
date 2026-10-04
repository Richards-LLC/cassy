import { test, expect, journeyPart } from "./journey";
import type { Route } from "@playwright/test";
import { SCOPES, type FleetWorld } from "./hub-double";
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

for (const width of [390, 1280]) for (const colorScheme of ["light", "dark"] as const) {
  test.describe(`fleet failure copy ${width} ${colorScheme} cas_a348`, () => {
    test.use({ viewport: { width, height: 844 }, colorScheme });
    test(`HUB-J17 fleet failure copy ${width} ${colorScheme} cas_a348`, journeyPart, async ({ page, journey }) => {
      await journey.hub({ machines: [ATLAS], paired: ["atlas"], scopes: { atlas: [...SCOPES, "factory-operate", "factory-manage"] }, fleet: { [PELICAN]: fleet() } });
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
        if (width === 390) await expect(page.locator("#fleet-phone-undo")).toContainText(text);
        await expect(row.locator(".status-chip")).toHaveText("active");
      });
      await journey.stage("A connection failure offers connection recovery without diagnostics", async () => {
        await page.unroute("**/v1/sessions/*/operations");
        await page.route("**/v1/sessions/*/operations", (route) => route.abort("failed"));
        await row.getByRole("button", { name: "Actions for swift-lark-3" }).click();
        await panel().getByRole("menuitem", { name: "Pause" }).click();
        await expect(result).toHaveText("Could not pause swift-lark-3: the machine could not be reached. Check its connection and try again.");
        await expect(row.locator(".fleet-ops-note")).not.toContainText(/POST|\/v1\/|Failed to fetch|500/);
        await expect(row.locator(".status-chip")).toHaveText("active");
      });
      await journey.stage("A successful retry clears failure feedback and offers Undo", async () => {
        await page.unroute("**/v1/sessions/*/operations");
        await row.getByRole("button", { name: "Actions for swift-lark-3" }).click();
        await panel().getByRole("menuitem", { name: "Pause" }).click();
        await expect(result).toHaveText("swift-lark-3 paused.");
        await expect(row.locator(".fleet-ops-note")).toHaveCount(0);
        await expect(row.locator(".status-chip")).toHaveText("held");
        await expect(page.getByRole("button", { name: "Undo", exact: true })).toBeVisible();
      });
    });
  });
}

test("HUB-J17 run the fleet from a conversation", async ({ page, journey }) => {
  const hub = await journey.hub({
    machines: [ATLAS, STUDIO],
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
    await expect(row("swift-lark-3").locator(".status-chip")).toHaveText("held");
    await expect(rail.locator(".fleet-ops-undo")).toContainText("swift-lark-3 paused.");
    await rail.getByRole("button", { name: "Undo" }).click();
    await expect(announcer).toHaveText("swift-lark-3 resumed.");
    await expect(row("swift-lark-3").locator(".status-chip")).toHaveText("active");
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
    await expect(confirm.locator(".fleet-ops-question")).toHaveText("Stop brisk-wren-9? Its task cas-1500 goes back to ready.");
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
    // It was the last row, so focus moves to the list, never to <body> (QA N1).
    await expect(rail).toBeFocused();
    expect(hub.operations.length).toBe(before + 1);
    expect(hub.operations.at(-1)?.body).toMatchObject({ op: { kind: "shutdown_workers", workers: ["brisk-wren-9"] }, expected: { worker: "brisk-wren-9", generation: 4 } });
    // No Undo for a destructive action.
    await expect(rail.locator(".fleet-ops-undo")).toHaveCount(0);
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
  });

  await journey.stage("A pairing without factory:manage sees Stop disabled with the command that adds it", async () => {
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /gabber-studio/ }).click();
    await expect(row("swift-lark-3")).toBeVisible();
    await row("swift-lark-3").getByRole("button", { name: "Actions for swift-lark-3" }).click();
    const stop = row("swift-lark-3").getByRole("menuitem", { name: "Stop…" });
    await expect(stop).toHaveAttribute("aria-disabled", "true");
    const reason = row("swift-lark-3").locator(`#${await stop.getAttribute("aria-describedby")}`);
    await expect(reason).toContainText("Not allowed on this pairing.");
    await expect(reason).toContainText(/--scopes machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt,factory:manage$/);
    // Pause needs factory:operate, which this control pairing may allow itself.
    await expect(row("swift-lark-3").getByRole("menuitem", { name: "Pause" })).toHaveAttribute("aria-disabled", "true");
    await expect(row("swift-lark-3").locator(".fleet-ops-reason").first()).toContainText("Allow managing workers in Paired machines.");
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
    const hub = await journey.hub({ machines: [ATLAS, STUDIO], paired: ["atlas", "studio"], scopes: { atlas: [...SCOPES, "factory-operate", "factory-manage"], studio: [...SCOPES, "factory-operate", "factory-manage"] }, fleet: { [PELICAN]: fleet(), [OTTER]: fleet() } });
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
      await undo.focus(); await page.keyboard.press("Enter"); await expect(result).toHaveText("swift-lark-3 resumed.");
      await openFleet();
    });
    await journey.stage("Search the full-height Assign and Focus pickers", async () => {
      await task("cas-2001").getByRole("button", { name: "Actions for cas-2001" }).click();
      await actions().getByRole("menuitem", { name: "Assign…" }).click();
      const search = actions().getByRole("searchbox", { name: "Search assign cas-2001 to" });
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
      await page.keyboard.press("Escape"); await expect(page.getByRole("button", { name: "Tasks & progress", exact: true })).toBeFocused();
    });
  });
  });
}

// M12: retaining async ownership through a real shell replacement must also
// retain usable navigation. A conversation notice cannot cover Terminal's
// return button, and a Terminal completion keeps its inline Undo/refusal.
for (const viewport of [{ width: 390, height: 844 }, { width: 844, height: 390 }]) {
  test.describe(`phone Terminal return ${viewport.width}×${viewport.height}`, () => {
    test.use({ viewport });
    for (const disposition of ["success", "stale"] as const) {
      test(`HUB-J17 phone Terminal return ${viewport.width}×${viewport.height}: pending and ${disposition} remain usable`, journeyPart, async ({ page, journey }) => {
        await journey.hub({ machines: [ATLAS], paired: ["atlas"], scopes: { atlas: [...SCOPES, "factory-operate", "factory-manage"] }, fleet: { [PELICAN]: fleet() } });
        let request: Route | undefined;
        await page.route("**/v1/sessions/*/operations", async (route) => { request = route; });
        const agent = () => page.locator(".status-agent", { hasText: "swift-lark-3" });
        const terminal = () => page.locator(".conversation-heading").getByRole("button", { name: "Terminal view", exact: true });
        const returning = () => page.locator("#conversation-return");
        await journey.open();
        await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
        await journey.stage("M12 pending notice permits normal Terminal return without losing ownership", async () => {
          await page.getByRole("button", { name: "Tasks & progress", exact: true }).click();
          await agent().getByRole("button", { name: "Actions for swift-lark-3" }).click();
          await page.locator("dialog.fleet-action-sheet").getByRole("menuitem", { name: "Pause" }).click();
          await expect.poll(() => Boolean(request)).toBe(true);
          await page.getByRole("button", { name: "Close tasks & progress" }).click();
          await expect(page.locator("#fleet-phone-undo")).toHaveText("Pausing swift-lark-3…");
          await terminal().click();
          await expect(page.locator("#fleet-phone-undo")).toHaveCount(0);
          await expect(agent().locator(".fleet-ops-progress")).toHaveText("Pausing swift-lark-3…");
          await expect(returning()).toBeVisible();
          // The real hit target, not force-clicking through the fixed overlay.
          expect(await returning().evaluate((button) => {
            const box = button.getBoundingClientRect();
            return button.contains(document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2));
          })).toBe(true);
          await returning().click();
          await expect(page.locator("#fleet-phone-undo")).toHaveText("Pausing swift-lark-3…");
        });
        await journey.stage("M12 Terminal completion preserves inline feedback and keyboard return", async () => {
          await terminal().click();
          const body = request!.request().postDataJSON();
          await request!.fulfill(disposition === "success"
            ? { json: { op_id: body.op_id, outcome: { kind: "set_worker_hold" } } }
            : { status: 409, json: { error: "stale", current: { worker: "swift-lark-3", generation: 3 } } });
          const text = disposition === "success" ? "swift-lark-3 paused." : "swift-lark-3 already restarted.";
          await expect(page.locator("#fleet-ops-announcer")).toHaveText(text);
          await expect(page.locator("#fleet-phone-undo")).toHaveCount(0);
          await expect(agent().locator(".fleet-ops-progress")).toHaveCount(0);
          if (disposition === "success") await expect(page.locator("#status-view .fleet-ops-undo")).toContainText(text);
          else await expect(agent().locator(".fleet-ops-note")).toHaveText(text);
          await returning().focus();
          await expect(returning()).toBeFocused();
          await page.keyboard.press("Enter");
          await expect(page.getByRole("button", { name: "Tasks & progress", exact: true })).toBeVisible();
          await expect(page.locator("#fleet-phone-undo")).toContainText(text);
          if (disposition === "success") await expect(page.locator("#fleet-phone-undo").getByRole("button", { name: "Undo", exact: true })).toBeVisible();
        });
      });
    }
  });
}
