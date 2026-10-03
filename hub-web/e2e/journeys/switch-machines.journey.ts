import type { Page } from "@playwright/test";
import { journeyStamp } from "./clock";
import { test, expect, expectWholeFocusRing } from "./journey";
import { ATLAS, STUDIO, PELICAN, OTTER } from "./world";
import type { Machine } from "./hub-double";

/**
 * cas-9772: wait for `ready` bounded by protocol events, not wall time. It
 * fails once `events()` has advanced more than `allowance` past where it stood
 * (catalog fetches, heartbeats) with `ready` still false — however slow the
 * host. On a loaded CI runner the fixed 12–30 s budgets here ran out while the
 * page was still on its way; a count of the page's own requests does not.
 * The test timeout remains only as a hang guard.
 */
async function within(page: Page, what: string, events: () => number, allowance: number, ready: () => Promise<boolean>): Promise<void> {
  const start = events();
  for (;;) {
    if (await ready()) return;
    const seen = events() - start;
    if (seen > allowance) throw new Error(`${what}: still not so after ${seen} protocol events (allowance ${allowance})`);
    await page.waitForTimeout(100);
  }
}

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
  // Fourteen stages including a pairing, palette/picker sweeps, a phone
  // viewport and a failing-heartbeat window (cas-bf07) that runs on the app's
  // real 5 s heartbeat: about two minutes idle. cas-9772: this is a hang
  // guard, not a budget for the work — every wait below is bounded by the
  // page's own requests, so a loaded CI host (2.7 m) is no longer a failure.
  test.setTimeout(360_000);
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
    await page.getByRole("button", { name: "Send to the gabber-studio supervisor", exact: true }).click();
    expect(await sent).toMatchObject({ machine: "studio", target: OTTER, text: "Is the Mac build green?" });
  });

  await journey.stage("Come back to the draft", async () => {
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(page.locator(".conversation-host")).toContainText("Atlas · Linux");
    await expect(composer).toHaveValue("Draft: ask about the flaky pairing test");
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
  });

  await journey.stage("Reopen the session picker after closing it", async () => {
    const picker = page.locator("#session-picker");
    const toggle = page.locator("#session-picker-toggle");
    const palette = page.locator("#command-palette");
    // The list search and the Terminal view button name the palette chord the
    // same way on this browser, which declares Linux (cas-2a33): Ctrl K, not
    // ⌘K (journey F16). The macOS part of HUB-J3 checks ⌘K.
    await expect(page.getByRole("searchbox", { name: "Search conversations" })).toHaveAttribute("placeholder", "Search conversations (Ctrl K)");
    await page.locator("#conversation-terminal").click();
    await expect(toggle).toBeVisible();
    // Its accessible name carries the visible chord, so "click Ctrl K" works
    // for a voice user (label in name, cas-3400 QA F02).
    const paletteButton = page.getByRole("button", { name: "Open command palette (Ctrl K)", exact: true });
    await expect(paletteButton).toHaveText("Ctrl K");
    await expect(paletteButton).toHaveAttribute("aria-keyshortcuts", "Control+K Meta+K");
    // The tab names the open conversation too.
    await expect(page).toHaveTitle("cas-src patient-pelican-9 — Cassy Cloud");
    // Closing the picker does not rebuild the shell, and the next periodic
    // render papers over a stale "open" state within a few seconds. So each
    // check reads the state once, as the click or key leaves it: the picker
    // must open, and say so, at once — not when a later render happens to
    // rebuild the shell. cas-9772: a one-shot read, not a 1 s retry window
    // that a loaded host could outrun.
    const now = async (locator: typeof picker, visible: boolean, expanded: "true" | "false") => {
      expect(await locator.isVisible(), "the picker's visibility, read at once").toBe(visible);
      expect(await toggle.getAttribute("aria-expanded"), "aria-expanded at once").toBe(expanded);
    };
    const open = async () => {
      await toggle.click();
      await now(picker, true, "true");
    };
    const closed = async () => {
      await now(picker, false, "false");
    };
    await open();
    await page.keyboard.press("Escape");
    await closed();
    // The first open rebuilt the shell; Escape still lands on the session
    // title, and Enter there reopens the picker (cas-7eaf).
    await expect(toggle).toBeFocused();
    // Focus there is visible: the whole ring shows, not clipped away by the
    // title's ellipsis clip (cas-cf10 QA F01).
    expect(await toggle.evaluate((element) => element.matches(":focus-visible"))).toBe(true);
    await expectWholeFocusRing(toggle, { vertical: true });
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
      // An empty result says so, not an empty dialog (journey F18).
      await expect(picker.getByRole("status")).toHaveText("No sessions match “zz”.");
      // A long unbroken query wraps inside the dialog (cas-a8db QA F01).
      const long = "z".repeat(120);
      await filter.fill(long);
      const status = picker.getByRole("status");
      await expect(status).toHaveText(`No sessions match “${long}”.`);
      const fit = await status.evaluate((line) => {
        const dialog = line.closest("dialog")!.getBoundingClientRect();
        const box = line.getBoundingClientRect();
        return { overflow: line.scrollWidth > line.clientWidth + 1, inside: box.left >= dialog.left && box.right <= dialog.right + 0.5 };
      });
      expect(fit, "the no-match line wraps inside the picker").toEqual({ overflow: false, inside: true });
      await filter.fill("zz");
      await close();
      await closed();
      await open();
      await expect(filter).toHaveValue("");
      await expect(picker.locator(".session-picker-entry:visible")).toHaveCount(everySession);
      await expect(picker.locator("#session-picker-no-match")).toBeHidden();
    }
    // A closed picker must not pop back open over the next dialog either.
    await page.keyboard.press("Escape");
    await closed();
    await page.keyboard.press("ControlOrMeta+k");
    expect(await palette.isVisible(), "the palette opens at once").toBe(true);
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
    // cas-9772: "several renders" is a full heartbeat round on both paired
    // machines — each answers another catalog fetch, and each fetch renders —
    // rather than six seconds of wall time.
    const round = { atlas: hub.catalogFetchCount("atlas"), studio: hub.catalogFetchCount("studio") };
    await hub.waitFor(() => hub.catalogFetchCount("atlas") >= round.atlas + 2 && hub.catalogFetchCount("studio") >= round.studio + 2);
    await expect(entry(firstSession!)).toBeFocused();
    // Tab to the next session: the same holds there.
    await page.keyboard.press("Tab");
    const second = picker.locator(".session-picker-entry").nth(1);
    await expect(second).toBeFocused();
    // A summary that changes the rows themselves rebuilds the list; focus
    // follows the same session onto its rebuilt row.
    const summary = (title: string, phase: string) => hub.send(OTTER, { SessionSummary: { summary: { title, description: title, phase, generated_at: journeyStamp() } } });
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
    await page.locator("#conversation-terminal").click();
    await expect(back).toBeVisible();
    await back.click();
    await expect(composer).toBeFocused();
  });

  await journey.stage("Keyboard focus lands somewhere real on every route", async () => {
    // cas-7eaf: none of these routes leaves focus on <body>.
    const offBody = () => page.evaluate(() => document.activeElement !== document.body && document.activeElement !== null);
    const terminal = page.locator("#conversation-terminal");
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
    // cas-9772: the landing waits for the terminal; let it attach and a full
    // render round pass, counted in the page's requests, not 2.5 s.
    await hub.waitFor(() => hub.attaches.includes(OTTER));
    const settled = hub.catalogFetchCount("studio");
    await hub.waitFor(() => hub.catalogFetchCount("studio") >= settled + 1);
    await expect(interrupt).toBeFocused();
    // Back in the conversation list: Enter on a row, and a mouse click on a
    // row, land in its reply box.
    await back.click();
    await expect(composer).toBeFocused();
    // The list stays beside the thread on a desktop (cas-479a).
    await list.getByRole("button", { name: /cas-src/ }).focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
    await expect(composer).toBeFocused();
    await list.getByRole("button", { name: /gabber-studio/ }).click();
    await expect(page.getByRole("button", { name: "Send to the gabber-studio supervisor", exact: true })).toBeVisible();
    await expect(composer).toBeFocused();
  });

  await journey.stage("Read every session's details on a phone", async () => {
    await page.locator("#conversation-terminal").click();
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
    await within(page, "the paired machine's sessions are listed", () => hub.catalogFetchCount("alpha"), 3, () => list.getByRole("button", { name: /orion/ }).isVisible());
    expect(await colour(/cas-src/), "Atlas keeps its accent").toBe(atlas);
    expect(await colour(/gabber-studio/), "Studio keeps its accent").toBe(studio);
    expect(await colour(/orion/), "the new machine gets its own accent").not.toBe(atlas);
    expect(await colour(/orion/)).not.toBe(studio);
    // And after a reload, from storage.
    await page.reload();
    await expect(list.getByRole("button", { name: /orion/ })).toBeVisible();
    expect([await colour(/cas-src/), await colour(/gabber-studio/)]).toEqual([atlas, studio]);
    // cas-7752: the draft started on the Linux machine survived the pair link
    // opened in this tab and the reload: it is there when I return to it...
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(composer).toHaveValue("Draft: ask about the flaky pairing test");
    // ...and once it is sent, it does not come back after another reload.
    const sent = hub.nextSend();
    await page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true }).click();
    expect(await sent).toMatchObject({ machine: "atlas", target: PELICAN, text: "Draft: ask about the flaky pairing test" });
    await expect(composer).toHaveValue("");
    await page.reload();
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
    await expect(composer).toHaveValue("");
  });

  await journey.stage("Know each session and machine by name in Terminal view", async () => {
    // 3.30.0 journey F2/F3: Terminal view leads with the project, as the list
    // and the conversation header do, and the machine rail's letters come from
    // the machine's own name ("Atlas · Linux" read "A·").
    await list.getByRole("button", { name: /cas-src/ }).click();
    await page.locator("#conversation-terminal").click();
    await expect(page.locator(".session-picker-name")).toHaveText("cas-src");
    // Just after the reload there is no latency sample yet: the header says it
    // is checking, never "Status unavailable" beside a green dot, and then
    // shows the first sample (journey F17).
    const latency = page.locator(".connection-summary [data-machine-latency]");
    await expect(latency).toHaveText(/^(Checking…|\d+ms)$/);
    // Read together, so a sample landing in between cannot split them: while it
    // says Checking… the dot is neutral, not green (cas-bf07 QA F02).
    const early = await page.locator(".connection-summary").evaluate((summary) => ({ text: summary.querySelector("[data-machine-latency]")?.textContent, live: summary.classList.contains("live") }));
    if (early.text === "Checking…") expect(early.live, "no green dot beside Checking…").toBe(false);
    const sample = async () => /^\d+ms$/.test((await latency.textContent()) ?? "");
    const probes = () => hub.machineProbes.get("atlas") ?? 0;
    await within(page, "the first latency sample shows", probes, 3, sample);
    // Heartbeats that keep failing: the header names the degradation with the
    // amber dot, as the rail does for the same machine, rather than promising a
    // check beside a green dot (cas-bf07 QA F01). Then it recovers.
    const chip = page.locator(".connection-summary");
    const railDot = page.locator("#machine-rail-list .machine-icon").filter({ hasText: "AT" }).locator(".machine-state");
    const heartbeat = "https://atlas.test/v1/machine";
    // cas-9772: the outage is measured in refused heartbeats, not seconds.
    let refused = 0;
    await page.route(heartbeat, (route) => { refused += 1; return route.abort(); });
    const shows = (text: string) => async () => (await latency.textContent()) === text;
    // Unsteady after two missed heartbeats (DEGRADED_AFTER_MISSED_HEARTBEATS; cas-a6f0 renamed the word).
    await within(page, "Unsteady after missed heartbeats", () => refused, 3, shows("Unsteady"));
    await expect(chip).toHaveClass(/\bdegraded\b/);
    await expect(railDot).toHaveClass(/\bdegraded\b/);
    // cas-71af (bf07 QA F01): the chip's tooltip reads the same machine state
    // as the chip, not the terminal attach's "live".
    await expect(chip).toHaveAttribute("title", /^unsteady · \d+ missed$/);
    // cas-a6f0 (journey F9): the rail beside it does not say All clear.
    await expect(page.locator("#attention-panel .attention-empty")).toHaveText(/^Not all clear\. Atlas · Linux: connection unsteady — checking\./);
    // bf07 QA F02: a longer outage does not leave the chip Unsteady. After
    // four missed heartbeats the machine reconnects, and it comes back.
    // Reconnecting after four (RECONNECT_AFTER_MISSED_HEARTBEATS).
    await within(page, "no longer Unsteady once the machine reconnects", () => refused, 6, async () => !(await shows("Unsteady")()));
    await page.unroute(heartbeat);
    await within(page, "a latency sample again after the outage", probes, 4, sample);
    await expect(chip).not.toHaveClass(/\bdegraded\b/);
    await expect(chip).toHaveAttribute("title", /^live · \d+ms$/);
    await expect(page.locator(".session-picker-codename")).toHaveText(PELICAN);
    // The title is announced as the open conversation and its machine, the
    // switch after it (journey F19): not "Switch session — 4 available".
    await expect(page.getByRole("heading", { level: 1, name: `cas-src ${PELICAN} on Atlas · Linux — switch session (4 available)`, exact: true })).toBeVisible();
    // The header's actions stay whole at every width, the title and chips
    // yielding instead, even beside a long machine name (cas-3400 QA F01: at
    // 900–1024px they were clipped out of the header).
    const header = page.locator(".session-header");
    for (const width of [390, 600, 849, 900, 1024, 1280]) {
      await page.setViewportSize({ width, height: 720 });
      for (const label of ["Atlas · Linux", "Build Server With A Very Long Hostname · Linux"]) {
        const fit = await header.evaluate((element, text) => {
          const chip = element.querySelector<HTMLElement>(".machine-chip");
          if (chip) chip.textContent = text;
          const box = element.getBoundingClientRect();
          const right = box.right - parseFloat(getComputedStyle(element).paddingRight) + 0.5;
          const clipped = [...element.querySelectorAll<HTMLElement>(".actions button")]
            .filter((button) => button.getBoundingClientRect().width > 0 && button.getBoundingClientRect().right > right)
            .map((button) => button.textContent);
          return { overflow: element.scrollWidth > element.clientWidth + 1, clipped };
        }, label);
        expect(fit, `header at ${width}px with "${label}"`).toEqual({ overflow: false, clipped: [] });
      }
    }
    // The same with the machine drawer open, which leaves the header 185–620px
    // of an 849–1440px window: the header sizes to its own column. Every
    // control stays on screen and clickable, inside the main column and not
    // under the context panel (cas-ac390); in the narrowest columns ⌘K, the
    // control and Interrupt become icons under their full accessible names,
    // Back keeps its ‹, and the title is what gives way (cas-3400 QA rounds
    // 2 and 3).
    await page.setViewportSize({ width: 1280, height: 720 });
    await page.locator("#machine-drawer-toggle").click();
    await expect(page.locator(".machine-navigation.drawer-open")).toHaveCount(1);
    for (const width of [849, 900, 990, 1024, 1280, 1440]) {
      await page.setViewportSize({ width, height: 720 });
      await expect(page.getByRole("button", { name: /^Open command palette \((Ctrl K|⌘K)\)$/ })).toBeVisible();
      await expect(page.getByRole("button", { name: /^(Release control|Take control|Force takeover)$/ })).toBeVisible();
      await expect(page.getByRole("button", { name: "Interrupt selected pane", exact: true })).toBeVisible();
      const fit = await header.evaluate((element) => {
        const box = element.getBoundingClientRect();
        const style = getComputedStyle(element);
        const left = box.left + parseFloat(style.paddingLeft) - 0.5;
        const right = box.right - parseFloat(style.paddingRight) + 0.5;
        const controls = [...element.querySelectorAll<HTMLElement>("#session-back, .actions button")];
        const unreachable = controls
          .filter((button) => {
            const b = button.getBoundingClientRect();
            if (b.width === 0) return true;
            const hit = document.elementFromPoint(b.left + b.width / 2, b.top + b.height / 2);
            return b.right > right || b.left < left || b.right > innerWidth || !hit || !button.contains(hit);
          })
          .map((button) => button.getAttribute("aria-label"));
        return { overflow: element.scrollWidth > element.clientWidth + 1, unreachable, controls: controls.length };
      });
      expect(fit.unreachable, `every header control is on screen and clickable with the drawer open at ${width}px`).toEqual([]);
      expect(fit.overflow, `header overflow at ${width}px`).toBe(false);
      expect(fit.controls, "palette, control and Interrupt at least").toBeGreaterThanOrEqual(3);
      if (await page.locator("#session-back").count()) await expect(page.locator("#session-back")).toBeVisible();
    }
    await page.setViewportSize({ width: 1280, height: 720 });
    await page.locator("#machine-drawer-close").click();
    await expect(page.locator(".machine-navigation.drawer-open")).toHaveCount(0);
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
    await page.locator("#conversation-terminal").click();
    const toggle = page.locator("#session-picker-toggle");
    await toggle.click();
    const picker = page.locator("#session-picker");
    await expect(picker).toBeVisible();
    await expect(picker.locator('.session-picker-entry[data-picker-session="lone-heron-2"] .session-meta')).toHaveText("supervisor lone-heron-2 · no workers · live");
    const pickerRows = await picker.locator(".session-picker-entry").count();
    expect(pickerRows).toBe(4);
    expect(jumpCount, "palette Jump rows").toBe(pickerRows);
    await expect(toggle).toHaveAttribute("aria-label", `cas-src ${PELICAN} on Atlas · Linux — switch session (${pickerRows} available)`);
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
    // Nothing open: the tab is the app's name alone.
    await expect(page).toHaveTitle("Cassy Cloud");
    // The summary and the refresh time start with their own words, not a
    // separator drawn before them (cas-e503): "2 machines · 2 sessions",
    // "16:56". The " · " stays only between the summary's items.
    for (const selector of [".fleet-board-summary", ".fleet-provenance"]) {
      expect(await board.locator(selector).evaluate((element) => getComputedStyle(element, "::before").content), selector).toBe("none");
    }
    // Journey F2: the Fleet overview reads as a product page, not a debug one.
    // Column headers are words at desktop width, not a numbered key.
    const plot = board.locator("table.fleet-plot");
    for (const label of ["Needs you", "Working", "Idle", "Stale", "Unreachable"]) {
      await expect(plot.locator("thead .fleet-track-label").filter({ hasText: new RegExp(`^${label}$`) }), label).toBeVisible();
    }
    await expect(plot.locator("thead .fleet-track-key")).toHaveCount(5);
    for (const key of await plot.locator("thead .fleet-track-key").all()) await expect(key).toBeHidden();
    // No session is working, so the Working column is not shaded.
    expect(await plot.locator("thead th").nth(2).evaluate((element) => getComputedStyle(element).backgroundColor), "Working header unshaded").toBe("rgba(0, 0, 0, 0)");
    await expect(board.locator(".fleet-figure-caption")).not.toContainText("Shaded");
    // One short line says when it was refreshed; the details are its hover title.
    await expect(board.locator(".fleet-provenance")).toHaveText(/^Last updated \d{2}:\d{2}$/);
    await expect(board.locator(".fleet-provenance")).toHaveAttribute("title", /Alpha · Linux · Live · Hub /);
    // Remove names the machine; the back control says Back on screen.
    await expect(page.locator("#remove-machine")).toHaveText("Remove Alpha · Linux from this browser");
    await expect(page.locator("#session-back")).toBeVisible();
    await expect(page.locator("#session-back .session-back-label")).toHaveText("Back");
    await expect(page.locator("#session-back")).toHaveAccessibleName(/^Back to /);
    // cas-ac390 QA F01: in a narrow column (900px with the drawer open) Back
    // yields to its glyph instead of running under ⌘K; its name is unchanged.
    const viewport = page.viewportSize()!;
    await page.setViewportSize({ width: 900, height: 720 });
    await expect(page.locator(".shell.drawer-open")).toHaveCount(1);
    await expect(page.locator("#session-back .session-back-label")).toBeHidden();
    await expect(page.locator("#session-back")).toHaveAccessibleName(/^Back to /);
    // Back and ⌘K both stay (cas-3400 QA round 3): neither is hidden to make room.
    await expect(page.locator("#session-back")).toBeVisible();
    await expect(page.locator("#command-palette-toggle")).toBeVisible();
    const [back, paletteKey] = await Promise.all([page.locator("#session-back").boundingBox(), page.locator("#command-palette-toggle").boundingBox()]);
    expect(back!.x + back!.width <= paletteKey!.x || paletteKey!.x + paletteKey!.width <= back!.x, "Back and ⌘K do not overlap at 900 with the drawer open").toBe(true);
    await page.setViewportSize(viewport);
    await expect(page.locator("#session-back .session-back-label")).toBeVisible();
    // Journey F1: every Fleet overview row leads with its project; the
    // codename is named once, in the line beneath.
    const row = (session: string) => board.locator(`button.fleet-session[data-fleet-session="${session}"]`);
    await expect(row("keen-lynx-1").locator(".session-name")).toHaveText("orion");
    await expect(row("keen-lynx-1").locator(".session-meta")).toHaveText("supervisor keen-lynx-1 · 1 worker · live");
    await expect(row(PELICAN).locator(".session-name")).toHaveText("cas-src");
    await expect(board.locator('.fleet-plot-row[data-fleet-session="keen-lynx-1"] .fleet-plot-name')).toHaveText("orion");
    for (const session of ["keen-lynx-1", "lone-heron-2", PELICAN, OTTER]) {
      expect((await row(session).innerText()).split(session).length - 1, `${session} named once in its Fleet overview row`).toBe(1);
    }
    // The machine drawer, opened by the rail click above, reads the same way.
    const drawer = page.locator("#machine-tree");
    await expect(drawer.locator(".nav-item .session-name").first()).toBeVisible();
    const drawerRows = await drawer.locator(".nav-item").evaluateAll((items) => items.map((item) => [item.querySelector(".session-name")?.textContent ?? "", item.querySelector(".session-meta, .session-summary-title")?.textContent ?? ""]));
    for (const [headline, meta] of drawerRows) {
      expect(["cas-src", "gabber-studio", "orion", "lighthouse"], `drawer row headline ${headline}`).toContain(headline);
      expect(meta, `drawer row ${headline} names its codename once, beneath`).not.toContain(headline);
    }
    // With nothing open, the title says so: the fleet, then the switch.
    await expect(page.getByRole("heading", { level: 1, name: `Fleet overview — switch session (${pickerRows} available)`, exact: true })).toBeVisible();
    // cas-598e QA F01: when several sessions share a project, each plot row
    // also carries the shortest tail of its codename that tells it apart, so
    // three cas-src rows never read "cas-src / cas-src / cas-src". Alpha
    // starts two more cas-src supervisors for a moment (the catalog refreshes
    // every 5 s), then stops them so the stages after this one see the fleet
    // they expect.
    const alpha = hub.machine("alpha").sessions;
    const extra = [
      { name: "brisk-otter-5", supervisor: "brisk-otter-5", project_dir: "/projects/cas-src", workers: ["w5"], liveness: "live" },
      { name: "quiet-heron-8", supervisor: "quiet-heron-8", project_dir: "/projects/cas-src", workers: [], liveness: "live" },
    ] as Machine["sessions"];
    alpha.push(...extra);
    // cas-ae5e: the same codename on a second machine (brisk-otter-5 on Atlas
    // too) is told apart by the machine's rail initials, which fit the narrow
    // label column where the full machine name did not.
    const atlasSessions = hub.machine("atlas").sessions;
    const twin = { ...extra[0]!, workers: [] } as Machine["sessions"][number];
    atlasSessions.push(twin);
    const plotName = (session: string) => board.locator(`.fleet-plot-row[data-fleet-session="${session}"] .fleet-plot-name`);
    // cas-9772: announce the change as the hub does and bound the wait by the
    // page's catalog fetches; the 5 s heartbeat (whose fetch aborts at 3 s)
    // timed out here at 15 s on a loaded host.
    await hub.announceCatalog("alpha", { added: extra.map((session) => session.name) });
    await hub.announceCatalog("atlas", { added: [twin.name] });
    const fetches = () => hub.catalogFetchCount("alpha") + hub.catalogFetchCount("atlas");
    await within(page, "the new plot rows are drawn", fetches, 4, async () => await plotName("quiet-heron-8").isVisible() && await plotName("brisk-otter-5").count() === 2);
    const plotLabels = async () => board.locator(".fleet-plot-row").evaluateAll((rows) => rows.map((row) => {
      const name = row.querySelector<HTMLElement>(".fleet-plot-name")!;
      const tag = name.querySelector<HTMLElement>(".fleet-plot-tag");
      const project = name.querySelector<HTMLElement>(".fleet-plot-project");
      return { session: (row as HTMLElement).dataset.fleetSession!, project: project?.textContent ?? name.textContent ?? "", projectWidth: project ? project.getBoundingClientRect().width : name.getBoundingClientRect().width, tag: tag?.textContent ?? "", tagWhole: !tag || tag.scrollWidth <= tag.clientWidth + 1 && tag.getBoundingClientRect().right <= name.getBoundingClientRect().right + 1 };
    }));
    for (const width of [viewport.width, 390]) {
      await page.setViewportSize({ width, height: viewport.height });
      const labels = await plotLabels();
      const casSrc = labels.filter((label) => label.project === "cas-src");
      expect(casSrc.map((label) => label.tag).sort(), `cas-src plot rows at ${width}px`).toEqual(["heron-8", "otter-5 · AL", "otter-5 · AT", "pelican-9"]);
      // The project keeps at least a letter beside the tag; it never collapses to a bare "·".
      expect(casSrc.every((label) => label.projectWidth >= 8), `the project stays visible beside the tag at ${width}px: ${JSON.stringify(casSrc)}`).toBe(true);
      expect(new Set(labels.map((label) => `${label.project} ${label.tag}`)).size, `every plot row reads differently at ${width}px`).toBe(labels.length);
      expect(casSrc.every((label) => label.tagWhole), `the tag is never cut at ${width}px`).toBe(true);
      // A project that appears once carries no tag.
      expect(labels.find((label) => label.session === "keen-lynx-1")?.tag).toBe("");
    }
    await page.setViewportSize(viewport);
    alpha.splice(alpha.length - extra.length, extra.length);
    atlasSessions.splice(atlasSessions.indexOf(twin), 1);
    await hub.announceCatalog("alpha", { removed: extra.map((session) => session.name) });
    await hub.announceCatalog("atlas", { removed: [twin.name] });
    await within(page, "the stopped sessions' plot rows clear", fetches, 4, async () => await plotName("quiet-heron-8").count() === 0 && await plotName("brisk-otter-5").count() === 0);
    await expect(board.locator(".fleet-plot-tag")).toHaveCount(0);
  });

  await journey.stage("Hear the open conversation as the Terminal view title", async () => {
    // Journey F19: the goal state (final.aria.yml) names the open conversation
    // and its machine in the page heading, the switch after it.
    await page.locator('#fleet-board button.fleet-session[data-fleet-session="patient-pelican-9"]').click();
    await expect(page.locator(".session-picker-name")).toHaveText("cas-src");
    await expect(page.locator("body")).toMatchAriaSnapshot(`- heading "cas-src ${PELICAN} on Atlas · Linux — switch session (4 available)" [level=1]`);
    expect(await page.locator("body").ariaSnapshot()).not.toContain('heading "Switch session');
  });

  await journey.stage("Tell one codename apart on two machines whose initials match", async () => {
    // cas-ae5e QA F01: Atlas and Attic share the rail initials AT. The paired
    // Alpha is renamed Attic in this browser, and brisk-otter-5 and
    // patient-pelican-9 run on both machines. The plot marks each machine by
    // the shortest part of its name that differs ("Atl" / "Att"), never the
    // full label. A twin tag is capped to fit the 132px column at 390 (QA
    // round 2), so a long tail is trimmed from the left: "…ter-5 · Atl",
    // "…can-9 · Att". The tag stays whole and the project keeps a letter, at
    // 1280 and 390.
    await page.evaluate(async () => {
      const db: IDBDatabase = await new Promise((ok, fail) => { const req = indexedDB.open("cas-commander-v1"); req.onsuccess = () => ok(req.result); req.onerror = () => fail(req.error); });
      await new Promise<void>((ok, fail) => {
        const tx = db.transaction("machines", "readwrite");
        const store = tx.objectStore("machines");
        const get = store.get("alpha");
        // A machine paired in the page keeps its visible record as the install's candidate.
        get.onsuccess = () => {
          const record = get.result;
          const renamed = { ...record, label: "Attic · Linux" };
          if (record.pairingInstall?.candidate) renamed.pairingInstall = { ...record.pairingInstall, candidate: { ...record.pairingInstall.candidate, label: "Attic · Linux" } };
          store.put(renamed);
        };
        tx.oncomplete = () => ok();
        tx.onerror = () => fail(tx.error);
      });
      db.close();
    });
    const twin = (): Machine["sessions"][number] => ({ name: "brisk-otter-5", supervisor: "brisk-otter-5", project_dir: "/projects/cas-src", workers: [], liveness: "live" });
    hub.machine("alpha").sessions.push(twin(), { name: PELICAN, supervisor: PELICAN, project_dir: "/projects/cas-src", workers: [], liveness: "live" });
    hub.machine("atlas").sessions.push(twin());
    await page.reload();
    const terminal = page.locator("#conversation-terminal");
    if (await terminal.isVisible()) await terminal.click();
    await page.locator("#machine-rail-list .machine-icon").filter({ hasText: "AT" }).first().click();
    const board = page.locator("#fleet-board");
    const rows = board.locator(`.fleet-plot-row:is([data-fleet-session="brisk-otter-5"], [data-fleet-session="${PELICAN}"])`);
    await within(page, "both machines' twins are plotted after the reload", () => hub.catalogFetchCount(), 8, async () => await rows.count() === 4);
    const viewport = page.viewportSize()!;
    for (const width of [viewport.width, 390]) {
      await page.setViewportSize({ width, height: viewport.height });
      const twins = await rows.evaluateAll((items) => items.map((row) => {
        const name = row.querySelector<HTMLElement>(".fleet-plot-name")!;
        const tag = name.querySelector<HTMLElement>(".fleet-plot-tag")!;
        const project = name.querySelector<HTMLElement>(".fleet-plot-project")!;
        return {
          project: project.textContent,
          projectWidth: project.getBoundingClientRect().width,
          tag: tag.textContent,
          tagWhole: tag.scrollWidth <= tag.clientWidth + 1 && tag.getBoundingClientRect().right <= name.getBoundingClientRect().right + 1,
        };
      }));
      expect(twins.map((row) => row.tag).sort(), `twin tags at ${width}px`).toEqual(["…can-9 · Atl", "…can-9 · Att", "…ter-5 · Atl", "…ter-5 · Att"]);
      expect(twins.every((row) => row.project === "cas-src" && row.projectWidth >= 8), `the project keeps a letter at ${width}px: ${JSON.stringify(twins)}`).toBe(true);
      expect(twins.every((row) => row.tagWhole), `the tag is never cut at ${width}px: ${JSON.stringify(twins)}`).toBe(true);
    }
    await page.setViewportSize(viewport);
  });
});
