import type { Locator, Page } from "@playwright/test";
import { test, expect, journeyPart, holdPageClock } from "./journey";
import type { HubDouble, Machine } from "./hub-double";
import { ATLAS, STUDIO, PELICAN, OTTER } from "./world";

/** A paired machine that is switched off: it never answers this visit (cas-b789). */
const SHED: Machine = { id: "shed", label: "Shed NAS · Linux", sessions: [] };
/** A machine paired from the phone mid-journey (cas-002e). */
const FORGE: Machine = { id: "forge", label: "Forge · Linux", sessions: [{ name: "steady-wren-3", supervisor: "steady-wren-3", project_dir: "/projects/forge-tools", workers: ["quick-finch-8"], liveness: "live" }] };
/** Ordinary hostname-style machine names, 19–23 characters (cas-e918 QA F01). */
const LONG_LABELS: Machine[] = [
  { id: "workstation", label: "pippenz-workstation · Linux", sessions: [{ name: "tiny-wren-3", supervisor: "tiny-wren-3", project_dir: "/projects/lab", workers: [], liveness: "live" }] },
  { id: "macbook", label: "Daniel's MacBook Pro · macOS", sessions: [{ name: "brisk-lark-5", supervisor: "brisk-lark-5", project_dir: "/projects/notes", workers: [], liveness: "live" }] },
  { id: "rack", label: "Build Server Rack Seven · Windows", sessions: [{ name: "patient-heron-12", supervisor: "patient-heron-12", project_dir: "/projects/infra", workers: [], liveness: "live" }] },
];

test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });

const conversationList = (page: Page) => page.getByRole("navigation", { name: "Choose a supervisor" });
/** A list row is named "<project> on <machine>, …"; the row's own controls are not. */
const conversationRow = (page: Page, project: string) => conversationList(page).getByRole("button", { name: new RegExp(`^${project} on `) });
const conversationRows = (page: Page) => conversationList(page).getByRole("button", { name: /^\S+ on [^,]+, / });
const sidebar = (page: Page) => page.getByRole("complementary", { name: "Supervisor conversations" });
const heading = (page: Page) => page.getByTestId("conversation-heading");
const thread = (page: Page) => page.getByTestId("conversation-thread");
const palette = (page: Page) => page.getByRole("dialog", { name: "Commands" });
const back = (page: Page) => page.getByRole("button", { name: "‹ Conversations", exact: true });

/**
 * Wait for `target` to show with no wall-clock budget of its own. It fails
 * only when the hub double has answered `allowance` more of the `events` it
 * is waiting on and the page still has not shown it, so a stall names the
 * protocol step that did not land and a slow host is not a failure
 * (cas-03b7, as HUB-J8's waits in cas-9772).
 */
async function shownWithin(hub: HubDouble, what: string, target: Locator, events: () => number, allowance: number): Promise<void> {
  const start = events();
  const overrun = hub.waitFor(() => events() - start > allowance).then(() => {
    throw new Error(`${what}: not shown after ${events() - start} protocol events (allowance ${allowance})`);
  });
  overrun.catch(() => undefined); // settled by the race below, or never
  await Promise.race([expect(target).toBeVisible({ timeout: 0 }), overrun]);
}

/**
 * cas-766c: the header's host line always shows some of the machine name;
 * its OS word is shown whole or not at all, never cut; and the machine is
 * only ever cut once the codename has stepped aside. The host line fits
 * itself to the header after layout, so each rule is polled until the line
 * settles rather than read once (cas-9fcc: the one-shot read was the flake).
 */
