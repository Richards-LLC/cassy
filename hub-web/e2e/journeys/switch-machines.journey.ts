import { test, expect } from "./journey";
import { ATLAS, STUDIO, PELICAN, OTTER } from "./world";
import type { Machine } from "./hub-double";

// A third machine, paired mid-journey. Its id sorts before both others and
// hashes to Atlas's accent, which is exactly what used to re-colour the fleet.
const ALPHA: Machine = {
  id: "alpha",
  label: "Alpha · Linux",
  sessions: [
    { name: "keen-lynx-1", supervisor: "keen-lynx-1", project_dir: "/projects/orion", workers: ["quick-wren-2"], liveness: "live" },
    // A live supervisor that has not spawned workers yet (cas-645e).
    { name: "lone-heron-2", supervisor: "lone-heron-2", project_dir: "/projects/lighthouse", workers: [], liveness: "live" },
    // Not reachable, and no supervisor: hidden on every surface, the fleet
    // board included (cas-645e QA F01).
    { name: "stale-owl-3", supervisor: "stale-owl-3", project_dir: "/projects/attic", workers: ["w1"], liveness: "stale_metadata" },
    { name: "headless-5", supervisor: "", project_dir: "/projects/nobody", workers: [], liveness: "live" },
  ] as Machine["sessions"],
};

