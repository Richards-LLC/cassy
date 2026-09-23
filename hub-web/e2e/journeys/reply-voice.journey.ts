import { test, expect } from "./journey";
import { installSpeechStub, speak } from "./hub-double";
import { ATLAS, PELICAN } from "./world";

test("HUB-J6 reply by voice", async ({ page, journey }) => {
  await installSpeechStub(page);
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
  const composer = page.getByRole("textbox", { name: "Your message" });

  await journey.stage("Open the conversation", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
    await expect(page.getByRole("button", { name: "Start listening" })).toBeEnabled();
  });

  await journey.stage("Dictate the reply", async () => {
    await page.getByRole("button", { name: "Start listening" }).click();
    await expect(page.getByRole("button", { name: "Stop listening" })).toHaveAttribute("aria-pressed", "true");
    await speak(page, "Hold the release until the Mac build is green");
    await expect(composer).toHaveValue("Hold the release until the Mac build is green");
    await expect(page.getByRole("button", { name: "Start listening" })).toBeVisible();
  });

  await journey.stage("Review and send", async () => {
    await composer.press("End");
    await composer.pressSequentially(".");
    const sent = hub.nextSend();
    await page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true }).click();
    expect((await sent).text).toBe("Hold the release until the Mac build is green.");
    hub.answerLatest(PELICAN, "Holding. I will ping you when the Mac lane is green.");
    await expect(page.getByRole("log").getByText("Holding. I will ping you when the Mac lane is green.")).toBeVisible();
  });
});