async function expectHeaderKeepsMachine(page: Page, machine: string, codename: string): Promise<void> {
  const where = heading(page).getByTitle(`${machine} · ${codename}`, { exact: true });
  await expect(where).toBeVisible();
  await expect.poll(() => where.evaluate((line) => {
    const host = line.querySelector<HTMLElement>(".host-machine")!;
    const os = host.querySelector<HTMLElement>(".host-os");
    const name = line.querySelector<HTMLElement>(".codename");
    const chars = host.getBoundingClientRect().width / (parseFloat(getComputedStyle(host).fontSize) * 0.6);
    // cas-d043 QA round 1: any overflow draws an ellipsis, so no tolerance.
    const machineCut = host.scrollWidth > host.clientWidth;
    // cas-8526: a part that steps aside is clipped to 1px, not removed.
    const osShown = os !== null && os.getBoundingClientRect().width > 1;
    const codenameShown = name !== null && name.getBoundingClientRect().width > 1;
    return {
      // The 4ch floor is for a cut machine; a whole name is on the line at any width (cas-bcf0).
      machineOnLine: machineCut ? chars >= 4 : chars > 1,
      osCutMidWord: osShown && machineCut,
      codenameBesideCutMachine: machineCut && codenameShown,
    };
  }), { message: `${machine} keeps its place ahead of ${codename}` }).toEqual({ machineOnLine: true, osCutMidWord: false, codenameBesideCutMachine: false });
  await expect(page.getByRole("status", { name: "Connection", exact: true, includeHidden: true })).toBeVisible();
  // cas-8526: whatever the line shows, it is heard whole.
  const heard = (await where.ariaSnapshot()).replace(/^\s*- text:\s*/gm, " ").replace(/\s+/g, " ");
  expect(heard, "the host line is heard whole").toContain(machine);
  expect(heard, "the host line is heard whole").toContain(codename);
}

/**
 * cas-1451: the empty-thread card under the title keeps the machine as the
 * header does: whole before any codename character, the OS word dropped
 * whole, and one separator between them, never a double gap.
 */
async function expectCardKeepsMachine(page: Page, machine: string, codename: string): Promise<void> {
  const where = thread(page).getByTitle(`${machine} · ${codename}`, { exact: true });
  await expect(where).toBeVisible();
  await expect.poll(() => where.evaluate((line) => {
    const host = line.querySelector<HTMLElement>(".proj2-machine")!;
    const os = host.querySelector<HTMLElement>(".host-os");
    const name = line.querySelector<HTMLElement>(".codename");
    const separator = line.querySelector<HTMLElement>(".proj2-sep");
    const visible = (node: HTMLElement | null) => node !== null && node.getClientRects().length > 0;
    const box = (node: HTMLElement) => node.getBoundingClientRect();
    const machineCut = host.scrollWidth > host.clientWidth + 1;
    const gaps = visible(name) && visible(separator) ? [box(separator!).left - box(host).right, box(name!).left - box(separator!).right] : [];
    return {
      codenameBesideCutMachine: machineCut && visible(name),
      osCutMidWord: visible(os) && machineCut,
      separatorGoesWithCodename: visible(separator) === visible(name),
      oneGap: gaps.every((gap) => Math.abs(gap) <= 1),
      separator: separator?.textContent ?? "",
    };
  }), { message: `the empty card keeps ${machine} ahead of ${codename}` }).toEqual({ codenameBesideCutMachine: false, osCutMidWord: false, separatorGoesWithCodename: true, oneGap: true, separator: " · " });
}

