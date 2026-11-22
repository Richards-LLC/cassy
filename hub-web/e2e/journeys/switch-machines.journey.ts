import { phoneLayout, phoneSwitch, showConversationList } from "./responsive-goals";
import { test, expect, journeyPart } from "./journey";
import { ATLAS, STUDIO, PELICAN, OTTER } from "./world";
import type { Machine } from "./hub-double";
import { journeyStamp } from "./clock";
import type { Page } from "@playwright/test";

const conversationList = (page: Page) => page.getByRole("navigation", { name: "Choose a supervisor" });
// A row is named "<project> on <machine>, …"; the row's own controls are not.
const conversationRow = (page: Page, project: string) => conversationList(page).getByRole("button", { name: new RegExp(`^${project} on `) });
const conversationRows = (page: Page) => conversationList(page).getByRole("button", { name: /^\S+ on / });
const heading = (page: Page) => page.getByTestId("conversation-heading");
const headingTitle = (page: Page) => heading(page).getByRole("heading", { level: 1 });
// The header's host line is titled "<machine> · <codename>".
const hostLine = (page: Page, title: string) => heading(page).getByTitle(title, { exact: true });
const commandPalette = (page: Page) => page.getByRole("dialog", { name: "Commands" });
const jumpCommands = (page: Page) => commandPalette(page).getByRole("button", { name: /^Jump to / });

/**
 * cas-9772: wait for `ready` bounded by protocol events, not wall time. It
 * fails once `events()` has advanced more than `allowance` past where it stood
 * (catalog fetches, heartbeats) with `ready` still false — however slow the
 * host. On a loaded CI runner the fixed 12–30 s budgets here ran out while the
 * page was still on its way; a count of the page's own requests does not.
 * The test timeout remains only as a hang guard.
 */
