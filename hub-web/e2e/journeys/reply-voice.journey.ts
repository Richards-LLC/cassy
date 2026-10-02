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
    // cas-71f4 (journey F21): on a desktop, the reply box takes focus with the
    // caret after the dictated words, ready to review without another click.
    await expect(composer).toBeFocused();
    const end = "Hold the release until the Mac build is green".length;
    expect(await composer.evaluate((field: HTMLTextAreaElement) => [field.selectionStart, field.selectionEnd])).toEqual([end, end]);
  });

  await journey.stage("Review and send", async () => {
    // Straight from the keyboard: no click back into the field first.
    await page.keyboard.type(".");
    const sent = hub.nextSend();
    await page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true }).click();
    expect((await sent).text).toBe("Hold the release until the Mac build is green.");
    hub.answerLatest(PELICAN, "Holding. I will ping you when the Mac lane is green.");
    await expect(page.getByRole("log").getByText("Holding. I will ping you when the Mac lane is green.")).toBeVisible();
  });
});
