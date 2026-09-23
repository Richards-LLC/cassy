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

  await journey.stage("See it delivered and answered", async () => {
    hub.answerLatest(PELICAN, "Understood — two lines per item, no process talk.");
    await expect(page.getByRole("log").getByText("Understood — two lines per item, no process talk.")).toBeVisible();
    await expect(page.locator('.conversation-turn[data-state="replied"]')).toHaveCount(1);
  });

  await journey.stage("A refused message can be edited and resent", async () => {
    await composer.fill("Ship it without the gate.");
    const refused = hub.nextSend();
    await send.click();
    hub.send(PELICAN, { Error: { client_ref: (await refused).client_ref, message: "Permission refused" } });
    await expect(page.locator('.conversation-turn[data-state="error"]')).toBeVisible();
    await page.getByRole("button", { name: "Edit message", exact: true }).click();
    await expect(composer).toHaveValue("Ship it without the gate.");
    await composer.fill("Ship it after the gate passes.");
    const resent = hub.nextSend();
    await send.click();
    expect((await resent).text).toBe("Ship it after the gate passes.");
  });
});