test("HUB-J8 switch between machines without losing my place", async ({ page, journey }) => {
  // Fourteen stages including a pairing, palette/picker sweeps, two render
  // waits (6 s each) and a phone viewport: past the 60 s default, and a loaded
  // factory host needs the headroom.
  test.setTimeout(120_000);
  const hub = await journey.hub({ machines: [ATLAS, STUDIO, ALPHA], paired: ["atlas", "studio"] });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const composer = page.getByRole("textbox", { name: "Your message" });
  // The list stays beside the thread on desktop, so a switch is one click on a row (F17).
  const back = page.getByRole("button", { name: "‹ Conversations", exact: true });

  await journey.stage("Start a draft on the Linux machine", async () => {
    await journey.open();
    // Each machine wears its own accent: the row avatars differ, and the
    // header avatar matches the row that was opened (journey F16).
    const avatar = (project: RegExp) => list.getByRole("button", { name: project }).locator(".conversation-avatar");
    const colour = (locator: ReturnType<typeof avatar>) => locator.evaluate((element) => getComputedStyle(element).backgroundColor);
    expect(await colour(avatar(/cas-src/))).not.toBe(await colour(avatar(/gabber-studio/)));
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(page.locator(".conversation-identity h1")).toHaveText("cas-src");
    expect(await page.locator(".conversation-identity .conversation-avatar").evaluate((element) => getComputedStyle(element).backgroundColor)).toBe(await colour(avatar(/cas-src/)));
    await expect(page.locator(".conversation-host")).toContainText(`Atlas · Linux · ${PELICAN}`);
    await composer.fill("Draft: ask about the flaky pairing test");
  });

  await journey.stage("Switch to the Mac and send there", async () => {
    await expect(back).toBeHidden();
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
    // The first open rebuilt the shell; Escape still lands on the session
    // title, and Enter there reopens the picker (cas-7eaf).
    await expect(toggle).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(picker).toBeVisible();
    await page.getByRole("button", { name: "Close session picker" }).click();
    await closed();
    await expect(toggle).toBeFocused();
    await open();
    // A filter left behind by × or Escape is gone on the next open, and every
    // session is listed again: the filter and the list never disagree
    // (cas-6f39e).
    const filter = page.getByRole("searchbox", { name: "Filter sessions" });
    const rows = picker.locator(".session-picker-entry");
    const everySession = await rows.count();
    const closeButton = page.getByRole("button", { name: "Close session picker" });
    for (const close of [
      () => closeButton.click(),
      // Escape inside a search field clears the text first, so close from
      // outside it: the filter text survives the close, which is the case.
      async () => {
        await closeButton.focus();
        await page.keyboard.press("Escape");
      },
    ]) {
      await filter.fill("zz");
      await expect(picker.locator(".session-picker-entry:visible")).toHaveCount(0);
      await close();
      await closed();
      await open();
      await expect(filter).toHaveValue("");
      await expect(picker.locator(".session-picker-entry:visible")).toHaveCount(everySession);
    }
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
    await expect(page.locator(".session-picker-name")).toHaveText("gabber-studio");
    await expect(page.locator(".session-picker-codename")).toHaveText(OTTER);
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
    // locator.press focuses and presses in one step, so a periodic header
    // re-render cannot slip between the two.
    await back.press("Enter");
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

  await journey.stage("Keyboard focus lands somewhere real on every route", async () => {
    // cas-7eaf: none of these routes leaves focus on <body>.
    const offBody = () => page.evaluate(() => document.activeElement !== document.body && document.activeElement !== null);
    const terminal = page.getByRole("button", { name: "Terminal view" });
    const back = page.locator("#conversation-return");
    // Entering Terminal view from the keyboard lands in the terminal (or, before
    // a pane attaches, on the way back), never on <body>.
    await terminal.focus();
    await page.keyboard.press("Enter");
    await expect(back).toBeVisible();
    await expect.poll(offBody).toBe(true);
    await expect.poll(() => page.evaluate(() => (document.activeElement as HTMLElement).matches(".t3-ghostty-input, #conversation-return"))).toBe(true);
    // The way back is the workspace's first control in the Tab order, though
    // it is still drawn at the foot.
    expect(await page.evaluate(() => document.querySelector(".shell main button, main button")?.id)).toBe("conversation-return");
    // Enter on a session in the picker lands where the next keystroke belongs.
    await page.locator("#session-picker-toggle").click();
    await page.locator("#session-picker").getByRole("button", { name: new RegExp(OTTER) }).focus();
    await page.keyboard.press("Enter");
    await expect(page.locator("#session-picker")).toBeHidden();
    await expect(page.locator(".session-picker-name")).toHaveText("gabber-studio");
    await expect.poll(offBody).toBe(true);
    // Focus the operator moves after the pick is theirs: the landing that
    // waits for the terminal must not pull it back (cas-7eaf QA F01).
    const interrupt = page.locator("#interrupt");
    await interrupt.focus();
    await page.waitForTimeout(2_500);
    await expect(interrupt).toBeFocused();
    // Back in the conversation list: Enter on a row, and a mouse click on a
    // row, land in its reply box.
    await back.click();
    await expect(composer).toBeFocused();
    // The list stays beside the thread on a desktop (cas-479a).
    await list.getByRole("button", { name: /cas-src/ }).focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
    await expect(composer).toBeFocused();
    await list.getByRole("button", { name: /gabber-studio/ }).click();
    await expect(page.getByRole("button", { name: `Send to ${OTTER}`, exact: true })).toBeVisible();
    await expect(composer).toBeFocused();
  });

  await journey.stage("Read every session's details on a phone", async () => {
    await page.getByRole("button", { name: "Terminal view" }).click();
    await page.setViewportSize({ width: 390, height: 844 });
    const picker = page.locator("#session-picker");
    await page.locator("#session-picker-toggle").click();
    await expect(picker).toBeVisible();
    await page.mouse.move(0, 0);
    const rows = picker.locator(".session-picker-entry");
    await expect(rows).toHaveCount(2);
    // Every row, the open one included, shows its project, role and status in
    // full: nothing is cut off by an ellipsis or the row edge.
    await expect(picker.locator(".picker-machine")).toHaveText(["Studio Mac · macOS", "Atlas · Linux"]);
    await expect(picker.locator('.session-picker-entry[aria-current="true"] .session-name')).toHaveText("gabber-studio");
    await expect(picker.locator('.session-picker-entry[aria-current="true"] .session-meta')).toHaveText("supervisor calm-otter-4 · 1 worker · live");
    await expect(picker.locator('.session-picker-entry:not([aria-current="true"]) .session-name')).toHaveText("cas-src");
    await expect(picker.locator('.session-picker-entry:not([aria-current="true"]) .session-meta')).toHaveText("supervisor patient-pelican-9 · 1 worker · live");
    const clipped = await rows.evaluateAll((entries) => entries.flatMap((entry) => {
      const box = entry.getBoundingClientRect();
      return [...entry.querySelectorAll<HTMLElement>(".session-name, .session-meta, .session-summary-title, .session-picker-current")]
        .filter((text) => {
          const r = text.getBoundingClientRect();
          return text.scrollWidth > text.clientWidth + 1 || r.right > box.right + 0.5 || r.left < box.left - 0.5;
        })
        .map((text) => text.textContent);
    }));
    expect(clipped).toEqual([]);
    // Left open: this stage's screenshot (J09.png) is the phone receipt.
  });

  await journey.stage("Pair a third machine; the others keep their colours", async () => {
    // cas-50a7: each machine's accent is stored when it first pairs, so a new
    // pairing never re-colours the fleet, and a third machine gets its own.
    // Back from the phone stage: desktop viewport, picker closed, out of
    // Terminal view to the conversation list.
    await page.setViewportSize({ width: 1280, height: 720 });
    await page.keyboard.press("Escape");
    await expect(page.locator("#session-picker")).toBeHidden();
    await page.locator("#conversation-return").click();
    await expect(composer).toBeVisible();
    const avatar = (project: RegExp) => list.getByRole("button", { name: project }).locator(".conversation-avatar");
    const colour = (project: RegExp) => avatar(project).evaluate((element) => getComputedStyle(element).backgroundColor);
    const atlas = await colour(/cas-src/);
    const studio = await colour(/gabber-studio/);
    await page.goto("about:blank");
    await page.goto(`./#pair=A1pha0xZt1nA4wLr9cYp2KdJ6sHf0uEiMgTxBvNyRaQ&hub=alpha&hub_url=${encodeURIComponent("https://alpha.test")}&machine=${encodeURIComponent("Alpha · Linux")}&scopes=machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt`);
    const dialog = page.locator("#pair-dialog");
    await dialog.getByRole("textbox", { name: /Your name/ }).fill("Daniel");
    await dialog.getByRole("button", { name: "Pair", exact: true }).click();
    await expect(dialog).toBeHidden();
    const back = page.getByRole("button", { name: "‹ Conversations", exact: true });
    if (await back.isVisible().catch(() => false)) await back.click();
    await expect(list.getByRole("button", { name: /orion/ })).toBeVisible({ timeout: 15_000 });
    expect(await colour(/cas-src/), "Atlas keeps its accent").toBe(atlas);
    expect(await colour(/gabber-studio/), "Studio keeps its accent").toBe(studio);
    expect(await colour(/orion/), "the new machine gets its own accent").not.toBe(atlas);
    expect(await colour(/orion/)).not.toBe(studio);
    // And after a reload, from storage.
    await page.reload();
    await expect(list.getByRole("button", { name: /orion/ })).toBeVisible();
    expect([await colour(/cas-src/), await colour(/gabber-studio/)]).toEqual([atlas, studio]);
  });

  await journey.stage("Know each session and machine by name in Terminal view", async () => {
    // 3.30.0 journey F2/F3: Terminal view leads with the project, as the list
    // and the conversation header do, and the machine rail's letters come from
    // the machine's own name ("Atlas · Linux" read "A·").
    await list.getByRole("button", { name: /cas-src/ }).click();
    await page.getByRole("button", { name: "Terminal view" }).click();
    await expect(page.locator(".session-picker-name")).toHaveText("cas-src");
    await expect(page.locator(".session-picker-codename")).toHaveText(PELICAN);
    const initials = page.locator("#machine-rail-list .machine-initials");
    await expect(initials).toHaveCount(3);
    expect((await initials.allTextContents()).sort()).toEqual(["AL", "AT", "SM"]);
    expect(await page.locator(".machine-chip").getAttribute("data-compact-label")).toBe("AT");
    // The picker: every row leads with its project, the codename beneath it.
    const picker = page.locator("#session-picker");
    await page.locator("#session-picker-toggle").click();
    await expect(picker).toBeVisible();
    const names = picker.locator(".session-picker-entry .session-name");
    expect((await names.allTextContents()).sort()).toEqual(["cas-src", "gabber-studio", "lighthouse", "orion"]);
    await expect(picker.locator(`.session-picker-entry[data-picker-session="${OTTER}"] .session-meta`)).toHaveText(`supervisor ${OTTER} · 1 worker · live`);
    await expect(picker.locator('.session-picker-entry[data-picker-session="keen-lynx-1"] .session-meta')).toHaveText("supervisor keen-lynx-1 · 1 worker · live");
    await page.keyboard.press("Escape");
    await expect(picker).toBeHidden();
    // The palette: "Jump to <project>", the codename first on the line beneath
    // (a session summary, when one has arrived, follows the machine).
    await page.keyboard.press("ControlOrMeta+k");
    const palette = page.locator("#command-palette");
    await expect(palette).toBeVisible();
    const jump = (project: string) => palette.locator(".palette-command[data-palette-session]").filter({ hasText: `Jump to ${project}` });
    await expect(jump("gabber-studio").locator("span")).toHaveText("Jump to gabber-studio");
    await expect(jump("gabber-studio").locator("small")).toHaveText(new RegExp(`^${OTTER} · Studio Mac · macOS`));
    await expect(jump("cas-src").locator("small")).toHaveText(new RegExp(`^${PELICAN} · Atlas · Linux`));
    await expect(jump("orion").locator("small")).toHaveText(/^keen-lynx-1 · Alpha · Linux/);
    await page.keyboard.press("Escape");
    await expect(palette).toBeHidden();
  });

  await journey.stage("A supervisor with no workers yet is listed everywhere", async () => {
    // cas-645e: the list, the palette, the picker and its count agree. A live
    // supervisor that has not spawned workers is one the operator can talk to.
    await page.locator("#conversation-return").click();
    await expect(list.getByRole("button", { name: /lighthouse/ })).toBeVisible();
    const listRows = await list.locator(".conversation-row").count();
    // Ctrl+K lands in the list search here; the palette sits behind the list's button.
    await page.getByRole("button", { name: "Appearance & commands" }).click();
    const palette = page.locator("#command-palette");
    await expect(palette).toBeVisible();
    const jumps = palette.locator(".palette-command[data-palette-session]");
    await expect(jumps.filter({ hasText: "Jump to lighthouse" })).toHaveCount(1);
    const jumpCount = await jumps.count();
    await page.keyboard.press("Escape");
    await expect(palette).toBeHidden();
    await page.getByRole("button", { name: "Terminal view" }).click();
    const toggle = page.locator("#session-picker-toggle");
    await toggle.click();
    const picker = page.locator("#session-picker");
    await expect(picker).toBeVisible();
    await expect(picker.locator('.session-picker-entry[data-picker-session="lone-heron-2"] .session-meta')).toHaveText("supervisor lone-heron-2 · no workers · live");
    const pickerRows = await picker.locator(".session-picker-entry").count();
    expect(pickerRows).toBe(4);
    expect(jumpCount, "palette Jump rows").toBe(pickerRows);
    await expect(toggle).toHaveAttribute("aria-label", `Switch session — ${pickerRows} available`);
    expect(listRows, "conversation list rows").toBe(pickerRows);
    const pickerSessions = (await picker.locator(".session-picker-entry").evaluateAll((rows) => rows.map((row) => (row as HTMLElement).dataset.pickerSession))).sort();
    await page.keyboard.press("Escape");
    await expect(picker).toBeHidden();
    // The fleet board, on the same screen, lists the same sessions and
    // counts them the same way (cas-645e QA F01).
    await page.locator("#machine-rail-list .machine-icon").filter({ hasText: "AL" }).click();
    const board = page.locator("#fleet-board");
    await expect(board.locator("button.fleet-session")).toHaveCount(pickerRows);
    expect((await board.locator("button.fleet-session").evaluateAll((cards) => cards.map((card) => (card as HTMLElement).dataset.fleetSession))).sort(), "fleet board sessions").toEqual(pickerSessions);
    await expect(board.locator(".fleet-board-summary")).toHaveText(new RegExp(`^3 machines · ${pickerRows} sessions`));
    // The summary and the refresh time start with their own words, not a
    // separator drawn before them (cas-e503): "2 machines · 2 sessions",
    // "16:56". The " · " stays only between the summary's items.
    for (const selector of [".fleet-board-summary", ".fleet-catalog-time"]) {
      expect(await board.locator(selector).evaluate((element) => getComputedStyle(element, "::before").content), selector).toBe("none");
    }
  });
});
