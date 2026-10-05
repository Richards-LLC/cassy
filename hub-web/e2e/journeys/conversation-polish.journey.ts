import { test, expect, journeyPart, RECEIPTS } from "./journey";
import { ATLAS, PELICAN } from "./world";
import { join } from "node:path";

test("HUB-J7 actual options and truthful progress (cas-6e3a)", journeyPart, async ({ page, journey }) => {
  const workers = ["young-otter-14", "rapid-kestrel-30", "steady-stork-35", "ready-owl-88", "warm-phoenix-6", "keen-otter-80"];
  const hub = await journey.hub({ machines: [{ ...ATLAS, sessions: [{ ...ATLAS.sessions[0]!, workers }] }], paired: ["atlas"], fleet: { [PELICAN]: {
    agents: [
      { name: PELICAN, role: "supervisor", status: "active", generation: 1, latest_activity: { summary: "RAW INTERNAL NOTE" } },
      { name: workers[0]!, status: "active", generation: 1, current_task: "cas-1234", latest_activity: { summary: "raw worker activity" } },
      { name: workers[1]!, status: "idle", generation: 1, current_task: null },
    ],
    tasks: [
      { id: "cas-1234", title: "Polish the conversation and keep questions readable", status: "InProgress", assignee: workers[0], updated_at: "2026-09-30T12:00:00Z" },
      { id: "cas-5678", title: "Review before merge", status: "AWAITINGMERGE", updated_at: "2026-09-30T12:00:00Z" },
    ], epics: [], focused_epic: null, spawnNames: [],
  } } });
  let ask = 0;
  await journey.stage("Read a question without supplied choices", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
    hub.supervisorSays(PELICAN, "**Earlier question:** which **route** is clearer?", { kind: "ask" });
    ask = hub.supervisorSays(PELICAN, "**Which plan** should we use?\n\nExplain your choice in a reply.", { kind: "ask" });
    await expect(page.locator(".pinned-ask")).toBeVisible();
    await expect(page.locator("#status-view")).toContainText("Polish the conversation");
    for (const scheme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme: scheme });
      for (const width of [1280, 390]) {
        await page.setViewportSize({ width, height: width === 1280 ? 800 : 844 });
        await page.screenshot({ path: join(RECEIPTS, "HUB-J7", `${scheme}-${width}.png`) });
      }
    }
    await page.setViewportSize({ width: 1280, height: 800 });
    await expect(page.getByRole("button", { name: "Yes, go ahead", exact: true })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Hold", exact: true })).toHaveCount(0);
    await expect(page.locator(`.obj[data-notification-id="${ask}"]`)).toHaveCount(1);
    await expect(page.locator(".pinned-ask .obj")).toHaveCount(0);
    await expect(page.locator(".context-text")).not.toContainText("**");
    await expect(page.locator(".pinned-ask")).not.toContainText("**");
    await expect(page.locator("#status-view")).toContainText("Workers · 6");
    await expect(page.locator("#status-view")).not.toContainText("RAW INTERNAL");
    await expect(page.locator("#status-view")).toContainText("In progress");
    await expect(page.locator("#status-view")).toContainText("Awaiting merge");
    await expect(page.locator(".status-agent")).toHaveCount(6);
    await expect(page.locator(".status-agent").filter({ hasText: workers[0]! })).toContainText("Polish the conversation");
    await expect(page.locator(".status-agent").filter({ hasText: workers[2]! })).toContainText("Current work not reported");
    expect(await page.locator("#status-view").evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
  });
  await journey.stage("Jump by keyboard and answer in the composer", async () => {
    await page.locator(".pinned-expand").focus(); await page.keyboard.press("Enter");
    await expect(page.getByRole("textbox", { name: "Your message" })).toBeFocused();
    await page.getByRole("textbox", { name: "Your message" }).fill("Use the quieter plan.");
    const sent = hub.nextSend();
    await page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true }).click();
    expect(await sent).toMatchObject({ text: "Use the quieter plan.", in_reply_to: ask });
    await expect(page.locator(`.obj[data-notification-id="${ask}"] .chip.sent`)).toHaveText("Use the quieter plan.");
  });
  await journey.stage("Reply with a declared option", async () => {
    const choice = hub.supervisorSays(PELICAN, "Choose the **next step**.", { kind: "ask", options: ["Keep editing", "Send for review"] });
    const question = page.locator(`.obj[data-notification-id="${choice}"]`);
    await expect(question.getByRole("button", { name: "Send for review", exact: true })).toBeVisible();
    const sent = hub.nextSend();
    await question.getByRole("button", { name: "Send for review", exact: true }).click();
    expect(await sent).toMatchObject({ text: "Send for review", in_reply_to: choice });
    await expect(question).toMatchAriaSnapshot(`
      - group "Question from ${PELICAN}":
        - paragraph:
          - text: Choose the
          - strong: next step
          - text: .
        - text: Send for review
    `);
    await page.emulateMedia({ forcedColors: "active", reducedMotion: "reduce" });
  });
});
