import { test, expect } from "./journey";
import { ATLAS, PELICAN } from "./world";

test("HUB-J11 the connection drops mid-conversation and recovers", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
  const composer = page.getByRole("textbox", { name: "Your message" });

  await journey.stage("Open the conversation", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
  });

  await journey.stage("The network drops", async () => {
    hub.drop(PELICAN);
    await expect(page.getByText(/Connection interrupted — reconnecting/)).toBeVisible();
  });

  await journey.stage("It reconnects on its own", async () => {
    await expect.poll(() => hub.hasSocket(PELICAN), { timeout: 30_000 }).toBe(true);
    await expect(page.getByText(/Connection interrupted — reconnecting/)).toBeHidden({ timeout: 15_000 });
  });

  await journey.stage("Sending works again", async () => {
    await composer.fill("Are we back?");
    const sent = hub.nextSend();
    await page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true }).click();
    expect((await sent).text).toBe("Are we back?");
    hub.answerLatest(PELICAN, "Back. Nothing was lost.");
    await expect(page.getByRole("log").getByText("Back. Nothing was lost.")).toBeVisible();
  });
});
