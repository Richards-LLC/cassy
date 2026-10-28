import { test, expect, journeyPart, type Journey } from "./journey";
import type { Page } from "@playwright/test";
import { ATLAS, PELICAN } from "./world";
import { writeFileSync } from "node:fs";

// Independent goals enter the same rendered conversation with a fresh live ask.
// Capturing four rendering modes must not spend the reply goal's test budget.
async function openQuestionless(page: Page, journey: Journey) {
  const workers = ["young-otter-14", "rapid-kestrel-30", "steady-stork-35", "ready-owl-88", "warm-phoenix-6", "keen-otter-80"];
  const hub = await journey.hub({ machines: [{ ...ATLAS, sessions: [{ ...ATLAS.sessions[0]!, workers }] }], paired: ["atlas"], fleet: { [PELICAN]: {
    agents: [
      { name: PELICAN, role: "supervisor", status: "active", generation: 1, latest_activity: { summary: "RAW INTERNAL NOTE" } },
      { name: workers[0]!, status: "active", generation: 1, current_task: "cas-1234", liveness: "working", latest_activity: { summary: "raw worker activity", created_at_rfc3339: "2026-09-30T12:00:00Z" } },
      { name: workers[1]!, status: "idle", generation: 1, current_task: null },
    ],
    tasks: [
      { id: "cas-1234", title: "Polish the conversation and keep questions readable", epic_id: "cas-release", status: "InProgress", assignee: workers[0], updated_at: "2026-09-30T12:00:00Z" },
      { id: "cas-5678", title: "Review before merge", epic_id: "cas-release", status: "AWAITINGMERGE", updated_at: "2026-09-30T12:00:00Z" },
      { id: "cas-ready", title: "Ready for the next worker", epic_id: "cas-release", status: "open", updated_at: "2026-09-30T12:00:00Z" },
      { id: "cas-other", title: "Another supervisor's task", epic_id: "cas-other-epic", status: "open", updated_at: "2026-09-30T12:00:00Z" },
      { id: "cas-closing", title: "Already delivered", epic_id: "cas-release", status: "AwaitingMerge", delivery_state: "merged", updated_at: "2026-09-30T12:00:00Z" },
    ], epics: [{ id: "cas-release", title: "Release 3.87", progress: { total: 8, merged: 3, merges_per_hour: 1.5, last_merged_at: "2026-09-30T12:00:00Z" } }], focused_epic: "cas-release", spawnNames: [],
  } } });
  await journey.open();
  await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
  await expect.poll(() => hub.hasSocket(PELICAN), { timeout: 15_000 }).toBe(true);
  hub.supervisorSays(PELICAN, "**Earlier question:** which **route** is clearer?", { kind: "ask" });
  const ask = hub.supervisorSays(PELICAN, "**Which plan** should we use?\n\nExplain your choice in a reply.", { kind: "ask" });
  await expect(page.locator(".pinned-ask")).toBeVisible();
  await expect(page.locator("#status-view")).toContainText("Polish the conversation");
  return { hub, ask, workers };
}

test("HUB-J7 question and progress across light, dark, desktop and phone", journeyPart, async ({ page, journey }, testInfo) => {
  await openQuestionless(page, journey);
  await journey.stage("Capture question and progress across rendering modes", async () => {
    const css = await (await page.request.get(new URL("app.css", page.url()).href)).text();
    for (const scheme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme: scheme });
      for (const width of [1280, 1600, 390]) {
        await page.setViewportSize({ width, height: width === 390 ? 844 : 800 });
        if (width === 390) await page.getByRole("button", { name: "Tasks & progress", exact: true }).click();
        await page.screenshot({ path: testInfo.outputPath(`${scheme}-${width}.png`) });
        // Preserve the actual rendered build DOM and exact build CSS for
        // offline strict mechanical QA; no reconstructed fixture markup.
        const dom = await page.evaluate(() => {
          const clone = document.documentElement.cloneNode(true) as HTMLElement;
          clone.querySelectorAll("script, link").forEach(node => node.remove());
          return clone.outerHTML;
        });
        writeFileSync(testInfo.outputPath(`${scheme}-${width}.html`), "<!doctype html>" + dom.replace("</head>", `<style>${css}</style></head>`));
        if (width === 390) await page.getByRole("button", { name: "Close tasks & progress" }).click();
      }
    }
  });
});