async function within(what: string, events: () => number, allowance: number, ready: () => Promise<boolean>): Promise<void> {
  const start = events();
  let seen = 0;
  let outcome = "waiting";
  // Poll the whole condition, with no time limit of its own (cas-c4b2).
  await expect.poll(async () => {
    if (await ready()) return (outcome = "ready");
    seen = events() - start;
    return (outcome = seen > allowance ? "over allowance" : "waiting");
  }, { message: what, timeout: 0 }).not.toBe("waiting");
  if (outcome !== "ready") throw new Error(`${what}: still not so after ${seen} protocol events (allowance ${allowance})`);
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
  if (await phoneLayout(page)) { await phoneSwitch(page, journey); return; }
  // Nine stages including a pairing and two reloads. cas-9772: this is a hang
  // guard, not a budget for the work — every wait below is bounded by the
  // page's own requests, so a loaded CI host is no longer a failure.
  test.setTimeout(180_000);
  const hub = await journey.hub({ machines: [ATLAS, STUDIO, ALPHA], paired: ["atlas", "studio"] });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const composer = page.getByRole("textbox", { name: "Your message" });
  // The list stays beside the thread on desktop, so a switch is one click on a row (F17).
  const back = page.getByRole("button", { name: "‹ Conversations", exact: true });

  await journey.stage("Start a draft on the Linux machine", async () => {
    await journey.open();
    // Each machine wears its own accent: the row avatars differ, and the
    // header avatar matches the row that was opened (journey F16).
    const avatar = (project: string) => conversationRow(page, project).getByTestId("conversation-avatar");
    const colour = (locator: ReturnType<typeof avatar>) => locator.evaluate((element) => getComputedStyle(element).backgroundColor);
    expect(await colour(avatar("cas-src"))).not.toBe(await colour(avatar("gabber-studio")));
    await conversationRow(page, "cas-src").click();
    await expect(headingTitle(page)).toHaveText("cas-src");
    expect(await heading(page).getByTestId("conversation-avatar").evaluate((element) => getComputedStyle(element).backgroundColor)).toBe(await colour(avatar("cas-src")));
    await expect(hostLine(page, `Atlas · Linux · ${PELICAN}`)).toContainText(`Atlas · Linux · ${PELICAN}`);
    await composer.fill("Draft: ask about the flaky pairing test");
  });

  await journey.stage("Switch to the Mac and send there", async () => {
    await expect(back).toBeHidden();
    await conversationRow(page, "gabber-studio").click();
    await expect(headingTitle(page)).toHaveText("gabber-studio");
    await expect(hostLine(page, `Studio Mac · macOS · ${OTTER}`)).toContainText(`Studio Mac · macOS · ${OTTER}`);
    await expect(composer).toHaveValue("");
    await composer.fill("Is the Mac build green?");
    const sent = hub.nextSend();
    await page.getByRole("button", { name: "Send to the gabber-studio supervisor", exact: true }).click();
    expect(await sent).toMatchObject({ machine: "studio", target: OTTER, text: "Is the Mac build green?" });
  });

  await journey.stage("Come back to the draft", async () => {
    await conversationRow(page, "cas-src").click();
    await expect(hostLine(page, `Atlas · Linux · ${PELICAN}`)).toBeVisible();
    await expect(composer).toHaveValue("Draft: ask about the flaky pairing test");
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
  });

  await journey.stage("Name the palette chord one way on every surface", async () => {
    // The list search and the Appearance & commands control name the palette
    // chord the same way on this browser, which declares Linux (cas-2a33):
    // Ctrl K, not ⌘K (journey F16). The macOS part of HUB-J3 checks ⌘K.
    const search = page.getByRole("searchbox", { name: "Search conversations" });
    await expect(search).toHaveAttribute("placeholder", /Ctrl K/);
    await expect(page.getByRole("button", { name: "Appearance & commands", exact: true })).toHaveAttribute("title", "Appearance & commands (Ctrl K twice)");
    // The tab names the open conversation too.
    await expect(page).toHaveTitle(`cas-src ${PELICAN} — Cassy Cloud`);
    // Conversations is the only surface (cas-0546): nothing offers a
    // Terminal view, a session picker or a machine rail.
    await expect(page.getByRole("button", { name: /terminal/i })).toHaveCount(0);
    await expect(page.locator("#session-picker, #session-picker-toggle, #machine-rail-list, #fleet-board")).toHaveCount(0);
    // Ctrl K reaches the search, again the palette; Escape closes it and
    // leaves the conversation as it was.
    await composer.focus();
    await page.keyboard.press("ControlOrMeta+k");
    await expect(search).toBeFocused();
    await page.keyboard.press("ControlOrMeta+k");
    const palette = commandPalette(page);
    await expect(palette).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(palette).toBeHidden();
    await expect(headingTitle(page)).toHaveText("cas-src");
    await expect(composer).toHaveValue("Draft: ask about the flaky pairing test");
  });

  await journey.stage("Keyboard focus lands somewhere real on every route", async () => {
    // cas-7eaf: none of these routes leaves focus on <body>.
    // Opening a conversation from the list by Enter or a click lands in its
    // reply box; the list stays beside the thread on a desktop (cas-479a).
    await conversationRow(page, "gabber-studio").focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("button", { name: "Send to the gabber-studio supervisor", exact: true })).toBeVisible();
    await expect(composer).toBeFocused();
    await conversationRow(page, "cas-src").click();
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
    await expect(composer).toBeFocused();
    // Focus the operator moves after the pick is theirs: the landing that
    // waits for the conversation must not pull it back (cas-7eaf QA F01).
    await conversationRow(page, "gabber-studio").click();
    const raw = page.getByRole("button", { name: "Raw output", exact: true });
    await raw.focus();
    // cas-9772: let it attach and a full render round pass, counted in the
    // page's requests, not seconds.
    await hub.waitFor(() => hub.attaches.includes(OTTER));
    const settled = hub.catalogFetchCount("studio");
    await hub.waitFor(() => hub.catalogFetchCount("studio") >= settled + 1);
    await expect(raw).toBeFocused();
    // The palette's Jump by keyboard lands in the reply box too.
    await page.keyboard.press("ControlOrMeta+k");
    await page.keyboard.press("ControlOrMeta+k");
    await page.getByRole("searchbox", { name: "Filter commands" }).fill("cas-src");
    await page.keyboard.press("Enter");
    await expect(commandPalette(page)).toBeHidden();
    await expect(headingTitle(page)).toHaveText("cas-src");
    await expect.poll(() => page.evaluate(() => document.activeElement !== document.body && document.activeElement !== null)).toBe(true);
  });

  await journey.stage("Pair a third machine; the others keep their colours", async () => {
    // cas-50a7: each machine's accent is stored when it first pairs, so a new
    // pairing never re-colours the fleet, and a third machine gets its own.
    await expect(composer).toBeVisible();
    const avatar = (project: string) => conversationRow(page, project).getByTestId("conversation-avatar");
    const colour = (project: string) => avatar(project).evaluate((element) => getComputedStyle(element).backgroundColor);
    const atlas = await colour("cas-src");
    const studio = await colour("gabber-studio");
    await page.goto("about:blank");
    await page.goto(`./#pair=A1pha0xZt1nA4wLr9cYp2KdJ6sHf0uEiMgTxBvNyRaQ&hub=alpha&hub_url=${encodeURIComponent("https://alpha.test")}&machine=${encodeURIComponent("Alpha · Linux")}&scopes=machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt`);
    const dialog = page.getByRole("dialog", { name: /^(?:Pair a machine|Machine authorized)$/ });
    await dialog.getByRole("textbox", { name: /Your name/ }).fill("Daniel");
    await dialog.getByRole("button", { name: "Pair", exact: true }).click();
    await expect(dialog).toBeHidden();
    const back = page.getByRole("button", { name: "‹ Conversations", exact: true });
    if (await back.isVisible().catch(() => false)) await back.click();
    await within("the paired machine's sessions are listed", () => hub.catalogFetchCount("alpha"), 3, () => conversationRow(page, "orion").isVisible());
    expect(await colour("cas-src"), "Atlas keeps its accent").toBe(atlas);
    expect(await colour("gabber-studio"), "Studio keeps its accent").toBe(studio);
    expect(await colour("orion"), "the new machine gets its own accent").not.toBe(atlas);
    expect(await colour("orion")).not.toBe(studio);
    // And after a reload, from storage.
    await page.reload();
    await expect(conversationRow(page, "orion")).toBeVisible();
    // cas-c9a3: rows refill their markup as each machine's catalog lands after
    // the reload (conversation-list.ts sets innerHTML when it changes), so a
    // one-shot read can hit an avatar that was just replaced and read "".
    // The colour is asserted on the avatar as rendered, re-resolved until it is.
    await expect(avatar("cas-src"), "Atlas keeps its accent after a reload").toHaveCSS("background-color", atlas);
    await expect(avatar("gabber-studio"), "Studio keeps its accent after a reload").toHaveCSS("background-color", studio);
    // cas-7752: the draft started on the Linux machine survived the pair link
    // opened in this tab and the reload: it is there when I return to it...
    await conversationRow(page, "cas-src").click();
    await expect(composer).toHaveValue("Draft: ask about the flaky pairing test");
    // ...and once it is sent, it does not come back after another reload.
    const sent = hub.nextSend();
    await page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true }).click();
    expect(await sent).toMatchObject({ machine: "atlas", target: PELICAN, text: "Draft: ask about the flaky pairing test" });
    await expect(composer).toHaveValue("");
    await page.reload();
    await conversationRow(page, "cas-src").click();
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
    await expect(composer).toHaveValue("");
  });

  await journey.stage("Know each conversation and machine by name", async () => {
    // 3.30.0 journey F2/F3: the list, the conversation header and the
    // palette lead with the project; the codename is secondary.
    await conversationRow(page, "cas-src").click();
    await expect(headingTitle(page)).toHaveText("cas-src");
    await expect(hostLine(page, `Atlas · Linux · ${PELICAN}`)).toContainText(`Atlas · Linux · ${PELICAN}`);
    // The header's avatar reads the machine's own name, never a separator.
    await expect(heading(page).getByTestId("conversation-avatar")).toHaveText("A");
    // Each list row's spoken name leads with its project, then its machine.
    for (const [project, machine] of [["cas-src", "Atlas"], ["gabber-studio", "Studio Mac"], ["orion", "Alpha"], ["lighthouse", "Alpha"]]) {
      await expect(list.getByRole("button", { name: new RegExp(`^${project} on ${machine}`) }), `${project} leads its row`).toHaveCount(1);
    }
    // The palette: "Jump to <project>", the codename first on the line beneath
    // (a session summary, when one has arrived, follows the machine).
    await page.getByRole("button", { name: "Appearance & commands" }).click();
    const palette = commandPalette(page);
    await expect(palette).toBeVisible();
    const jump = (project: string) => jumpCommands(page).filter({ hasText: `Jump to ${project}` });
    // The line beneath is the command's only text that does not start "Jump to".
    const beneath = (project: string, line: RegExp) => jump(project).getByText(line);
    await expect(jump("gabber-studio").getByText("Jump to gabber-studio", { exact: true })).toContainText("gabber-studio");
    await expect(beneath("gabber-studio", new RegExp(`^${OTTER} · Studio Mac · macOS`))).toHaveText(new RegExp(`^${OTTER} · Studio Mac · macOS`));
    await expect(beneath("cas-src", new RegExp(`^${PELICAN} · Atlas · Linux`))).toHaveText(new RegExp(`^${PELICAN} · Atlas · Linux`));
    await expect(beneath("orion", /^keen-lynx-1 · Alpha · Linux/)).toHaveText(/^keen-lynx-1 · Alpha · Linux/);
    await page.keyboard.press("Escape");
    await expect(palette).toBeHidden();
  });

  await journey.stage("A supervisor with no workers yet is listed everywhere", async () => {
    // cas-645e: the list and the palette agree. A live supervisor that has
    // not spawned workers is one the operator can talk to; a stale or
    // supervisor-less session is hidden from both.
    await expect(conversationRow(page, "lighthouse")).toBeVisible();
    const listRows = await conversationRows(page).count();
    await page.getByRole("button", { name: "Appearance & commands" }).click();
    const palette = commandPalette(page);
    await expect(palette).toBeVisible();
    const jumps = jumpCommands(page);
    await expect(jumps.filter({ hasText: "Jump to lighthouse" })).toHaveCount(1);
    const jumpSessions = (await jumps.evaluateAll((rows) => rows.map((row) => (row as HTMLElement).dataset.paletteSession ?? ""))).sort();
    await page.keyboard.press("Escape");
    await expect(palette).toBeHidden();
    expect(jumpSessions, "palette Jump rows").toHaveLength(4);
    expect(listRows, "conversation list rows").toBe(jumpSessions.length);
    for (const hidden of ["attic", "nobody"]) await expect(conversationRow(page, hidden)).toHaveCount(0);
  });

  await journey.stage("Hear the open conversation as the page heading", async () => {
    // Journey F19: the goal state (final.aria.yml) names the open
    // conversation in the page heading and the tab title.
    await conversationRow(page, "cas-src").click();
    await expect(page.locator("body")).toMatchAriaSnapshot(`- heading "cas-src" [level=1]`);
    await expect(page).toHaveTitle(`cas-src ${PELICAN} — Cassy Cloud`);
  });
});