test("HUB-J9 on a phone: from the list to a reply and back", async ({ page, journey }) => {
  // Ten stages over nine machines, two palette sweeps, a long thread and a
  // relay pairing. Every wait is a DOM assertion or bounded by the hub
  // double's own events; this is a hang guard sized from the loaded runtime
  // (34 s idle, 39 s at merge-queue load 63, past 60 s at load 258; cas-03b7).
  test.setTimeout(180_000);
  const hub = await journey.hub({ machines: [ATLAS, STUDIO, SHED, FORGE, ...LONG_LABELS], paired: ["atlas", "studio", "shed", ...LONG_LABELS.map((machine) => machine.id)], relay: { machine: "forge", claimAfter: 2, authorizeAfter: 4 } });
  await page.route("https://shed.test/**", (route) => route.abort("connectionrefused"));
  await page.routeWebSocket(/shed\.test/, (ws) => { void ws.close({ code: 1006 }); });
  const list = conversationList(page);
  const composer = page.getByRole("textbox", { name: "Your message" });

  await journey.stage("Open the list on a phone", async () => {
    // The list must not claim "Not paired", "No live supervisors" or
    // "Reconnecting" before the rows arrive (journey F14): record every state
    // the empty-list notice takes during the cold load.
    await page.addInitScript(() => {
      const seen = new Set<string>();
      (window as unknown as { __coldLoadText: Set<string> }).__coldLoadText = seen;
      const record = () => {
        const empty = document.querySelector<HTMLElement>("#conversation-empty:not([hidden])");
        if (empty?.dataset.state) seen.add(empty.dataset.state);
      };
      new MutationObserver(record).observe(document, { subtree: true, childList: true, characterData: true, attributes: true });
    });
    await journey.open();
    await expect(conversationRows(page)).toHaveCount(5);
    const coldLoad = await page.evaluate(() => [...(window as unknown as { __coldLoadText: Set<string> }).__coldLoadText]);
    expect(coldLoad).not.toContain("text");
    expect(coldLoad).toContain("loading");
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "no sideways scrolling").toBe(true);
    // A phone has no Ctrl K to press, so the search does not offer one (3.30.0 journey F10).
    await expect(page.getByRole("searchbox", { name: "Search conversations" })).toHaveAttribute("placeholder", /^(?!.*Ctrl K).+$/);
    // The compose button keeps clear of the status footer (journey F15).
    const compose = page.getByRole("button", { name: "Write to a supervisor" });
    await expect(compose).toBeVisible();
    const footer = sidebar(page).locator("footer");
    await expect.poll(async () => {
      const [button, bar] = await Promise.all([compose.boundingBox(), footer.boundingBox()]);
      return button !== null && bar !== null && button.y + button.height <= bar.y;
    }, { message: "compose button above the status footer" }).toBe(true);
  });

  await journey.stage("Tap a conversation", async () => {
    await conversationRow(page, "cas-src").tap();
    await expect(list).toBeHidden();
    await expect(back(page)).toBeVisible();
    // The header drops the OS word before it cuts the codename (journey F14).
    await expectHeaderKeepsMachine(page, "Atlas · Linux", PELICAN);
  });

  await journey.stage("Reply with the phone keyboard", async () => {
    await composer.tap();
    await composer.fill("On my phone — go ahead with the cut.");
    const sent = hub.nextSend();
    await page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true }).tap();
    expect((await sent).text).toBe("On my phone — go ahead with the cut.");
    await hub.answerLatest(PELICAN, "Cutting now.");
    await expect(page.getByRole("log").getByText("Cutting now.")).toBeVisible();
  });

  await journey.stage("Go back to the list", async () => {
    await back(page).tap();
    await expect(list).toBeVisible();
    await expect(conversationRow(page, "cas-src")).toHaveAccessibleName(/Cutting now\./);
  });

  await journey.stage("Jump from the palette with a tap", async () => {
    await page.getByRole("button", { name: "Appearance & commands" }).tap();
    // On a phone every command name reads in full: the description drops under
    // it instead of squeezing it (cas-cfcb, cas-5478).
    const commands = palette(page).getByRole("button").filter({ visible: true });
    await expect(commands).not.toHaveCount(0);
    // Advanced too: its long plain names are the ones that used to clip first.
    const shown = await commands.count();
    await palette(page).getByText("Advanced", { exact: true }).tap();
    await expect.poll(() => commands.count(), { message: "Advanced shows its commands" }).toBeGreaterThan(shown);
    // No command leads out of the conversations to a terminal (cas-0546).
    await expect(palette(page).getByRole("button", { name: /terminal/i })).toHaveCount(0);
    const clipped = await commands.evaluateAll((buttons) => buttons
      .map((button) => button.querySelector<HTMLElement>(":scope > span"))
      .filter((span): span is HTMLElement => span !== null && span.scrollWidth > span.clientWidth + 1)
      .map((span) => span.textContent));
    expect(clipped, "command names cut off at 390 px").toEqual([]);
    const jump = palette(page).getByRole("button", { name: /Jump to gabber-studio/ });
    await expect(jump.getByText(OTTER)).toBeVisible();
    await jump.tap();
    await expect(palette(page)).toBeHidden();
    await expect(page.getByRole("button", { name: "Send to the gabber-studio supervisor", exact: true })).toBeVisible();
    await expectHeaderKeepsMachine(page, "Studio Mac · macOS", OTTER);
    // Like a tap on a list row: land to read, with no soft keyboard raised.
    await expect(composer).not.toBeFocused();
  });

  await journey.stage("Scroll back through a long thread", async () => {
    // "Jump to latest" takes its own row above the composer instead of
    // floating over a turn (cas-97ea).
    for (let turn = 1; turn <= 14; turn += 1) await hub.supervisorSays(OTTER, `Build step ${turn} of 14 finished; moving on to the next one after checking its logs.`, { kind: "answer" });
    const latest = page.getByRole("log").getByText("Build step 14 of 14 finished; moving on to the next one after checking its logs.");
    await expect(latest).toBeVisible();
    await thread(page).evaluate((element) => { element.scrollTop = 0; element.dispatchEvent(new Event("scroll")); });
    const jump = page.getByRole("button", { name: "Jump to latest" });
    await expect(jump).toBeVisible();
    await expect.poll(async () => {
      const [chip, reading] = await Promise.all([jump.boundingBox(), thread(page).boundingBox()]);
      return chip !== null && reading !== null && chip.y >= reading.y + reading.height - 1;
    }, { message: "Jump to latest sits below the thread, not over it" }).toBe(true);
    await jump.tap();
    await expect(jump).toBeHidden();
    await expect(latest).toBeInViewport();
  });

  await journey.stage("Jump from the palette with the keyboard's Enter", async () => {
    // The keyboard's Enter opens the match with that keyboard gone: no text
    // field keeps focus (cas-990d).
    await back(page).tap();
    await page.getByRole("button", { name: "Appearance & commands" }).tap();
    const filter = page.getByRole("searchbox", { name: "Filter commands" });
    await expect(filter).toBeFocused();
    await filter.pressSequentially(PELICAN);
    await filter.press("Enter");
    await expect(palette(page)).toBeHidden();
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
    await expect(composer).not.toBeFocused();
    // The thread takes focus, so no text field holds it and no soft keyboard is up.
    await expect(thread(page)).toBeFocused();
  });

  await journey.stage("A long machine name keeps its place ahead of the codename in the header", async () => {
    // cas-766c: on a phone the header is the only place that names the
    // machine, so the generated codename yields to it, not the other way round.
    for (const [project, machine, codename] of [["lab", "pippenz-workstation · Linux", "tiny-wren-3"], ["notes", "Daniel's MacBook Pro · macOS", "brisk-lark-5"], ["infra", "Build Server Rack Seven · Windows", "patient-heron-12"]] as const) {
      if (await back(page).isVisible()) await back(page).tap();
      await conversationRow(page, project).tap();
      await expect(heading(page).getByRole("heading", { level: 1 })).toHaveText(project);
      await expectHeaderKeepsMachine(page, machine, codename);
      // cas-d043 G01: each of these machine names is drawn whole at 390 in
      // light and dark, and the codename is whole or aside, never beside a
      // cut machine. The line refits after the scheme changes, so poll it.
      const where = heading(page).getByTitle(`${machine} · ${codename}`, { exact: true });
      const name = machine.split(" · ")[0]!;
      for (const scheme of ["light", "dark"] as const) {
        await page.emulateMedia({ colorScheme: scheme });
        await expect(page.locator("html")).toHaveAttribute("data-scheme", scheme);
        await expect.poll(() => where.evaluate((line) => {
          const host = line.querySelector<HTMLElement>(".host-machine")!;
          const text = [...host.childNodes].find((node) => node.nodeType === Node.TEXT_NODE)!;
          const range = document.createRange(); range.selectNodeContents(text);
          const clips = [host, line, line.closest<HTMLElement>(".conversation-host")!].map((box) => box.getBoundingClientRect().right);
          const codename = line.querySelector<HTMLElement>(".codename");
          let codenameWhole = true;
          if (codename !== null && codename.getBoundingClientRect().width > 1) {
            const words = document.createRange(); words.selectNodeContents(codename);
            codenameWhole = words.getBoundingClientRect().right <= Math.min(codename.getBoundingClientRect().right, ...clips.slice(1)) && codename.scrollWidth <= codename.clientWidth;
          }
          // Any overflow, even a fraction of a pixel, swaps the last glyphs for "…".
          return { text: text.textContent ?? "", drawnWhole: range.getBoundingClientRect().right <= Math.min(...clips), hostCut: host.scrollWidth > host.clientWidth, codenameWhole };
        }), { message: `${name} is drawn whole and ${codename} is whole or aside in ${scheme}` }).toEqual({ text: name, drawnWhole: true, hostCut: false, codenameWhole: true });
      }
      await page.emulateMedia({ colorScheme: "light" });
      // These sessions have not written yet, so the empty card names them too.
      await expectCardKeepsMachine(page, machine, codename);
    }
  });

  await journey.stage("See the switched-off machine named plainly", async () => {
    // cas-0739 (journey F10): the footer names the machine that is down, not
    // "5 connected", and the dialog opens with it on screen.
    await back(page).tap();
    const footer = sidebar(page).getByRole("button", { name: /Shed NAS/ });
    await expect(footer.getByTestId("machine-badge-state")).toBeVisible();
    expect(await footer.evaluate((button) => button.scrollWidth <= button.clientWidth + 1), "the footer names it without overflowing").toBe(true);
    await footer.tap();
    const dialog = page.getByRole("dialog", { name: "Paired machines" });
    const machines = dialog.getByTestId("paired-machine");
    const shed = machines.filter({ has: page.getByRole("heading", { name: SHED.label, exact: true }) });
    await expect(shed.getByRole("heading", { name: SHED.label, exact: true })).toBeInViewport({ ratio: 1 });
    await expect(shed.getByTestId("paired-machine-state")).toBeInViewport({ ratio: 1 });
    // Machines that are not connected are listed first.
    await expect(machines.first()).toHaveAttribute("data-machine-id", "shed");
    // One clock, the thread's 12-hour one (cas-73dd9).
    const seen = (await dialog.getByTestId("paired-machine-seen").allTextContents()).filter((text) => /\d:\d{2}/.test(text));
    expect(seen.length, "a machine that was seen says when").toBeGreaterThan(0);
    for (const text of seen) expect(text).toMatch(/ · \d{1,2}:\d{2}\s[AP]M$/);
    // A version the machine has not reported yet is still said (3.30.0 journey F10).
    await expect(shed.getByTestId("paired-machine-runtime")).toBeVisible();
  });

  await journey.stage("Pair another machine and read its header at once", async () => {
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog", { name: "Paired machines" })).toBeHidden();
    const dialog = page.getByRole("dialog", { name: /^(?:Pair a machine|Machine authorized)$/ });
    await sidebar(page).getByRole("button", { name: "Pair a machine", exact: true }).tap();
    await dialog.getByRole("button", { name: "Create pairing code" }).tap();
    // The relay authorizes on its fourth poll; a few more polls without the
    // heading is a stall, however long the polls take.
    await shownWithin(hub, "the relay's authorization", dialog.getByRole("heading", { name: "Machine authorized" }), () => hub.relayPolls, 6);
    await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Daniel");
    await dialog.getByRole("button", { name: "Pair", exact: true }).tap();
    await expect(dialog).toBeHidden();
    const toast = page.getByRole("status", { name: "Notice" });
    // Connected, then its conversation opened (journey F8): both follow
    // Forge's first session list, so they are bounded by its catalog fetches.
    await shownWithin(hub, "Forge's connected notice", toast.filter({ hasText: "Forge · Linux" }), () => hub.catalogFetchCount("forge"), 3);
    await shownWithin(hub, "Forge's conversation", heading(page).getByRole("heading", { level: 1, name: "forge-tools", exact: true }), () => hub.catalogFetchCount("forge"), 3);
    // cas-71af: focus lands on the opened thread, not the reply box.
    await expect(thread(page)).toBeFocused();
    // The notice sits below the thread header, never over the back link,
    // project and host (cas-002e). It slides in, so poll the settled box.
    await expect.poll(async () => {
      const [notice, header] = await Promise.all([toast.boundingBox(), heading(page).boundingBox()]);
      return notice !== null && header !== null && notice.y >= header.y + header.height;
    }, { message: "toast below the thread header" }).toBe(true);
    await holdPageClock(page);
    try {
      // The public notice lasts 3.2 seconds; advance its browser clock.
      await page.clock.fastForward(3_200);
      // Once expired the status region is empty, so it is no longer read.
      await expect(toast).toBeEmpty();
    } finally {
      await page.clock.resume();
    }
  });
});

