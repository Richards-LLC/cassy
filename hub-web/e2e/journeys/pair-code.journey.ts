import { activate, phoneLayout, showConversationList } from "./responsive-goals";
import type { Locator } from "@playwright/test";
import { test, expect, expectWholeFocusRing } from "./journey";
import { ATLAS, PELICAN } from "./world";

/** Every field in the dialog is visible without scrolling it (F4). */
async function everyFieldAboveTheFold(dialog: Locator): Promise<void> {
  const fields = dialog.locator("input:visible");
  for (let index = 0; index < await fields.count(); index += 1) await expect(fields.nth(index)).toBeInViewport({ ratio: 1 });
  expect(await dialog.locator("form, .pair-flow").first().evaluate((scroller: Element) => scroller.scrollTop), "the dialog opens unscrolled").toBe(0);
}

test("HUB-J1 first open and pair a machine with a code", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], relay: { machine: "atlas", claimAfter: 2, authorizeAfter: 4 } });
  const dialog = page.locator("#pair-dialog");
  const phone = await phoneLayout(page);

  await journey.stage("Open Cassy Commander for the first time", async () => {
    await journey.open();
    await expect(page.getByRole("heading", { name: phone ? "Conversations" : "Stay close to the work.", exact: true })).toBeVisible();
    await expect(page.getByText("Pair a machine to start your first conversation.")).toBeVisible();
  });

  await journey.stage("Ask for a pairing code", async () => {
    await activate(page, page.getByRole("button", { name: "Pair a machine" }).filter({ visible: true }));
    await expect(dialog.getByRole("heading", { name: "Pair a machine" })).toBeVisible();
    await expect(dialog.getByText("This browser will be able to:")).toBeVisible();
    await expect(dialog.getByText("Technical details")).toBeVisible();
    // cas-d043 G11: the dialog is named by its heading.
    await expect(page.getByRole("dialog", { name: "Pair a machine", exact: true })).toBeVisible();
    // cas-d043 H15: with nothing scrolled beneath it, the action bar is the
    // sheet itself (its hairline only), never a whiter slab.
    const bar = dialog.locator(".dialog-actions");
    const fieldsScroll = await dialog.locator(".pair-flow").evaluate((flow) => flow.scrollHeight > flow.clientHeight + 1);
    if (!fieldsScroll) expect(await bar.evaluate((node) => getComputedStyle(node).backgroundColor)).toBe("rgba(0, 0, 0, 0)");
    await everyFieldAboveTheFold(dialog);
    // cas-d8a5 (journey F31): every value the countdown shows, from the code
    // to the machine's claim, so it can be checked to only go down.
    await page.evaluate(() => {
      const w = window as unknown as { __countdown: number[] };
      w.__countdown = [];
      const read = () => {
        const text = document.querySelector("#pair-countdown")?.textContent?.trim();
        const match = text ? /^(\d+):(\d{2})$/.exec(text) : null;
        if (!match) return;
        const seconds = Number(match[1]) * 60 + Number(match[2]);
        if (w.__countdown.at(-1) !== seconds) w.__countdown.push(seconds);
      };
      new MutationObserver(read).observe(document.body, { subtree: true, childList: true, characterData: true });
    });
    await activate(page, dialog.getByRole("button", { name: "Create pairing code" }));
    await expect(dialog.getByText("cas hub authorize KQ7M-4XTR")).toBeVisible();
  });

  await journey.stage("Approve on the machine", async () => {
    await expect(dialog.getByRole("heading", { name: "Machine authorized" })).toBeVisible({ timeout: 15_000 });
    await expect(dialog.getByText("Atlas · Linux").first()).toBeVisible();
    await expect(dialog.getByText("Check this is your machine.")).toBeVisible();
    // One heading per step: "Machine authorized" is said once (cas-b2e4 F01).
    await expect(dialog.getByText("Machine authorized")).toHaveCount(1);
    await expect(dialog.getByText("Add your name, then press Pair.")).toBeVisible();
    await expectWholeFocusRing(dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }));
    await expect(dialog.getByText(/device credential/)).toHaveCount(0);
    await everyFieldAboveTheFold(dialog);
    // The claim rebuilt the dialog; its countdown carried on from where it
    // was, never back up to 10:00.
    const shown = await page.evaluate(() => (window as unknown as { __countdown: number[] }).__countdown);
    expect(shown.length, "the countdown was seen").toBeGreaterThan(0);
    expect(shown.every((seconds, index) => index === 0 || seconds <= shown[index - 1]!), `countdown only goes down: ${shown.join(" → ")}`).toBe(true);
  });

  await journey.stage("Confirm and pair this browser", async () => {
    await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Daniel");
    await activate(page, dialog.getByRole("button", { name: "Pair", exact: true }));
    await expect(dialog).toBeHidden();
    // Desktop returns to Pair; phone focuses the reading region without
    // opening the soft keyboard (cas-12c29).
    await expect(page.locator(phone ? ".conversation-reading.thread" : "#pair-toggle")).toBeFocused();
    if (phone) await expect(page.getByRole("textbox", { name: "Your message" })).not.toBeFocused();
    expect(hub.exchanges).toHaveLength(1);
    expect(hub.exchanges[0]).toMatchObject({ hub_id: "atlas", operator_label: "Daniel", token: "journey-invitation" });
  });

  await journey.stage("See the machine's supervisor ready to talk to", async () => {
    await expect(page.getByRole("status").filter({ hasText: "Atlas · Linux connected" })).toBeVisible({ timeout: 15_000 });
    await showConversationList(page);
    const row = page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ });
    await expect(row).toBeVisible();
    // cas-d8a5 (journey F32): the row's spoken name reads as words: no run-on
    // badge, no empty part, no stray " , " before its time.
    const spoken = await row.getAttribute("aria-label");
    expect(spoken).toMatch(/^cas-src on Atlas/);
    expect(spoken).not.toMatch(/ , |,,|\.,/);
    await activate(page, row);
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
    // cas-010f: a supervisor that has not written yet reads plainly. The
    // conversation is the only surface (cas-0546): the card offers no other
    // view, and the header carries Raw output and Interrupt.
    const empty = page.locator(".thread .empty");
    await expect(empty.locator(".said")).toHaveText("No messages from the cas-src supervisor in this session yet — nothing is waiting on you.");
    await expect(empty).not.toContainText("Commander");
    await expect(empty.getByRole("button")).toHaveCount(0);
    await expect(page.getByRole("button", { name: /Terminal view/i })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Raw output", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Interrupt the cas-src supervisor", exact: true })).toBeVisible();
    // The "connected" toast sits at the top, clear of the message box (journey F12).
    // Measure #toast itself: it keeps its box after it fades, so a slow run
    // that outlasts the 3.2 s display cannot hang on the .visible class.
    const toast = page.locator("#toast");
    await expect(toast).toHaveText("Atlas · Linux connected");
    const [notice, composer] = await Promise.all([toast.boundingBox(), page.locator(".conversation-composer").boundingBox()]);
    expect(notice!.y + notice!.height, "toast above the composer").toBeLessThan(composer!.y);
    // It covers no heading either: at the top right it used to land on the
    // context rail's "Tasks & progress" (3.30.0 journey F8).
    const covered = await page.evaluate(() => {
      const t = document.querySelector<HTMLElement>("#toast")!.getBoundingClientRect();
      return [...document.querySelectorAll<HTMLElement>("h1, h2, h3")].filter((h) => h.getClientRects().length > 0).filter((h) => {
        const r = h.getBoundingClientRect();
        return r.width > 0 && t.left < r.right && t.right > r.left && t.top < r.bottom && t.bottom > r.top;
      }).map((h) => h.textContent?.trim());
    });
    expect(covered, "headings under the toast").toEqual([]);
  });
});
