import type { Locator } from "@playwright/test";
import { test, expect } from "./journey";
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
  /** At 390 px the header reads "<machine> · <codename> · <state>": no OS word, the codename whole, the state visible. */
  const expectHeaderKeepsCodename = async (machine: string, codename: string, machineWhole = true) => {
    const where = page.locator(".conversation-identity .host-where");
    await expect(where).toHaveAttribute("title", new RegExp(`· ${codename}$`));
    await expect(page.locator(".conversation-identity .host-os")).toBeHidden();
    // Rendered text only (the hidden OS word drops out); flex items come back one per line.
    expect((await where.innerText()).split("\n").join("")).toBe(`${machine} · ${codename}`);
    // The codename is never the part that is cut (QA F01); the machine name yields first.
    const name = where.locator(".codename");
    expect(await name.evaluate((element) => element.scrollWidth > element.clientWidth + 1), `the header shows ${codename} whole at 390 px`).toBe(false);
    const machineCut = await where.locator(".host-machine").evaluate((element) => element.scrollWidth > element.clientWidth + 1);
    if (machineWhole) expect(machineCut, `${machine} fits beside ${codename}`).toBe(false);
    else expect(machineCut, `${machine} is the part that ellipsises`).toBe(true);
    await expect(page.locator("#conversation-connection")).toBeVisible();
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
    await expectHeaderKeepsCodename("Atlas", PELICAN);
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
    await expect(page.getByRole("button", { name: /Open the terminal view/ })).toBeVisible();
    const clipped = await titles.evaluateAll((spans) => spans.filter((span) => span.getClientRects().length > 0 && span.scrollWidth > span.clientWidth + 1).map((span) => span.textContent));
    expect(clipped, "command names cut off at 390 px").toEqual([]);
    await expect(page.locator("#command-palette [data-palette-machine] small").filter({ hasText: OTTER })).toBeVisible();
    await page.getByRole("button", { name: /Jump to gabber-studio/ }).tap();
    await expect(page.locator("#command-palette")).toBeHidden();
    await expect(page.getByRole("button", { name: "Send to the gabber-studio supervisor", exact: true })).toBeVisible();
    await expectHeaderKeepsCodename("Studio Mac", OTTER);
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

  await journey.stage("A long machine name yields to the codename in the header", async () => {
    // cas-e918 QA F01: hostname-style names of 19–23 characters used to keep
    // their room and cut the codename to "ti…" or drop it entirely.
    for (const [project, machine, codename] of [["lab", "pippenz-workstation", "tiny-wren-3"], ["notes", "Daniel's MacBook Pro", "brisk-lark-5"], ["infra", "Build Server Rack Seven", "patient-heron-12"]] as const) {
      const back = page.getByRole("button", { name: "‹ Conversations", exact: true });
      if (await back.isVisible()) await back.tap();
      await list.getByRole("button", { name: new RegExp(project) }).tap();
      await expect(page.locator(".conversation-identity h1")).toHaveText(project);
      await expectHeaderKeepsCodename(machine, codename, false);
    }
  });

  await journey.stage("See the switched-off machine named plainly", async () => {
    // Never live and failing: "Can't reach · retrying" in the dialog, not
    // "Connecting…" forever; the footer counts it and its dot is not all-clear.
    await page.getByRole("button", { name: "‹ Conversations", exact: true }).tap();
    const footer = page.locator("#paired-machines-toggle");
    await expect(footer).toContainText("5 connected");
    await expect(footer.locator(".pairing-dot")).toHaveClass("pairing-dot partial");
    await footer.tap();
    const dialog = page.locator("#paired-machines-dialog");
    await expect(dialog.getByText("Shed NAS · Linux")).toBeVisible();
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
