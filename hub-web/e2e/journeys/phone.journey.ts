import type { Locator, Page } from "@playwright/test";
import { test, expect, journeyPart } from "./journey";
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

test("HUB-J9 on a phone: from the list to a reply and back", async ({ page, journey }) => {
  // Ten stages over nine machines, two palette sweeps, a long thread and a
  // relay pairing. Idle it takes about 34 s, about 16 s of which is the
  // journey fixture (screencast, trace, receipts), and that also counts
  // against this budget. On the loaded merge-queue host it passed in 34–39 s
  // at load 63 but ran past the 60 s default at load 258 (cas-03b7). The
  // waits inside are DOM assertions or bounded by the hub double's own
  // events, so this is a hang guard sized from the loaded runtime: more than
  // 3× the idle run, and 4.6× the worst loaded pass.
  test.setTimeout(180_000);
  const hub = await journey.hub({ machines: [ATLAS, STUDIO, SHED, FORGE, ...LONG_LABELS], paired: ["atlas", "studio", "shed", ...LONG_LABELS.map((machine) => machine.id)], relay: { machine: "forge", claimAfter: 2, authorizeAfter: 4 } });
  await page.route("https://shed.test/**", (route) => route.abort("connectionrefused"));
  await page.routeWebSocket(/shed\.test/, (ws) => { void ws.close({ code: 1006 }); });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const composer = page.getByRole("textbox", { name: "Your message" });
  /**
   * cas-766c: the header's host line always shows some of the machine name;
   * its OS word is shown whole or not at all, never cut; and the machine is
   * only ever cut once the codename has stepped aside. The title keeps both.
   */
  const expectHeaderKeepsMachine = async (machine: string, codename: string) => {
    const where = page.locator(".conversation-identity .host-where");
    await expect(where).toHaveAttribute("title", `${machine} · ${codename}`);
    const shown = await where.evaluate((line) => {
      const host = line.querySelector<HTMLElement>(".host-machine")!;
      const os = host.querySelector<HTMLElement>(".host-os");
      const name = line.querySelector<HTMLElement>(".codename");
      const ch = parseFloat(getComputedStyle(host).fontSize) * 0.6;
      return {
        machineChars: host.getBoundingClientRect().width / ch,
        // cas-d043 QA round 1: any overflow draws an ellipsis, so no tolerance.
        machineCut: host.scrollWidth > host.clientWidth,
        // cas-8526: a part that steps aside is clipped to 1px, not removed,
        // so "shown" means drawn wider than that.
        osShown: os !== null && os.getBoundingClientRect().width > 1,
        codenameShown: name !== null && name.getBoundingClientRect().width > 1,
      };
    });
    expect(shown.machineChars, `some of ${machine} is on the line`).toBeGreaterThanOrEqual(4);
    expect(shown.osShown && shown.machineCut, "the OS word is never cut mid-word").toBe(false);
    if (shown.machineCut) expect(shown.codenameShown, `${codename} steps aside before ${machine} is cut`).toBe(false);
    await expect(page.locator("#conversation-connection")).toBeVisible();
    // cas-8526: whatever the line shows, it is heard whole: the machine, its
    // OS word and the codename, not only the parts that fit.
    const heard = (await page.locator(".conversation-identity .conversation-host").ariaSnapshot()).replace(/^\s*- text:\s*/gm, " ").replace(/\s+/g, " ");
    expect(heard, "the host line is heard whole").toContain(machine);
    expect(heard, "the host line is heard whole").toContain(codename);
  };

  /**
   * cas-1451: the empty-thread card under the title keeps the machine as the
   * header does: whole before any codename character, the OS word dropped
   * whole, and one separator between them, never a double gap.
   */
  const expectCardKeepsMachine = async (machine: string, codename: string) => {
    const where = page.locator(".thread .empty .proj2");
    await expect(where).toHaveAttribute("title", `${machine} · ${codename}`);
    const shown = await where.evaluate((line) => {
      const host = line.querySelector<HTMLElement>(".proj2-machine")!;
      const os = host.querySelector<HTMLElement>(".host-os");
      const name = line.querySelector<HTMLElement>(".codename");
      const separator = line.querySelector<HTMLElement>(".proj2-sep");
      const visible = (node: HTMLElement | null) => node !== null && node.getClientRects().length > 0;
      const box = (node: HTMLElement) => node.getBoundingClientRect();
      return {
        machineCut: host.scrollWidth > host.clientWidth + 1,
        osShown: visible(os),
        codenameShown: visible(name),
        codenameChars: visible(name) ? box(name!).width / (parseFloat(getComputedStyle(name!).fontSize) * 0.6) : 0,
        separatorShown: visible(separator),
        gaps: visible(name) && visible(separator) ? [box(separator!).left - box(host).right, box(name!).left - box(separator!).right] : [],
        separatorText: separator?.textContent ?? "",
      };
    });
    expect(shown.machineCut && shown.codenameShown, `${codename} yields before ${machine} is cut`).toBe(false);
    expect(shown.osShown && shown.machineCut, "the OS word is never cut mid-word").toBe(false);
    expect(shown.separatorShown, "the separator goes with the codename").toBe(shown.codenameShown);
    for (const gap of shown.gaps) expect(Math.abs(gap), "one separator, no double gap").toBeLessThanOrEqual(1);
    expect(shown.separatorText).toBe(" · ");
  };

  await journey.stage("Open the list on a phone", async () => {
    // Record every word the list and the footer badge show during the cold
    // load: they must not claim "Not paired", "No live supervisors" or
    // "Reconnecting" before the rows arrive (journey F14).
    await page.addInitScript(() => {
      const seen = new Set<string>();
      (window as unknown as { __coldLoadText: Set<string> }).__coldLoadText = seen;
      const record = () => {
        for (const selector of ["#hub-footer-badges", "#conversation-empty:not([hidden])"]) {
          const text = document.querySelector(selector)?.textContent?.trim();
          if (text) seen.add(text);
        }
      };
      new MutationObserver(record).observe(document, { subtree: true, childList: true, characterData: true, attributes: true });
    });
    await journey.open();
    await expect(list.getByRole("button")).toHaveCount(5);
    const coldLoad = await page.evaluate(() => [...(window as unknown as { __coldLoadText: Set<string> }).__coldLoadText]);
    expect(coldLoad.join(" | "), "cold-load list and footer text").not.toMatch(/Not paired|No live supervisors|Reconnecting/);
    expect(coldLoad.some((text) => text.includes("Loading")), "the cold load shows it is loading").toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "no sideways scrolling").toBe(true);
    // A phone has no Ctrl K to press, so the search does not offer one (3.30.0 journey F10).
    await expect(page.getByRole("searchbox", { name: "Search conversations" })).toHaveAttribute("placeholder", "Search conversations");
    // The compose button says what it does and keeps clear of the status footer (journey F15).
    const compose = page.getByRole("button", { name: "Write to a supervisor" });
    await expect(compose).toHaveText("Write to a supervisor");
    const [button, footer] = await Promise.all([compose.boundingBox(), page.locator(".conversation-sidebar > footer").boundingBox()]);
    expect(button!.y + button!.height, "compose button above the status footer").toBeLessThanOrEqual(footer!.y);
  });

  await journey.stage("Tap a conversation", async () => {
    await list.getByRole("button", { name: /cas-src/ }).tap();
    await expect(list).toBeHidden();
    await expect(page.getByRole("button", { name: "‹ Conversations", exact: true })).toBeVisible();
    // The header drops the OS word before it cuts the codename (journey F14).
    await expectHeaderKeepsMachine("Atlas · Linux", PELICAN);
  });

  await journey.stage("Reply with the phone keyboard", async () => {
    await composer.tap();
    await composer.fill("On my phone — go ahead with the cut.");
    const sent = hub.nextSend();
    await page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true }).tap();
    expect((await sent).text).toBe("On my phone — go ahead with the cut.");
    hub.answerLatest(PELICAN, "Cutting now.");
    await expect(page.getByRole("log").getByText("Cutting now.")).toBeVisible();
  });

  await journey.stage("Go back to the list", async () => {
    await page.getByRole("button", { name: "‹ Conversations", exact: true }).tap();
    await expect(list).toBeVisible();
    await expect(list.getByRole("button", { name: /cas-src/ })).toContainText("Cutting now.");
  });

  await journey.stage("Jump from the palette with a tap", async () => {
    await page.getByRole("button", { name: "Appearance & commands" }).tap();
    // On a phone every command name reads in full: the description drops under
    // it instead of squeezing it (cas-cfcb, cas-5478).
    const titles = page.locator("#command-palette .palette-command:not([hidden]) > span");
    await expect(titles.first()).toBeVisible();
    // Advanced too: its long plain names are the ones that used to clip first.
    await page.locator("#command-palette .palette-advanced > summary").tap();
    await expect(page.locator("#command-palette .palette-advanced .palette-command").filter({ visible: true })).not.toHaveCount(0);
    // No command leads out of the conversations to a terminal (cas-0546).
    await expect(page.locator("#command-palette").getByRole("button", { name: /terminal/i })).toHaveCount(0);
    const clipped = await titles.evaluateAll((spans) => spans.filter((span) => span.getClientRects().length > 0 && span.scrollWidth > span.clientWidth + 1).map((span) => span.textContent));
    expect(clipped, "command names cut off at 390 px").toEqual([]);
    await expect(page.locator("#command-palette [data-palette-machine] small").filter({ hasText: OTTER })).toBeVisible();
    await page.getByRole("button", { name: /Jump to gabber-studio/ }).tap();
    await expect(page.locator("#command-palette")).toBeHidden();
    await expect(page.getByRole("button", { name: "Send to the gabber-studio supervisor", exact: true })).toBeVisible();
    await expectHeaderKeepsMachine("Studio Mac · macOS", OTTER);
    // Like a tap on a list row: land to read, with no soft keyboard raised
    // over the conversation just opened.
    await expect(composer).not.toBeFocused();
  });

  await journey.stage("Scroll back through a long thread", async () => {
    // Enough turns to scroll, then read from the top: "Jump to latest" takes its
    // own row above the composer instead of floating over a turn (cas-97ea).
    for (let turn = 1; turn <= 14; turn += 1) hub.supervisorSays(OTTER, `Build step ${turn} of 14 finished; moving on to the next one after checking its logs.`, { kind: "answer" });
    const thread = page.locator(".conversation-reading.thread");
    await expect(page.getByRole("log").getByText("Build step 14 of 14 finished; moving on to the next one after checking its logs.")).toBeVisible();
    await thread.evaluate((element) => { element.scrollTop = 0; element.dispatchEvent(new Event("scroll")); });
    const jump = page.getByRole("button", { name: "Jump to latest" });
    await expect(jump).toBeVisible();
    const [chip, reading] = await Promise.all([jump.boundingBox(), thread.boundingBox()]);
    expect(chip!.y, "Jump to latest sits below the thread, not over it").toBeGreaterThanOrEqual(reading!.y + reading!.height - 1);
    await jump.tap();
    await expect(jump).toBeHidden();
    await expect(page.getByRole("log").getByText("Build step 14 of 14 finished; moving on to the next one after checking its logs.")).toBeInViewport();
  });

  await journey.stage("Jump from the palette with the keyboard's Enter", async () => {
    // A tap opens the palette with its filter focused under the soft keyboard.
    // The keyboard's Enter opens the match with that keyboard gone: no text
    // field keeps focus, so it cannot stay up over the thread (cas-990d).
    await page.getByRole("button", { name: "‹ Conversations", exact: true }).tap();
    await page.getByRole("button", { name: "Appearance & commands" }).tap();
    const filter = page.getByRole("searchbox", { name: "Filter commands" });
    await expect(filter).toBeFocused();
    await filter.pressSequentially(PELICAN);
    await filter.press("Enter");
    await expect(page.locator("#command-palette")).toBeHidden();
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
    await expect(composer).not.toBeFocused();
    // The thread takes focus, so no text field holds it and no soft keyboard is up.
    await expect(page.locator(".conversation-reading.thread")).toBeFocused();
    expect(await page.evaluate(() => {
      const active = document.activeElement as HTMLElement | null;
      return Boolean(active && (active.tagName === "INPUT" || active.tagName === "TEXTAREA" || active.isContentEditable));
    }), "no text field holds focus, so no soft keyboard is up").toBe(false);
    // The thread it lands on draws the house focus ring, never the browser's
    // default 1px outline (cas-0bf5).
    const ring = await page.locator(".conversation-reading.thread").evaluate((thread) => {
      const style = getComputedStyle(thread);
      const probe = document.createElement("span");
      probe.style.color = "var(--color-focus)";
      thread.append(probe);
      const focus = getComputedStyle(probe).color;
      probe.remove();
      return { focused: document.activeElement === thread, style: style.outlineStyle, width: style.outlineWidth, color: style.outlineColor, focus };
    });
    expect(ring.focused, "the jump lands on the thread").toBe(true);
    expect(ring).toMatchObject({ style: "solid", width: "2px", color: ring.focus });
  });

  await journey.stage("A long machine name keeps its place ahead of the codename in the header", async () => {
    // cas-766c: on a phone the header is the only place that names the
    // machine, so the generated codename yields to it, not the other way round.
    for (const [project, machine, codename] of [["lab", "pippenz-workstation · Linux", "tiny-wren-3"], ["notes", "Daniel's MacBook Pro · macOS", "brisk-lark-5"], ["infra", "Build Server Rack Seven · Windows", "patient-heron-12"]] as const) {
      const back = page.getByRole("button", { name: "‹ Conversations", exact: true });
      if (await back.isVisible()) await back.tap();
      await list.getByRole("button", { name: new RegExp(project) }).tap();
      await expect(page.locator(".conversation-identity h1")).toHaveText(project);
      await expectHeaderKeepsMachine(machine, codename);
      // cas-d043 G01 (QA round 1): on its own header row, each of these
      // machine names is drawn whole at 390, in light and dark: the laid-out
      // name ends inside every box that clips it (the machine span and the
      // host line), and the codename is either aside or whole up to its own
      // ellipsis, never beside a cut machine.
      for (const scheme of ["light", "dark"] as const) {
        await page.emulateMedia({ colorScheme: scheme });
        await expect(page.locator("html")).toHaveAttribute("data-scheme", scheme);
        const drawn = await page.locator(".conversation-identity .host-where").evaluate((line) => {
          const host = line.querySelector<HTMLElement>(".host-machine")!;
          const text = [...host.childNodes].find((node) => node.nodeType === Node.TEXT_NODE)!;
          const range = document.createRange(); range.selectNodeContents(text);
          const name = range.getBoundingClientRect();
          const clips = [host, line, line.closest<HTMLElement>(".conversation-host")!].map((box) => box.getBoundingClientRect().right);
          const codename = line.querySelector<HTMLElement>(".codename");
          const codenameShown = codename !== null && codename.getBoundingClientRect().width > 1;
          let codenameWhole = true;
          if (codenameShown) {
            const words = document.createRange(); words.selectNodeContents(codename!);
            codenameWhole = words.getBoundingClientRect().right <= Math.min(codename!.getBoundingClientRect().right, ...clips.slice(1)) && codename!.scrollWidth <= codename!.clientWidth;
          }
          return { text: text.textContent ?? "", nameRight: name.right, clipRight: Math.min(...clips), hostCut: host.scrollWidth > host.clientWidth, codenameShown, codenameWhole };
        });
        const name = machine.split(" · ")[0]!;
        expect(drawn.text, `${name} is the machine's text`).toBe(name);
        // Any overflow, even a fraction of a pixel, swaps the last glyphs for "…".
        expect(drawn.nameRight, `${name} is drawn whole at 390 in ${scheme}`).toBeLessThanOrEqual(drawn.clipRight);
        expect(drawn.hostCut, `${name} has no ellipsis in ${scheme}`).toBe(false);
        expect(drawn.codenameWhole, `${codename} is whole or aside beside ${name} in ${scheme}`).toBe(true);
      }
      await page.emulateMedia({ colorScheme: "light" });
      // These sessions have not written yet, so the empty card names them too.
      await expect(page.locator(".thread .empty .said")).toBeVisible();
      await expectCardKeepsMachine(machine, codename);
    }
  });

  await journey.stage("See the switched-off machine named plainly", async () => {
    // Never live and failing: "Can't reach · retrying" in the dialog, not
    // "Connecting…" forever; the footer counts it and its dot is not all-clear.
    await page.getByRole("button", { name: "‹ Conversations", exact: true }).tap();
    const footer = page.locator("#paired-machines-toggle");
    // cas-0739 (journey F10): the footer names the machine that is down,
    // not "5 connected", and the dialog opens with it on screen.
    await expect(footer.locator(".machine-badge-state")).toHaveText("Can't reach Shed NAS");
    await expect(footer.locator(".pairing-dot")).toHaveClass("pairing-dot partial");
    expect(await footer.evaluate((button) => button.scrollWidth <= button.clientWidth + 1), "the footer names it without overflowing").toBe(true);
    await footer.tap();
    const dialog = page.locator("#paired-machines-dialog");
    await expect(dialog.locator('[data-machine-id="shed"] h3')).toBeInViewport({ ratio: 1 });
    await expect(dialog.locator('[data-machine-id="shed"] .paired-machine-state')).toBeInViewport({ ratio: 1 });
    await expect(dialog.locator(".paired-machine").first()).toHaveAttribute("data-machine-id", "shed");
    await expect(dialog).toContainText("Can't reach · retrying");
    await expect(dialog).not.toContainText("Connecting");
    // One clock, the thread's 24-hour one, and plain words for a version the
    // machine has not reported yet (3.30.0 journey F10).
    await expect(dialog.locator(".paired-machine-seen").filter({ hasText: "Last seen" }).first()).toHaveText(/ · \d{2}:\d{2}$/);
    await expect(dialog).not.toContainText(/\b(AM|PM)\b/);
    await expect(dialog).not.toContainText("Runtime not yet received");
    await expect(dialog.locator('[data-machine-id="shed"] .paired-machine-runtime')).toHaveText("Version unknown until it connects");
  });

  await journey.stage("Pair another machine and read its header at once", async () => {
    // The paired-machines dialog from the stage before is still open.
    await page.keyboard.press("Escape");
    await expect(page.locator("#paired-machines-dialog")).toBeHidden();
    const dialog = page.locator("#pair-dialog");
    await page.getByRole("button", { name: "Pair a machine" }).filter({ visible: true }).first().tap();
    await dialog.getByRole("button", { name: "Create pairing code" }).tap();
    // The relay authorizes on its fourth poll; a few more polls without the
    // heading is a stall, however long the polls take.
    await shownWithin(hub, "the relay's authorization", dialog.getByRole("heading", { name: "Machine authorized" }), () => hub.relayPolls, 6);
    await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Daniel");
    await dialog.getByRole("button", { name: "Pair", exact: true }).tap();
    await expect(dialog).toBeHidden();
    const toast = page.locator("#toast");
    // Connected, then its conversation opened: both follow Forge's first
    // session list, so they are bounded by Forge's catalog fetches.
    await shownWithin(hub, "Forge's connected notice", toast.filter({ hasText: "Forge · Linux connected" }), () => hub.catalogFetchCount("forge"), 3);
    // Pairing from the phone opens the new machine's conversation; no list
    // tap in between (journey F8).
    await shownWithin(hub, "Forge's conversation", page.locator(".conversation-identity h1").filter({ hasText: /^forge-tools$/ }), () => hub.catalogFetchCount("forge"), 3);
    // cas-71af (dfb2 QA F02): focus lands on the opened thread, not the page
    // body (and not the reply box, which would raise the phone keyboard).
    await expect(page.locator(".conversation-reading.thread")).toBeFocused();
    // The "connected" toast sits below the thread header, never over the
    // back link, project and host (cas-002e).
    const [notice, heading] = await Promise.all([toast.boundingBox(), page.locator(".conversation-heading").boundingBox()]);
    expect(notice!.y, "toast below the thread header").toBeGreaterThanOrEqual(heading!.y + heading!.height);
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
async function underTheBands(page: Page, scope: string): Promise<string[]> {
  const width = page.viewportSize()!.width;
  return page.locator(`${scope} :is(button, a[href], input, textarea, select, summary, [role="button"], [tabindex]:not([tabindex="-1"]))`).evaluateAll(
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
      await expect(list.getByRole("button", { name: /cas-src/ })).toBeVisible();
      await expect(page.getByRole("button", { name: "Write to a supervisor" })).toBeVisible();
      expect(await underTheBands(page, ".conversation-sidebar"), "list rows, Write to a supervisor and the footer clear both notches").toEqual([]);
    });

    await journey.stage("Open a conversation that waits on me", async () => {
      await list.getByRole("button", { name: /cas-src/ }).click();
      await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
      hub.supervisorSays(PELICAN, "Fix the warning in-train, or ship allowlisted?", { kind: "ask", options: ["Fix in-train", "Ship allowlisted"] });
      await expect(page.getByRole("region", { name: `Waiting on you: question from ${PELICAN}` })).toBeVisible();
      await page.getByRole("textbox", { name: "Your message" }).fill("Looking now");
      expect(await underTheBands(page, ".conversation-shell"), "the header, thread, Waiting-on-you strip, composer and Send clear both notches").toEqual([]);
    });

    await journey.stage("Read the raw output sideways", async () => {
      await page.getByRole("button", { name: "Raw output", exact: true }).click();
      const drawer = page.getByRole("dialog", { name: "Raw output" });
      await expect(drawer).toBeVisible();
      expect(await underTheBands(page, ".raw-output-drawer"), "the raw-output title and Close clear both notches").toEqual([]);
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