// cas-4646: the operator's fleet on the day of the report. One machine runs
// two projects; the other runs two sessions of one project, which the list
// groups ("gabber-studio · 2 conversations on prowl"), by supervisor name (cas-ade2).
const SOUNDWAVE: Machine = {
  id: "soundwave",
  label: "soundwave · Linux",
  sessions: [
    { name: "true-panda-85", supervisor: "true-panda-85", project_dir: "/projects/violet_ps", workers: ["quick-ant-1"], liveness: "live" },
    { name: "keen-fox-5", supervisor: "keen-fox-5", project_dir: "/projects/accounting", workers: ["slow-elk-2"], liveness: "live" },
  ],
};
const PROWL: Machine = {
  id: "prowl",
  label: "prowl · Linux",
  sessions: [
    { name: "jolly-wolf-99", supervisor: "jolly-wolf-99", project_dir: "/projects/gabber-studio", workers: ["able-yak-3"], liveness: "live", last_activity_at: journeyStamp(-3_600_000) },
    { name: "calm-raven-72", supervisor: "calm-raven-72", project_dir: "/projects/gabber-studio", workers: ["deft-owl-4"], liveness: "live", last_activity_at: journeyStamp(-3_700_000) },
  ],
};
// A session that starts and ends on soundwave while the operator taps. Its
// project sorts first, so its card moves every other row (cas-ade2).
const PASSING = { name: "brisk-elk-6", supervisor: "brisk-elk-6", project_dir: "/projects/aardvark", workers: ["tame-gnu-7"], liveness: "live" as const };