// cas-5072: a phone turned sideways, notch on either side. Safari gives the
// page side insets (viewport-fit=cover) in a tab as well as installed, so
// every row, button, field and sheet control must sit clear of both bands.
// Chromium's CDP override supplies the insets to the production bundle.
// 844×390 shows Raw output as a bottom sheet; 932×430 (a larger phone) as the
// side drawer, which must not gain an empty left gutter (cas-5072 QA F01).
const SIDEWAYS = [{ width: 844, height: 390 }, { width: 932, height: 430 }] as const;
const NOTCH = 44;

async function notchInsets(page: Page): Promise<void> {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Emulation.setSafeAreaInsetsOverride", { insets: { top: 0, left: NOTCH, right: NOTCH, bottom: 21 } });
}

/** Visible, reachable controls inside `scope` whose box enters a notch band. */
async function underTheBands(page: Page, scope: Locator): Promise<string[]> {
  const width = page.viewportSize()!.width;
  return scope.locator(`:is(button, a[href], input, textarea, select, summary, [role="button"], [tabindex]:not([tabindex="-1"]))`).evaluateAll(
    (nodes, [width, notch]) =>
      nodes
        .filter((node) => {
          const element = node as HTMLElement;
          if (element.closest("[inert], [hidden], [aria-hidden='true']")) return false;
          if (!element.checkVisibility({ visibilityProperty: true, opacityProperty: true })) return false;
          // A focusable scroll region (the thread's log) spans the page; what it holds is checked itself.
          if (["auto", "scroll"].includes(getComputedStyle(element).overflowY)) return false;
          const box = element.getBoundingClientRect();
          // 1px clipped helpers (visually hidden text, skip links off-screen) are not targets.
          if (box.width <= 1 || box.height <= 1) return false;
          if (box.right <= 0 || box.left >= width || box.bottom <= 0 || box.top >= window.innerHeight) return false;
          return box.left < notch || box.right > width - notch;
        })
        .map((node) => {
          const element = node as HTMLElement;
          const box = element.getBoundingClientRect();
          const name = element.getAttribute("aria-label") || element.id || element.textContent?.trim().slice(0, 32) || element.tagName;
          return `${name} (left ${Math.round(box.left)}, right ${Math.round(box.right)})`;
        }),
    [width, NOTCH] as const,
  );
}

