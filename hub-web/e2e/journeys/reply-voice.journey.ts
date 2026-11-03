import { test, expect } from "./journey";
import { installSpeechStub, speak } from "./hub-double";
import { ATLAS, PELICAN } from "./world";

test("HUB-J6 reply by voice", async ({ page, journey }) => {
  const emojiRequests: string[] = [];
  page.on("request", request => { if (/chunk-emoji-picker/.test(request.url())) emojiRequests.push(request.url()); });
  await installSpeechStub(page);
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
  const composer = page.getByRole("textbox", { name: "Your message" });

  await journey.stage("Open the conversation", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
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

  await journey.stage("Choose emoji at the caret on a phone", async () => {
    expect(emojiRequests).toHaveLength(0);
    await page.setViewportSize({ width: 390, height: 544 });
    await page.emulateMedia({ reducedMotion: "reduce" });
    const trigger = page.getByRole("button", { name: "Insert emoji", exact: true });
    await trigger.focus(); await page.keyboard.press("Enter");
    const dialog = page.getByRole("dialog", { name: "Insert emoji" });
    const search = dialog.getByRole("searchbox", { name: "Search emoji" });
    await expect(search).toBeFocused();
    expect(emojiRequests.length).toBeGreaterThan(0);
    const bounds = await dialog.boundingBox();
    expect(bounds!.x).toBeGreaterThanOrEqual(0); expect(bounds!.y).toBeGreaterThanOrEqual(0);
    expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(544);
    await search.fill("nothing matches this emoji query");
    await expect(dialog.getByRole("status")).toHaveText("No emoji found. Try another search.");
    await search.fill("thumbs up: medium skin tone");
    await search.press("ArrowDown");
    await expect(dialog.getByRole("button", { name: "thumbs up: medium skin tone", exact: true })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(composer).toHaveValue("Hold the release until the Mac build is green👍🏽");
    await expect(composer).toBeFocused();
    await trigger.click();
    await expect(dialog.getByRole("group", { name: "Recent emoji" }).getByRole("button", { name: "thumbs up: medium skin tone", exact: true })).toBeVisible();
    await page.keyboard.press("Escape"); await expect(trigger).toBeFocused();
    await expect(dialog).toHaveCount(0);
    await page.setViewportSize({ width: 1280, height: 800 });
    await composer.focus();
  });

  await journey.stage("Review and send", async () => {
    // Straight from the keyboard: no click back into the field first.
    await page.keyboard.type(".");
    const sent = hub.nextSend();
    await page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true }).click();
    expect((await sent).text).toBe("Hold the release until the Mac build is green👍🏽.");
    hub.answerLatest(PELICAN, "Holding. I will ping you when the Mac lane is green.");
    await expect(page.getByRole("log").getByText("Holding. I will ping you when the Mac lane is green.")).toBeVisible();
  });
});
