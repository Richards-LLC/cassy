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
    await expect(list.getByRole("group", { name: "gabber-studio · 3 conversations on Atlas", exact: true }).locator(".conversation-row")).toHaveCount(3);
    const rows = list.locator(".conversation-row");
    // cas-ade2: rows sort by supervisor name; Most recent marks the newest where it stands.
    await expect(rows.locator(".conversation-supervisor")).toHaveText(["calm-puma-34Most recent", "noble-cheetah-84", "wild-shark-68"]);
    await expect(row("calm-puma-34")).toHaveAttribute("data-most-recent", "true");
    // Each row's time is its own session's last activity, not one shared time.
    await expect(rows.locator(".conversation-when")).toHaveText(["2m", /^\d+h$/, "40m"]);
    await expect(row("calm-puma-34").locator(".conversation-when")).toHaveAttribute("title", "Last activity 2m ago · Messaged bright-robin-85");
    // cas-5d2c: under a heading that names the project and machine once, each
    // row leads with its codename and says what its session last did, before
    // any of them is opened; the heading and footer count the same noun.
    await expect(rows.locator(".conversation-title")).toHaveCount(3);
    for (const title of await rows.locator(".conversation-title").all()) await expect(title).toBeHidden();
    await expect(rows.locator(".conversation-preview")).toHaveText(["Messaged bright-robin-85", "Mixdown preview rendered: 3 stems.", "You wrote to it"]);
    await expect(row("noble-cheetah-84").getByLabel("1 unread")).toBeVisible();
    await expect(page.locator(".hub-footer-meta")).toContainText("3 conversations");
    // A useful incoming preview can grow the last row beyond the list's
    // available height. Keep it reachable in the list's own scroll region.
    const incoming = rows.last();
    await incoming.scrollIntoViewIfNeeded();
    await expect(incoming).toBeInViewport({ ratio: 1 });
  });

  await journey.stage("Open a session that has not written yet", async () => {
    await row("calm-puma-34").click();
    const empty = page.locator(".thread .empty");
    // cas-010f: plain words, no product codename or queue jargon. The card
    // offers no way out of the conversation (cas-0546): the header carries
    // Raw output and Interrupt.
    await expect(empty.locator(".said")).toHaveText("No messages from the gabber-studio supervisor in this session yet — nothing is waiting on you.");
    await expect(empty.locator(".empty-activity")).toHaveText("Last active 2m ago");
    await expect(empty).not.toContainText("Commander");
    await expect(empty).not.toContainText("→");
    await expect(empty.getByRole("button")).toHaveCount(0);
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

  await journey.stage("Read the empty session's raw output", async () => {
    // The session has not written to the operator, but its supervisor is
    // running: Raw output shows what its terminal shows (cas-0546).
    const raw = page.locator("#conversation-raw-output");
    await raw.click();
    const drawer = page.getByRole("dialog", { name: "Raw output" });
    await expect(drawer).toBeVisible();
    await expect(drawer).toContainText("The supervisor is ready.");
    await page.keyboard.press("Escape");
    await expect(drawer).toBeHidden();
    await expect(raw).toBeFocused();
    await expect(page.locator(".thread .empty .said")).toBeVisible();
  });

  await journey.stage("The empty thread follows the connection", async () => {
    // cas-010f: off the network, the card says why nothing new can arrive;
    // back on, it is the plain live copy again.
    const empty = page.locator(".thread .empty");
    const header = page.locator("#conversation-connection");
    // cas-0546: the header's Raw output and Interrupt stay in place and say
    // why they wait, to the eye (dashed, title) and to a screen reader
    // (description); pressing one explains instead of acting.
    const raw = page.locator("#conversation-raw-output");
    const interrupt = page.locator("#conversation-interrupt");
    for (const action of [raw, interrupt]) await expect(action).not.toHaveAttribute("aria-disabled", "true");
    await hub.down("atlas", { sockets: "close" });
    await expect(header).toContainText("Reconnecting");
    await expect(empty.locator(".said")).toHaveText("No messages from the gabber-studio supervisor in this session yet. Reconnecting to Atlas · Linux — anything new will show here once it's back.");
    await expect(empty.getByRole("button")).toHaveCount(0);
    const unavailable = "Lost connection to Atlas · Linux. Interrupt and raw output return when it reconnects.";
    for (const action of [raw, interrupt]) {
      await expect(action).toHaveAttribute("aria-disabled", "true");
      await expect(action).toHaveAccessibleDescription(unavailable);
      await expect(action).toHaveAttribute("title", unavailable);
    }
    await expect(raw).toHaveAccessibleName("Raw output");
    await expect(interrupt).toHaveAccessibleName("Interrupt the gabber-studio supervisor");
    await raw.focus();
    await page.keyboard.press("Enter");
    await expect(page.locator("#toast")).toHaveText(unavailable);
    await expect(page.getByRole("dialog", { name: "Raw output" })).toBeHidden();
    await expect(empty.locator(".said")).toBeVisible();
    await hub.up("atlas");
    await expect(header).toContainText("Live", { timeout: 20_000 });
    await expect(empty.locator(".said")).toHaveText("No messages from the gabber-studio supervisor in this session yet — nothing is waiting on you.");
    for (const action of [raw, interrupt]) {
      await expect(action).not.toHaveAttribute("aria-disabled", "true");
      await expect(action).toHaveAccessibleDescription("");
    }
  });

  await journey.stage("On a phone, the header's Raw output and Interrupt are full-size targets", async () => {
    // cas-6b75 (F01), carried to the header actions (cas-0546): at 390 each
    // is at least 44 px each way, the coarse-pointer minimum, and Raw output
    // still opens.
    const viewport = page.viewportSize()!;
    await page.setViewportSize({ width: 390, height: 844 });
    for (const action of [page.locator("#conversation-raw-output"), page.locator("#conversation-interrupt")]) {
      await expect(action).toBeVisible();
      const box = (await action.boundingBox())!;
      expect(box.height, "at least 44 px tall").toBeGreaterThanOrEqual(44);
      expect(box.width, "at least 44 px wide").toBeGreaterThanOrEqual(44);
    }
    await page.locator("#conversation-raw-output").click();
    const drawer = page.getByRole("dialog", { name: "Raw output" });
    await expect(drawer).toBeVisible();
    await drawer.getByRole("button", { name: "Close raw output" }).click();
    await expect(drawer).toBeHidden();
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
    // cas-5d2c: a lower row keeps its place when it opens; the list does
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
    // cas-ade2: noble-cheetah-84 is the second row by supervisor name.
    const end = list.locator(".conversation-end").nth(1);
    // cas-f60a: a double-click on End session opens the confirmation and
    // nothing more. Its destructive button is never where the pointer was.
    const ask = end.getByRole("button", { name: "End session noble-cheetah-84 on Atlas" });
    const box = (await ask.boundingBox())!;
    const point = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
    await ask.dblclick();
    await expect(end.locator(".conversation-end-question")).toHaveText("End noble-cheetah-84 on Atlas? Its supervisor and workers stop.");
    expect(hub.ends).toEqual([]);
    // cas-488f: ask the hit test a yes/no question. Whatever is under the
    // pointer (Cancel, the question, or nothing), it must not be End session.
    const underPointer = await page.evaluate(({ x, y }) => {
      const hit = document.elementFromPoint(x, y);
      return { hit: hit ? `${hit.tagName.toLowerCase()}.${hit.className}` : "nothing", confirm: Boolean(hit?.closest(".conversation-end-confirm")) };
    }, point);
    expect(underPointer.confirm, `the confirm is not under the pointer (hit: ${underPointer.hit})`).toBe(false);
    expect(await end.locator("button").allTextContents()).toEqual(["Cancel", "End session"]);
    // cas-d6bf: a lower row's confirmation is in view and focused, and the list stays whole.
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

test("HUB-J14 a project group ends before the next project's conversation (cas-6024)", journeyPart, async ({ page, journey }) => {
  const machine = atlas();
  machine.sessions = [...machine.sessions!.slice(0, 2), {
    name: "pulse-card-solo", supervisor: "solo-otter-12", project_dir: "/projects/pulse-card",
    workers: [], liveness: "live", last_activity_at: journeyStamp(-60_000), last_activity: "supervisor → Commander",
  }];
  await journey.hub({ machines: [machine], paired: ["atlas"] });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  await journey.stage("A bounded project sits above an independent project on desktop and phone", async () => {
    await journey.open();
    for (const width of [1280, 390]) for (const scheme of ["light", "dark", "forced-colors"] as const) {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 800 });
      await page.emulateMedia({ colorScheme: scheme === "dark" ? "dark" : "light", forcedColors: scheme === "forced-colors" ? "active" : "none" });
      await expect(list.locator(".project-card-name").first()).toBeVisible();
      await expect(list.locator(".conversation-row")).toHaveCount(3);
      if (process.env.GROUP_BOUNDARY_CAPTURE_DIR) await page.screenshot({ path: `${process.env.GROUP_BOUNDARY_CAPTURE_DIR}/${width}-${scheme}.png` });
    }
    const group = list.getByRole("group", { name: "gabber-studio · 2 conversations on Atlas", exact: true });
    await expect(group.locator(".conversation-row")).toHaveCount(2);
    await expect(list.getByRole("group", { name: "pulse-card · 1 conversation on Atlas", exact: true }).locator(".conversation-row")).toHaveCount(1);
    await expect(group.locator(".conversation-session-mark")).toHaveText("Most recent");
    await group.locator(".conversation-row").first().focus();
    await page.keyboard.press("Tab");
    await expect(group.locator(".conversation-row").last()).toBeFocused();
    const singleton = list.getByRole("group", { name: "pulse-card · 1 conversation on Atlas", exact: true });
    await singleton.locator(".project-card-toggle").focus();
    await page.keyboard.press("Enter");
    await expect(singleton.locator(".project-card-toggle")).toHaveAttribute("aria-expanded", "false");
    await page.keyboard.press("Enter");
    await singleton.locator(".conversation-row").focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("button", { name: "Send to the pulse-card supervisor", exact: true })).toBeVisible();
    await expect(page.locator('.conversation-row[aria-current="true"]')).toHaveCount(1);
  });
});

test("HUB-J14 project navigation spans machines, decisions and device collapse (cas-6024)", journeyPart, async ({ page, journey }) => {
  const machine = atlas();
  const ended = (name: string, offset: number) => {
    const at = journeyStamp(offset);
    return { name, supervisor: name, project: "pulse-card", project_dir: "/projects/pulse-card", machine_online: false, workers: [], liveness: "stale_metadata" as const, last_activity: "Session ended", last_activity_at: at, ended: { at, reason: "prompt_input_exit", workers_stopped: [], orphans: [] } };
  };
  machine.sessions = [{ ...machine.sessions[1]!, project: "github.com/acme/studio", project_name: "Studio", harness: "codex", account_email: "operator@example.test", machine_online: true, workers: ["worker-one"], last_activity: "supervisor → Commander", last_activity_at: journeyStamp(-30_000) }, {
    name: "golden-jaguar-44", supervisor: "golden-jaguar-44", project: "pulse-card", project_dir: "/projects/pulse-card", machine_online: true, workers: [], liveness: "live", last_activity: "supervisor → Commander", last_activity_at: journeyStamp(-30_000),
  }, ended("watchful-heron-16", -3_600_000), ended("solid-parrot-40", -2 * 3_600_000), ended("patient-heron-27", -3 * 3_600_000)];
  const other: Machine = { id: "borealis", label: "Borealis · macOS", sessions: [{ ...session(SHARK, "wild-shark-68", journeyStamp(-60_000), "supervisor → Commander"), project_dir: "/other/studio", project: "github.com/acme/studio", project_name: "Studio", harness: "claude", machine_online: true }] };
  const hub = await journey.hub({ machines: [machine, other], paired: ["atlas", "borealis"], history: {
    [PUMA]: [{ has_earlier: false, messages: [], replies: [sup(701, "Which mix should I export?", PUMA, journeyStamp(-30_000), "ask")] }],
  } });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const filters = page.locator("#conversation-filters");
  const studio = () => list.getByRole("group", { name: /^Studio ·/ });
  const back = async () => { if (await page.locator("#conversation-back").isVisible()) await page.locator("#conversation-back").click(); };

  await journey.stage("A project contains supervisors on both machines and pins its decision", async () => {
    await journey.open();
    await expect(studio().locator(".conversation-row")).toHaveCount(2);
    // cas-6f79: the card reads the project's display name; its identity stays a detail.
    await expect(studio().locator(".project-card-name")).toHaveText("Studio");
    await expect(studio().locator(".project-card-name")).toHaveAttribute("title", "github.com/acme/studio");
    await expect(studio().getByRole("button", { name: "Filter conversations on Atlas, online", exact: true })).toBeVisible();
    await expect(studio().getByRole("button", { name: "Filter conversations on Borealis, online", exact: true })).toBeVisible();
    await expect(studio().locator(".conversation-account")).toHaveText("operator@example.test");
    // DESIGN.md Avatars: each provider's mark at a legible 24px, not a glyph.
    for (const [harness, provider] of [["codex", "OpenAI"], ["claude", "Claude"]]) {
      await expect(studio().locator(`[data-harness="${harness}"]`)).toHaveAttribute("title", provider);
      expect((await studio().locator(`[data-harness="${harness}"] svg.provider-mark`).boundingBox())?.width).toBeGreaterThanOrEqual(20);
    }
    await expect(studio().locator(".project-card-summary")).toHaveText("1 live worker");
    const old = list.getByRole("group", { name: /^pulse-card ·/ });
    await expect(old.locator(".conversation-account")).toHaveCount(0);
    await expect(old.locator("[data-harness]")).toHaveCount(0);
    await expect(old.locator(".project-card-summary")).toBeHidden();
    const pulseMachine = old.getByRole("button", { name: "Filter conversations on Atlas, online", exact: true });
    await expect(pulseMachine).toBeVisible();
    expect(await pulseMachine.evaluate((chip) => getComputedStyle(chip).borderRadius)).toBe(await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--radius-pill").trim()));
    const liveSupervisor = old.locator(".conversation-row").filter({ hasText: "golden-jaguar-44" });
    await expect(liveSupervisor.locator(".supervisor-state")).not.toHaveClass(/offline/);
    const ended = old.locator(".conversation-row").filter({ hasText: "watchful-heron-16" });
    await expect(ended.locator(".conversation-preview")).toHaveText("Ended");
    await expect(ended.locator(".conversation-when")).toBeVisible();
    await expect(old.getByRole("button", { name: "Dismiss ended session golden-jaguar-44", exact: true })).toHaveCount(0);
    const clearEnded = old.getByRole("button", { name: "Clear ended sessions in pulse-card", exact: true });
    await clearEnded.scrollIntoViewIfNeeded();
    await expect(clearEnded).toBeInViewport();
  });

  await journey.stage("An ended row offers Dismiss, while the live row stays protected", async () => {
    const old = list.getByRole("group", { name: /^pulse-card ·/ });
    const liveSupervisor = old.locator(".conversation-row").filter({ hasText: "golden-jaguar-44" });
    const ended = old.locator(".conversation-row").filter({ hasText: "watchful-heron-16" });
    await ended.scrollIntoViewIfNeeded();
    await expect(old.getByRole("button", { name: "Dismiss ended session watchful-heron-16", exact: true })).toBeVisible();
    await expect(old.getByRole("button", { name: "Dismiss ended session golden-jaguar-44", exact: true })).toHaveCount(0);
    await expect(liveSupervisor).toBeVisible();
  });

  await journey.stage("Dismiss and clear ended sessions without deleting their history", async () => {
    const old = list.getByRole("group", { name: /^pulse-card ·/ });
    const liveSupervisor = old.locator(".conversation-row").filter({ hasText: "golden-jaguar-44" });
    const ended = old.locator(".conversation-row").filter({ hasText: "watchful-heron-16" });
    await old.getByRole("button", { name: "Dismiss ended session watchful-heron-16", exact: true }).click();
    await expect(ended).toHaveCount(0);
    await expect(old.locator(".conversation-row").filter({ hasText: "solid-parrot-40" })).toHaveCount(1);
    expect(hub.machine("atlas").sessions.filter((session) => session.ended)).toHaveLength(3);
    await old.getByRole("button", { name: "Clear ended sessions in pulse-card", exact: true }).click();
    await expect(old.locator(".conversation-row")).toHaveCount(1);
    await expect(old.locator(".conversation-topline .conversation-action")).toBeHidden();
    await expect(liveSupervisor).toBeVisible();
    expect(hub.machine("atlas").sessions.filter((session) => session.ended)).toHaveLength(3);
    await page.reload();
    await expect(old.locator(".conversation-row")).toHaveCount(1);
    await expect(old.locator(".conversation-row").filter({ hasText: "solid-parrot-40" })).toHaveCount(0);
    await expect(list.getByRole("region", { name: "Needs you", exact: true }).getByRole("button")).toContainText("calm-puma-34");
    const capture = process.env.FULL_NAV_CAPTURE_DIR;
    const widths = capture ? [1280, 390] : [390];
    const schemes = capture ? ["light", "dark", "forced-colors"] as const : ["forced-colors"] as const;
    for (const width of widths) for (const scheme of schemes) {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 800 });
      await page.emulateMedia({ colorScheme: scheme === "dark" ? "dark" : "light", forcedColors: scheme === "forced-colors" ? "active" : "none", reducedMotion: "reduce" });
      expect(await page.evaluate(() => matchMedia("(prefers-reduced-motion: reduce)").matches)).toBe(true);
      await expect(studio().locator(".project-card-toggle")).toBeVisible();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      if (process.env.FULL_NAV_CAPTURE_DIR) await page.screenshot({ path: `${process.env.FULL_NAV_CAPTURE_DIR}/${width}-${scheme}.png` });
    }
    await page.emulateMedia({ colorScheme: "light", forcedColors: "none", contrast: "more" });
    expect(await page.evaluate(() => matchMedia("(prefers-contrast: more)").matches)).toBe(true);
    if (process.env.FULL_NAV_CAPTURE_DIR) await page.screenshot({ path: `${process.env.FULL_NAV_CAPTURE_DIR}/a11y-contrast-more.png` });
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.emulateMedia({ colorScheme: "light", forcedColors: "none", contrast: "no-preference" });
  });

  await journey.stage("Filters intersect search, and a decision opens its own session", async () => {
    await filters.getByRole("button", { name: "Needs you", exact: true }).click();
    await expect(list.locator(".conversation-row")).toHaveCount(1);
    await filters.getByRole("button", { name: "Borealis", exact: true }).click();
    await expect(list.locator(".conversation-row")).toHaveCount(1);
    await expect(list.locator(".conversation-row")).toContainText("wild-shark-68");
    await page.getByRole("searchbox", { name: "Search conversations" }).fill("calm-puma");
    await expect(list.locator(".conversation-row")).toHaveCount(0);
    await expect(list.getByText("No conversations match this filter. Choose All to see your projects.")).toBeVisible();
    await filters.getByRole("button", { name: "All", exact: true }).click();
    await expect(list.locator(".conversation-row")).toHaveCount(1);
    await list.getByRole("region", { name: "Needs you", exact: true }).getByRole("button").click();
    await expect(page.getByRole("log")).toContainText("Which mix should I export?");
    await back();
    expect(hub.sends).toHaveLength(0);
  });

  await journey.stage("Keyboard collapse survives reload and footer filters the same register", async () => {
    const toggle = studio().locator(".project-card-toggle");
    await toggle.focus();
    await page.keyboard.press("Enter");
    await expect(toggle).toBeFocused();
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await page.reload();
    await expect(studio().locator(".project-card-toggle")).toHaveAttribute("aria-expanded", "false");
    await expect(studio().locator(".conversation-row").first()).toBeHidden();
    await studio().locator(".project-card-toggle").click();
    await page.locator("#paired-machines-toggle").click();
    const dialog = page.locator("#paired-machines-dialog");
    await dialog.getByRole("group", { name: "Filter conversations" }).getByRole("button", { name: "Borealis", exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(filters.getByRole("button", { name: "Borealis", exact: true })).toHaveAttribute("aria-pressed", "true");
    await expect(list.locator(".conversation-row")).toHaveCount(1);
    await expect(list.locator(".conversation-row")).toContainText("wild-shark-68");
    await expect(list.getByRole("region", { name: "Needs you", exact: true })).toHaveCount(0);
    await filters.getByRole("button", { name: "All", exact: true }).click();
    await expect(list.locator(".conversation-row")).toHaveCount(3);
    // A kept catalog's positive online bit must yield to a lost connection.
    await hub.down("atlas", { sockets: "close" });
    await expect(studio().getByRole("button", { name: "Filter conversations on Atlas, offline", exact: true })).toBeVisible();
    await hub.up("atlas");
    await expect(studio().getByRole("button", { name: "Filter conversations on Atlas, online", exact: true })).toBeVisible();
  });
});