for (const { width, height } of SIDEWAYS) test.describe(`sideways with a notch ${width}x${height}`, () => {
  test.use({ viewport: { width, height }, hasTouch: true, isMobile: true, colorScheme: "dark" });

  test(`HUB-J9 sideways phone with a notch at ${width}x${height}: nothing to tap sits under the side bands (cas-5072)`, journeyPart, async ({ page, journey }) => {
    const hub = await journey.hub({ machines: [ATLAS, STUDIO], paired: ["atlas", "studio"] });
    await notchInsets(page);
    const list = page.getByRole("navigation", { name: "Choose a supervisor" });

    await journey.stage("Turn the phone sideways with the notch at one edge", async () => {
      await journey.open();
      await expect(conversationRow(page, "cas-src")).toBeVisible();
      await expect(page.getByRole("button", { name: "Write to a supervisor" })).toBeVisible();
      expect(await underTheBands(page, sidebar(page)), "list rows, Write to a supervisor and the footer clear both notches").toEqual([]);
    });

    await journey.stage("Open a conversation that waits on me", async () => {
      await conversationRow(page, "cas-src").click();
      await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
      await hub.socketOpen(PELICAN);
      await hub.supervisorSays(PELICAN, "Fix the warning in-train, or ship allowlisted?", { kind: "ask", options: ["Fix in-train", "Ship allowlisted"] });
      await expect(page.getByRole("region", { name: `Waiting on you: question from ${PELICAN}` })).toBeVisible();
      await page.getByRole("textbox", { name: "Your message" }).fill("Looking now");
      expect(await underTheBands(page, page.getByRole("main")), "the header, thread, Waiting-on-you strip, composer and Send clear both notches").toEqual([]);
    });

    await journey.stage("Read the raw output sideways", async () => {
      await page.getByRole("button", { name: "Raw output", exact: true }).click();
      const drawer = page.getByRole("dialog", { name: "Raw output" });
      await expect(drawer).toBeVisible();
      expect(await underTheBands(page, drawer), "the raw-output title and Close clear both notches").toEqual([]);
      const title = (await drawer.getByRole("heading", { name: "Raw output" }).boundingBox())!;
      expect(title.x, "the raw-output title clears the left notch").toBeGreaterThanOrEqual(NOTCH);
      const sheet = (await drawer.boundingBox())!;
      if (sheet.x > 0) {
        // The side drawer sits on the right edge, away from the left band: its
        // title keeps the ordinary padding, with no notch-width gutter.
        expect(title.x - sheet.x, "no empty left gutter on the side drawer").toBeLessThan(NOTCH);
      }
      await page.keyboard.press("Escape");
      await expect(drawer).toBeHidden();
    });
  });
});
