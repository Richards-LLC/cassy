import { test, expect } from "./journey";
import { ATLAS, STUDIO, PELICAN, OTTER } from "./world";

test("HUB-J3 find the conversation that needs me", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS, STUDIO], paired: ["atlas", "studio"] });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const search = page.getByRole("searchbox", { name: "Search conversations" });
  const filter = page.getByRole("searchbox", { name: "Filter commands" });
  // Ctrl/Cmd+K lands in the list search; pressed again from there, it opens
  // the command palette.
  const openPaletteFromKeyboard = async () => {
    await page.keyboard.press("ControlOrMeta+k");
    await expect(search).toBeFocused();
    await page.keyboard.press("ControlOrMeta+k");
    await expect(filter).toBeFocused();
  };

  await journey.stage("See every machine's supervisors in one list", async () => {
    await journey.open();
    await expect(list.getByRole("button")).toHaveCount(2);
    // Rows are titled by project, then machine; the codename is tertiary.
    await expect(list.locator(".conversation-project")).toHaveText(["cas-src", "gabber-studio"]);
    await expect(list.locator(".conversation-machine")).toHaveText(["Atlas", "Studio Mac"]);
    await expect(list.locator(".conversation-supervisor")).toHaveText([PELICAN, OTTER]);
    await expect(search).toBeVisible();
    await expect(search).toHaveAttribute("placeholder", "Search conversations (Ctrl K)");
  });

  await journey.stage("Notice a new reply while away", async () => {
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
    await page.getByRole("button", { name: "‹ Conversations", exact: true }).click();
    hub.supervisorSays(PELICAN, "The staging deploy finished; nothing needs you yet.", { kind: "status" });
    await expect(list.getByRole("button", { name: /cas-src/ }).getByLabel("1 unread")).toBeVisible();
  });

  await journey.stage("Find the conversation through the list search", async () => {
    // A project name leaves its conversation as the only row.
    await search.fill("gabber");
    await expect(list.getByRole("button")).toHaveCount(1);
    await expect(list.getByRole("button").first()).toContainText("gabber-studio");
    // The machine and the supervisor codename find it too.
    await search.fill("atlas");
    await expect(list.getByRole("button")).toHaveCount(1);
    await expect(list.getByRole("button").first()).toContainText("cas-src");
    await search.fill(OTTER);
    await expect(list.getByRole("button")).toHaveCount(1);
    await expect(list.getByRole("button").first()).toContainText("gabber-studio");
    await search.fill("no-such-project");
    await expect(list.getByRole("button")).toHaveCount(0);
    await expect(page.locator("#conversation-empty")).toContainText("No conversation matches “no-such-project”.");
    // Escape brings the whole list back.
    await search.press("Escape");
    await expect(search).toHaveValue("");
    await expect(list.getByRole("button")).toHaveCount(2);
    await search.fill("gabber-studio");
    await list.getByRole("button", { name: /gabber-studio/ }).click();
    await expect(page.getByRole("button", { name: `Send to ${OTTER}`, exact: true })).toBeVisible();
    // The header names the project once; machine and codename sit beneath it.
    await expect(page.locator(".conversation-identity h1")).toHaveText("gabber-studio");
    await expect(page.locator(".conversation-host")).toContainText(`Studio Mac · macOS · ${OTTER}`);
    await search.fill("");
    await search.blur();
    await expect(list.getByRole("button")).toHaveCount(2);
  });

  await journey.stage("Find the conversation from the keyboard", async () => {
    // Ctrl+K from anywhere lands in the search; Enter opens the leading match
    // with the reply box focused, and the list is whole again.
    await page.keyboard.press("ControlOrMeta+k");
    await expect(search).toBeFocused();
    await page.keyboard.type("cas-src");
    await expect(list.getByRole("button")).toHaveCount(1);
    await page.keyboard.press("Enter");
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
    await expect(page.locator(".conversation-identity h1")).toHaveText("cas-src");
    await expect(page.getByRole("textbox", { name: "Your message" })).toBeFocused();
    await expect(search).toHaveValue("");
    await expect(list.getByRole("button")).toHaveCount(2);
  });

  await journey.stage("Jump to a supervisor by name", async () => {
    await page.getByRole("button", { name: "Appearance & commands" }).click();
    // Commands are grouped; the debugging switches wait, collapsed, in Advanced.
    const palette = page.locator("#command-palette");
    await expect(palette.locator(".palette-group-heading")).toHaveText(["Conversations", "Appearance", "Advanced"]);
    await expect(palette.getByRole("button", { name: /Show worker panes/ })).toBeHidden();
    await expect(palette.getByRole("button", { name: /Open the terminal view/ })).toBeHidden();
    await filter.fill("worker");
    await expect(palette.getByRole("button", { name: /Show worker panes/ })).toBeVisible();
    await filter.fill(OTTER);
    await expect(palette.getByRole("button", { name: /Show worker panes/ })).toBeHidden();
    const commands = page.locator("#command-palette .palette-command");
    await expect(commands.visible()).toHaveCount(1);
    await expect(commands.visible().first()).toContainText(`Jump to ${OTTER}`);
    await expect(page.getByRole("button", { name: /Appearance · Dark/ })).toBeHidden();
    await page.getByRole("button", { name: new RegExp(`Jump to ${OTTER}`) }).click();
    await expect(page.locator("#command-palette")).toBeHidden();
    await expect(page.getByRole("button", { name: `Send to ${OTTER}`, exact: true })).toBeVisible();
    await expect(page.locator(".conversation-identity h1")).toHaveText("gabber-studio");
    // A mouse jump lands in the opened conversation's composer too, and
    // landing there must not freeze the shell at its pre-load state: once
    // this first visit's lease loads, the palette offers "Release control".
    await expect(page.getByRole("textbox", { name: "Your message" })).toBeFocused();
    await expect(page.locator('#command-palette [data-palette-action="control"]')).toContainText("Release control");
    await expect(page.getByRole("textbox", { name: "Your message" })).toBeFocused();
  });

  await journey.stage("Jump to a supervisor from the keyboard", async () => {
    // Mid-draft in the composer: closing the palette hands focus back to it,
    // which must not hold the switch back either.
    await page.getByRole("textbox", { name: "Your message" }).fill("Half a thought");
    await openPaletteFromKeyboard();
    await filter.fill(PELICAN);
    await filter.press("Enter");
    // Enter in the filter picks the leading "Jump to" row; the palette must
    // close with it rather than keep the modal up over the opened session.
    await expect(page.locator("#command-palette")).toBeHidden();
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
    await expect(page.locator(".conversation-identity h1")).toHaveText("cas-src");
    // Focus lands in the opened conversation's composer, so the next keystroke
    // is part of the reply; the other conversation's draft stays its own.
    const composer = page.getByRole("textbox", { name: "Your message" });
    await expect(composer).toBeFocused();
    await expect(composer).toHaveValue("");
    await page.keyboard.type("On it");
    await expect(composer).toHaveValue("On it");
    // From the focused composer, arrowing onto the row must keep the filter:
    // opening the palette mid-draft owes a rebuild that must not land here.
    await openPaletteFromKeyboard();
    await filter.fill(OTTER);
    await filter.press("ArrowDown");
    await expect(page.getByRole("button", { name: new RegExp(`Jump to ${OTTER}`) })).toBeFocused();
    await expect(filter).toHaveValue(OTTER);
    await page.keyboard.press("Enter");
    await expect(page.locator("#command-palette")).toBeHidden();
    await expect(page.getByRole("button", { name: `Send to ${OTTER}`, exact: true })).toBeVisible();
    await expect(composer).toBeFocused();
    await expect(composer).toHaveValue("Half a thought");
  });

  await journey.stage("A live update while a palette row is focused", async () => {
    // Land on a first visit and type straight away, so the conversation's
    // status and lease load while the reply box is busy and a rebuild is owed.
    const composer = page.getByRole("textbox", { name: "Your message" });
    await page.goto("./");
    await expect(page.locator("#command-palette")).toHaveCount(1);
    await openPaletteFromKeyboard();
    await filter.fill(OTTER);
    await filter.press("Enter");
    await expect(composer).toBeFocused();
    await page.keyboard.type("hi");
    await openPaletteFromKeyboard();
    await filter.fill(PELICAN);
    await filter.press("ArrowDown");
    const row = page.getByRole("button", { name: new RegExp(`Jump to ${PELICAN}`) });
    await expect(row).toBeFocused();
    // Any render while the row has focus must leave the palette alone.
    hub.supervisorSays(OTTER, "Still here.", { kind: "status" });
    await expect(page.locator(".conversation-host")).toBeVisible();
    await page.waitForTimeout(300);
    await expect(filter).toHaveValue(PELICAN);
    await expect(row).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(page.locator("#command-palette")).toBeHidden();
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
    await expect(composer).toBeFocused();
  });

  await journey.stage("Reopen the palette to the full list", async () => {
    const palette = page.locator("#command-palette");
    const commands = page.locator("#command-palette .palette-command");
    const noMatch = page.locator("#palette-no-match");
    let all = 0;
    const reopen = async () => {
      await openPaletteFromKeyboard();
      await expect(filter).toHaveValue("");
      await expect(commands.visible()).toHaveCount(all);
      await expect(noMatch).toBeHidden();
    };
    // Start with focus outside any field (a fresh load), so no deferred
    // rebuild happens to replace the dialog on close and hide the bug.
    await page.goto("./");
    await expect(palette).toHaveCount(1);
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
    // Close with ×, which does not rebuild the shell: the dialog comes back.
    await openPaletteFromKeyboard();
    // The full list is every command on screen, Advanced still collapsed.
    all = await commands.visible().count();
    expect(all).toBeGreaterThan(0);
    await filter.fill("zzzz");
    await expect(noMatch).toHaveText("No commands or sessions match “zzzz”.");
    await page.getByRole("button", { name: "Close command palette" }).click();
    await expect(palette).toBeHidden();
    await reopen();
    // A setting row closes it the same way.
    await filter.fill("Appearance · Light");
    await filter.press("Enter");
    await expect(palette).toBeHidden();
    await reopen();
    // So does jumping to the conversation that is already open.
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
    await filter.fill(PELICAN);
    await filter.press("Enter");
    await expect(palette).toBeHidden();
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
    await reopen();
    // Typing after the reopen filters from scratch.
    await filter.pressSequentially(OTTER.slice(0, 6));
    await expect(commands.visible().first()).toContainText(`Jump to ${OTTER}`);
    await page.keyboard.press("Escape");
  });
});
