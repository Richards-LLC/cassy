import { test, expect, journeyPart } from "./journey";
import { ATLAS, PELICAN } from "./world";
import type { Page } from "@playwright/test";

// A row is named "<project> on <machine>, …"; the row's own controls are not.
const conversationRow = (page: Page) => page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /^cas-src on / });
const messageStatus = (page: Page) => page.getByTestId("message-status");
const turns = (page: Page) => page.getByRole("log").getByTestId("conversation-turn");
const turn = (page: Page, text: string) => turns(page).filter({ hasText: text });
// A turn's delivery line (Delivered or Queued for supervisor).
const deliveryLine = (page: Page, text: string) => turn(page, text).getByTestId("conversation-delivered");

test("HUB-J5 message-only pairing sends without taking pane control (cas-d402)", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], scopes: { atlas: ["machine-read", "session-read", "pane-read", "message-send"] } });
  const composer = page.getByRole("textbox", { name: "Your message" });
  const send = page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true });
  const status = messageStatus(page);
  // A message-only pairing can send without acquiring a terminal input lease.
  let taken = false;
  await page.route(/\/v1\/sessions\/[^/]+\/lease$/, (route) => {
    if (route.request().method() === "POST") { taken = true; return route.fallback(); }
    if (!taken) return route.fulfill({ json: { held_by_me: false, controller_label: null } });
    return route.fallback();
  });
  await journey.stage("Send while this device does not hold the session", async () => {
    await journey.open();
    await conversationRow(page).click();
    await expect(send).toBeVisible();
    await composer.fill("Send this without terminal access.");
    const sent = hub.nextSend();
    await send.click();
    expect((await sent).text).toBe("Send this without terminal access.");
    expect(taken, "semantic send does not acquire pane control").toBe(false);
  });
  await journey.stage("Delivered without taking pane control", async () => {
    await hub.deliverLatest(PELICAN);
    await expect(turn(page, "Send this without terminal access.")).toHaveAttribute("data-state", "delivered");
    await expect(deliveryLine(page, "Send this without terminal access.")).toBeVisible();
    await expect(status).toBeHidden();
  });
});

test("HUB-J5 a revoked send keeps its text and draft and points to re-pairing (cas-5e60)", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
  const composer = page.getByRole("textbox", { name: "Your message" });
  const bubble = turn(page, "Keep the refused message.");
  await journey.stage("A correlated revoked send names pairing while a new draft survives", async () => {
    await journey.open();
    await conversationRow(page).click();
    await composer.fill("Keep the refused message.");
    const outbound = hub.nextSend();
    await page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true }).click();
    const sent = await outbound;
    await composer.fill("Keep my newer draft too.");
    // The real handler/wire regression supplies this actionable pairing detail;
    // here the daemon-error route proves the shared failed-send projection.
    await hub.send(PELICAN, { Error: { client_ref: sent.client_ref, message: "This device's pairing is no longer accepted. Pair this device again, then retry." } });
    await expect(bubble).toHaveAttribute("data-state", "error");
    await expect(bubble).toContainText("Keep the refused message.");
    await expect(bubble.getByRole("status")).toBeVisible();
    await expect(bubble.getByRole("button", { name: "Take control of the session", exact: true })).toHaveCount(0);
    await expect(composer).toHaveValue("Keep my newer draft too.");
    await bubble.getByRole("button", { name: "Edit message", exact: true }).click();
    await expect(composer).toHaveValue("Keep my newer draft too.");
    await expect(messageStatus(page)).toBeVisible();

    // A file-only draft has its own recovery, without overwriting either draft.
    await composer.fill("");
    const chooser = page.waitForEvent("filechooser");
    await page.getByRole("button", { name: "Attach files", exact: true }).click();
    await (await chooser).setFiles({ name: "newer-draft.txt", mimeType: "text/plain", buffer: Buffer.from("Keep my newer file.") });
    const remove = page.getByRole("button", { name: "Remove newer-draft.txt", exact: true });
    await expect(remove).toBeVisible();
    await bubble.getByRole("button", { name: "Edit message", exact: true }).click();

    await expect(composer).toHaveValue("");
    await expect(remove).toBeVisible();
    await expect(bubble).toContainText("Keep the refused message.");
    await remove.click();
    await expect(remove).toHaveCount(0);
    await composer.fill("Keep my newer draft too.");
  });
  await journey.stage("The header offers usable Re-pair while Edit still recovers the refused text", async () => {
    hub.refuseProofs("atlas", 1_000, "revoked", false);
    await hub.down("atlas", { sockets: "close" });
    await hub.up("atlas");
    await page.clock.fastForward(1_000);
    const repair = page.getByRole("button", { name: "Re-pair Atlas · Linux", exact: true }).first();
    await expect(repair).toBeVisible();
    await expect(bubble).toHaveAttribute("data-state", "error");
    await expect(bubble.getByRole("status")).toContainText(/\S/);

    await expect(composer).toHaveValue("Keep my newer draft too.");
    await composer.fill("");
    await bubble.getByRole("button", { name: "Edit message", exact: true }).click();
    await expect(composer).toHaveValue("Keep the refused message.");
    await repair.click();
    await expect(page.getByRole("dialog", { name: "Pair a machine", exact: true })).toBeVisible();
  });
});

test("HUB-J5 Send again delivers without taking pane control (cas-d402)", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
  const composer = page.getByRole("textbox", { name: "Your message" });
  const send = page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true });
  const status = messageStatus(page);
  let lapsed = false;
  let taken = false;
  await page.route(/\/v1\/sessions\/[^/]+\/lease$/, (route) => {
    if (route.request().method() === "POST") { if (lapsed) taken = true; return route.fallback(); }
    return route.fulfill({ json: { held_by_me: false, controller_label: lapsed ? "Studio Mac" : null } });
  });
  await journey.stage("A message whose receipt never came settles as Not confirmed", async () => {
    await journey.open();
    await conversationRow(page).click();
    await expect(send).toBeVisible();
    await composer.fill("Did the Mac tests start?");
    const unreceipted = hub.nextSend();
    await send.click();
    expect((await unreceipted).text).toBe("Did the Mac tests start?");
    await hub.supervisorSays(PELICAN, "Gate run 3 of 3 is going.");
    await expect(page.getByRole("log")).toContainText("Gate run 3 of 3 is going.");
    await page.clock.fastForward(5_000);
    await hub.supervisorSays(PELICAN, "Tests are running on the Mac.");
    const bubble = turn(page, "Did the Mac tests start?");
    await expect(bubble).toHaveAttribute("data-state", "unconfirmed");
    await expect(bubble).toHaveAttribute("data-settled", "true");
  });
  await journey.stage("The lease lapses; Send again delivers without taking control", async () => {
    lapsed = true;
    // The next lease read finds this device no longer in control.
    await page.clock.fastForward(30_000);
    await expect.poll(() => taken || page.evaluate(() => document.querySelector(".conversation-send-again") !== null)).toBe(true);
    const resent = hub.nextSend();
    await page.getByRole("button", { name: "Send this message again" }).click();
    expect((await resent).text).toBe("Did the Mac tests start?");
    expect(taken, "Send again does not acquire pane control").toBe(false);
    await hub.deliverLatest(PELICAN);
    await expect(deliveryLine(page, "Did the Mac tests start?")).toBeVisible();
    await expect(page.getByRole("log").getByText("Did the Mac tests start?")).toHaveCount(1);
    await expect(status).toBeHidden();
  });
});
