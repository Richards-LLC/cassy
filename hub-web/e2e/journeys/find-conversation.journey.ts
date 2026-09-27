import { join } from "node:path";
import { test, expect, RECEIPTS } from "./journey";
import { ATLAS, STUDIO, PELICAN, OTTER } from "./world";
import type { Machine } from "./hub-double";

// A third machine with a 40-character name (cas-1ca1): its row must ellipsise
// the name and keep the time stamp readable.
const FORGE: Machine = {
  id: "forge",
  label: "Forge build box with an unusual hostname · Linux",
  sessions: [{ name: "quiet-heron-7", supervisor: "quiet-heron-7", project_dir: "/projects/lighthouse", workers: ["swift-lark-3"], liveness: "live" }],
};

test("HUB-J3 find the conversation that needs me", async ({ page, journey }) => {
  // Eight stages, two searches and the palette: under a loaded factory host it
  // ran at the project's 60 s budget (QA N01/N3), so it gets its own headroom.
  test.setTimeout(120_000);
  const hub = await journey.hub({ machines: [ATLAS, STUDIO, FORGE], paired: ["atlas", "studio", "forge"] });
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

  // The no-selection canvas: no context column (not even the folded strip),
  // the welcome centred in the canvas (journey F15, cas-9225).
  const welcomeLayout = () => page.evaluate(() => {
    const shell = document.querySelector<HTMLElement>(".conversation-shell")!;
    const main = document.querySelector<HTMLElement>(".conversation-main")!.getBoundingClientRect();
    const welcome = document.querySelector<HTMLElement>(".conversation-welcome h2")!.getBoundingClientRect();
    const rail = document.querySelector<HTMLElement>(".conversation-context");
    return {
      contextOpen: shell.classList.contains("context-open"),
      railWidth: rail ? Math.round(rail.getBoundingClientRect().width) : 0,
      mainRight: Math.round(main.right),
      welcomeLeft: Math.round(welcome.left),
      centred: Math.abs((welcome.left + welcome.right) / 2 - (main.left + main.right) / 2) <= 1,
    };
  });
  let firstWelcome: Awaited<ReturnType<typeof welcomeLayout>> | undefined;

  await journey.stage("See every machine's supervisors in one list", async () => {
    await journey.open();
    await expect(list.getByRole("button")).toHaveCount(3);
    firstWelcome = await welcomeLayout();
    expect(firstWelcome).toMatchObject({ contextOpen: false, railWidth: 0, mainRight: 1280, centred: true });
    // Machines list in id order (atlas, forge, studio). Rows are titled by project, then machine; the codename is tertiary.
    await expect(list.locator(".conversation-project")).toHaveText(["cas-src", "lighthouse", "gabber-studio"]);
    await expect(list.locator(".conversation-machine")).toHaveText(["Atlas", "Forge build box with an unusual hostname", "Studio Mac"]);
    await expect(list.locator(".conversation-supervisor")).toHaveText([PELICAN, "quiet-heron-7", OTTER]);
    await expect(search).toBeVisible();
    await expect(search).toHaveAttribute("placeholder", "Search conversations (Ctrl K)");
  });

  await journey.stage("Notice a new reply while away", async () => {
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
    // The list is on screen beside the thread, so there is no back step (F17):
    // moving to another conversation is one click on its row.
    await expect(page.getByRole("button", { name: "‹ Conversations", exact: true })).toBeHidden();
    await list.getByRole("button", { name: /gabber-studio/ }).click();
    await expect(page.getByRole("button", { name: `Send to ${OTTER}`, exact: true })).toBeVisible();
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
    await expect(list.getByRole("button")).toHaveCount(3);
    await search.fill("gabber-studio");
    await list.getByRole("button", { name: /gabber-studio/ }).click();
    await expect(page.getByRole("button", { name: `Send to ${OTTER}`, exact: true })).toBeVisible();
    // The header names the project once; machine and codename sit beneath it.
    await expect(page.locator(".conversation-identity h1")).toHaveText("gabber-studio");
    await expect(page.locator(".conversation-host")).toContainText(`Studio Mac · macOS · ${OTTER}`);
    await search.fill("");
    await search.blur();
    await expect(list.getByRole("button")).toHaveCount(3);
  });

  // Catalog step titles verbatim (docs/qa/journeys.md HUB-J3, cas-0e5c).
  await journey.stage("An empty thread's card also leads with the project, with machine and codename beneath it", async () => {
    // gabber-studio is open with no turns yet: its card is titled by the
    // project, as the header and the row are, with machine · codename beneath
    // and the codename as its own identifier (cas-1ca1).
    const card = page.locator(".thread .empty");
    await expect(card).toBeVisible();
    await expect(card.locator("b")).toHaveText("gabber-studio");
    await expect(card.locator("b")).not.toHaveClass(/codename/);
    await expect(card.locator(".proj2")).toHaveText(`Studio Mac · macOS · ${OTTER}`);
    await expect(card.locator(".proj2 > .codename")).toHaveText(OTTER);
    await expect(card.getByText("Project unavailable")).toHaveCount(0);
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
    await expect(list.getByRole("button")).toHaveCount(3);
  });

  await journey.stage("A 40-character machine name ellipsises in its row and never runs under the time stamp, on desktop and at 390px", async () => {
    const row = list.getByRole("button", { name: /lighthouse/ });
    const clearOfTime = () => row.evaluate((node) => {
      const name = node.querySelector(".conversation-machine-name")!.getBoundingClientRect();
      const time = node.querySelector(".conversation-when")?.getBoundingClientRect();
      const label = node.querySelector<HTMLElement>(".conversation-machine-name")!;
      return { ellipsised: label.scrollWidth > label.clientWidth, clear: !time || name.right <= time.left + 0.5 };
    });
    // A machine that wraps to its own line never starts it with the separator
    // dot: the dot sits past the title's left edge, clipped (cas-1ca1 F02).
    const noLeadingDot = () => list.locator(".conversation-row").evaluateAll((rows) => rows.every((node) => {
      const title = node.querySelector(".conversation-title")!.getBoundingClientRect();
      const project = node.querySelector(".conversation-project")!.getBoundingClientRect();
      const machine = node.querySelector(".conversation-machine-name")!.getBoundingClientRect();
      const dot = node.querySelector(".conversation-sep")!.getBoundingClientRect();
      const wrapped = machine.top > project.top + 4;
      return wrapped ? dot.right <= title.left + 0.5 : dot.left >= project.right;
    }));
    expect(await noLeadingDot()).toBe(true);
    // Desktop: the 40-character name ellipsises before the time stamp.
    await expect(row.locator(".conversation-when")).toBeVisible();
    expect(await clearOfTime()).toEqual({ ellipsised: true, clear: true });
    await expect(row.locator(".conversation-machine")).toHaveAttribute("title", "Forge build box with an unusual hostname");
    // Phone width: the same, with the list as the whole page.
    await page.setViewportSize({ width: 390, height: 844 });
    await page.getByRole("button", { name: "‹ Conversations", exact: true }).click();
    await expect(row).toBeVisible();
    expect(await clearOfTime()).toEqual({ ellipsised: true, clear: true });
    expect(await noLeadingDot()).toBe(true);
    // Receipt beside the stage screenshots: the 390 px list with the long name.
    await page.screenshot({ path: join(RECEIPTS, journey.id, "long-machine-phone.png") });
    await page.setViewportSize({ width: 1280, height: 720 });
    // Back at desktop width with nothing open, after a thread had filled the
    // context rail: the canvas is the first load's, not an empty 240px column
    // beside a shifted welcome (journey F15).
    await expect(page.locator(".conversation-welcome")).toBeVisible();
    expect(await welcomeLayout()).toEqual(firstWelcome);
  });

  await journey.stage("Jump to a supervisor by name", async () => {
    await page.getByRole("button", { name: "Appearance & commands" }).click();
    // Commands are grouped; the debugging switches wait, collapsed, in Advanced.
    const palette = page.locator("#command-palette");
    // Commands sit with what they act on: the lease with this session,
    // Paired machines with the machines, and "Dismiss all info" only when
    // there is something to dismiss (3.30.0 journey F4).
    // With no session open there is no "This session" group at all.
    await expect(palette.locator(".palette-group-heading:visible")).toHaveText(["Conversations", "Machines", "Appearance", "Advanced"]);
    await expect(palette.locator('[data-palette-group="conversations"] .palette-command:not([data-palette-session])')).toHaveCount(0);
    await expect(palette.locator('[data-palette-group="machines"]')).toContainText("Paired machines");
    await expect(palette.getByRole("button", { name: /Dismiss all info/ })).toHaveCount(0);
    await expect(palette.getByRole("button", { name: /Show worker panes/ })).toBeHidden();
    await expect(palette.getByRole("button", { name: /Open the terminal view/ })).toBeHidden();
    // cas-71af (9ecd QA F01): the collapsed Advanced row speaks the palette's
    // own words, machines and terminal, with no leftover "sessions".
    await expect(palette.locator('[data-palette-action="terminal-view"] small')).toHaveText("Machines and terminal controls");
    await filter.fill("worker");
    await expect(palette.getByRole("button", { name: /Show worker panes/ })).toBeVisible();
    // A project name finds its session too, and the row names that project.
    const rows = palette.locator(".palette-command");
    await filter.fill("gabber");
    await expect(rows.visible()).toHaveCount(1);
    await expect(rows.visible().first()).toContainText("Jump to gabber-studio");
    await expect(rows.visible().first()).toContainText(`${OTTER} · Studio Mac`);
    // Every word must match, not the whole phrase, as in the list search.
    await filter.fill("gabber studio");
    await expect(rows.visible()).toHaveCount(1);
    await expect(rows.visible().first()).toContainText("Jump to gabber-studio");
    await filter.fill(OTTER);
    await expect(palette.getByRole("button", { name: /Show worker panes/ })).toBeHidden();
    const commands = page.locator("#command-palette .palette-command");
    await expect(commands.visible()).toHaveCount(1);
    await expect(commands.visible().first()).toContainText("Jump to gabber-studio");
    await expect(page.getByRole("button", { name: /Appearance · Dark/ })).toBeHidden();
    await page.getByRole("button", { name: /Jump to gabber-studio/ }).click();
    await expect(page.locator("#command-palette")).toBeHidden();
    await expect(page.getByRole("button", { name: `Send to ${OTTER}`, exact: true })).toBeVisible();
    await expect(page.locator(".conversation-identity h1")).toHaveText("gabber-studio");
    // A mouse jump lands in the opened conversation's composer too, and
    // landing there must not freeze the shell at its pre-load state: once
    // this first visit's lease loads, the palette offers to let other devices
    // type here, "Release control" kept as its hint (journey F16).
    await expect(page.getByRole("textbox", { name: "Your message" })).toBeFocused();
    const control = page.locator('#command-palette [data-palette-action="control"]');
    await expect(control.locator("span")).toHaveText("Let other devices type here");
    await expect(control.locator("small")).toHaveText("Release control of this conversation");
    // The lease command sits in its own group once a conversation is open,
    // and the palette speaks of conversations, not sessions.
    await expect(page.locator("#palette-group-session")).toHaveText("This conversation");
    await expect(page.locator("#command-palette-query")).toHaveAttribute("placeholder", "Type a command or conversation");
    await expect(page.locator('#command-palette [data-palette-group="session"] [data-palette-action="control"]')).toHaveCount(1);
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
    await expect(page.getByRole("button", { name: /Jump to gabber-studio/ })).toBeFocused();
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
    const row = page.getByRole("button", { name: /Jump to cas-src/ });
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
    await expect(noMatch).toHaveText("No commands or conversations match “zzzz”.");
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
    await expect(commands.visible().first()).toContainText("Jump to gabber-studio");
    await page.keyboard.press("Escape");
  });

  await journey.stage("Open Paired machines from the palette, then a conversation", async () => {
    // Paired machines replaces the palette; the palette must stay closed
    // afterwards, not come back over the next conversation opened (cas-dfc8).
    const palette = page.locator("#command-palette");
    const paired = page.locator("#paired-machines-dialog");
    // The last stage's Escape only cleared the filter; close the palette first.
    if (await palette.isVisible()) await page.getByRole("button", { name: "Close command palette" }).click();
    await expect(palette).toBeHidden();
    await page.getByRole("button", { name: "Appearance & commands" }).click();
    await expect(palette).toBeVisible();
    await palette.getByRole("button", { name: /Paired machines/ }).click();
    await expect(paired).toBeVisible();
    await expect(palette).toBeHidden();
    await page.locator("#paired-machines-close").click();
    await expect(paired).toBeHidden();
    await list.getByRole("button", { name: /gabber-studio/ }).click();
    await expect(page.getByRole("button", { name: `Send to ${OTTER}`, exact: true })).toBeVisible();
    await expect(palette).toBeHidden();
    // And once more from the other conversation: still closed.
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
    await expect(palette).toBeHidden();
  });

  await journey.stage("Open a conversation over a slow relay: one calm line, and the footer stays Connected", async () => {
    // Journey F3: opening a conversation used to flash "ATTEMPT 1 · dialing
    // the relay" and drop the footer to Reconnecting / "2 connected" while the
    // machine itself stayed connected. The double holds the attach for 2 s,
    // as a real relay does, and every frame of the footer and the pane is
    // recorded while it opens.
    const footerState = page.locator("#hub-footer-badges .machine-badge-state");
    await expect(footerState).toHaveText("Connected");
    await page.evaluate(() => {
      const seen = { footer: new Set<string>(), pane: new Set<string>() };
      (window as unknown as { __attachSeen: typeof seen }).__attachSeen = seen;
      const sample = () => {
        const footer = document.querySelector<HTMLElement>("#hub-footer-badges .machine-badge-state");
        if (footer) seen.footer.add(footer.innerText.trim());
        const pane = document.querySelector<HTMLElement>(".conversation-pane-slot");
        if (pane) seen.pane.add(pane.innerText.trim());
      };
      new MutationObserver(sample).observe(document.body, { subtree: true, childList: true, characterData: true, attributes: true });
    });
    hub.delayAttach("quiet-heron-7", 2_000);
    await list.getByRole("button", { name: /lighthouse/ }).click();
    const opening = page.locator(".conversation-pane-slot .terminal-connecting-title");
    await expect(opening).toHaveText("Opening the conversation…");
    // Past the quiet window the attempt and stage are offered behind Details, closed.
    const details = page.locator(".conversation-pane-slot .connection-details");
    await expect(details.getByText("Details", { exact: true })).toBeVisible();
    await expect(details).not.toHaveAttribute("open", "");
    await expect(details.locator(".connection-timeline")).toBeHidden();
    await expect(page.getByRole("button", { name: "Send to quiet-heron-7", exact: true })).toBeVisible();
    await expect(page.locator(".thread .empty b")).toHaveText("lighthouse", { timeout: 10_000 });
    const seen = await page.evaluate(() => {
      const { footer, pane } = (window as unknown as { __attachSeen: { footer: Set<string>; pane: Set<string> } }).__attachSeen;
      return { footer: [...footer], pane: [...pane] };
    });
    // cas-71af (e918 QA F02): the empty card's machine · codename line yields
    // the 40-character machine name first; the codename stays whole on a phone.
    await page.setViewportSize({ width: 390, height: 844 });
    const meta = page.locator(".thread .empty .proj2");
    expect(await meta.locator(".codename").evaluate((element) => element.scrollWidth <= element.clientWidth + 1), "codename whole at 390px").toBe(true);
    expect(await meta.locator(".proj2-machine").evaluate((element) => element.scrollWidth > element.clientWidth), "machine ellipsised at 390px").toBe(true);
    await page.setViewportSize({ width: 1280, height: 720 });
    expect(seen.footer, "the footer while the conversation opened").toEqual(["Connected"]);
    const jargon = seen.pane.filter((text) => /relay|attempt|authori[sz]ation|handshake|heartbeat|resolving|dialing/i.test(text));
    expect(jargon, "relay-stage words on the default attach surface").toEqual([]);
    await expect(footerState).toHaveText("Connected");
  });

  await journey.stage("A first open that misses the 3-second mark retries calmly, and the footer stays Connected", async () => {
    // cas-28df: a conversation whose first attach sends no session state
    // within 3 s retries. On a live machine that used to drop the footer to
    // "1 connected" and flash "Terminal unavailable" and the full retry
    // timeline. A fresh visit reopens lighthouse; the double answers its first
    // attach only after 3.5 s, so the first try times out and the retry opens it.
    await page.addInitScript(() => {
      const seen = { footer: [] as string[], pane: [] as string[], connected: false };
      (window as unknown as { __retrySeen: typeof seen }).__retrySeen = seen;
      const sample = () => {
        const footer = document.querySelector<HTMLElement>("#hub-footer-badges .machine-badge-state")?.innerText.trim();
        // Machines come up one by one on load; watch the footer once all are.
        if (footer === "Connected") seen.connected = true;
        if (seen.connected && footer && seen.footer.at(-1) !== footer) seen.footer.push(footer);
        const pane = document.querySelector<HTMLElement>(".conversation-pane-slot")?.innerText.trim();
        if (pane && seen.pane.at(-1) !== pane) seen.pane.push(pane);
      };
      document.addEventListener("DOMContentLoaded", () => new MutationObserver(sample).observe(document.body, { subtree: true, childList: true, characterData: true, attributes: true }));
    });
    hub.delayAttach("quiet-heron-7", 3_500);
    await page.reload();
    const opening = page.locator(".conversation-pane-slot .terminal-connecting-title");
    await expect(opening).toHaveText("Opening the conversation…");
    await expect(page.locator(".thread .empty b")).toHaveText("lighthouse", { timeout: 15_000 });
    const seen = await page.evaluate(() => (window as unknown as { __retrySeen: { footer: string[]; pane: string[] } }).__retrySeen);
    expect(seen.pane.some((text) => text.startsWith("Opening the conversation…")), "the pane said it was opening").toBe(true);
    expect(seen.footer, "the footer once every machine was up").toEqual(["Connected"]);
    const alarm = seen.pane.filter((text) => /Terminal unavailable|interrupted|retrying|Try again|relay|attempt|diagnostic|handshake/i.test(text));
    expect(alarm, "retry wording on the default surface while it opened").toEqual([]);
    await expect(page.locator("#hub-footer-badges .machine-badge-state")).toHaveText("Connected");
  });
});
