import { test, expect, journeyPart } from "./journey";
import { ATLAS } from "./world";
import { HubDouble } from "./hub-double";
import { ProtocolClock } from "./protocol-clock";

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
      await hub.emitMachineEvent("atlas", "session_added", "final-supervisor");
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
