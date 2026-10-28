import { activate, phoneLayout, phoneFind, showConversationList } from "./responsive-goals";
import { join } from "node:path";
import { test, expect, journeyPart, RECEIPTS } from "./journey";
import { journeyStamp } from "./clock";
import { ATLAS, STUDIO, PELICAN, OTTER } from "./world";
import type { DoubleOptions, Machine } from "./hub-double";
import { LATEST_RELEASE_URL } from "./hub-double";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";

test("HUB-J3 machine versions distinguish installed runtimes, cached updates and the web build", journeyPart, async ({ page, journey }) => {
  let checks = 0, unavailable = false;
  await page.route(LATEST_RELEASE_URL, route => {
    checks++;
    return unavailable ? route.abort() : route.fulfill({ json: { tag_name: "v3.88.0" } });
  });
  const versions = { atlas: "3.9.0", studio: "3.88.0" };
  await journey.hub({ machines: [ATLAS, STUDIO], paired: ["atlas", "studio"], versions });
  await journey.open();
  // seedPaired clears localStorage during fixture setup. Measure the cache
  // after the real returning-user navigation, independently of that warm-up.
  await expect.poll(() => page.evaluate(() => JSON.parse(localStorage.getItem("commander.latest-release.v1") ?? "null")?.version)).toBe("3.88.0");
  const initialChecks = checks;
  const atlas = page.locator('.project-machine-chip[data-project-machine="atlas"]');
  const studio = page.locator('.project-machine-chip[data-project-machine="studio"]');
  await expect(atlas).toContainText("Cassy 3.9.0");
  await expect(atlas).toContainText("Update available → 3.88.0");
  await expect(studio).toContainText("Cassy 3.88.0");
  await expect(studio.locator(".machine-version-update")).toHaveCount(0);
  await expect(page.locator('[title="Web client release and build"]')).toHaveText(/^Web \d+\.\d+\.\d+ · [a-f0-9]{8}$/);
  const captureVersions = async (name: string) => {
    if (!process.env.VERSION_QA) return;
    // Preserve the real production DOM and CSS for the strict visual polish
    // inspector; only asset URLs are inlined, with no replacement markup.
    mkdirSync(process.env.VERSION_QA, { recursive: true });
    const css = readFileSync("dist/app.css", "utf8").replace(/url\((?:["']?)([^)'"\s]+\.woff2)(?:["']?)\)/g, (_match, url: string) =>
      `url(data:font/woff2;base64,${readFileSync(`dist/${url.split("/").at(-1)}`).toString("base64")})`);
    const html = await page.evaluate(() => {
      const root = document.documentElement.cloneNode(true) as HTMLElement;
      root.querySelectorAll("script, link").forEach(node => node.remove());
      return `<!doctype html>${root.outerHTML}`;
    });
    writeFileSync(`${process.env.VERSION_QA}/machine-versions-${name}.html`, html.replace("</head>", `<style>${css}</style></head>`));
  };
  for (const width of [1280, 390]) for (const colorScheme of ["light", "dark"] as const) {
    await page.setViewportSize({ width, height: width === 390 ? 844 : 800 });
    await page.emulateMedia({ colorScheme });
    await journey.stage(`Read installed and available versions at ${width} in ${colorScheme}`, async () => {
      await expect(atlas).toBeVisible();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await expect(page.locator('[title="Web client release and build"]')).toBeVisible();
    });
    await captureVersions(`${width}-${colorScheme}`);
  }
  await journey.stage("The machine register repeats the installed and target versions", async () => {
    await page.locator("#paired-machines-toggle").click();
    await expect(page.locator('[data-machine-id="atlas"] .paired-machine-runtime')).toContainText("Update available → 3.88.0");
    await expect(page.locator('[data-machine-id="studio"] .paired-machine-runtime')).toHaveText("Cassy 3.88.0");
    await page.getByRole("button", { name: "Close paired machines" }).click();
  });
  await journey.stage("Reloads and unknown machine versions do not invent update warnings or extra checks", async () => {
    versions.studio = "unknown";
    await page.reload();
    await expect(studio).toContainText("Version unknown");
    await expect(studio.locator(".machine-version-update")).toHaveCount(0);
    await expect(atlas).toContainText("Update available → 3.88.0");
    expect(checks).toBe(initialChecks);
  });
  await journey.stage("An hourly failed check retains the last known release without retrying on reload", async () => {
    unavailable = true;
    await page.clock.fastForward(60 * 60_000 + 1);
    await expect.poll(() => checks).toBe(initialChecks + 1);
    await expect(atlas).toContainText("Update available → 3.88.0");
    await page.reload();
    await expect(atlas).toContainText("Update available → 3.88.0");
    expect(checks).toBe(initialChecks + 1);
  });
});

// A third machine with a 40-character name (cas-1ca1): its row must ellipsise
// the name and keep the time stamp readable.
const FORGE: Machine = {
  id: "forge",
  label: "Forge build box with an unusual hostname · Linux",
  sessions: [{ name: "quiet-heron-7", supervisor: "quiet-heron-7", project_dir: "/projects/lighthouse", workers: ["swift-lark-3"], liveness: "live", last_activity_at: journeyStamp(-3 * 3_600_000), last_activity: "supervisor → swift-lark-3" }],
};

for (const width of [1280, 390]) for (const long of [true, false]) {
  test(`HUB-J3 a ${long ? "wrapped" : "short"} footer name keeps its status dot on the first line at ${width} (cas-94eb)`, journeyPart, async ({ page, journey }) => {
    const label = long ? "soundwave — a very long personal workstation name with several extra words and anunbrokentailthatneedstowrap" : "Atlas · Linux";
    await page.setViewportSize({ width, height: width === 390 ? 844 : 800 });
    await journey.hub({ machines: [{ ...ATLAS, label }], paired: [ATLAS.id] });
    await journey.stage("Read the machine name and its connection together", async () => {
      await journey.open();
      const footer = page.locator("#paired-machines-toggle");
      await expect(footer.locator(".machine-badge-state")).toHaveText("Connected");
      await expect(footer.locator("span").nth(1)).toHaveAttribute("title", label);
      const geometry = await footer.evaluate((button) => {
        const name = button.children[1] as HTMLElement;
        const range = document.createRange();
        range.setStart(name.firstChild!, 0); range.setEnd(name.firstChild!, 1);
        const first = range.getBoundingClientRect();
        const dot = button.children[0]!.getBoundingClientRect();
        const state = button.children[2]!.getBoundingClientRect();
        const bounds = button.getBoundingClientRect();
        return { firstTop: first.top, firstBottom: first.bottom, nameHeight: name.getBoundingClientRect().height,
          dotCentre: dot.top + dot.height / 2, dotRight: dot.right, nameLeft: first.left, stateCentre: state.top + state.height / 2,
          stateRight: state.right, right: bounds.right, textFits: name.scrollWidth <= name.clientWidth + 1 };
      });
      expect(geometry.dotCentre, "status dot belongs to the label's first line").toBeGreaterThanOrEqual(geometry.firstTop);
      expect(geometry.dotCentre, "status dot belongs to the label's first line").toBeLessThanOrEqual(geometry.firstBottom);
      expect(geometry.dotRight).toBeLessThan(geometry.nameLeft);
      expect(geometry.stateRight).toBeLessThanOrEqual(geometry.right);
      if (width === 1280 && long) {
        expect(geometry.nameHeight).toBeGreaterThan(geometry.firstBottom - geometry.firstTop);
        expect(geometry.textFits, "the full desktop machine name wraps inside the footer").toBe(true);
      }
      if (!long) {
        expect(geometry.stateCentre, "short names keep their state on the same row").toBeGreaterThanOrEqual(geometry.firstTop);
        expect(geometry.stateCentre).toBeLessThanOrEqual(geometry.firstBottom);
      }
      await footer.focus(); await page.keyboard.press("Enter");
      await expect(page.locator("#paired-machines-list h3")).toHaveText(label);
      await page.locator("#paired-machines-close").click();
      await expect(footer).toBeFocused();
    });
  });
}

test("HUB-J3 find the conversation that needs me", async ({ page, journey }) => {
  if (await phoneLayout(page)) { await phoneFind(page, journey); return; }
  // Fourteen stages, two searches, three page loads and the palette. On a
  // quiet host the test takes about 72 s (50 s of stages); on the loaded
  // merge-queue host its passing runs took up to 2.2 min and 6 of 10 at
  // --workers=2 ran past a 120 s budget in the late palette stages (cas-dff1).
  // Every wait inside is event-driven (hub double, DOM, page clock), so the
  // whole-test budget is sized from that loaded runtime with headroom.
  test.setTimeout(240_000);
  const atlas: Machine = { ...ATLAS, sessions: ATLAS.sessions.map((session) => ({ ...session })) };
  const atlasSession = atlas.sessions[0]!;
  const history: NonNullable<DoubleOptions["history"]> = {
    [PELICAN]: [{
      messages: [], has_earlier: false,
      replies: [{ notification_id: 999, reply_to: null, message: "The bank feed for October is ready. Should I reconcile it against the statement now?", summary: "", device_id: "journey-device", kind: "answer", attachments: [], session: PELICAN, at: journeyStamp(-1_000) }],
    }],
  };
  const hub = await journey.hub({ machines: [atlas, STUDIO, FORGE], paired: ["atlas", "studio", "forge"], history });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const search = page.getByRole("searchbox", { name: "Search conversations" });
  /** "Conversations" and its New session control sit on one line (cas-865c). */
  const expectOneRowHeading = async () => {
    const heading = page.locator(".conversation-list-title h1");
    const control = page.locator(".conversation-list-title #new-session-toggle");
    await expect(control).toHaveText("+ New session");
    const [h1, button] = await Promise.all([heading.boundingBox(), control.boundingBox()]);
    expect(Math.abs((h1!.y + h1!.height / 2) - (button!.y + button!.height / 2)), "heading and New session on one row").toBeLessThan(6);
    await expect(page.locator(".conversation-list-top #pair-toggle")).toBeVisible();
  };
  const filter = page.getByRole("searchbox", { name: "Filter commands" });
  /**
   * cas-d4a9: the name Chromium's accessibility tree gives a node, which is
   * what a screen reader speaks. CSS generated content counts toward it unless
   * it carries empty alternative text.
   */
  const spokenName = async (selector: string) => {
    const cdp = await page.context().newCDPSession(page);
    try {
      const { root } = await cdp.send("DOM.getDocument", { depth: 0 });
      const { nodeId } = await cdp.send("DOM.querySelector", { nodeId: root.nodeId, selector });
      const { nodes } = await cdp.send("Accessibility.getPartialAXTree", { nodeId, fetchRelatives: false });
      return String(nodes[0]?.name?.value ?? "");
    } finally {
      await cdp.detach();
    }
  };
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
    await expect(list.locator(".conversation-row")).toHaveCount(3);
    // cas-865c: the heading and New session share one row; nothing wraps.
    await expectOneRowHeading();
    firstWelcome = await welcomeLayout();
    expect(firstWelcome).toMatchObject({ contextOpen: false, railWidth: 0, mainRight: 1280, centred: true });
    // Machines list in id order (atlas, forge, studio). Rows are titled by project, then machine; the codename is tertiary.
    await expect(list.locator(".conversation-project")).toHaveText(["cas-src", "lighthouse", "gabber-studio"]);
    await expect(list.locator(".conversation-machine")).toHaveText(["Atlas", "Forge build box with an unusual hostname", "Studio Mac"]);
    await expect(list.locator(".conversation-supervisor")).toHaveText([PELICAN, "quiet-heron-7", OTTER]);
    await expect(search).toBeVisible();
    await expect(search).toHaveAttribute("placeholder", "Search conversations (Ctrl K)");
  });

  await journey.stage("Each row's time is its own session's activity", async () => {
    // cas-6acf: a time comes only from the session's own activity, in words for
    // assistive tech, and never the catalog check every poll refreshes to "now".
    const lighthouse = list.getByRole("button", { name: /lighthouse/ }).and(list.locator(".conversation-row"));
    await expect(lighthouse.locator(".conversation-when")).toHaveText("3h");
    await expect(lighthouse).toHaveAccessibleName(/3 hours ago/);
    await expect(lighthouse).not.toHaveAccessibleName(/\b3h\b/);
    const times = () => list.locator(".conversation-row").evaluateAll((rows) => rows.map((node) => {
      const time = node.querySelector<HTMLElement>(".conversation-when");
      return time ? `${time.textContent}|${time.title}` : "none";
    }));
    const before = await times();
    expect(before.some((entry) => entry.includes("Catalog checked")), before.join(", ")).toBe(false);
    // Across a catalog poll nothing turns "now" and nothing runs backwards.
    // the catalog refetch is driven by a machine event (cas-9772), not waited out.
    await Promise.all([hub.announceCatalog("atlas"), hub.announceCatalog("studio"), hub.announceCatalog("forge")]);
    expect(await times()).toEqual(before);
  });

  await journey.stage("Notice a new reply while away", async () => {
    // A new device knows only the session catalog. When the supervisor queues
    // a reply, it must fetch that thread's first history page before the row
    // can show the unread count and preview.
    atlasSession.last_activity_at = journeyStamp(-1_000);
    atlasSession.last_activity = "supervisor → Commander";
    await hub.announceCatalog("atlas");
    const unopened = list.getByRole("button", { name: /cas-src/ }).and(list.locator(".conversation-row"));
    await expect(unopened.getByLabel("1 unread")).toBeVisible();
    await expect(unopened.locator(".conversation-preview")).toContainText("The bank feed for October is ready.");
  });

  await journey.stage("Open the conversation, then notice its next reply", async () => {
    await list.getByRole("button", { name: /cas-src/ }).and(list.locator(".conversation-row")).click();
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
    await expect(list.getByRole("button", { name: /cas-src/ }).and(list.locator(".conversation-row")).getByLabel("1 unread")).toHaveCount(0);
    // The list is on screen beside the thread, so there is no back step (F17):
    // moving to another conversation is one click on its row.
    await expect(page.getByRole("button", { name: "‹ Conversations", exact: true })).toBeHidden();
    await list.getByRole("button", { name: /gabber-studio/ }).and(list.locator(".conversation-row")).click();
    await expect(page.getByRole("button", { name: "Send to the gabber-studio supervisor", exact: true })).toBeVisible();
    hub.supervisorSays(PELICAN, "The staging deploy finished; nothing needs you yet.", { kind: "status" });
    await expect(list.getByRole("button", { name: /cas-src/ }).and(list.locator(".conversation-row")).getByLabel("1 unread")).toBeVisible();
  });

  await journey.stage("Find the conversation through the list search", async () => {
    // A project name leaves its conversation as the only row.
    await search.fill("gabber");
    await expect(list.locator(".conversation-row")).toHaveCount(1);
    await expect(list.locator(".conversation-row").first()).toContainText("gabber-studio");
    // The machine and the supervisor codename find it too.
    await search.fill("atlas");
    await expect(list.locator(".conversation-row")).toHaveCount(1);
    await expect(list.locator(".conversation-row").first()).toContainText("cas-src");
    // cas-786a (journey F29): the row Enter opens carries its "Enter ↵" hint
    // and its unread count side by side, never one drawn over the other. Every
    // point of the count is the count's own, not the hint painted on top.
    await expect(list.locator('.conversation-row[data-enter-target="true"]').getByLabel("1 unread")).toBeVisible();
    const covered = await list.locator('.conversation-row[data-enter-target="true"] .conversation-unread').evaluate((badge) => {
      const box = badge.getBoundingClientRect();
      const points = [[0.5, 0.5], [0.15, 0.2], [0.85, 0.2], [0.15, 0.8], [0.85, 0.8]].map(([x, y]) => [box.left + box.width * x, box.top + box.height * y]);
      return points.filter(([x, y]) => { const hit = document.elementFromPoint(x, y); return hit !== badge && !badge.contains(hit); }).length;
    });
    expect.soft(covered, "points of the unread count covered by the Enter hint").toBe(0);
    // cas-d4a9: the "Enter ↵" hint is for the eye. The row is still heard by
    // its own name and unread count, not "… Enter ↵ 1 unread"; the field's
    // aria-activedescendant already says which row Enter opens.
    const hinted = '.conversation-sidebar .conversation-row[data-enter-target="true"]';
    expect(await spokenName(hinted), "the row's spoken name").not.toContain("Enter");
    expect(await page.locator(hinted).ariaSnapshot(), "the row's aria snapshot").not.toContain("Enter");
    await search.fill(OTTER);
    await expect(list.locator(".conversation-row")).toHaveCount(1);
    await expect(list.locator(".conversation-row").first()).toContainText("gabber-studio");
    await search.fill("no-such-project");
    await expect(list.locator(".conversation-row")).toHaveCount(0);
    await expect(page.locator("#conversation-empty")).toContainText("No conversation matches “no-such-project”.");
    // Escape brings the whole list back.
    await search.press("Escape");
    await expect(search).toHaveValue("");
    await expect(list.locator(".conversation-row")).toHaveCount(3);
    await search.fill("gabber-studio");
    // cas-537f: the row Enter would open is marked, and named to assistive
    // tech as the field's active descendant.
    const target = list.locator('.conversation-row[data-enter-target="true"]');
    await expect(target).toHaveCount(1);
    await expect(target).toContainText("gabber-studio");
    await expect(search).toHaveAttribute("aria-activedescendant", (await target.getAttribute("id"))!);
    await list.getByRole("button", { name: /gabber-studio/ }).and(list.locator(".conversation-row")).click();
    await expect(page.getByRole("button", { name: "Send to the gabber-studio supervisor", exact: true })).toBeVisible();
    // The header names the project once; machine and codename sit beneath it.
    await expect(page.locator(".conversation-identity h1")).toHaveText("gabber-studio");
    await expect(page.locator(".conversation-host")).toContainText(`Studio Mac · macOS · ${OTTER}`);
    // cas-537f: opening a result clears the search, as Enter does; the whole
    // list (and every other row's unread count) is back.
    await expect(search).toHaveValue("");
    await expect(list.locator(".conversation-row")).toHaveCount(3);
    await expect(search).not.toHaveAttribute("aria-activedescendant");
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
    await expect(list.locator(".conversation-row")).toHaveCount(1);
    await page.keyboard.press("Enter");
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
    await expect(page.locator(".conversation-identity h1")).toHaveText("cas-src");
    await expect(page.getByRole("textbox", { name: "Your message" })).toBeFocused();
    await expect(search).toHaveValue("");
    await expect(list.locator(".conversation-row")).toHaveCount(3);
  });

  await journey.stage("A 40-character machine name wraps in its project header and stays separate from session times", async () => {
    const project = list.getByRole("group", { name: /^lighthouse ·/ });
    const chip = project.getByRole("button", { name: "Filter conversations on Forge build box with an unusual hostname, online", exact: true });
    // Project cards move the machine identity out of the row's timestamp
    // column. Its complete name wraps in the chip at both viewport widths.
    const clearOfRows = () => project.evaluate((node) => {
      const chip = node.querySelector<HTMLElement>(".project-machine-chip")!;
      const name = chip.getBoundingClientRect();
      const card = node.getBoundingClientRect();
      const row = node.querySelector(".conversation-row")!.getBoundingClientRect();
      return { whole: chip.scrollWidth <= chip.clientWidth + 1, contained: name.left >= card.left && name.right <= card.right, separate: name.bottom <= row.top };
    });
    await expect(chip).toContainText("Forge build box with an unusual hostname");
    expect(await clearOfRows()).toEqual({ whole: true, contained: true, separate: true });
    await chip.focus();
    await page.keyboard.press("Shift+Tab");
    await page.keyboard.press("Tab");
    await expect(chip).toBeFocused();
    await expect(chip).toHaveCSS("outline-style", "solid");
    await page.screenshot({ path: join(RECEIPTS, journey.id, "long-machine-focused.png") });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.getByRole("button", { name: "‹ Conversations", exact: true }).click();
    await expect(chip).toBeVisible();
    await expectOneRowHeading();
    expect(await clearOfRows()).toEqual({ whole: true, contained: true, separate: true });
    await page.screenshot({ path: join(RECEIPTS, journey.id, "long-machine-phone.png") });
    await page.setViewportSize({ width: 1280, height: 720 });
    await expect(page.locator(".conversation-welcome")).toBeVisible();
    expect(await welcomeLayout()).toEqual(firstWelcome);
  });

  await journey.stage("Jump to a supervisor by name", async () => {
    await page.getByRole("button", { name: "Appearance & commands" }).click();
    // Commands are grouped; the debugging switches wait, collapsed, in Advanced.
    const palette = page.locator("#command-palette");
    // Commands sit with what they act on: Paired machines with the
    // machines, and "Dismiss all info" only when there is something to
    // dismiss (3.30.0 journey F4).
    await expect(palette.locator(".palette-group-heading:visible")).toHaveText(["Conversations", "Machines", "Appearance", "Advanced"]);
    await expect(palette.locator('[data-palette-group="conversations"] .palette-command:not([data-palette-session])')).toHaveCount(0);
    await expect(palette.locator('[data-palette-group="machines"]')).toContainText("Paired machines");
    await expect(palette.getByRole("button", { name: /Dismiss all info/ })).toHaveCount(0);
    // cas-0546: Conversations is the only surface. No command opens a
    // terminal, shows worker panes or takes control; Advanced holds only the
    // dormant-session switch.
    await expect(palette.getByRole("button", { name: /terminal|worker panes|control/i, includeHidden: true })).toHaveCount(0);
    await expect(palette.locator(".palette-advanced .palette-command")).toHaveText([/^Show dormant sessions/]);
    await filter.fill("dormant");
    await expect(palette.getByRole("button", { name: /Show dormant sessions/ })).toBeVisible();
    // cas-537f: "light" matches a conversation and an appearance; the one
    // Enter runs is marked, and named as the filter's active descendant.
    await filter.fill("light");
    const enterTarget = palette.locator('.palette-command[data-enter-target="true"]');
    await expect(enterTarget).toHaveCount(1);
    await expect(enterTarget).toContainText("Jump to lighthouse");
    await expect(filter).toHaveAttribute("aria-activedescendant", (await enterTarget.getAttribute("id"))!);
    // The hint is drawn with empty alternative text, so it is seen and not
    // read into the command's name (cas-d4a9).
    expect(await enterTarget.evaluate((node) => getComputedStyle(node, "::after").content), "the Enter hint").toBe('"Enter ↵" / ""');
    expect(await spokenName('#command-palette .palette-command[data-enter-target="true"]'), "the command's spoken name").not.toContain("Enter");
    expect(await enterTarget.ariaSnapshot(), "the command's aria snapshot").not.toContain("Enter");
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
    const commands = page.locator("#command-palette .palette-command");
    await expect(commands.visible()).toHaveCount(1);
    await expect(commands.visible().first()).toContainText("Jump to gabber-studio");
    await expect(page.getByRole("button", { name: /Appearance · Dark/ })).toBeHidden();
    await page.getByRole("button", { name: /Jump to gabber-studio/ }).click();
    await expect(page.locator("#command-palette")).toBeHidden();
    await expect(page.getByRole("button", { name: "Send to the gabber-studio supervisor", exact: true })).toBeVisible();
    await expect(page.locator(".conversation-identity h1")).toHaveText("gabber-studio");
    // A mouse jump lands in the opened conversation's composer too. With a
    // conversation open the palette keeps the same groups: its actions
    // (Raw output, Interrupt) are in the conversation header, and the
    // palette speaks of conversations, not sessions (cas-0546).
    await expect(page.getByRole("textbox", { name: "Your message" })).toBeFocused();
    await expect(page.locator("#command-palette .palette-group-heading")).toHaveText(["Conversations", "Machines", "Appearance", "Advanced"]);
    await expect(page.locator('#command-palette [data-palette-action="control"]')).toHaveCount(0);
    await expect(page.locator("#command-palette-query")).toHaveAttribute("placeholder", "Type a command or conversation");
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
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
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
    await expect(page.getByRole("button", { name: "Send to the gabber-studio supervisor", exact: true })).toBeVisible();
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
    // The render the update causes has happened (cas-dff1: on the turn itself, not 300 ms).
    await expect(page.locator(".thread")).toContainText("Still here.");
    await expect(filter).toHaveValue(PELICAN);
    await expect(row).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(page.locator("#command-palette")).toBeHidden();
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
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
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
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
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
    await filter.fill(PELICAN);
    await filter.press("Enter");
    await expect(palette).toBeHidden();
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
    // cas-786a (journey F30): with no filter, Enter goes to the next
    // conversation that needs the operator, never the one already open, and
    // the open one says so. gabber-studio gets a reply while cas-src is open
    // (visited once since the reload, so its machine is attached).
    await list.getByRole("button", { name: /gabber-studio/ }).and(list.locator(".conversation-row")).click();
    await expect(page.getByRole("button", { name: "Send to the gabber-studio supervisor", exact: true })).toBeVisible();
    await expect.poll(() => hub.hasSocket(OTTER)).toBe(true);
    await list.getByRole("button", { name: /cas-src/ }).and(list.locator(".conversation-row")).click();
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
    hub.supervisorSays(OTTER, "The Mac tests are green.", { kind: "status" });
    await expect(list.getByRole("button", { name: /gabber-studio/ }).and(list.locator(".conversation-row")).getByLabel("1 unread")).toBeVisible();
    await reopen();
    const current = palette.locator('.palette-command[data-palette-current="true"]');
    await expect(current).toHaveCount(1);
    await expect(current).toContainText("Jump to cas-src");
    await expect(current).toHaveAttribute("aria-current", "true");
    expect(await current.locator("small").evaluate((node) => getComputedStyle(node, "::before").content), "the open conversation says so").toContain('"Open now · "');
    const enterTarget = palette.locator('.palette-command[data-enter-target="true"]');
    await expect(enterTarget).toHaveCount(1);
    await expect(enterTarget).toContainText("Jump to gabber-studio");
    await expect(filter).toHaveAttribute("aria-activedescendant", (await enterTarget.getAttribute("id"))!);
    await filter.press("Enter");
    await expect(palette).toBeHidden();
    await expect(page.getByRole("button", { name: "Send to the gabber-studio supervisor", exact: true })).toBeVisible();
    // Nothing waits now: Enter offers the first other conversation, here cas-src.
    await reopen();
    await expect(current).toContainText("Jump to gabber-studio");
    await expect(enterTarget).toContainText("Jump to cas-src");
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
    await list.getByRole("button", { name: /gabber-studio/ }).and(list.locator(".conversation-row")).click();
    await expect(page.getByRole("button", { name: "Send to the gabber-studio supervisor", exact: true })).toBeVisible();
    await expect(palette).toBeHidden();
    // And once more from the other conversation: still closed.
    await list.getByRole("button", { name: /cas-src/ }).and(list.locator(".conversation-row")).click();
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
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
      // cas-813a: also every loading look, where its line sits, the header
      // and row words, and the composer's width, frame by frame.
      const seen = { footer: new Set<string>(), pane: new Set<string>(), looks: new Set<string>(), centres: new Set<string>(), status: new Set<string>(), composer: new Set<number>(), details: new Set<string>() };
      (window as unknown as { __attachSeen: typeof seen }).__attachSeen = seen;
      const sample = () => {
        const footer = document.querySelector<HTMLElement>("#hub-footer-badges .machine-badge-state");
        if (footer) seen.footer.add(footer.innerText.trim());
        const pane = document.querySelector<HTMLElement>(".conversation-pane-slot");
        if (pane) seen.pane.add(pane.innerText.trim());
        if (!pane) return;
        for (const details of pane.querySelectorAll<HTMLDetailsElement>(".connection-details")) seen.details.add(`${details.querySelector("summary")?.innerText.trim()} ${details.open ? "open" : "closed"}`);
        for (const verdict of pane.querySelectorAll(".terminal-state")) if ((verdict as HTMLElement).offsetParent) seen.looks.add("verdict card");
        for (const line of pane.querySelectorAll<HTMLElement>(".conversation-loading")) {
          if (!line.offsetParent) continue;
          seen.looks.add(`line: ${line.innerText.trim()}`);
          const box = line.getBoundingClientRect();
          seen.centres.add(`${Math.round(box.x + box.width / 2)},${Math.round(box.y + box.height / 2)}`);
        }
        const header = document.querySelector<HTMLElement>("#conversation-connection")?.innerText.replace("·", "").trim();
        const row = document.querySelector<HTMLElement>('.conversation-row[aria-current="true"] .conversation-preview')?.innerText.trim();
        if (header) seen.status.add(`header: ${header}`);
        if (row) seen.status.add(`row: ${row}`);
        const composer = document.querySelector<HTMLElement>("#message-text");
        if (composer?.offsetParent) seen.composer.add(Math.round(composer.getBoundingClientRect().width));
      };
      new MutationObserver(sample).observe(document.body, { subtree: true, childList: true, characterData: true, attributes: true });
    });
    hub.delayAttach("quiet-heron-7", 2_000);
    await list.getByRole("button", { name: /lighthouse/ }).and(list.locator(".conversation-row")).click();
    const opening = page.locator(".conversation-pane-slot .conversation-loading");
    await expect(opening).toHaveText("Opening the conversation…");
    // cas-813a: still for the first second, then a quiet pulse.
    const dot = opening.locator(".dots i").first();
    // Its first frames are still: the pulse is scheduled about a second after the open.
    expect(parseInt(await dot.evaluate((element) => element.style.animationDelay), 10), "motion waits after the open").toBeGreaterThanOrEqual(500);
    expect(await dot.evaluate((element) => element.getAnimations().map((animation) => (animation as CSSAnimation).animationName)), "the quiet pulse that follows").toEqual(["opening-dots"]);
    await expect(page.getByRole("button", { name: "Send to the lighthouse supervisor", exact: true })).toBeVisible();
    await expect(page.locator(".thread .empty b")).toHaveText("lighthouse", { timeout: 10_000 });
    const seen = await page.evaluate(() => {
      const { footer, pane, looks, centres, status, composer, details } = (window as unknown as { __attachSeen: Record<string, Set<string | number>> }).__attachSeen;
      return { footer: [...footer], pane: [...pane], looks: [...looks], centres: [...centres], status: [...status], composer: [...composer], details: [...details] };
    });
    // Past the quiet window the attempt and stage were offered behind Details, closed.
    expect(seen.details, "Details while it opened").toEqual(["Details closed"]);
    // cas-813a: one loading look, in one place, centred in the reading area;
    // the header and row stay Live and the composer keeps its width.
    expect(seen.looks, "loading looks while it opened").toEqual(["line: Opening the conversation…"]);
    expect(seen.centres, "where the opening line sat").toHaveLength(1);
    const slot = (await page.locator(".conversation-pane-slot").boundingBox())!;
    const [cx, cy] = String(seen.centres[0]).split(",").map(Number);
    expect(Math.abs(cx - (slot.x + slot.width / 2)), "line centred across the reading area").toBeLessThan(4);
    expect(Math.abs(cy - (slot.y + slot.height / 2)), "line centred down the reading area").toBeLessThan(4);
    expect(seen.status.filter((text) => String(text).startsWith("header:")), "header words while it opened").toEqual(["header: Live"]);
    expect(seen.status.filter((text) => /Connecting|Opening/.test(String(text))), "row or header flipping while it opened").toEqual([]);
    expect(seen.composer, "composer widths while it opened").toHaveLength(1);
    // cas-766c: on a phone the empty card's machine · codename line keeps
    // the 40-character machine name (at least 16ch, its OS word dropped, never
    // cut mid-word) and the generated codename yields first.
    // The header's host line, at desktop and phone: the machine is on it, and
    // its OS word is whole or gone, never "· Lin…" (cas-766c).
    const headerHost = () => page.locator(".conversation-identity .host-where").evaluate((line) => {
      const machine = line.querySelector<HTMLElement>(".host-machine")!;
      const ch = parseFloat(getComputedStyle(machine).fontSize) * 0.6;
      return { machineChars: machine.getBoundingClientRect().width / ch, osCut: (machine.querySelector<HTMLElement>(".host-os")?.getBoundingClientRect().width ?? 0) > 1 && machine.scrollWidth > machine.clientWidth + 1 };
    });
    for (const header of [await headerHost()]) { expect(header.machineChars, "machine on the header at 1280").toBeGreaterThanOrEqual(15.5); expect(header.osCut, "OS word cut at 1280").toBe(false); }
    await page.setViewportSize({ width: 390, height: 844 });
    for (const header of [await headerHost()]) { expect(header.machineChars, "machine on the header at 390").toBeGreaterThanOrEqual(4); expect(header.osCut, "OS word cut at 390").toBe(false); }
    const meta = page.locator(".thread .empty .proj2");
    const measureFit = () => meta.evaluate((line) => {
      const machine = line.querySelector<HTMLElement>(".proj2-machine")!;
      const ch = parseFloat(getComputedStyle(machine).fontSize) * 0.6;
      return { machineChars: machine.getBoundingClientRect().width / ch, os: (machine.querySelector<HTMLElement>(".host-os")?.getBoundingClientRect().width ?? 0) > 1, title: line.getAttribute("title") };
    });
    // Viewport resizing resolves before ResizeObserver has necessarily fitted the line.
    await expect.poll(measureFit, { message: "the phone metadata finishes fitting after resize" }).toMatchObject({ os: false });
    const fitted = await measureFit();
    expect(fitted.machineChars, "the machine keeps 16ch at 390px").toBeGreaterThanOrEqual(15.5);
    expect(fitted.os, "the OS word goes before the name is cut").toBe(false);
    expect(fitted.title).toBe("Forge build box with an unusual hostname · Linux · quiet-heron-7");
    await page.setViewportSize({ width: 1280, height: 720 });
    expect(seen.footer, "the footer while the conversation opened").toEqual(["Connected"]);
    const jargon = seen.pane.filter((text) => /relay|attempt|authori[sz]ation|handshake|heartbeat|resolving|dialing/i.test(String(text)));
    expect(jargon, "relay-stage words on the default attach surface").toEqual([]);
    await expect(footerState).toHaveText("Connected");
  });

  await journey.stage("A first open that misses the 3-second mark retries calmly, and the footer stays Connected", async () => {
    // cas-28df: a conversation whose first attach sends no session state
    // within 3 s retries. On a live machine that used to drop the footer to
    // "1 connected" and flash an unavailable notice and the full retry
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
    const opening = page.locator(".conversation-pane-slot .conversation-loading");
    await expect(opening).toHaveText("Opening the conversation…");
    await expect(page.locator(".thread .empty b")).toHaveText("lighthouse", { timeout: 15_000 });
    const seen = await page.evaluate(() => (window as unknown as { __retrySeen: { footer: string[]; pane: string[] } }).__retrySeen);
    expect(seen.pane.some((text) => text.startsWith("Opening the conversation…")), "the pane said it was opening").toBe(true);
    expect(seen.footer, "the footer once every machine was up").toEqual(["Connected"]);
    const alarm = seen.pane.filter((text) => /unavailable|interrupted|retrying|Try again|relay|attempt|diagnostic|handshake/i.test(text));
    expect(alarm, "retry wording on the default surface while it opened").toEqual([]);
    await expect(page.locator("#hub-footer-badges .machine-badge-state")).toHaveText("Connected");
  });
});

