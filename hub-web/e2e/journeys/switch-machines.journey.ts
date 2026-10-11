import { phoneLayout, phoneSwitch, showConversationList } from "./responsive-goals";
import type { Page } from "@playwright/test";
import { test, expect, journeyPart } from "./journey";
import { ATLAS, STUDIO, PELICAN, OTTER } from "./world";
import type { Machine } from "./hub-double";
import { journeyStamp } from "./clock";

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

  await journey.stage("Name the palette chord one way on every surface", async () => {
    // The list search and the Appearance & commands control name the palette
    // chord the same way on this browser, which declares Linux (cas-2a33):
    // Ctrl K, not ⌘K (journey F16). The macOS part of HUB-J3 checks ⌘K.
    const search = page.getByRole("searchbox", { name: "Search conversations" });
    await expect(search).toHaveAttribute("placeholder", "Search conversations (Ctrl K)");
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
    const palette = page.locator("#command-palette");
    await expect(palette).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(palette).toBeHidden();
    await expect(page.locator(".conversation-identity h1")).toHaveText("cas-src");
    await expect(composer).toHaveValue("Draft: ask about the flaky pairing test");
  });

  await journey.stage("Keyboard focus lands somewhere real on every route", async () => {
    // cas-7eaf: none of these routes leaves focus on <body>.
    // Opening a conversation from the list by Enter or a click lands in its
    // reply box; the list stays beside the thread on a desktop (cas-479a).
    await list.getByRole("button", { name: /gabber-studio/ }).focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("button", { name: "Send to the gabber-studio supervisor", exact: true })).toBeVisible();
    await expect(composer).toBeFocused();
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
    await expect(composer).toBeFocused();
    // Focus the operator moves after the pick is theirs: the landing that
    // waits for the conversation must not pull it back (cas-7eaf QA F01).
    await list.getByRole("button", { name: /gabber-studio/ }).click();
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
    await expect(page.locator("#command-palette")).toBeHidden();
    await expect(page.locator(".conversation-identity h1")).toHaveText("cas-src");
    await expect.poll(() => page.evaluate(() => document.activeElement !== document.body && document.activeElement !== null)).toBe(true);
  });

  await journey.stage("Pair a third machine; the others keep their colours", async () => {
    // cas-50a7: each machine's accent is stored when it first pairs, so a new
    // pairing never re-colours the fleet, and a third machine gets its own.
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

  await journey.stage("Know each conversation and machine by name", async () => {
    // 3.30.0 journey F2/F3: the list, the conversation header and the
    // palette lead with the project; the codename is secondary.
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(page.locator(".conversation-identity h1")).toHaveText("cas-src");
    await expect(page.locator(".conversation-host")).toContainText(`Atlas · Linux · ${PELICAN}`);
    // The header's avatar reads the machine's own name, never a separator.
    await expect(page.locator(".conversation-identity .conversation-avatar")).toHaveText("A");
    // Each list row's spoken name leads with its project, then its machine.
    for (const [project, machine] of [["cas-src", "Atlas"], ["gabber-studio", "Studio Mac"], ["orion", "Alpha"], ["lighthouse", "Alpha"]]) {
      await expect(list.getByRole("button", { name: new RegExp(`^${project} on ${machine}`) }), `${project} leads its row`).toHaveCount(1);
    }
    // The palette: "Jump to <project>", the codename first on the line beneath
    // (a session summary, when one has arrived, follows the machine).
    await page.getByRole("button", { name: "Appearance & commands" }).click();
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
    // cas-645e: the list and the palette agree. A live supervisor that has
    // not spawned workers is one the operator can talk to; a stale or
    // supervisor-less session is hidden from both.
    await expect(list.getByRole("button", { name: /lighthouse/ })).toBeVisible();
    const listRows = await list.locator(".conversation-row").count();
    await page.getByRole("button", { name: "Appearance & commands" }).click();
    const palette = page.locator("#command-palette");
    await expect(palette).toBeVisible();
    const jumps = palette.locator(".palette-command[data-palette-session]");
    await expect(jumps.filter({ hasText: "Jump to lighthouse" })).toHaveCount(1);
    const jumpSessions = (await jumps.evaluateAll((rows) => rows.map((row) => (row as HTMLElement).dataset.paletteSession ?? ""))).sort();
    await page.keyboard.press("Escape");
    await expect(palette).toBeHidden();
    expect(jumpSessions, "palette Jump rows").toHaveLength(4);
    expect(listRows, "conversation list rows").toBe(jumpSessions.length);
    for (const hidden of [/attic/, /nobody/]) await expect(list.getByRole("button", { name: hidden })).toHaveCount(0);
  });

  await journey.stage("Hear the open conversation as the page heading", async () => {
    // Journey F19: the goal state (final.aria.yml) names the open
    // conversation in the page heading and the tab title.
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(page.locator("body")).toMatchAriaSnapshot(`- heading "cas-src" [level=1]`);
    await expect(page).toHaveTitle(`cas-src ${PELICAN} — Cassy Cloud`);
    expect(await page.locator("body").ariaSnapshot()).not.toContain("Switch session");
  });
});