test("HUB-J7 actual options and truthful progress (cas-6e3a)", journeyPart, async ({ page, journey }, testInfo) => {
  const { hub, ask, workers } = await openQuestionless(page, journey);
  await journey.stage("Read a question without supplied choices", async () => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await expect(page.getByRole("button", { name: "Yes, go ahead", exact: true })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Hold", exact: true })).toHaveCount(0);
    await expect(page.locator(`.obj[data-notification-id="${ask}"]`)).toHaveCount(1);
    await expect(page.locator(".pinned-ask .obj")).toHaveCount(0);
    // cas-97d58 F09: the rail lists both open questions, each without markdown.
    for (const entry of await page.locator(".context-text").all()) await expect(entry).not.toContainText("**");
    await expect(page.locator(".pinned-ask")).not.toContainText("**");
    await expect(page.locator("#status-view")).toContainText("Workers · 6");
    await expect(page.locator("#status-view")).not.toContainText("RAW INTERNAL");
    await expect(page.locator("#status-view")).toContainText("In progress");
    await expect(page.locator("#status-view")).toContainText("Awaiting merge");
    await expect(page.locator(".status-agent")).toHaveCount(6);
    await expect(page.locator(".status-agent").filter({ hasText: workers[0]! })).toContainText("Polish the conversation");
    await expect(page.locator(".status-agent").filter({ hasText: workers[2]! })).toContainText("Current work not reported");
    expect(await page.locator("#status-view").evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    await expect(page.locator(".release-progress")).toContainText("3 of 8 merged");
    await expect(page.locator(".release-progress")).toContainText("1.5 merges/hour");
    await expect(page.locator(".release-progress")).toContainText("Last merge 12:00 PM");
    await expect(page.locator(".release-progress progress")).toHaveAttribute("value", "3");
    await expect(page.locator(".status-task").filter({ hasText: "Already delivered" })).toContainText("Merged, closing");
    await expect(page.locator(".status-task").filter({ hasText: "Another supervisor" })).toHaveCount(0);
    await page.getByRole("button", { name: "All tasks", exact: true }).click();
    await expect(page.locator(".status-task").filter({ hasText: "Another supervisor" })).toBeVisible();
    await page.getByRole("button", { name: "Supervisor tasks", exact: true }).click();
    await expect(page.locator(".status-task").filter({ hasText: "Another supervisor" })).toHaveCount(0);
    const grip = page.getByRole("separator", { name: "Resize tasks and progress" });
    await grip.focus(); await page.keyboard.press("ArrowLeft");
    await expect(grip).toHaveAttribute("aria-valuenow", "336");
    const bounds = (await grip.boundingBox())!;
    await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2);
    await page.mouse.down(); await page.mouse.move(bounds.x - 64, bounds.y + bounds.height / 2); await page.mouse.up();
    const resized = await grip.getAttribute("aria-valuenow");
    expect(Number(resized)).toBeGreaterThan(336);
    expect(await page.evaluate(() => localStorage.getItem("cassy.context-width"))).toBe(resized);
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(grip).toBeHidden();
    expect(await page.locator(".conversation-shell").evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    await page.setViewportSize({ width: 1280, height: 800 });
    // Restore the default width so the rest of this conversation stays comparable.
    await grip.focus(); await page.keyboard.press("Home");

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
    for (const [name, query, media] of [
      ["forced-colors", "(forced-colors: active)", { forcedColors: "active" }],
      ["reduced-motion", "(prefers-reduced-motion: reduce)", { reducedMotion: "reduce" }],
      ["contrast-more", "(prefers-contrast: more)", { contrast: "more" }],
    ] as const) {
      await page.emulateMedia({ forcedColors: null, reducedMotion: null, contrast: null });
      await page.emulateMedia(media);
      expect(await page.evaluate(query => matchMedia(query).matches, query)).toBe(true);
      await expect(question.locator(".chip.sent")).toHaveText("Send for review");
      await page.screenshot({ path: testInfo.outputPath(`a11y-${name}.png`) });
    }
  });
});

// Adjacent empty session and long identifiers cannot borrow another session's
// worker detail or widen the context rail.
test("HUB-J7 empty roster and long unreported work (cas-6e3a)", journeyPart, async ({ page, journey }) => {
  const name = "worker-" + "long".repeat(40);
  const hub = await journey.hub({ machines: [{ ...ATLAS, sessions: [
    { ...ATLAS.sessions[0]!, workers: [name] },
    { ...ATLAS.sessions[0]!, name: "empty-session", supervisor: "empty-session", project_dir: "/projects/empty-world", workers: [] },
  ] }], paired: ["atlas"], fleet: {
    [PELICAN]: { agents: [{ name, status: "active", current_task: "cas-" + "9".repeat(100), generation: 1 }], tasks: [], epics: [], focused_epic: null, spawnNames: [] },
    "empty-session": { agents: [{ name: PELICAN, role: "supervisor", status: "active", generation: 1 }], tasks: [], epics: [], focused_epic: null, spawnNames: [] },
  } });
  await journey.stage("Long names and missing task details stay within the rail", async () => {
    await page.setViewportSize({ width: 1280, height: 800 }); await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
    await expect(page.locator("#status-view")).toContainText("Task details not reported");
    await expect(page.locator(".status-agent")).toHaveCount(1);
    expect(await page.locator("#status-view").evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    hub.supervisorSays(PELICAN, "Choose a route with a readable question.\n\n" + "More detail about this decision. ".repeat(60), { kind: "ask", options: [] });
    await expect(page.locator(".pinned-expand")).toBeVisible();
    // cas-053c (cas-d043): hovering the bar keeps its amber fill in both
    // schemes; the generic hover wash put its text at 1.17:1 in dark.
    for (const scheme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme: scheme });
      await page.mouse.move(0, 0);
      const rest = await page.locator(".pinned-expand").evaluate((node) => getComputedStyle(node).backgroundColor);
      await page.locator(".pinned-expand").hover();
      await expect.poll(() => page.locator(".pinned-expand").evaluate((node) => getComputedStyle(node).backgroundColor), `${scheme}: the hovered bar keeps its fill`).toBe(rest);
    }
    await page.mouse.move(0, 0);
    await page.emulateMedia({ colorScheme: "light" });
    await expect(page.getByRole("log").locator("button.chip")).toHaveCount(0);
    await page.setViewportSize({ width: 390, height: 440 });
    await page.getByRole("textbox", { name: "Your message" }).focus();
    await expect(page.getByRole("textbox", { name: "Your message" })).toBeInViewport();
    expect(await page.locator(".pinned-bar").evaluate(el => el.getBoundingClientRect().height)).toBeLessThanOrEqual(48);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
  await journey.stage("An empty session shows no borrowed workers", async () => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /empty-world/ }).click();
    await expect(page.locator(".status-agent")).toHaveCount(0);
    await expect(page.locator("#status-view")).not.toContainText(name);
    await expect(page.locator(".pinned-ask")).toBeHidden();
    await expect(page.getByRole("textbox", { name: "Your message" })).toHaveAttribute("placeholder", "Message the empty-world supervisor");
  });
});