// cas-2a33: the journeys above run on a browser that declares Linux, so they
// read "Ctrl K". On a Mac every surface names the palette chord "⌘K" instead,
// never both, and ⌘K reaches the search and then the palette.
test.describe("on a Mac", () => {
  test.use({ journeyPlatform: "mac" });

  test("HUB-J3 on a Mac, every surface names the palette chord ⌘K (cas-2a33)", journeyPart, async ({ page, journey }) => {
    await journey.stage("On a Mac, the search and the Appearance & commands control say ⌘K, and ⌘K opens them", async () => {
      await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
      await journey.open();
      const phone = await phoneLayout(page);
      const search = page.getByRole("searchbox", { name: "Search conversations" });
      await expect(search).toHaveAttribute("placeholder", phone ? "Search conversations" : "Search conversations (⌘K)");
      if (phone) await activate(page, page.getByRole("button", { name: "Appearance & commands", exact: true }));
      else {
        await page.keyboard.press("Meta+k");
        await expect(search).toBeFocused();
        await page.keyboard.press("Meta+k");
      }
      await expect(page.getByRole("searchbox", { name: "Filter commands" })).toBeFocused();
      if (phone) await activate(page, page.getByRole("button", { name: "Close command palette", exact: true }));
      else await page.keyboard.press("Escape");
      await activate(page, page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")));
      await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
      await showConversationList(page);
      // The list returns through Back on phone; its
      // Appearance & commands control names the same chord in its tooltip.
      await expect(page.getByRole("button", { name: "Appearance & commands", exact: true })).toHaveAttribute("title", "Appearance & commands (⌘K twice)");
      await expect(search).toHaveAttribute("placeholder", phone ? "Search conversations" : "Search conversations (⌘K)");
      await expect(page.locator("body")).not.toContainText("Ctrl K");
      expect(await page.evaluate(() => [...document.querySelectorAll("[title], [placeholder], [aria-label]")].map((node) => `${node.getAttribute("title") ?? ""} ${node.getAttribute("placeholder") ?? ""} ${node.getAttribute("aria-label") ?? ""}`).filter((text) => text.includes("Ctrl K")))).toEqual([]);
    });
  });
});

for (const width of [1280, 390]) {
  test(`HUB-J3 supervisor exit ends the conversation at ${width} (cas-cec1)`, journeyPart, async ({ page, journey }) => {
    await page.setViewportSize({ width, height: width === 390 ? 844 : 800 });
    const machine: Machine = { ...ATLAS, sessions: [{ ...ATLAS.sessions[0]!, workers: ["worker-a", "worker-b", "worker-c"] }] };
    const session = machine.sessions[0]!;
    const hub = await journey.hub({ machines: [machine], paired: [machine.id] });
    await journey.open();
    await page.locator(".conversation-row").first().click();
    await expect(page.locator("#message-text")).toBeVisible();
    await journey.stage("Supervisor and workers are live", async () => {
      await expect(page.locator("#conversation-connection")).toContainText("Live");
      await page.locator("#message-text").fill("Keep this draft");
    });
    session.ended = { at: journeyStamp(), reason: "prompt_input_exit", workers_stopped: ["worker-a", "worker-b"], orphans: ["worker-c"], unverified: [] };
    session.liveness = "stale_metadata";
    session.workers = ["worker-c"];
    await hub.announceCatalog(machine.id);
    await journey.stage("Session ended accounts for every worker", async () => {
      await expect(page.locator("#conversation-connection")).toContainText("Ended");
      await expect(page.locator(".session-end-receipt")).toContainText("Supervisor and 2 workers stopped");
      await expect(page.locator(".session-end-receipt")).toContainText("/exit");
      await expect(page.locator(".session-end-receipt .session-end-orphans")).toContainText("Still alive: worker-c");
      await expect(page.locator("#message-send")).toBeDisabled();
      await expect(page.locator("#message-status")).toContainText("New session");
      await expect(page.locator("#message-text")).toHaveValue("Keep this draft");
      await expect(page.locator(".terminal-disconnected-banner")).toHaveCount(0);
      if (width === 390) await page.locator("#conversation-fleet").click();
      await expect(page.locator("#status-view")).toContainText("Session ended");
      await expect(page.locator("#status-view")).toContainText("Stopped: worker-a, worker-b");
      await expect(page.locator("#status-view")).toContainText("Still alive: worker-c");
      await expect(page.locator("#status-view .status-agent")).toHaveCount(0);
      if (width === 390) await page.getByRole("button", { name: "Close tasks & progress", exact: true }).click();
    });
    await showConversationList(page);
    await expect(page.locator(".conversation-row").first()).toContainText("Ended");
  });
}
