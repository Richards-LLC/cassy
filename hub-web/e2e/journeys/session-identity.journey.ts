import { test, expect, journeyPart } from "./journey";
import { journeyDay, journeyStamp } from "./clock";
import { SCOPES, type Machine } from "./hub-double";

// cas-55a4: three live gabber-studio sessions on one machine, as the
// operator's Pixel saw them on 2026-09-30. noble-cheetah-84 wrote to Commander
// yesterday and is idle; calm-puma-34 is the busy one and has never written
// to Commander; wild-shark-68 has its own short conversation.
const NOBLE = "gabber-studio-noble-cheetah-84";
const PUMA = "gabber-studio-calm-puma-34";
const SHARK = "gabber-studio-wild-shark-68";
const session = (name: string, supervisor: string, lastActivityAt: string, lastActivity: string) => ({
  name, supervisor, project_dir: "/projects/gabber-studio", workers: [], liveness: "live" as const, last_activity_at: lastActivityAt, last_activity: lastActivity,
});
// Stamped per test, never at file load: a stamp taken between tests picks up
// the previous test's elapsed time and lands late (cas-4e52 mechanism).
const atlas = (): Machine => ({
  id: "atlas",
  label: "Atlas · Linux",
  sessions: [
    session(NOBLE, "noble-cheetah-84", journeyDay(1, 21, 41), "supervisor → Commander"),
    session(PUMA, "calm-puma-34", journeyStamp(-2 * 60_000), "supervisor → bright-robin-85"),
    session(SHARK, "wild-shark-68", journeyStamp(-40 * 60_000), "Commander → supervisor"),
  ],
});

const you = (id: number, text: string, from: string, at: string) => ({ notification_id: id, target: "supervisor", text, state: "acknowledged", stamped: true, device_id: "journey-device", operator_label: "Pixel 10", session: from, at });
const sup = (id: number, message: string, from: string, at: string, kind = "answer") => ({ notification_id: id, reply_to: null, message, summary: "", device_id: "journey-device", kind, attachments: [], session: from, at });

