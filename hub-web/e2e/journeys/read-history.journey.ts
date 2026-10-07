import { test, expect, journeyPart } from "./journey";
import { journeyDay } from "./clock";
import { ATLAS, PELICAN } from "./world";

const you = (id: number, text: string, at: string) => ({ notification_id: id, target: PELICAN, text, state: "acknowledged", stamped: true, device_id: "journey-device", operator_label: "Daniel", at });
const sup = (id: number, replyTo: number, message: string, at: string, attachments: unknown[] = []) => ({ notification_id: id, reply_to: replyTo, message, summary: "", device_id: "journey-device", kind: "answer", attachments, at });
const file = (artifact_id: string, name: string) => ({ artifact_id, name, mime: "application/pdf", size_bytes: 88_064, sha256: "9f".repeat(32) });

test("HUB-J4 read the conversation history", async ({ page, journey }) => {
  const hub = await journey.hub({
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
  });
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
    await page.evaluate(() => new Promise((ok) => requestAnimationFrame(() => requestAnimationFrame(ok))));
    expect(await atTail(), "late layout growth keeps the thread at its latest turn").toBe(true);
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
    await expect(page.locator("#pane-grid .conversation-thread-slot")).toHaveCSS("filter", "none");
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

  await journey.stage("Open a report the supervisor sent", async () => {
    // cassy#910: the report card opens its hosted copy through a short-lived
    // signed link the machine asks Cloud for, in a new tab on the store's own
    // origin. A file that never reached Cloud says so instead.
    await page.context().route("https://store.test/**", (route) => route.fulfill({ contentType: "text/html", body: "<h1>Release report card</h1>" }));
    const report = log.locator('a[data-artifact-id="art-report"]');
    await report.scrollIntoViewIfNeeded();
    const opened = page.waitForEvent("popup");
    await report.click();
    const tab = await opened;
    await tab.waitForURL(/^https:\/\/store\.test\/view\/art-report\?sig=journey$/);
    await expect(tab.getByRole("heading", { name: "Release report card" })).toBeVisible();
    await tab.close();
    expect(hub.artifactRequests).toEqual(["art-report"]);
    expect(page.url()).not.toContain("#artifact:");

    // Journey F6: every failure is said on the card that was pressed, not in
    // a toast at the top of the thread, and no tab is left open.
    const toast = page.locator("#toast.visible");
    const note = (id: string) => log.locator(`a[data-artifact-id="${id}"] .fnote`);
    const local = log.locator('a[data-artifact-id="art-local-draft"]');
    await local.click();
    await expect(note("art-local-draft")).toHaveText("This file was only saved on Atlas · Linux. It was never uploaded to Cloud, so it can't open here.");
    await expect(local).toHaveAccessibleName(/never uploaded to Cloud/);
    expect(hub.artifactRequests).toEqual(["art-report", "art-local-draft"]);
    // Known now: opening it again opens no tab at all.
    let popups = 0;
    const countPopup = () => { popups += 1; };
    page.on("popup", countPopup);
    await local.click();
    await expect.poll(() => hub.artifactRequests.length).toBe(3);
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
      await expect.poll(() => anchorY(anchor.key), { message: `"${anchor.key}" stays where the reader left it ${what}` }).toBeGreaterThanOrEqual(anchor.y - 3);
      expect(await anchorY(anchor.key)).toBeLessThanOrEqual(anchor.y + 3);
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
    // announcement, through its own role or the thread's log around it.
    await page.waitForTimeout(1_500);
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
    // Past the rebuild: focus is still where the reader put it.
    await page.waitForTimeout(500);
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
      // Past the rebuild and the put-back of the reading position.
      await page.waitForTimeout(650);
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
