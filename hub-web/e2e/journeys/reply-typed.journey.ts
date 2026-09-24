import { test, expect } from "./journey";
import type { Machine } from "./hub-double";
import { ATLAS, STUDIO, PELICAN } from "./world";

// A machine whose supervisor has a 64-character codename (cas-1334).
const LONG_NAME = "an-extraordinarily-long-supervisor-name-for-truncation-checks-77";
const FORGE: Machine = { id: "forge", label: "Forge · Linux", sessions: [{ name: LONG_NAME, supervisor: LONG_NAME, project_dir: "/projects/forge-tools", workers: ["steady-wren-3"], liveness: "live" }] };

test("HUB-J5 reply by typing", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS, STUDIO, FORGE], paired: ["atlas", "studio", "forge"] });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const composer = page.getByRole("textbox", { name: "Your message" });
  const send = page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true });

  await journey.stage("Open the conversation", async () => {
    await journey.open();
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(send).toBeVisible();
  });

  await journey.stage("Write and send", async () => {
    await composer.fill("Please keep the release notes short this time.");
    const sent = hub.nextSend();
    await send.click();
    expect((await sent).text).toBe("Please keep the release notes short this time.");
    await expect(page.locator('.conversation-turn[data-state="sending"]')).toBeVisible();
    await expect(composer).toHaveValue("");
  });

  let queued = 0;
  await journey.stage("See it delivered", async () => {
    queued = hub.deliverLatest(PELICAN);
    const delivered = page.locator('.conversation-turn[data-state="acknowledged"]');
    await expect(delivered.getByRole("status")).toHaveText("Delivered");
    await expect(page.getByText(`Sending to ${PELICAN}…`)).toBeHidden();
  });

  await journey.stage("See it answered", async () => {
    hub.answerQueued(PELICAN, queued, "Understood — two lines per item, no process talk.");
    await expect(page.getByRole("log").getByText("Understood — two lines per item, no process talk.")).toBeVisible();
    await expect(page.locator('.conversation-turn[data-state="replied"]')).toHaveCount(1);
    await expect(page.locator(".conversation-delivered")).toHaveCount(0);
    // A screen reader hears who spoke and when (cas-17e3), not bare paragraphs.
    await expect(page.getByRole("log")).toMatchAriaSnapshot(`
      - group /^You, \\d{1,2}:\\d{2}/:
        - paragraph: Please keep the release notes short this time.
      - group /^patient-pelican-9, \\d{1,2}:\\d{2}/:
        - paragraph: Understood — two lines per item, no process talk.
    `);
    await expect(page.locator("#conversation-connection")).toMatchAriaSnapshot(`- status: Live`);
    await expect(page.getByRole("button", { name: /Attach a file/ })).toHaveCount(0);
  });

  await journey.stage("A refused message says why", async () => {
    await composer.fill("Ship it without the gate.");
    const refused = hub.nextSend();
    await send.click();
    hub.send(PELICAN, { Error: { client_ref: (await refused).client_ref, message: "forbidden" } });
    const bubble = page.locator('.conversation-turn[data-state="error"]');
    await expect(bubble).toBeVisible();
    await expect(bubble.getByRole("status")).toContainText("This device isn't the one in control of the session.");
    await expect(bubble.getByRole("status")).toContainText("Take control, then retry.");
    // cas-3433: the control the refusal names is on the message itself.
    await expect(bubble.getByRole("button", { name: "Take control of the session", exact: true })).toBeVisible();
    await expect(bubble.getByRole("status")).not.toContainText("forbidden");
    await expect(list.getByText("Not sent: Ship it without the gate.")).toBeVisible();
    // The reason is said once, on the message; the composer only points at it (cas-4d92).
    await expect(page.locator("#message-status")).toHaveText("Not sent — see the message above.");
    await expect(page.getByText("This device isn't the one in control of the session.")).toHaveCount(1);
  });

  await journey.stage("Take control from the message, then retry", async () => {
    // The refusal says "Take control, then retry". The conversation header
    // has no Take control (cas-3433), so the operator follows the instruction
    // on the refused message itself.
    const bubble = page.locator('.conversation-turn[data-state="error"]');
    const leaseRequest = page.waitForRequest((request) => request.method() === "POST" && new URL(request.url()).pathname.endsWith(`/sessions/${PELICAN}/lease`));
    await bubble.getByRole("button", { name: "Take control of the session", exact: true }).click();
    await leaseRequest;
    await expect(page.locator("#message-status")).toHaveText("You control this session now. Retry to send the message.");
    const retried = hub.nextSend();
    await bubble.getByRole("button", { name: "Retry sending", exact: true }).click();
    expect((await retried).text).toBe("Ship it without the gate.");
    await expect(page.locator('.conversation-turn[data-state="sending"]')).toBeVisible();
    await expect(page.locator('.conversation-turn[data-state="error"]')).toHaveCount(0);
    // The hub refuses it again, so the next stage has a refused message to edit.
    hub.send(PELICAN, { Error: { client_ref: (await retried).client_ref, message: "forbidden" } });
    await expect(page.locator('.conversation-turn[data-state="error"]')).toHaveCount(1);
  });

  await journey.stage("Edit and resend retires the refused message", async () => {
    await page.getByRole("button", { name: "Edit message", exact: true }).click();
    await expect(composer).toHaveValue("Ship it without the gate.");
    await composer.fill("Ship it after the gate passes.");
    const resent = hub.nextSend();
    await send.click();
    expect((await resent).text).toBe("Ship it after the gate passes.");
    const retired = page.locator('.conversation-turn[data-state="error"]');
    await expect(retired.getByRole("status")).toHaveText("Not sent · replaced by your edit");
    await expect(page.getByRole("button", { name: "Retry sending" })).toHaveCount(0);
    await expect(list.getByText("You: Ship it after the gate passes.")).toBeVisible();
    await expect(list.getByText(/Not sent: /)).toHaveCount(0);
  });

  await journey.stage("A message the hub never confirms offers Retry", async () => {
    hub.deliverLatest(PELICAN);
    await expect(page.locator('.conversation-turn[data-state="acknowledged"]')).toHaveCount(1);
    await composer.fill("Is the gate green yet?");
    const unreceipted = hub.nextSend();
    await send.click();
    expect((await unreceipted).text).toBe("Is the gate green yet?");
    // No receipt comes; the supervisor talks on, so the receipt is overdue (cas-1622).
    hub.supervisorSays(PELICAN, "Still running the release gate.");
    const bubble = page.locator('.conversation-turn[data-state="unconfirmed"]');
    await expect(bubble.getByRole("status")).toHaveText(`Not confirmed · The hub never confirmed this reached ${PELICAN}. Retry sends it again.`);
    await expect(page.locator('.conversation-turn[data-state="sending"]')).toHaveCount(0);
    await expect(page.getByText(`Sending to ${PELICAN}…`)).toBeHidden();
    const retried = hub.nextSend();
    await bubble.getByRole("button", { name: "Retry sending" }).click();
    expect((await retried).text).toBe("Is the gate green yet?");
    hub.deliverLatest(PELICAN);
    await expect(page.locator(".conversation-delivered")).toHaveText("Delivered");
    await expect(page.locator('.conversation-turn[data-state="unconfirmed"]')).toHaveCount(0);
    await expect(page.getByRole("log").getByText("Is the gate green yet?")).toHaveCount(1);
  });

  await journey.stage("A long supervisor name leaves the message box usable", async () => {
    await list.getByRole("button", { name: /forge-tools/ }).click();
    const longSend = page.getByRole("button", { name: `Send to ${LONG_NAME}`, exact: true });
    await expect(longSend).toBeVisible();
    const [field, button, row] = await Promise.all([composer.boundingBox(), longSend.boundingBox(), page.locator(".conversation-composer").boundingBox()]);
    // The field keeps about its usual share of the row (a normal name leaves
    // it ~40%); the send group takes at most half and its label ellipsises.
    expect(field!.width, "message field width").toBeGreaterThan(row!.width * 0.33);
    expect(button!.x + button!.width, "Send stays inside the composer").toBeLessThanOrEqual(row!.x + row!.width);
    expect(await longSend.locator(".send-label").evaluate((label) => label.scrollWidth > label.clientWidth), "the label ellipsises").toBe(true);
    // The empty-thread card wraps the long name, and nothing scrolls sideways,
    // at desktop, a narrow laptop and a phone (QA F2, rounds 1 and 2).
    const thread = page.locator(".conversation-reading.thread");
    const desktop = page.viewportSize()!;
    for (const width of [desktop.width, 1024, 390]) {
      await page.setViewportSize({ width, height: desktop.height });
      await expect.poll(() => thread.evaluate((element) => element.scrollWidth - element.clientWidth), { message: `thread sideways overflow at ${width}px` }).toBeLessThanOrEqual(1);
    }
    await page.setViewportSize(desktop);
    const title = page.locator(".thread .empty b");
    expect(await title.evaluate((element) => element.scrollWidth <= element.clientWidth + 1), "the empty-thread title is not clipped").toBe(true);
  });
});