for (const width of [1280, 390]) {
  test.describe(`HUB-J8 at ${width}`, () => {
    // cas-ade2: activity no longer reorders rows, so the card that starts and
    // ends above them is what moves a row under the finger. The whole list
    // fits, so a scrolled list cannot clamp or anchor that movement away.
    test.use(width === 390 ? { viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true } : { viewport: { width: 1280, height: 1200 } });
    test(`HUB-J8 every tap opens its conversation while the list re-renders under the finger at ${width} (cas-4646)`, journeyPart, async ({ page, journey }) => {
      test.setTimeout(240_000);
      // Each part streams into its own copy of the fleet.
      const soundwave = structuredClone(SOUNDWAVE);
      const prowl = structuredClone(PROWL);
      const tapped = [...soundwave.sessions, ...prowl.sessions];
      const hub = await journey.hub({ machines: [soundwave, prowl], paired: ["soundwave", "prowl"] });
      await journey.open();
      const list = page.getByRole("navigation", { name: "Choose a supervisor" });
      await expect(conversationRows(page)).toHaveCount(4);
      // Raw input, as a hand gives it: a press and a release with nothing
      // between them but the live updates (no action overlay pauses).
      const input = await page.context().newCDPSession(page);
      let at = { x: 0, y: 0 };
      const press = async (x: number, y: number) => {
        at = { x, y };
        if (width === 390) await input.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ x, y }] });
        else {
          await input.send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
          await input.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", buttons: 1, clickCount: 1 });
        }
      };
      const release = async () => {
        if (width === 390) await input.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
        else await input.send("Input.dispatchMouseEvent", { type: "mouseReleased", ...at, button: "left", buttons: 0, clickCount: 1 });
      };
      // Highlighted within one frame of the release, and that conversation
      // open within about 100 ms.
      const opens = async (key: string, supervisor: string, what: string) => {
        const highlighted = await page.evaluate(() => new Promise<string | undefined>((resolve) => requestAnimationFrame(() =>
          resolve(document.querySelector<HTMLElement>('.conversation-row[aria-current="true"]')?.dataset.threadKey))));
        expect(highlighted, `${what} on ${key}: highlighted`).toBe(key);
        const opened = await page.evaluate((name) => new Promise<number>((resolve) => {
          const start = performance.now();
          const check = () => {
            if (document.querySelector(".conversation-host .codename")?.textContent?.trim() === name) resolve(performance.now() - start);
            else if (performance.now() - start > 2_000) resolve(Infinity);
            else requestAnimationFrame(check);
          };
          check();
        }), supervisor);
        expect(opened, `${what}: ${supervisor} opened`).toBeLessThanOrEqual(150);
      };
      // A row by its spoken name: "<project> on <machine>, <codename>, …".
      const rowOf = (owner: Machine, session: { supervisor: string; project_dir: string }) =>
        list.getByRole("button", { name: new RegExp(`^${session.project_dir.split("/").at(-1)} on ${owner.label.split(" · ")[0]}, ${session.supervisor}(?:,|$)`) });
      let stamp = -3_600_000;
      let last = "";
      await journey.stage("Every tap opens the conversation it pressed, while the list is changing", async () => {
        // Four cycles over the four targets. The cycle fixes which toggle each
        // target sees, so more cycles only repeat the same press/update pairs.
        for (let tap = 0; tap < 16; tap++) {
          // soundwave, prowl, soundwave, prowl: 0, 2, 1, 3, …
          const target = tapped[[0, 2, 1, 3][tap % 4]!]!;
          const owner = soundwave.sessions.includes(target) ? soundwave : prowl;
          const key = `${owner.id}:${target.name}`;
          last = key;
          if (width === 390) await showConversationList(page);
          const row = rowOf(owner, target);
          await row.scrollIntoViewIfNeeded();
          const box = (await row.boundingBox())!;
          const point = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
          const pressed = (await row.elementHandle())!;
          await press(point.x, point.y);
          try {
            // While the finger is down, both kinds of live update land: the
            // other prowl session just did something, which leaves its
            // alphabetical place alone (cas-ade2), and a session starts or ends
            // on soundwave in the first card, which moves every row below it.
            const [, other] = [...prowl.sessions].sort((a, b) => Date.parse(b.last_activity_at!) - Date.parse(a.last_activity_at!));
            stamp += 61_000;
            other!.last_activity_at = journeyStamp(stamp);
            await hub.announceCatalog(prowl.id);
            await expect(list.getByRole("button", { name: /^\S+ on prowl, / }).first(),
              `tap ${tap}: activity never re-sorts the prowl group`).toHaveAttribute("data-thread-key", `${prowl.id}:calm-raven-72`);
            if (soundwave.sessions.some((session) => session.name === PASSING.name && session.liveness === "live")) {
              soundwave.sessions = soundwave.sessions.map((session) => session.name === PASSING.name ? { ...session, liveness: "stale_metadata" as const } : session);
              await hub.announceCatalog(soundwave.id, { removed: [PASSING.name] });
            } else {
              // Insert before both targets: even a render that keeps its keyed
              // nodes must move the soundwave row out from under this press.
              soundwave.sessions = [PASSING, ...soundwave.sessions.filter((session) => session.name !== PASSING.name)];
              await hub.announceCatalog(soundwave.id, { added: [PASSING.name] });
            }
            // A fulfilled catalog request is not a rendered catalog. Observe
            // each toggle before the next tap can toggle it back, otherwise
            // coalesced reads can skip the intervening start/end altogether.
            await expect(rowOf(soundwave, PASSING),
              `tap ${tap}: the catalog rendered the session start/end`).toHaveCount(soundwave.sessions.some(session => session.name === PASSING.name && session.liveness === "live") ? 1 : 0);
            // Compare this press's node, not a sticky flag on reused rows that
            // may have been pressed in an earlier iteration.
            await expect.poll(() => pressed.evaluate((node, { x, y }) => node.ownerDocument.elementFromPoint(x, y)?.closest(".conversation-row") !== node, point),
              { message: `tap ${tap}: the row under the finger was replaced or moved` }).toBe(true);
          } finally {
            await release();
            await pressed.dispose();
          }
          await opens(key, target.supervisor, `tap ${tap}`);
        }
      });
      await journey.stage("A machine that has gone quiet still opens its conversations at once", async () => {
        // The cached threads open without waiting on soundwave's sockets or history.
        await hub.down(soundwave.id);
        for (const target of soundwave.sessions.filter((session) => session.name !== PASSING.name)) {
          const key = `${soundwave.id}:${target.name}`;
          last = key;
          if (width === 390) await showConversationList(page);
          const row = rowOf(soundwave, target);
          await row.scrollIntoViewIfNeeded();
          const box = (await row.boundingBox())!;
          await press(box.x + box.width / 2, box.y + box.height / 2);
          await release();
          await opens(key, target.supervisor, `quiet ${target.name}`);
        }
      });
      // The last tap won: the open conversation and the list agree.
      await expect(heading(page).getByTestId("codename")).toHaveText(last.split(":")[1]!);
      // (On a phone the list is a screen of its own; going back to it leaves the conversation.)
      // getByRole has no aria-current filter; only rows carry aria-current in the list.
      if (width !== 390) await expect(list.locator('[aria-current="true"]')).toHaveAttribute("data-thread-key", last);
    });
  });
}
