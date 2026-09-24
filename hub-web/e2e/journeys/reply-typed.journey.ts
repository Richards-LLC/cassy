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

  await journey.stage("A refused Take control keeps focus on the message", async () => {
    // Another device holds the session, so the hub refuses the take. The
    // message keeps its Take control, and the keyboard user keeps their place
    // on it rather than landing on the page body (cas-008f).
    const bubble = page.locator('.conversation-turn[data-state="error"]');
    const take = bubble.getByRole("button", { name: "Take control of the session", exact: true });
    const lease = (url: URL) => /\/v1\/sessions\/[^/]+\/lease$/.test(url.pathname);
    await page.route(lease, (route) => route.request().method() === "POST"
      ? route.fulfill({ status: 409, json: { error: "lease held" } })
      : route.fulfill({ json: { held_by_me: false, controller_label: "Studio iPad" } }));
    await take.focus();
    await page.keyboard.press("Enter");
    await expect(page.locator("#message-status")).toContainText("Studio iPad controls this session");
    await expect(take).toBeVisible();
    await expect(take).toBeFocused();
    // cas-1730 (cas-008f N01): the message agrees with the composer — it names
    // the device in control and says to take control once it is released.
    await expect(bubble.getByRole("status")).toContainText("Studio iPad is in control. Take control when it's released, then retry.");
    await page.unroute(lease);
  });

  await journey.stage("Take control from the message, then retry", async () => {
    // The refusal says "Take control, then retry". The conversation header
    // has no Take control (cas-3433), so the operator follows the instruction
    // on the refused message itself.
    const bubble = page.locator('.conversation-turn[data-state="error"]');
    const leaseRequest = page.waitForRequest((request) => request.method() === "POST" && new URL(request.url()).pathname.endsWith(`/sessions/${PELICAN}/lease`));
    // By keyboard, as QA round 1 did (C06): Enter on the focused control.
    await bubble.getByRole("button", { name: "Take control of the session", exact: true }).focus();
    await page.keyboard.press("Enter");
    await leaseRequest;
    await expect(page.locator("#message-status")).toHaveText("You control this session now. Retry to send the message.");
    // cas-8e0a: the message itself agrees. Take control leaves it, and it no
    // longer says this device isn't in control.
    await expect(bubble.getByRole("button", { name: "Take control of the session", exact: true })).toHaveCount(0);
    await expect(bubble.getByRole("status")).toHaveText("Not sent · This device controls the session now. Retry to send it.");
    // F01 (round 1): focus moves to Retry, the next step, not to the page body.
    await expect(bubble.getByRole("button", { name: "Retry sending", exact: true })).toBeFocused();
    // On a phone its actions are 44px targets.
    const desktop = page.viewportSize()!;
    await page.setViewportSize({ width: 390, height: 844 });
    for (const name of ["Edit message", "Retry sending"]) {
      await expect.poll(async () => (await bubble.getByRole("button", { name, exact: true }).boundingBox())?.height ?? 0, { message: `${name} target height at 390px` }).toBeGreaterThanOrEqual(44);
    }
    await page.setViewportSize(desktop);
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

  await journey.stage("A late receipt after the supervisor talks on never offers Retry", async () => {
    hub.deliverLatest(PELICAN);
    await expect(page.locator('.conversation-turn[data-state="acknowledged"]')).toHaveCount(1);
    // Watch every frame: a "Not confirmed" that flashes before a late receipt
    // is a Retry that sends the message twice (cas-1185).
    await page.evaluate(() => {
      const w = window as unknown as { __unconfirmedFrames: number; __watchUnconfirmed: boolean };
      w.__unconfirmedFrames = 0;
      w.__watchUnconfirmed = true;
      const tick = () => {
        if (document.querySelector('.conversation-turn[data-state="unconfirmed"]')) w.__unconfirmedFrames += 1;
        if (w.__watchUnconfirmed) requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });
    await composer.fill("Run the gate once more.");
    const crossing = hub.nextSend();
    await send.click();
    expect((await crossing).text).toBe("Run the gate once more.");
    // The supervisor's turn crosses the send, and the receipt comes 3.4 s
    // later: the slowest late receipt measured in the cas-1622 QA.
    hub.supervisorSays(PELICAN, "Gate run 2 of 3 is going.");
    await page.waitForTimeout(3_400);
    hub.deliverLatest(PELICAN);
    const crossed = page.locator('.conversation-turn[data-state="acknowledged"]').filter({ hasText: "Run the gate once more." });
    await expect(crossed).toHaveCount(1);
    const flashed = await page.evaluate(() => {
      const w = window as unknown as { __unconfirmedFrames: number; __watchUnconfirmed: boolean };
      w.__watchUnconfirmed = false;
      return w.__unconfirmedFrames;
    });
    expect(flashed, "frames that showed Not confirmed before the late receipt").toBe(0);
    await expect(page.getByRole("button", { name: "Retry sending" })).toHaveCount(0);
    await expect(page.getByRole("log").getByText("Run the gate once more.")).toHaveCount(1);
  });

  await journey.stage("A message the hub never confirms offers Retry", async () => {
    await composer.fill("Is the gate green yet?");
    const unreceipted = hub.nextSend();
    await send.click();
    expect((await unreceipted).text).toBe("Is the gate green yet?");
    // No receipt comes; the supervisor talks on, so the receipt is overdue (cas-1622).
    hub.supervisorSays(PELICAN, "Still running the release gate.");
    const bubble = page.locator('.conversation-turn[data-state="unconfirmed"]');
    // It gives up 5 s after that turn arrived (cas-1185), not at once.
    await expect(bubble.getByRole("status")).toHaveText(`Not confirmed · The hub never confirmed this reached ${PELICAN}. Retry sends it again.`, { timeout: 10_000 });
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
    // The empty field stays one line tall (a 64-character name used to grow it
    // to three), and the header's machine · codename line stays one line with
    // the connection state visible (3.30.0 journey F9).
    const lineHeight = await composer.evaluate((field) => parseFloat(getComputedStyle(field).lineHeight));
    const oneLine = await composer.evaluate((field) => field.getBoundingClientRect().height);
    expect(oneLine, "empty composer height").toBeLessThan(2 * lineHeight + 26);
    expect(await composer.getAttribute("placeholder")).toMatch(/…$/);
    const host = page.locator(".conversation-host");
    const hostLine = await host.evaluate((element) => parseFloat(getComputedStyle(element).lineHeight));
    expect((await host.boundingBox())!.height, "header meta on one line").toBeLessThan(hostLine * 1.5);
    await expect(page.locator("#conversation-connection")).toBeVisible();
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