test("HUB-J14 tell a project's live sessions apart", async ({ page, journey }) => {
  const hub = await journey.hub({
    machines: [atlas()],
    paired: ["atlas"],
    scopes: { atlas: [...SCOPES, "factory-manage"] },
    history: {
      // A daemon with session-bound history: no turns of its own, and the
      // older session's thread beside the page, never in it.
      [PUMA]: [{ has_earlier: false, messages: [], replies: [],
        earlier_messages: [you(101, "Render the mixdown preview.", NOBLE, journeyDay(1, 21, 30))],
        earlier_replies: [
          sup(102, "Mixdown preview rendered: 3 stems.", NOBLE, journeyDay(1, 21, 41)),
          sup(103, "worker died: daring-robin-43", "", journeyDay(1, 17, 20), "blocker"),
        ] }],
      [SHARK]: [{ has_earlier: false, messages: [you(201, "Status on the stem export?", SHARK, journeyStamp(-45 * 60_000))], replies: [sup(202, "Stem export is at 60%.", SHARK, journeyStamp(-40 * 60_000))] }],
      // A daemon from before cas-55a4 still sends the project-wide page:
      // the other session's turn in it is filed beside the thread.
      [NOBLE]: [{ has_earlier: false,
        messages: [you(101, "Render the mixdown preview.", NOBLE, journeyDay(1, 21, 30)), you(201, "Status on the stem export?", SHARK, journeyStamp(-45 * 60_000))],
        replies: [sup(102, "Mixdown preview rendered: 3 stems.", NOBLE, journeyDay(1, 21, 41)), sup(202, "Stem export is at 60%.", SHARK, journeyStamp(-40 * 60_000))] }],
    },
  });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const log = page.getByRole("log");
  const earlier = page.getByRole("region", { name: "Earlier sessions" });
  /** The list sits beside the thread on a desktop; a narrow layout goes back to it. */
  const backToList = async () => { const back = page.locator("#conversation-back"); if (await back.isVisible()) await back.click(); };
  const row = (codename: string) => list.locator(".conversation-row", { hasText: codename });

  await journey.stage("See a project's live sessions together", async () => {
    await journey.open();
    await expect(list.locator(".conversation-group-head")).toHaveText("gabber-studio · 3 conversations on Atlas");
    const rows = list.locator(".conversation-row");
    await expect(rows.locator(".conversation-supervisor")).toHaveText(["calm-puma-34Most recent", "wild-shark-68", "noble-cheetah-84"]);
    await expect(row("calm-puma-34")).toHaveAttribute("data-most-recent", "true");
    // Each row's time is its own session's last activity, not one shared time.
    await expect(rows.locator(".conversation-when")).toHaveText(["2m", "40m", /^\d+h$/]);
    await expect(row("calm-puma-34").locator(".conversation-when")).toHaveAttribute("title", "Last activity 2m ago · Messaged bright-robin-85");
    // cas-5d2c: under a heading that names the project and machine once, each
    // row leads with its codename and says what its session last did, before
    // any of them is opened; the heading and footer count the same noun.
    await expect(rows.locator(".conversation-title")).toHaveCount(3);
    for (const title of await rows.locator(".conversation-title").all()) await expect(title).toBeHidden();
    await expect(rows.locator(".conversation-preview")).toHaveText(["Messaged bright-robin-85", "You wrote to it", "Wrote to you"]);
    await expect(page.locator(".hub-footer-meta")).toContainText("3 conversations");
    // Every session of the group is in view at 1280×720.
    for (const node of await rows.all()) await expect(node).toBeInViewport({ ratio: 1 });
  });

  await journey.stage("Open a session that has not written yet", async () => {
    await row("calm-puma-34").click();
    const empty = page.locator(".thread .empty");
    // cas-010f: plain words, no product codename or queue jargon, and
    // Terminal view named as the header names it.
    await expect(empty.locator(".said")).toHaveText("No messages from the gabber-studio supervisor in this session yet — nothing is waiting on you.");
    await expect(empty.locator(".empty-activity")).toHaveText("Last active 2m ago");
    await expect(empty).not.toContainText("Commander");
    await expect(empty).not.toContainText("→");
    await expect(empty.getByRole("button")).toHaveText(["Terminal view"]);
    await expect(page.locator(".thread .msgs")).not.toContainText("Mixdown preview rendered");
    await expect(page.locator(".pinned-ask")).toBeHidden();
    // The older session's thread is a collapsed, labelled section, with dates.
    await expect(earlier.locator("summary")).toHaveText([/^Earlier session noble-cheetah-84, Yesterday/, /^Earlier messages with no session recorded, Yesterday/]);
    await expect(earlier.locator("details[open]")).toHaveCount(0);
  });

  await journey.stage("Read an earlier session's messages", async () => {
    const noble = earlier.locator('details[data-session="gabber-studio-noble-cheetah-84"]');
    await noble.locator("summary").click();
    await expect(noble.locator(".earlier-turn")).toHaveCount(2);
    await expect(noble.locator(".earlier-turn").nth(1)).toContainText("Mixdown preview rendered: 3 stems.");
    await expect(noble.locator(".earlier-turn time").nth(1)).toHaveText("Sep 29, 21:41");
    await expect(noble.getByRole("button")).toHaveCount(0);
    await noble.locator("summary").click();
  });

  await journey.stage("Open Terminal view from the empty session", async () => {
    await page.locator(".thread .empty").getByRole("button", { name: "Terminal view" }).click();
    await expect(page.locator("#conversation-return")).toBeVisible();
    // The pane header says what it has seen, not "No activity" beside a
    // session that was active two minutes ago (cas-010f).
    for (const stamp of await page.locator(".pane-last-activity").filter({ visible: true }).allTextContents()) expect(stamp).not.toBe("No activity yet");
    await page.locator("#conversation-return").click();
    await expect(page.locator(".thread .empty .said")).toBeVisible();
  });

  await journey.stage("The empty thread follows the connection", async () => {
    // cas-010f: off the network, the card says why nothing new can arrive and
    // stops offering Terminal view; back on, it is the plain live copy again.
    const empty = page.locator(".thread .empty");
    const header = page.locator("#conversation-connection");
    const headerTerminal = page.locator("#conversation-terminal");
    await expect(headerTerminal).not.toHaveAttribute("aria-disabled", "true");
    await hub.down("atlas", { sockets: "close" });
    await expect(header).toContainText("Reconnecting");
    await expect(empty.locator(".said")).toHaveText("No messages from the gabber-studio supervisor in this session yet. Reconnecting to Atlas · Linux — anything new will show here once it's back.");
    await expect(empty.getByRole("button", { name: "Terminal view" })).toHaveCount(0);
    // cas-6b75 (F03): the header doesn't offer the terminal either; it says
    // why, to the eye (dimmed, title) and to a screen reader (description),
    // and pressing it explains instead of opening a terminal it can't reach.
    const unavailable = "Reconnecting to Atlas · Linux — Terminal view opens once it's back.";
    await expect(headerTerminal).toHaveAttribute("aria-disabled", "true");
    await expect(headerTerminal).toHaveAccessibleName("Terminal view");
    await expect(headerTerminal).toHaveAccessibleDescription(unavailable);
    await expect(headerTerminal).toHaveAttribute("title", unavailable);
    await headerTerminal.focus();
    await page.keyboard.press("Enter");
    await expect(page.locator("#toast")).toHaveText(unavailable);
    await expect(page.locator("#conversation-return")).toBeHidden();
    await expect(empty.locator(".said")).toBeVisible();
    await hub.up("atlas");
    await expect(header).toContainText("Live", { timeout: 20_000 });
    await expect(empty.locator(".said")).toHaveText("No messages from the gabber-studio supervisor in this session yet — nothing is waiting on you.");
    await expect(empty.getByRole("button", { name: "Terminal view" })).toBeVisible();
    await expect(headerTerminal).not.toHaveAttribute("aria-disabled", "true");
    await expect(headerTerminal).toHaveAccessibleDescription("");
  });

  await journey.stage("On a phone, Terminal view on the empty card is a full-size target", async () => {
    // cas-6b75 (F01): at 390 the card's Terminal view is at least 44 px each
    // way, the coarse-pointer minimum, and still opens Terminal view.
    const viewport = page.viewportSize()!;
    await page.setViewportSize({ width: 390, height: 844 });
    const open = page.locator(".thread .empty").getByRole("button", { name: "Terminal view" });
    await expect(open).toBeVisible();
    const box = (await open.boundingBox())!;
    expect(box.height, "Terminal view is at least 44 px tall").toBeGreaterThanOrEqual(44);
    expect(box.width, "Terminal view is at least 44 px wide").toBeGreaterThanOrEqual(44);
    await open.click();
    await expect(page.locator("#conversation-return")).toBeVisible();
    await page.locator("#conversation-return").click();
    await expect(page.locator(".thread .empty .said")).toBeVisible();
    await page.setViewportSize(viewport);
  });

  await journey.stage("Each session shows its own conversation", async () => {
    await backToList();
    // cas-010f: a conversation with history never claims, even for a frame,
    // that it has no messages while its first page is on its way.
    await page.evaluate(() => {
      const claims: string[] = [];
      (window as unknown as { emptyClaims: string[] }).emptyClaims = claims;
      // Only wild-shark-68's own card counts: calm-puma-34's honest empty card
      // can still be on screen for a frame while the switch mounts the next thread.
      const check = () => {
        for (const card of document.querySelectorAll<HTMLElement>(".thread .empty:not([hidden])")) {
          if (!card.querySelector(".proj2")?.textContent?.includes("wild-shark-68")) continue;
          const said = card.querySelector(".said")?.textContent ?? "";
          if (/^No (Commander )?messages/.test(said)) claims.push(said);
        }
      };
      new MutationObserver(check).observe(document.body, { subtree: true, childList: true, characterData: true, attributes: true, attributeFilter: ["hidden"] });
    });
    await row("wild-shark-68").click();
    await expect(log).toContainText("Stem export is at 60%.");
    expect(await page.evaluate(() => (window as unknown as { emptyClaims: string[] }).emptyClaims), "no empty claim while history loads").toEqual([]);
    await expect(log).not.toContainText("Mixdown preview rendered");
    await expect(earlier).toBeHidden();
    await backToList();
    // cas-5d2c: the lowest row keeps its place when it opens; the list does
    // not jump back to its top and leave it below the fold.
    await row("noble-cheetah-84").scrollIntoViewIfNeeded();
    await row("noble-cheetah-84").click();
    await expect(row("noble-cheetah-84")).toHaveAttribute("aria-current", "true");
    await expect(row("noble-cheetah-84")).toBeInViewport({ ratio: 1 });
    await expect(log).toContainText("Mixdown preview rendered: 3 stems.");
    // The old daemon's project-wide page: wild-shark-68's turns are beside the thread.
    await expect(log).not.toContainText("Stem export is at 60%.");
    await expect(earlier.locator("summary")).toHaveText([/^Earlier session wild-shark-68, Today/]);
  });

  await journey.stage("End a stale session", async () => {
    await backToList();
    const end = list.locator(".conversation-end").nth(2);
    // cas-f60a: a double-click on End session opens the confirmation and
    // nothing more. Its destructive button is never where the pointer was.
    const ask = end.getByRole("button", { name: "End session noble-cheetah-84 on Atlas" });
    const box = (await ask.boundingBox())!;
    const point = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
    await ask.dblclick();
    await expect(end.locator(".conversation-end-question")).toHaveText("End noble-cheetah-84 on Atlas? Its supervisor and workers stop.");
    expect(hub.ends).toEqual([]);
    expect(await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.closest("button")?.className ?? null, point), "the confirm is not under the pointer").not.toContain("conversation-end-confirm");
    expect(await end.locator("button").allTextContents()).toEqual(["Cancel", "End session"]);
    // cas-d6bf: the last row's confirmation is in view and focused, and the list stays whole.
    await expect(end.getByRole("button", { name: "Cancel" })).toBeFocused();
    await expect(end.getByRole("button", { name: "End session", exact: true })).toBeInViewport({ ratio: 1 });
    await expect(list.locator(".conversation-row")).toHaveCount(3);
    expect(hub.ends).toEqual([]);
    await end.getByRole("button", { name: "End session", exact: true }).click();
    await expect(row("noble-cheetah-84")).toHaveCount(0);
    expect(hub.ends).toEqual([{ machine: "atlas", session: NOBLE, scopes: [...SCOPES, "factory-manage"] }]);
    await expect(list.locator(".conversation-group-head")).toHaveText("gabber-studio · 2 conversations on Atlas");
    // cas-f60a: the list says it ended where the row was, and so does a polite live region.
    await expect(list.locator(".conversation-ended")).toHaveText("noble-cheetah-84 on Atlas ended.");
    await expect(page.locator(".conversation-ended-status[role=status]")).toHaveText("noble-cheetah-84 on Atlas ended.");
  });
});

