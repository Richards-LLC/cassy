import { test, expect } from "./journey";
import { ATLAS, PELICAN } from "./world";

const at = (hoursAgo: number) => new Date(Date.now() - hoursAgo * 3_600_000).toISOString();
const you = (id: number, text: string, hoursAgo: number) => ({ notification_id: id, target: PELICAN, text, state: "acknowledged", stamped: true, device_id: "journey-device", operator_label: "Daniel", at: at(hoursAgo) });
const sup = (id: number, replyTo: number, message: string, hoursAgo: number, attachments: unknown[] = []) => ({ notification_id: id, reply_to: replyTo, message, summary: "", device_id: "journey-device", kind: "answer", attachments, at: at(hoursAgo) });
const file = (artifact_id: string, name: string) => ({ artifact_id, name, mime: "application/pdf", size_bytes: 88_064, sha256: "9f".repeat(32) });

test("HUB-J4 read the conversation history", async ({ page, journey }) => {
  const hub = await journey.hub({
    machines: [ATLAS],
    paired: ["atlas"],
    history: {
      [PELICAN]: [
        { has_earlier: true, next_before: 20, messages: [you(21, "Is the release ready to cut?", 2)], replies: [sup(22, 21, "Yes. The gate is green on the release branch.", 1.9, [file("art-report", "Release report card.pdf"), file("art-local-draft", "Draft notes.pdf"), file("art-cloud-down", "Gate log.pdf"), file("art-offline", "Bench results.pdf")])] },
        { has_earlier: false, messages: [you(11, "Start the QA epic tomorrow morning.", 30)], replies: [sup(12, 11, "Scheduled for 09:00 with three workers.", 29.9)] },
      ],
    },
  });
  const log = page.getByRole("log");

  await journey.stage("Open the conversation and see the recent turns", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
    await expect(log.getByText("Yes. The gate is green on the release branch.")).toBeVisible();
    await expect(log.getByText("Is the release ready to cut?")).toBeVisible();
  });

  await journey.stage("Load earlier turns", async () => {
    await page.getByRole("button", { name: "Load earlier" }).click();
    await expect(log.getByText("Scheduled for 09:00 with three workers.")).toBeVisible();
    // The pointer now rests on the thread: its surface stays the cream canvas,
    // not brightened to white by the main-action button hover (journey F11).
    await log.hover();
    await expect(page.locator("#pane-grid .pane.primary")).toHaveCSS("filter", "none");
    expect(hub.historyRequests.at(-1)).toMatchObject({ before: 20 });
  });

  await journey.stage("Reach the start of the conversation", async () => {
    await expect(page.getByText("No earlier history")).toBeVisible();
    await expect(page.getByRole("button", { name: "Load earlier" })).toBeHidden();
    await expect(log.getByText("Yesterday")).toBeVisible();
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
    hub.hold(PELICAN);
    hub.drop(PELICAN);
    await expect(header).toContainText("Reconnecting");
    await log.locator('a[data-artifact-id="art-offline"]').click();
    await expect(note("art-offline")).toHaveText("Couldn't reach Atlas · Linux. Check that it's on and connected, then open the file again.");
    hub.release(PELICAN);
    await expect(header).toHaveText(" · Live", { timeout: 30_000 });
    await expect(note("art-offline")).toHaveCount(0);
    await expect(note("art-cloud-down")).toHaveCount(0);
    await expect(note("art-local-draft")).toContainText("only saved on Atlas · Linux");
    expect(page.context().pages()).toHaveLength(1);
  });
});
