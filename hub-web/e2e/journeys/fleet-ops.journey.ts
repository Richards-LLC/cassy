import { test, expect } from "./journey";
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
    await expect(preview.locator(".fleet-ops-preview-text")).toHaveText("Please merge cas-1999 (Pairing wording). It is awaiting merge at branch factory/wren-cas-1999, tip 9ffb3897.");
    await expect(preview.getByRole("button", { name: "Cancel" })).toBeFocused();
    await preview.getByRole("button", { name: "Send" }).click();
    await expect(announcer).toHaveText("Asked the supervisor to merge cas-1999.");
    await expect(task("cas-1999").locator(".fleet-ops-asked")).toHaveText(/^Asked (just now|\d+[smh] ago)$/);
    expect(hub.operations.filter((call) => (call.body.op as Record<string, unknown>).kind === "request_merge")).toHaveLength(1);
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
