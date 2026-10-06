import type { Route } from "@playwright/test";
import { test, expect, journeyPart } from "./journey";
import { ATLAS } from "./world";

test("HUB-J1 approved pairing health check has a deadline and keeps its invitation (cas-2b3a5)", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], relay: { machine: "atlas", claimAfter: 1, authorizeAfter: 1 } });
  let held: Route | undefined;
  let block = true;
  await page.route("https://atlas.test/v1/health", route => {
    if (block) { held = route; return; }
    return route.fallback();
  });
  await journey.open();
  const dialog = page.locator("#pair-dialog");
  await page.getByRole("button", { name: "Pair a machine" }).filter({ visible: true }).click();
  await dialog.getByRole("button", { name: "Create pairing code" }).click();
  await journey.stage("The approved browser probe finishes even when it is held", async () => {
    await expect.poll(() => Boolean(held)).toBe(true);
    await expect(dialog).toContainText("Checking that this device can reach the machine");
    await page.clock.runFor(3_001);
    await expect(dialog).toContainText("reachability check for Atlas · Linux failed");
    await expect(dialog).toContainText("Local network access");
    await expect(dialog).toContainText("press Pair to try this approved invitation");
    await expect(dialog.getByRole("button", { name: "Pair", exact: true })).toBeEnabled();
    expect(hub.exchanges).toHaveLength(0);
  });
  await journey.stage("The same approved invitation pairs once the route works", async () => {
    block = false; await held!.abort("failed").catch(() => {});
    await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Operator");
    await dialog.getByRole("button", { name: "Pair", exact: true }).click();
    await expect(dialog).toBeHidden(); expect(hub.exchanges).toHaveLength(1);
    await expect(page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ })).toBeVisible();
  });
});

test("HUB-J2 unanswered pairing exchange times out safely and can be retried (cas-2b3a5)", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS] });
  let blocked = true;
  let held: Route | undefined;
  await page.route("https://atlas.test/v1/auth/pairing/exchange", route => {
    if (blocked) { held = route; return; }
    return route.fallback();
  });
  const dialog = page.locator("#pair-dialog");
  const token = "q3VbXo8Zt1nA4wLr9cYp2KdJ6sHf0uEiMgTxBvNyRaQ";
  await page.goto(`./#pair=${token}&hub=atlas&hub_url=https%3A%2F%2Fatlas.test&machine=Atlas&scopes=machine:read,session:read,pane:read`);
  await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Operator");
  await journey.stage("A browser-held exchange has a finite wait", async () => {
    await dialog.getByRole("button", { name: "Pair", exact: true }).click();
    await expect.poll(() => Boolean(held)).toBe(true);
    await expect(dialog).toContainText("Updating this browser installation");
    // Advance the entire deadline: protocol-idle cannot drain a held fetch
    // before its abort, so this test deliberately drives the browser clock.
    await page.clock.runFor(10_001);
    await expect(dialog).toContainText("Pairing timed out after 10s");
    await expect(dialog).toContainText("Allow Local network access");
    await expect(dialog).toContainText("This invitation may already have been used");
    await expect(dialog.getByRole("button", { name: "Pair", exact: true })).toBeEnabled();
    expect(hub.exchanges).toHaveLength(0);
    const count = await page.evaluate(async () => {
      const db = await new Promise<IDBDatabase>((resolve, reject) => {
        const request = indexedDB.open("cas-commander-v1");
        request.onsuccess = () => resolve(request.result); request.onerror = () => reject(request.error);
      });
      const count = await new Promise<number>((resolve, reject) => {
        const request = db.transaction("machines").objectStore("machines").count();
        request.onsuccess = () => resolve(request.result); request.onerror = () => reject(request.error);
      });
      db.close(); return count;
    });
    expect(count).toBe(0);
  });
  await journey.stage("Allowing the route lets the retained invitation pair", async () => {
    blocked = false;
    await held!.abort("failed").catch(() => {});
    await dialog.getByRole("button", { name: "Pair", exact: true }).click();
    await expect(dialog).toBeHidden();
    expect(hub.exchanges).toHaveLength(1);
    await expect(page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ })).toBeVisible();
  });
});