// cas-4646: the operator's fleet on the day of the report. One machine runs
// two projects; the other runs two sessions of one project, which the list
// groups ("gabber-studio · 2 conversations on prowl") with the most recent first.
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
// A session that starts and ends on soundwave while the operator taps.
const PASSING = { name: "brisk-elk-6", supervisor: "brisk-elk-6", project_dir: "/projects/scratch", workers: ["tame-gnu-7"], liveness: "live" as const };

for (const width of [1280, 390]) {
  test.describe(`HUB-J8 at ${width}`, () => {
    test.use(width === 390 ? { viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true } : { viewport: { width: 1280, height: 800 } });
    test(`HUB-J8 every tap opens its conversation while the list re-renders under the finger at ${width} (cas-4646)`, journeyPart, async ({ page, journey }) => {
      test.setTimeout(240_000);
      // Each part streams into its own copy of the fleet.
      const soundwave = structuredClone(SOUNDWAVE);
      const prowl = structuredClone(PROWL);
      const tapped = [...soundwave.sessions, ...prowl.sessions];
      const hub = await journey.hub({ machines: [soundwave, prowl], paired: ["soundwave", "prowl"] });
      await journey.open();
      const list = page.getByRole("navigation", { name: "Choose a supervisor" });
      await expect(list.locator(".conversation-row")).toHaveCount(4);
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
      let stamp = -3_600_000;
      let last = "";
      await journey.stage("Every tap opens the conversation it pressed, while the list is changing", async () => {
        for (let tap = 0; tap < 50; tap++) {
          // soundwave, prowl, soundwave, prowl: 0, 2, 1, 3, …
          const target = tapped[[0, 2, 1, 3][tap % 4]!]!;
          const owner = soundwave.sessions.includes(target) ? soundwave : prowl;
          const key = `${owner.id}:${target.name}`;
          last = key;
          if (width === 390) await showConversationList(page);
          const row = list.locator(`.conversation-row[data-thread-key="${key}"]`);
          await row.scrollIntoViewIfNeeded();
          const box = (await row.boundingBox())!;
          const point = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
          await row.evaluate((node) => { (node as HTMLElement & { pressed?: boolean }).pressed = true; });
          await press(point.x, point.y);
          // While the finger is down, both kinds of live update land: the
          // prowl pair trades places (the other one just did something), and
          // a session starts or ends on soundwave, which rebuilds the list.
          const [, other] = [...prowl.sessions].sort((a, b) => Date.parse(b.last_activity_at!) - Date.parse(a.last_activity_at!));
          stamp += 61_000;
          other!.last_activity_at = journeyStamp(stamp);
          await hub.announceCatalog(prowl.id);
          if (soundwave.sessions.some((session) => session.name === PASSING.name)) {
            soundwave.sessions = soundwave.sessions.filter((session) => session.name !== PASSING.name);
            await hub.announceCatalog(soundwave.id, { removed: [PASSING.name] });
          } else {
            soundwave.sessions = [...soundwave.sessions, PASSING];
            await hub.announceCatalog(soundwave.id, { added: [PASSING.name] });
          }
          await expect.poll(() => page.evaluate(({ x, y }) => (document.elementFromPoint(x, y)?.closest(".conversation-row") as HTMLElement & { pressed?: boolean } | null)?.pressed !== true, point),
            { message: `tap ${tap}: the row under the finger was replaced or moved` }).toBe(true);
          await release();
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
          const row = list.locator(`.conversation-row[data-thread-key="${key}"]`);
          await row.scrollIntoViewIfNeeded();
          const box = (await row.boundingBox())!;
          await press(box.x + box.width / 2, box.y + box.height / 2);
          await release();
          await opens(key, target.supervisor, `quiet ${target.name}`);
        }
      });
      // The last tap won: the open conversation and the list agree.
      await expect(page.locator(".conversation-host .codename")).toHaveText(last.split(":")[1]!);
      // (On a phone the list is a screen of its own; going back to it leaves the conversation.)
      if (width !== 390) await expect(list.locator('.conversation-row[aria-current="true"]')).toHaveAttribute("data-thread-key", last);
    });
  });
}
