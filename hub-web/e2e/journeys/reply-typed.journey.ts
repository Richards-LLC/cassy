import { test, expect } from "./journey";
import { ATLAS, STUDIO, PELICAN } from "./world";

test("HUB-J5 reply by typing", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS, STUDIO], paired: ["atlas", "studio"] });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const composer = page.getByRole("textbox", { name: "Your message" });
  const send = page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true });

  await journey.stage("Open the conversation", async () => {
    await journey.open();
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(send).toBeVisible();
  });

  await journey.stage("Write and send", async () => {
    await composer.fill("Please keep the release notes short this time.");
    const sent = hub.nextSend();
    await send.click();
    expect((await sent).text).toBe("Please keep the release notes short this time.");
    await expect(page.locator('.conversation-turn[data-state="sending"]')).toBeVisible();
    await expect(composer).toHaveValue("");
  });

  let queued = 0;
  await journey.stage("See it delivered", async () => {
    queued = hub.deliverLatest(PELICAN);
    const delivered = page.locator('.conversation-turn[data-state="acknowledged"]');
    await expect(delivered.getByRole("status")).toHaveText("Delivered");
    await expect(page.getByText(`Sending to ${PELICAN}…`)).toBeHidden();
  });

  await journey.stage("See it answered", async () => {
    hub.answerQueued(PELICAN, queued, "Understood — two lines per item, no process talk.");
    await expect(page.getByRole("log").getByText("Understood — two lines per item, no process talk.")).toBeVisible();
    await expect(page.locator('.conversation-turn[data-state="replied"]')).toHaveCount(1);
    await expect(page.locator(".conversation-delivered")).toHaveCount(0);
  });

  await journey.stage("A refused message says why", async () => {
    await composer.fill("Ship it without the gate.");
    const refused = hub.nextSend();
    await send.click();
    hub.send(PELICAN, { Error: { client_ref: (await refused).client_ref, message: "forbidden" } });
    const bubble = page.locator('.conversation-turn[data-state="error"]');
    await expect(bubble).toBeVisible();
    await expect(bubble.getByRole("status")).toContainText("This device isn't the one in control of the session.");
    await expect(bubble.getByRole("status")).toContainText("Take control from the header, then retry.");
    await expect(bubble.getByRole("status")).not.toContainText("forbidden");
    await expect(list.getByText("Not sent: Ship it without the gate.")).toBeVisible();
  });

  await journey.stage("Edit and resend retires the refused message", async () => {
    await page.getByRole("button", { name: "Edit message", exact: true }).click();
    await expect(composer).toHaveValue("Ship it without the gate.");
    await composer.fill("Ship it after the gate passes.");
    const resent = hub.nextSend();
    await send.click();
    expect((await resent).text).toBe("Ship it after the gate passes.");
    const retired = page.locator('.conversation-turn[data-state="error"]');
    await expect(retired.getByRole("status")).toHaveText("Not sent · replaced by your edit");
    await expect(page.getByRole("button", { name: "Retry sending" })).toHaveCount(0);
    await expect(list.getByText("You: Ship it after the gate passes.")).toBeVisible();
    await expect(list.getByText(/Not sent: /)).toHaveCount(0);
  });
});
