import type { Page, Request } from "@playwright/test";
import { test, expect, journeyPart } from "./journey";
import { RELAY, type FleetWorld, type Machine } from "./hub-double";
import { ATLAS, PELICAN, STUDIO } from "./world";

// cas-eaa3: Commander ↔ Explorer, one Cassy Cloud. From Commander the
// operator reaches Explorer on the cloud origin: from the list's app switcher
// and, for the open conversation, from Tasks & progress scoped to its project.
// Explorer itself is stubbed at the cloud origin; the journey proves where
// Commander sends the operator and that nothing but the path and project id
// travels (no referrer, credential or fragment).
const CLOUD_ID = "github.com/richards-llc/cas-src";
const PROJECT_EXPLORER = `${RELAY}/explorer/tasks?project_id=${encodeURIComponent(CLOUD_ID)}`;
const EXPLORER_HOME = `${RELAY}/explorer`;

const fleet = (): FleetWorld => ({
  agents: [{ name: "young-otter-14", status: "active", current_task: "cas-1234", generation: 1 }],
  tasks: [{ id: "cas-1234", title: "Ingest request files", status: "in_progress", assignee: "young-otter-14", updated_at: "2026-10-10T18:00:00Z" }],
  epics: [{ id: "cas-571d" }],
  focused_epic: "cas-571d",
  spawnNames: [],
});
const atlas: Machine = { ...ATLAS, sessions: ATLAS.sessions.map((session) => ({ ...session, cloud_project_id: CLOUD_ID })) };

/** Stub Explorer on the cloud origin and record every request Commander's links make to it. */
async function stubExplorer(page: Page): Promise<Request[]> {
  const requests: Request[] = [];
  await page.context().route(`${RELAY}/explorer**`, async (route) => {
    requests.push(route.request());
    await route.fulfill({ status: 200, contentType: "text/html; charset=utf-8", body: "<!doctype html><meta charset=\"utf-8\"><title>Cassy Cloud — Explorer</title><h1>Explorer</h1>" });
  });
  return requests;
}

/** Follow one Explorer link into its own tab and prove what it carried. */
async function followToExplorer(page: Page, link: ReturnType<Page["locator"]>, requests: Request[], expected: string) {
  await expect(link).toHaveAttribute("href", expected);
  await expect(link).toHaveAttribute("target", "cassy-explorer");
  await expect(link).toHaveAttribute("rel", "noopener noreferrer");
  const before = requests.length;
  const [explorer] = await Promise.all([page.context().waitForEvent("page"), link.click()]);
  await explorer.waitForLoadState();
  await expect(explorer).toHaveTitle("Cassy Cloud — Explorer");
  expect(explorer.url()).toBe(expected);
  const request = requests.at(before)!;
  expect(request.url()).toBe(expected);
  expect(request.headers()["referer"], "Commander sends no referrer to Explorer").toBeUndefined();
  expect(request.headers()["authorization"]).toBeUndefined();
  expect(request.headers()["cookie"]).toBeUndefined();
  await explorer.close();
}

for (const [width, colorScheme, part] of [[1280, "light", false], [390, "dark", true]] as const) {
  test.describe(`app switcher ${width} ${colorScheme} cas_eaa3`, () => {
    test.use({ viewport: { width, height: 844 }, colorScheme });
    const body = async ({ page, journey }: { page: Page; journey: any }) => {
      const requests = await stubExplorer(page);
      await journey.hub({ machines: [atlas, STUDIO], paired: ["atlas", "studio"], fleet: { [PELICAN]: fleet() } });
      const switcher = page.getByRole("navigation", { name: "Cassy Cloud apps" });
      const list = page.getByRole("navigation", { name: "Choose a supervisor" });
      await journey.stage("Commander is the current app", async () => {
        await journey.open();
        await expect(switcher).toBeVisible();
        await expect(switcher.locator('[aria-current="page"]')).toHaveText("Commander");
        await expect(switcher.getByRole("link", { name: "Explorer (opens in a new tab)" })).toBeVisible();
      });
      await journey.stage("Switch to Explorer from the list", async () => {
        await followToExplorer(page, switcher.getByRole("link", { name: "Explorer (opens in a new tab)" }), requests, EXPLORER_HOME);
        await expect(page.locator(".conversation-list-heading")).toBeVisible();
      });
      await journey.stage("Open this project's tasks in Explorer", async () => {
        await list.getByRole("button", { name: /cas-src/ }).click();
        if (width === 390) await page.getByRole("button", { name: "Tasks & progress", exact: true }).click();
        const link = page.locator("#status-view, .conversation-context").getByRole("link", { name: "All tasks in Explorer (opens in a new tab)" });
        await expect(link).toBeVisible();
        await followToExplorer(page, link, requests, PROJECT_EXPLORER);
        if (width === 1280) await expect(switcher.getByRole("link", { name: "Explorer (opens in a new tab)" })).toHaveAttribute("href", PROJECT_EXPLORER);
      });
      await journey.stage("A project without a cloud identity opens Explorer's home", async () => {
        if (width === 390) {
          await page.keyboard.press("Escape");
          await page.getByRole("button", { name: "‹ Conversations", exact: true }).click();
        }
        await list.getByRole("button", { name: /gabber-studio/ }).click();
        await expect(page.getByRole("link", { name: "All tasks in Explorer (opens in a new tab)" })).toHaveCount(0);
        if (width === 390) await page.getByRole("button", { name: "‹ Conversations", exact: true }).click();
        await expect(switcher.getByRole("link", { name: "Explorer (opens in a new tab)" })).toHaveAttribute("href", EXPLORER_HOME);
      });
    };
    if (part) test(`HUB-J21 app switcher ${width} ${colorScheme} cas_eaa3`, journeyPart, body);
    else test(`HUB-J21 app switcher ${width} ${colorScheme} cas_eaa3`, body);
  });
}
