import { test, expect } from "./journey";
import { ATLAS, STUDIO, PELICAN, OTTER } from "./world";

test("HUB-J8 switch between machines without losing my place", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS, STUDIO], paired: ["atlas", "studio"] });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const composer = page.getByRole("textbox", { name: "Your message" });
  const back = page.getByRole("button", { name: "‹ Conversations", exact: true });

  await journey.stage("Start a draft on the Linux machine", async () => {
    await journey.open();
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(page.locator(".conversation-identity h1")).toHaveText("cas-src");
    await expect(page.locator(".conversation-host")).toContainText(`Atlas · Linux · ${PELICAN}`);
    await composer.fill("Draft: ask about the flaky pairing test");
  });

  await journey.stage("Switch to the Mac and send there", async () => {
    await back.click();
    await list.getByRole("button", { name: /gabber-studio/ }).click();
    await expect(page.locator(".conversation-identity h1")).toHaveText("gabber-studio");
    await expect(page.locator(".conversation-host")).toContainText(`Studio Mac · macOS · ${OTTER}`);
    await expect(composer).toHaveValue("");
    await composer.fill("Is the Mac build green?");
    const sent = hub.nextSend();
    await page.getByRole("button", { name: `Send to ${OTTER}`, exact: true }).click();
    expect(await sent).toMatchObject({ machine: "studio", target: OTTER, text: "Is the Mac build green?" });
  });

  await journey.stage("Come back to the draft", async () => {
    await back.click();
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(page.locator(".conversation-host")).toContainText("Atlas · Linux");
    await expect(composer).toHaveValue("Draft: ask about the flaky pairing test");
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
  });

  await journey.stage("Reopen the session picker after closing it", async () => {
    const picker = page.locator("#session-picker");
    const toggle = page.locator("#session-picker-toggle");
    const palette = page.locator("#command-palette");
    await page.getByRole("button", { name: "Terminal view" }).click();
    await expect(toggle).toBeVisible();
    // Closing the picker does not rebuild the shell, and the next periodic
    // render papers over a stale "open" state within a few seconds. So each
    // check is short: the picker must open, and say so, at once — not when a
    // later render happens to rebuild the shell.
    const soon = { timeout: 1_000 };
    const open = async () => {
      await toggle.click();
      await expect(picker).toBeVisible(soon);
      await expect(toggle).toHaveAttribute("aria-expanded", "true", soon);
    };
    const closed = async () => {
      await expect(picker).toBeHidden(soon);
      await expect(toggle).toHaveAttribute("aria-expanded", "false", soon);
    };
    await open();
    await page.keyboard.press("Escape");
    await closed();
    await open();
    await page.getByRole("button", { name: "Close session picker" }).click();
    await closed();
    await open();
    // A closed picker must not pop back open over the next dialog either.
    await page.keyboard.press("Escape");
    await closed();
    await page.keyboard.press("ControlOrMeta+k");
    await expect(palette).toBeVisible(soon);
    await expect(picker).toBeHidden();
    await page.keyboard.press("Escape");
    await expect(palette).toBeHidden();
    await expect(picker).toBeHidden();
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
  });

  await journey.stage("Keep my place in the session picker while updates arrive", async () => {
    const picker = page.locator("#session-picker");
    const filter = page.getByRole("searchbox", { name: "Filter sessions" });
    const entry = (session: string) => picker.locator(`.session-picker-entry[data-picker-session="${session}"]`);
    await page.locator("#session-picker-toggle").click();
    await expect(picker).toBeVisible();
    await expect(filter).toBeFocused();
    // Arrow onto the first session, then wait out several renders (the header
    // latency tick) while a hub update arrives: focus must stay on that row.
    await filter.press("ArrowDown");
    const first = picker.locator(".session-picker-entry").first();
    const firstSession = await first.getAttribute("data-picker-session");
    await expect(first).toBeFocused();
    hub.supervisorSays(OTTER, "Tests are running on the Mac.", { kind: "status" });
    await page.waitForTimeout(6_000);
    await expect(entry(firstSession!)).toBeFocused();
    // Tab to the next session: the same holds there.
    await page.keyboard.press("Tab");
    const second = picker.locator(".session-picker-entry").nth(1);
    await expect(second).toBeFocused();
    // A summary that changes the rows themselves rebuilds the list; focus
    // follows the same session onto its rebuilt row.
    const summary = (title: string, phase: string) => hub.send(OTTER, { SessionSummary: { summary: { title, description: title, phase, generated_at: new Date().toISOString() } } });
    const secondSession = await second.getAttribute("data-picker-session");
    summary("Running the Mac tests", "testing");
    await expect(entry(OTTER)).toContainText("Running the Mac tests");
    await expect(entry(secondSession!)).toBeFocused();
    // Typing in the filter while the rows are rebuilt: the filter still
    // applies to the rebuilt rows and the caret stays in it.
    await filter.fill(PELICAN);
    summary("Waiting for review", "reviewing");
    await expect(entry(OTTER)).toContainText("Waiting for review");
    await expect(filter).toBeFocused();
    await expect(picker.locator(".session-picker-entry:visible")).toHaveCount(1);
    await expect(entry(PELICAN)).toBeVisible();
    // A filtered list keeps its filter and the row under focus.
    await filter.fill(OTTER);
    await filter.press("ArrowDown");
    await expect(entry(OTTER)).toBeFocused();
    summary("Queueing the deploy", "building");
    await expect(entry(OTTER)).toContainText("Queueing the deploy");
    await expect(entry(OTTER)).toBeFocused();
    await expect(filter).toHaveValue(OTTER);
    await expect(picker.locator(".session-picker-entry:visible")).toHaveCount(1);
    await page.keyboard.press("Enter");
    await expect(picker).toBeHidden();
    await expect(page.locator(".session-picker-name")).toHaveText(OTTER);
  });

  await journey.stage("See which session is open while pointing at it", async () => {
    const picker = page.locator("#session-picker");
    const open = picker.locator('.session-picker-entry[aria-current="true"]');
    const other = picker.locator('.session-picker-entry:not([aria-current="true"])').first();
    const background = (entry: typeof open) => entry.evaluate((element) => getComputedStyle(element).backgroundColor);
    for (const scheme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme: scheme });
      await page.locator("#session-picker-toggle").click();
      await expect(picker).toBeVisible();
      await expect(open).toContainText(OTTER);
      // Resting: the open session is tinted, the others are not.
      await page.mouse.move(0, 0);
      const openRest = await background(open);
      const otherRest = await background(other);
      expect(openRest, `${scheme}: open session tinted at rest`).not.toBe(otherRest);
      // Pointing at the open session keeps it distinct from an ordinary hover.
      await other.hover();
      await expect.poll(() => background(other)).not.toBe(otherRest);
      const otherHover = await background(other);
      // It keeps its own tint (not the plain hover surface) and lifts under the
      // pointer, as a hovered chip does.
      await open.hover();
      await expect.poll(() => open.evaluate((element) => getComputedStyle(element).boxShadow)).not.toBe("none");
      expect(await background(open), `${scheme}: hovered open session keeps its tint`).toBe(openRest);
      expect(await background(open), `${scheme}: hovered open session still marked`).not.toBe(otherHover);
      await page.keyboard.press("Escape");
      await expect(picker).toBeHidden();
    }
    await page.emulateMedia({ colorScheme: "light" });
  });

  await journey.stage("Come back from the terminal to the reply box", async () => {
    const back = page.locator("#conversation-return");
    // From the keyboard: Enter on the return control lands in the reply box,
    // so the next keystrokes are the reply.
    await back.focus();
    await page.keyboard.press("Enter");
    await expect(composer).toBeFocused();
    await page.keyboard.type("Back from the terminal");
    await expect(composer).toHaveValue("Back from the terminal");
    await composer.fill("");
    // With the mouse: the same.
    await page.getByRole("button", { name: "Terminal view" }).click();
    await expect(back).toBeVisible();
    await back.click();
    await expect(composer).toBeFocused();
  });
});
