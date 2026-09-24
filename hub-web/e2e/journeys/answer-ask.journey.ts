import { test, expect } from "./journey";
import type { Page } from "@playwright/test";
import { ATLAS, STUDIO, PELICAN, OTTER } from "./world";

/** The operator's latest message group: its label time, and the day separator above it. */
async function lastSend(page: Page): Promise<{ label: string; day: string }> {
  return page.evaluate(() => {
    const groups = [...document.querySelectorAll<HTMLElement>('.msgs [role="group"][aria-label^="You, "]')];
    const group = groups.at(-1)!;
    let day = "";
    for (let node: Element | null = group; node; node = node.previousElementSibling) if (node.classList.contains("day")) { day = node.textContent ?? ""; break; }
    return { label: group.getAttribute("aria-label") ?? "", day };
  });
}
/** "You, HH:MM" for this browser's clock now, or a minute either side if the clock ticks over. */
function youNow(): string[] {
  return [-60_000, 0, 60_000].map((offset) => { const d = new Date(Date.now() + offset); return `You, ${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`; });
}

test("HUB-J7 answer a pinned question", async ({ page, journey }) => {
  // The machine's clock runs five minutes ahead of this browser's, and its
  // durable history holds an open blocker (cas-ce17).
  const ahead = new Date(Date.now() + 300_000).toISOString();
  // The second machine's clock runs a whole day ahead (cas-ac1f).
  const dayAhead = new Date(Date.now() + 86_400_000).toISOString();
  const hub = await journey.hub({
    machines: [ATLAS, STUDIO],
    paired: ["atlas", "studio"],
    history: {
      [PELICAN]: [{ has_earlier: false, messages: [], replies: [{ notification_id: 900, reply_to: null, message: "The release gate went red; the train is held.", summary: "", device_id: "journey-device", kind: "blocker", attachments: [], at: ahead }] }],
      [OTTER]: [{ has_earlier: false, messages: [], replies: [{ notification_id: 901, reply_to: null, message: "Mac build is queued behind the nightly.", summary: "", device_id: "journey-device", kind: "answer", attachments: [], at: dayAhead }] }],
    },
  });
  const waiting = page.locator('[aria-label="Conversation context"] [data-section="waiting"]');
  const pinned = page.getByRole("region", { name: `Waiting on you: question from ${PELICAN}` });
  let ask = 0;

  await journey.stage("Open the conversation", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
  });

  await journey.stage("The supervisor asks a question", async () => {
    ask = hub.supervisorSays(PELICAN, "The gate failed on one clippy warning. Fix it in the train, or ship with it allowlisted?", { kind: "ask", options: ["Fix in-train", "Ship with allowlist"] });
    await expect(pinned).toBeVisible();
    await expect(pinned.getByRole("button", { name: "Fix in-train" })).toBeVisible();
    // The pinned card is the one place to answer (F18): the thread keeps a
    // one-line reference and the rail does not list this question again.
    const reference = page.getByRole("log").locator(".obj.t-a.ask-collapsed");
    await expect(reference.locator(".ask-excerpt")).toHaveText("The gate failed on one clippy warning. Fix it in the train, or ship with it allowlisted?");
    // One line, ellipsised if it must be (cas-97ea).
    const lines = await reference.locator(".ask-excerpt").evaluate((element) => Math.round(element.getBoundingClientRect().height / parseFloat(getComputedStyle(element).lineHeight)));
    expect(lines, "the reference is one line").toBe(1);
    await expect(reference.getByRole("button")).toHaveCount(0);
    // The rail lists only the hydrated blocker (cas-ce17), never the pinned question.
    await expect(waiting.locator("li")).toHaveCount(1);
    await expect(waiting.locator('.context-jump[data-kind="blocker"]')).toHaveCount(1);
    await expect(waiting.locator('.context-jump[data-kind="ask"]')).toHaveCount(0);
  });

  await journey.stage("Answer with one tap", async () => {
    const sent = hub.nextSend();
    await pinned.getByRole("button", { name: "Fix in-train" }).click();
    expect(await sent).toMatchObject({ text: "Fix in-train", in_reply_to: ask });
    await expect(pinned).toBeHidden();
    // The question stays in the thread with the chosen answer and no open choices.
    await expect(page.getByRole("log").getByRole("group", { name: `Question from ${PELICAN}` })).toMatchAriaSnapshot(`
      - group "Question from ${PELICAN}":
        - paragraph: /clippy warning/
        - text: Fix in-train
    `);
    await expect(page.getByRole("button", { name: "Ship with allowlist" })).toHaveCount(0);
    // The answer comes after everything shown, so nothing is left waiting in the rail (cas-ce17).
    await expect(waiting).toBeHidden();
    // It sorts after the machine's future-stamped blocker, but shows the time it was sent, under today (cas-ac1f).
    const sent5 = await lastSend(page);
    expect(youNow()).toContain(sent5.label);
    expect(sent5.day).toBe("Today");
  });

  await journey.stage("See the supervisor act on the answer", async () => {
    hub.answerLatest(PELICAN, "Fixing the warning in the train now.");
    await expect(page.getByRole("log").getByText("Fixing the warning in the train now.")).toBeVisible();
    // A new blocker after the answer does wait: the answer does not reach forward in time.
    hub.supervisorSays(PELICAN, "A second gate went red; the train is still held.", { kind: "blocker" });
    await expect(waiting.locator("li")).toHaveCount(1);
    await expect(waiting).toContainText("A second gate went red");
  });

  await journey.stage("Reply to a machine a day ahead", async () => {
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /gabber-studio/ }).click();
    await expect(page.getByRole("log").getByText("Mac build is queued behind the nightly.")).toBeVisible();
    const composer = page.getByRole("textbox", { name: "Your message" });
    await composer.fill("Thanks — ping me when it starts.");
    const sent = hub.nextSend();
    await page.getByRole("button", { name: `Send to ${OTTER}`, exact: true }).click();
    await sent;
    // The machine's turn sits under its own (tomorrow's) day; the send is under today, at the time it was sent.
    const reply = await lastSend(page);
    expect(youNow()).toContain(reply.label);
    expect(reply.day).toBe("Today");
  });
});
