import { test, expect, journeyPart } from "./journey";
import { ATLAS } from "./world";
import { HubDouble } from "./hub-double";
import { ProtocolClock } from "./protocol-clock";
import { journeyDay } from "./clock";

test("HUB-J12 event flood leaves the final supervisor visible without hammering sessions (cas-b55b)", journeyPart, async ({ page, journey }) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  const clock = new ProtocolClock(page);
  const machine = { ...ATLAS, sessions: ATLAS.sessions.map(session => ({ ...session })) };
  const hub = new HubDouble(page, { machines: [machine], paired: ["atlas"], multiplex: true, time: clock });
  await hub.install();
  await page.goto("./"); await hub.seedPaired(); await clock.start(); await page.goto("./");
  const navigation = page.getByRole("navigation", { name: "Choose a supervisor" });
  // A row is named "<project> on <machine>, …"; the row's own controls are not.
  await expect(navigation.getByRole("button", { name: /^cas-src on / })).toBeVisible();
  const before = hub.catalogFetchCount("atlas");
  await journey.stage("A hundred machine events arrive in one second", async () => {
    for (let sequence = 1; sequence <= 100; sequence++) {
      if (sequence === 100) machine.sessions.push({
        ...machine.sessions[0]!, name: "final-supervisor", supervisor: "final-supervisor", project_dir: "/projects/final-catalog",
      });
      await page.evaluate(sequence => (window as unknown as {
        __journeyMachineEvent: (host: string, data: string) => number;
      }).__journeyMachineEvent("atlas.test", JSON.stringify({ kind: "session_added", session: "final-supervisor", sequence })), sequence);
      await clock.advance(10);
    }
    expect(hub.catalogFetchCount("atlas") - before).toBeLessThanOrEqual(2);
  });
  await journey.stage("The last supervisor appears after the trailing refresh", async () => {
    await clock.advance(1_000);
    const final = navigation.getByRole("button", { name: /^final-catalog on / });
    await expect(final).toBeVisible();
    expect(hub.catalogFetchCount("atlas") - before).toBeLessThanOrEqual(2);
  });
  await journey.stage("An idle catalog issues no event refreshes", async () => {
    await clock.advance(1_000);
    expect(hub.catalogFetchCount("atlas") - before).toBeLessThanOrEqual(2);
    expect(errors).toEqual([]);
    await expect(page.getByRole("search").getByRole("searchbox")).toHaveAccessibleName(/\S/);
  });
});

test("HUB-J12 a fresh catalog clears a cached ghost conversation (cas-228f)", journeyPart, async ({ page, journey }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const lastActivityAt = journeyDay(25);
  const live = { ...ATLAS.sessions[0]!, last_activity_at: lastActivityAt };
  const legacy = { ...live, name: "legacy-session", supervisor: "legacy-supervisor", project_dir: "/projects/legacy-project" };
  const machine = { ...ATLAS, sessions: [live, legacy] };
  await journey.hub({ machines: [machine], paired: [machine.id] });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const ghost = list.getByRole("button", { name: /^legacy-project on / });
  const current = list.getByRole("button", { name: /^cas-src on / });
  await journey.stage("Save the last known conversations", async () => {
    await journey.open();
    await expect(ghost).toBeVisible();
    await expect(current).toBeVisible();
  });
  await journey.stage("The hub answers without the old session or an end record", async () => {
    machine.sessions = [live];
    const catalog = page.waitForResponse(response => new URL(response.url()).pathname === "/v1/sessions" && response.ok());
    await page.reload();
    await catalog;
    await expect(ghost).toHaveCount(0);
    await expect(current).toBeVisible();
  });
});