// cas-f50f: on a phone the list and the thread take turns. Back from a lower
// row's conversation returns the list to where it was, with that row in view
// and focused, not the list's top with focus on its first row.
test.describe("on a phone", () => {
  test.use({ viewport: { width: 390, height: 600 }, hasTouch: true, isMobile: true });
  test("HUB-J14 Back from a lower grouped row returns the list to that row (cas-f50f)", journeyPart, async ({ page, journey }) => {
    // Enough live sessions in one project that a phone's list scrolls.
    const crowded = atlas();
    for (const [index, name] of ["amber-heron-11", "brisk-lynx-22", "copper-wren-33", "dusky-otter-44", "eager-finch-55", "fable-moth-66"].entries()) {
      crowded.sessions!.push(session(`gabber-studio-${name}`, name, journeyStamp(-(5 + index) * 60_000), "supervisor → Commander"));
    }
    await journey.hub({ machines: [crowded], paired: ["atlas"], scopes: { atlas: [...SCOPES, "factory-manage"] } });
    const list = page.getByRole("navigation", { name: "Choose a supervisor" });
    const scroller = page.locator("#conversation-list");
    const row = (codename: string) => list.locator(".conversation-row", { hasText: codename });
    await journey.stage("Open the lowest grouped row from a list scrolled down to it", async () => {
      await journey.open();
      await expect(list.locator(".conversation-row")).toHaveCount(9);
      await row("noble-cheetah-84").scrollIntoViewIfNeeded();
      const scrolled = await scroller.evaluate((node) => node.scrollTop);
      expect(scrolled, "the list scrolls on a phone, so its place matters").toBeGreaterThan(0);
      await row("noble-cheetah-84").tap();
      await expect(page.locator("#conversation-back")).toBeVisible();
    });
    await journey.stage("Back returns to that row, in view and focused", async () => {
      await page.locator("#conversation-back").tap();
      await expect(row("noble-cheetah-84")).toBeFocused();
      await expect(row("noble-cheetah-84")).toBeInViewport({ ratio: 1 });
      expect(await scroller.evaluate((node) => node.scrollTop), "the list did not jump back to its top").toBeGreaterThan(0);
    });
  });
});
