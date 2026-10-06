import { phoneLayout, phoneReply } from "./responsive-goals";
import { test, expect, journeyPart } from "./journey";
import type { Machine } from "./hub-double";
import { expectDraft, installDraftDiagnostic } from "./draft-diagnostic";
import { ATLAS, STUDIO, PELICAN, OTTER } from "./world";
import { journeyNow } from "./clock";

// A machine whose supervisor has a 64-character codename (cas-1334).
const LONG_NAME = "an-extraordinarily-long-supervisor-name-for-truncation-checks-77";
const FORGE: Machine = { id: "forge", label: "Forge · Linux", sessions: [{ name: LONG_NAME, supervisor: LONG_NAME, project_dir: "/projects/forge-tools", workers: ["steady-wren-3"], liveness: "live" }] };

/** A finger swiping the first match sideways by `dx`; returns whether it was gone right after the finger lifted. */
async function swipeAway(locator: import("@playwright/test").Locator, dx: number): Promise<boolean> {
  return locator.first().evaluate((element, dx) => {
    const box = element.getBoundingClientRect();
    const y = box.top + Math.min(24, box.height / 2);
    const x0 = box.left + box.width / 2;
    const fire = (type: string, x: number) => element.dispatchEvent(new PointerEvent(type, { bubbles: true, cancelable: true, pointerType: "touch", pointerId: 7, isPrimary: true, clientX: x, clientY: y }));
    fire("pointerdown", x0);
    for (let step = 1; step <= 6; step += 1) fire("pointermove", x0 + (dx * step) / 6);
    fire("pointerup", x0 + dx);
    return !element.isConnected;
  }, dx);
}

test("HUB-J5 reply by typing", async ({ page, journey }, testInfo) => {
  if (await phoneLayout(page)) { await phoneReply(page, journey); return; }
  // Nineteen stages; on a loaded host (load ~50) the full run took 108 s of
  // the old 120 s budget (cas-f657 QA N2). Every wait inside is event-driven,
  // so the budget only needs headroom, as HUB-J3 has.
  test.setTimeout(240_000);
  await installDraftDiagnostic(page);
  const hub = await journey.hub({ machines: [ATLAS, STUDIO, FORGE], paired: ["atlas", "studio", "forge"] });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const composer = page.getByRole("textbox", { name: "Your message" });
  const send = page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true });

  await journey.stage("Open the conversation", async () => {
    await journey.open();
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(send).toBeVisible();
  });

  await journey.stage("Keep a half-written reply and its focus through a heartbeat", async () => {
    await composer.fill("Please verify the gate first.\nKeep this half-written reply.");
    await composer.evaluate((field: HTMLTextAreaElement) => {
      field.setSelectionRange(7, 7);
      (window as unknown as { __draftField: HTMLTextAreaElement }).__draftField = field;
    });
    // Observe a real background request, rather than waiting for a string in
    // main.ts to claim that a render happened.
    await page.waitForResponse((response) => new URL(response.url()).pathname === "/v1/sessions");
    await expectDraft(page, composer, "Please verify the gate first.\nKeep this half-written reply.", testInfo);
    await expect(composer).toBeFocused();
    // The same field keeps the caret where it was: a collapsed selection at 7.
    // selectionDirection is not part of that: for a collapsed caret it is
    // platform-defined ("none" on macOS Chromium, "forward" elsewhere), so
    // pinning it failed every macOS run without saying anything (cas-d2b5).
    expect(await composer.evaluate((field: HTMLTextAreaElement) => ({
      sameNode: field === (window as unknown as { __draftField: HTMLTextAreaElement }).__draftField,
      start: field.selectionStart, end: field.selectionEnd,
    }))).toEqual({ sameNode: true, start: 7, end: 7 });
  });

  await journey.stage("Restore the draft and focus after switching conversations rebuilds the shell", async () => {
    await list.getByRole("button", { name: /gabber-studio/ }).click();
    await expect(composer).toHaveValue("");
    await expect(composer).toBeFocused();
    // Prove a shell replacement, distinct from the steady heartbeat above.
    expect(await page.evaluate(() =>
      (window as unknown as { __draftField: HTMLTextAreaElement }).__draftField.isConnected,
    )).toBe(false);
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expectDraft(page, composer, "Please verify the gate first.\nKeep this half-written reply.", testInfo);
    await expect(composer).toBeFocused();
    expect(await composer.evaluate((field: HTMLTextAreaElement) => field.selectionStart)).toBe(7);
  });

  await journey.stage("Write and send", async () => {
    await composer.fill("Please keep the release notes short this time.");
    const sent = hub.nextSend();
    await send.click();
    expect((await sent).text).toBe("Please keep the release notes short this time.");
    await expect(page.locator('.conversation-turn[data-state="sending"]')).toBeVisible();
    await expect(composer).toHaveValue("");
    // cas-71f4 (journey F20): one sending signal, the bubble's own "Sending…";
    // no composer line naming the supervisor's codename.
    await expect(page.getByText(/Sending/).filter({ visible: true })).toHaveCount(1);
    await expect(page.locator(".conversation-composer")).not.toContainText(PELICAN);
  });

  let queued = 0;
  await journey.stage("See it delivered", async () => {
    queued = hub.deliverLatest(PELICAN);
    const delivered = page.locator('.conversation-turn[data-state="acknowledged"]');
    await expect(delivered.getByRole("status")).toHaveText("Delivered");
    await expect(page.getByText(/Sending to /)).toHaveCount(0);
    // cas-387e: a Delivered message never sits above "will go out by itself".
    await expect(page.locator("#message-status")).toBeHidden();
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
      - group /^cas-src supervisor, \\d{1,2}:\\d{2}/:
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
    const iPad = { held_by_me: false, controller_label: "Studio iPad" };
    // cas-54ed: only the take that Enter sends is refused. The lease heartbeat
    // POSTs the same URL; refused too, it invalidated the lease and redrew the
    // bubble under the focused control, and the Enter went nowhere. So:
    // - a lease POST the page issued before arming (a heartbeat, even one
    //   still in flight) goes on to the hub double, unchanged;
    // - the page clock is held from arming until the take is refused, so no
    //   heartbeat can be issued in between: the first armed POST is the take;
    // - after that, a heartbeat gets the iPad's state, never a 409.
    let armed = false;
    let refusals = 0;
    const isLeasePost = (request: { method(): string; url(): string }) => request.method() === "POST" && lease(new URL(request.url()));
    const beforeArming = new Set<unknown>();
    const noteBeforeArming = (request: { method(): string; url(): string }) => { if (!armed && isLeasePost(request)) beforeArming.add(request); };
    page.on("request", noteBeforeArming);
    await page.route(lease, (route) => {
      const request = route.request();
      if (request.method() !== "POST") return route.fulfill({ json: iPad });
      if (!armed || beforeArming.has(request)) return route.fallback();
      if (refusals === 0) { refusals += 1; return route.fulfill({ status: 409, json: { error: "lease held" } }); }
      return route.fulfill({ json: iPad });
    });
    await take.focus();
    // Just ahead of the page's clock, which started at the test's start instant.
    await page.clock.pauseAt(journeyNow() + 1_000);
    // A round trip after the hold: any heartbeat the jump fired has been issued.
    await page.evaluate(() => 0);
    armed = true;
    page.off("request", noteBeforeArming);
    const refusedTake = page.waitForResponse((response) => isLeasePost(response.request()) && response.status() === 409);
    const holderRead = page.waitForResponse((response) => lease(new URL(response.url())) && response.request().method() === "GET");
    await page.keyboard.press("Enter");
    await refusedTake;
    // The take is refused; the clock runs again before the app reads who holds
    // the lease (its request path uses timers).
    await page.clock.resume();
    await holderRead;
    // Journey F5: the reason is said once, on the message, in plain words; the
    // composer only points at it.
    await expect(page.locator("#message-status")).toHaveText("Not sent — see the message above.");
    await expect(bubble.getByRole("status")).toContainText("Studio iPad is in control. Take control when it's released, then retry.");
    await expect(page.getByText(/Studio iPad (is in control|controls)/)).toHaveCount(1);
    await expect(page.locator("#message-status")).not.toContainText(/\bhub\b|controller/);
    await expect(bubble.getByRole("status")).not.toContainText(/\bhub\b|controller/);
    // A take that would be refused again is not offered as pressable: it says
    // who is being waited on, and the keyboard user keeps their place on it.
    const waiting = bubble.getByRole("button", { name: "Waiting for Studio iPad to release control", exact: true });
    await expect(waiting).toHaveText("Waiting for Studio iPad");
    await expect(waiting).toHaveAttribute("aria-disabled", "true");
    await expect(waiting).toBeFocused();
    expect(await waiting.evaluate((element) => getComputedStyle(element).cursor), "the waiting pill does not look pressable").toBe("default");
    await expect(take).toHaveCount(0);
    // cas-b00c (journey F18): Retry beside it would only be refused again, so
    // it is not pressable either while the iPad holds control.
    const retryWhileWaiting = bubble.getByRole("button", { name: "Retry sending", exact: true });
    await expect(retryWhileWaiting).toHaveAttribute("aria-disabled", "true");
    await expect(retryWhileWaiting).toHaveAccessibleDescription("Waiting for Studio iPad to release control");
    // When the iPad releases control, Take control comes back on its own,
    // and the keyboard user parked on the pill is on it (cas-88d86 QA F01).
    await page.unroute(lease);
    await expect(take).toBeVisible({ timeout: 10_000 });
    await expect(take).not.toHaveAttribute("aria-disabled", "true");
    await expect(take).toBeFocused();
    await expect(retryWhileWaiting).not.toHaveAttribute("aria-disabled", "true");
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
    // cas-b00c (journey F18): the message says it once; the composer does not repeat it.
    await expect(page.locator("#message-status")).toBeHidden();
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

  await journey.stage("Dismiss a refused message and bring it back", async () => {
    // cas-16eed: a refused message can leave the thread so it does not take
    // the screen; a small chip keeps the way back, and Edit and Retry come
    // back with it.
    const bubble = page.locator('.conversation-turn[data-state="error"]:not([data-replaced="true"])');
    const unsent = page.getByRole("button", { name: "Show 1 unsent message", exact: true });
    const dismiss = bubble.getByRole("button", { name: "Dismiss unsent message", exact: true });
    // Desktop, light: Dismiss by keyboard. The card leaves, the composer stops
    // pointing at it, the list stops previewing it, and focus lands on the chip.
    await dismiss.focus();
    await page.keyboard.press("Enter");
    await expect(bubble).toHaveCount(0);
    await expect(unsent).toBeVisible();
    await expect(unsent).toContainText("1 unsent message");
    await expect(unsent).toBeFocused();
    await expect(page.locator("#message-status")).toBeHidden();
    await expect(list.getByText(/Not sent: /)).toHaveCount(0);
    await page.keyboard.press("Enter");
    await expect(bubble).toHaveCount(1);
    await expect(unsent).toBeHidden();
    await expect(bubble.getByRole("button", { name: "Retry sending", exact: true })).toBeFocused();
    await expect(bubble.getByRole("button", { name: "Edit message", exact: true })).toBeVisible();
    // Phone, dark: a swipe takes it off; the chip is a 44px target and brings it back.
    const desktop = page.viewportSize()!;
    await page.emulateMedia({ colorScheme: "dark" });
    await page.setViewportSize({ width: 390, height: 844 });
    await expect.poll(async () => (await dismiss.boundingBox())?.height ?? 0, { message: "Dismiss target height at 390px" }).toBeGreaterThanOrEqual(44);
    await swipeAway(bubble, -300);
    await expect(bubble).toHaveCount(0);
    await expect(unsent).toBeVisible();
    expect((await unsent.boundingBox())!.height, "unsent chip height at 390px").toBeGreaterThanOrEqual(44);
    await unsent.click();
    await expect(bubble).toHaveCount(1);
    // A swipe short of the threshold settles the card back.
    await swipeAway(bubble, -40);
    await expect(bubble).toHaveCount(1);
    await expect.poll(() => bubble.evaluate((element) => element.style.transform)).toBe("");
    // Reduced motion: the swipe is an instant dismiss, with no slide.
    await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
    expect(await swipeAway(bubble, 300), "gone the moment the finger lifts").toBe(true);
    await expect(unsent).toBeVisible();
    await unsent.click();
    await expect(bubble).toHaveCount(1);
    await page.emulateMedia({ colorScheme: "light", reducedMotion: "no-preference" });
    await page.setViewportSize(desktop);
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
    // Journey F4: the late-receipt message ends visibly delivered, and the
    // unrelated "Gate run 2 of 3" turn did not take the tick off the earlier
    // "Ship it after the gate passes." either.
    await expect(crossed.locator(".conversation-delivered")).toHaveText("Delivered");
    await expect(page.locator('.conversation-turn[data-state="acknowledged"]').filter({ hasText: "Ship it after the gate passes." }).locator(".conversation-delivered")).toHaveText("Delivered");
  });

  await journey.stage("A message Cassy can't confirm offers Retry", async () => {
    await composer.fill("Is the gate green yet?");
    const unreceipted = hub.nextSend();
    await send.click();
    expect((await unreceipted).text).toBe("Is the gate green yet?");
    // No receipt comes; the supervisor talks on, so the receipt is overdue (cas-1622).
    hub.supervisorSays(PELICAN, "Still running the release gate.");
    const bubble = page.locator('.conversation-turn[data-state="unconfirmed"]');
    // It gives up 5 s after that turn arrived (cas-1185), not at once. The
    // page clock is moved through those 5 s rather than waiting them out on a
    // wall clock a loaded host stretches (cas-54ed).
    await expect(page.getByRole("log")).toContainText("Still running the release gate.");
    await expect(page.locator('.conversation-turn[data-state="sending"]').filter({ hasText: "Is the gate green yet?" })).toHaveCount(1);
    await page.clock.fastForward(5_000);
    await expect(bubble.getByRole("status")).toHaveText("Not confirmed · Cassy couldn't confirm delivery to the cas-src supervisor. Retry sends it again.");
    await expect(page.locator('.conversation-turn[data-state="sending"]')).toHaveCount(0);
    await expect(page.getByText(/Sending to /)).toHaveCount(0);
    const retried = hub.nextSend();
    await bubble.getByRole("button", { name: "Retry sending" }).click();
    expect((await retried).text).toBe("Is the gate green yet?");
    hub.deliverLatest(PELICAN);
    await expect(page.locator(".conversation-turn").filter({ hasText: "Is the gate green yet?" }).locator(".conversation-delivered")).toHaveText("Delivered");
    await expect(page.locator('.conversation-turn[data-state="unconfirmed"]')).toHaveCount(0);
    await expect(page.getByRole("log").getByText("Is the gate green yet?")).toHaveCount(1);
    // cas-387e: Retry on a live session never says the message waits for the connection.
    await expect(page.locator("#message-status")).toBeHidden();
  });

  await journey.stage("Not confirmed settles once the supervisor replies after it", async () => {
    // Journey F10: a later supervisor turn means the send most likely
    // arrived, so the card stops inviting a duplicate send.
    await composer.fill("Did the Mac tests start?");
    const unreceipted = hub.nextSend();
    await send.click();
    expect((await unreceipted).text).toBe("Did the Mac tests start?");
    hub.supervisorSays(PELICAN, "Gate run 3 of 3 is going.");
    const bubble = page.locator('.conversation-turn[data-state="unconfirmed"]').filter({ hasText: "Did the Mac tests start?" });
    await expect(page.getByRole("log")).toContainText("Gate run 3 of 3 is going.");
    await page.clock.fastForward(5_000);
    await expect(bubble.getByRole("button", { name: "Retry sending" })).toBeVisible();
    hub.supervisorSays(PELICAN, "Tests are running on the Mac.");
    await expect(bubble.getByRole("status")).toHaveText("Not confirmed · The supervisor has replied since; send it again only if it missed this.");
    await expect(bubble).toHaveAttribute("data-settled", "true");
    await expect(bubble.getByRole("button", { name: "Retry sending" })).toHaveCount(0);
    await expect(page.getByRole("log").locator(".conversation-unconfirmed")).not.toContainText(/\bhub\b/i);
    // cas-470e: the copy says "send it again", so the card carries a quiet
    // text-weight Send again (no pill, no fill), and it resends the message
    // without the operator retyping it.
    const again = bubble.getByRole("button", { name: "Send this message again" });
    await expect(again).toHaveText("Send again");
    const look = await again.evaluate((button) => { const style = getComputedStyle(button); return { border: style.borderTopWidth, background: style.backgroundColor, line: style.textDecorationLine }; });
    expect(look).toEqual({ border: "0px", background: "rgba(0, 0, 0, 0)", line: "underline" });
    const resent = hub.nextSend();
    await again.click();
    expect((await resent).text).toBe("Did the Mac tests start?");
    hub.deliverLatest(PELICAN);
    await expect(page.locator(".conversation-turn").filter({ hasText: "Did the Mac tests start?" }).locator(".conversation-delivered")).toHaveText("Delivered");
    await expect(page.locator('.conversation-turn[data-state="unconfirmed"]')).toHaveCount(0);
    await expect(page.getByRole("log").getByText("Did the Mac tests start?")).toHaveCount(1);
    await expect(page.locator("#message-status")).toBeHidden();
  });

  await journey.stage("A long supervisor name leaves the message box usable", async () => {
    await list.getByRole("button", { name: /forge-tools/ }).click();
    const longSend = page.getByRole("button", { name: "Send to the forge-tools supervisor", exact: true });
    await expect(longSend).toBeVisible();
    const [field, button, row] = await Promise.all([composer.boundingBox(), longSend.boundingBox(), page.locator(".conversation-composer").boundingBox()]);
    // The field keeps about its usual share of the row; the button reads
    // "Send" and its accessible name keeps the codename whole (journey F13).
    expect(field!.width, "message field width").toBeGreaterThan(row!.width * 0.33);
    expect(button!.x + button!.width, "Send stays inside the composer").toBeLessThanOrEqual(row!.x + row!.width);
    await expect(longSend.locator(".send-label")).toHaveText("Send");
    // The empty field stays one line tall (a 64-character name used to grow it
    // to three), and the header's machine · codename line stays one line with
    // the connection state visible (3.30.0 journey F9).
    const lineHeight = await composer.evaluate((field) => parseFloat(getComputedStyle(field).lineHeight));
    const oneLine = await composer.evaluate((field) => field.getBoundingClientRect().height);
    expect(oneLine, "empty composer height").toBeLessThan(2 * lineHeight + 26);
    // The placeholder addresses the project's supervisor, not the codename.
    await expect(composer).toHaveAttribute("placeholder", "Message the forge-tools supervisor");
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
    // The empty state names the role; the codename is in the meta line only
    // (cas-010f) and never breaks at its hyphens, at desktop or on a phone
    // (journey F13).
    const said = page.locator(".thread .empty .said");
    await expect(said).toHaveText("No messages from the forge-tools supervisor in this session yet — nothing is waiting on you.");
    const oneLineCodename = (selector: string) => page.locator(selector).evaluate((element) => {
      const lineHeight = parseFloat(getComputedStyle(element).lineHeight);
      return { lines: element.getClientRects().length, oneLine: element.getBoundingClientRect().height < lineHeight * 1.5, ellipsised: element.scrollWidth > element.clientWidth, title: element.getAttribute("title") };
    });
    for (const width of [desktop.width, 390]) {
      await page.setViewportSize({ width, height: desktop.height });
      expect(await oneLineCodename(".thread .empty .proj2"), `meta line at ${width}px`).toMatchObject({ lines: 1, oneLine: true, title: `Forge · Linux · ${LONG_NAME}` });
      // cas-766c: the codename yields to the machine; a codename this long ellipsises.
      expect(await oneLineCodename(".thread .empty .proj2 > .codename"), `meta codename at ${width}px`).toMatchObject({ lines: 1, oneLine: true, ...(width === 390 ? { ellipsised: true } : {}) });
      // cas-766c: the header and the empty card always show the machine,
      // and its OS word is shown whole or not at all, never cut.
      for (const machine of [".conversation-identity .host-machine", ".thread .empty .proj2-machine"]) {
        const shown = await page.locator(machine).evaluate((element) => ({ width: element.getBoundingClientRect().width, ch: parseFloat(getComputedStyle(element).fontSize) * 0.5, cut: element.scrollWidth > element.clientWidth + 1, os: (element.querySelector(".host-os")?.getClientRects().length ?? 0) > 0 }));
        expect(shown.width, `${machine} at ${width}px`).toBeGreaterThanOrEqual(shown.ch * 4);
        expect(shown.os && shown.cut, `${machine}'s OS word cut at ${width}px`).toBe(false);
      }
    }
    await page.setViewportSize(desktop);
  });

  await journey.stage("On a phone the placeholder reads whole for every project", async () => {
    // cas-1e0f QA F01/F02: "Message the gabber-studio supervisor" overflowed
    // the 245px phone field and was cut mid-word. The composer now keeps the
    // longest wording that fits: the full phrase, then "Message <project>",
    // then "Message the supervisor", never a cut one.
    const desktop = page.viewportSize()!;
    const fit = () => composer.evaluate((field: HTMLTextAreaElement) => {
      const style = getComputedStyle(field);
      const probe = document.createElement("span");
      probe.style.cssText = "position:absolute;left:-10000px;visibility:hidden;white-space:pre;";
      probe.style.font = style.font; probe.style.letterSpacing = style.letterSpacing;
      probe.textContent = field.placeholder; document.body.append(probe);
      const width = probe.getBoundingClientRect().width; probe.remove();
      return { text: field.placeholder, fits: width <= field.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight) + 0.5 };
    });
    const expected: Record<string, string[]> = {
      "cas-src": ["Message the cas-src supervisor"],
      // cas-71f4: never "Message <project>", which read as messaging a project.
      "gabber-studio": ["Message the supervisor", "Message the gabber-studio supervisor"],
      "forge-tools": ["Message the forge-tools supervisor", "Message the supervisor"],
    };
    await page.setViewportSize({ width: 390, height: 844 });
    for (const [project, wordings] of Object.entries(expected)) {
      const back = page.getByRole("button", { name: "‹ Conversations", exact: true });
      if (await back.isVisible()) await back.click();
      await list.getByRole("button", { name: new RegExp(project) }).click();
      await expect(composer).toBeVisible();
      await expect.poll(async () => (await fit()).fits, { message: `${project}: the placeholder fits the 390px field` }).toBe(true);
      const { text } = await fit();
      expect(wordings, `${project}: a whole wording, not a cut one`).toContain(text);
      expect(text).not.toContain("…");
    }
    // Back on a desktop field the full phrase returns.
    await page.setViewportSize(desktop);
    await expect(composer).toHaveAttribute("placeholder", "Message the forge-tools supervisor");
  });

  await journey.stage("Focus on the opening card moves into the conversation", async () => {
    // cas-9a96: keyboard focus on the connection card's Details fell to the
    // page body when the opened conversation replaced the card. It lands in
    // the composer now; focus elsewhere is left where it is.
    const details = page.locator(".conversation-pane-slot .connection-details summary");
    await page.reload();
    await expect(list.getByRole("button", { name: /gabber-studio/ })).toBeVisible();
    // cas-54ed: the attach is held until the card has focus, not for 3 s a
    // loaded host can spend before the focus lands.
    const releaseOtter = hub.holdAttach(OTTER);
    await list.getByRole("button", { name: /gabber-studio/ }).click();
    await expect(details).toBeVisible();
    await details.focus();
    await expect(details).toBeFocused();
    releaseOtter();
    await expect(page.locator(".conversation-pane-slot :is(.terminal-connecting, .conversation-opening)")).toHaveCount(0, { timeout: 10_000 });
    await expect(composer).toBeFocused();
    // Focus outside the card (the list search) stays there when it is replaced.
    await page.reload();
    await expect(list.getByRole("button", { name: /cas-src/ })).toBeVisible();
    const releasePelican = hub.holdAttach(PELICAN);
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(details).toBeVisible();
    const search = page.getByRole("searchbox", { name: "Search conversations" });
    await search.focus();
    await expect(search).toBeFocused();
    releasePelican();
    await expect(page.locator(".conversation-pane-slot :is(.terminal-connecting, .conversation-opening)")).toHaveCount(0, { timeout: 10_000 });
    await page.waitForTimeout(500);
    await expect(search).toBeFocused();
  });

  await journey.stage("A draft too long to keep across a reload says so", async () => {
    // cas-adfc: a draft over the store's 64k bound is not kept on disk; the
    // composer says so instead of losing it silently on the next reload.
    const note = page.locator("#message-draft-note");
    await expect(note).toBeHidden();
    await composer.fill("a".repeat(70_000));
    await expect(note).toBeVisible();
    await expect(note).toHaveText("This draft is too long to keep if the page reloads. Send it, or copy it somewhere safe, before you leave.");
    await composer.fill("Short enough to keep.");
    await expect(note).toBeHidden();
    await page.reload();
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(composer).toHaveValue("Short enough to keep.");
    await expect(note).toBeHidden();
  });

  await journey.stage("A draft the full browser storage refuses says so, and stays on screen", async () => {
    // cas-f657: localStorage is full, so writing the drafts entry throws
    // QuotaExceededError. The draft stays in the field, the composer says it
    // won't survive a reload, and the field is described by that note.
    const note = page.locator("#message-draft-note");
    await page.evaluate(() => {
      const original = Storage.prototype.setItem;
      (window as unknown as { restoreSetItem: () => void }).restoreSetItem = () => { Storage.prototype.setItem = original; };
      Storage.prototype.setItem = function (key: string, value: string) {
        if (key.includes(":drafts:")) throw new DOMException("The quota has been exceeded.", "QuotaExceededError");
        return original.call(this, key, value);
      };
    });
    await composer.fill("A reply typed while the browser's storage is full.");
    await expect(note).toBeVisible();
    await expect(note).toHaveText("This browser couldn't save this draft (its storage is full or blocked), so it won't survive a reload. Send it, or copy it somewhere safe, before you leave.");
    await expect(composer).toHaveAttribute("aria-describedby", /\bmessage-draft-note\b/);
    await expect(composer).toHaveValue("A reply typed while the browser's storage is full.");
    // Room again: the next keystroke saves it, and the note goes.
    await page.evaluate(() => (window as unknown as { restoreSetItem: () => void }).restoreSetItem());
    await composer.press("End");
    await composer.pressSequentially("!");
    await expect(note).toBeHidden();
    await expect(composer).not.toHaveAttribute("aria-describedby", /\bmessage-draft-note\b/);
  });
});

test("HUB-J5 sending without control takes it, and the take notice clears once the message goes (cas-cff2)", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const composer = page.getByRole("textbox", { name: "Your message" });
  const send = page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true });
  const status = page.locator("#message-status");
  // The lease lapsed while the operator was away: this device reads as not
  // in control, and nobody else holds it, until the page takes it itself.
  let taken = false;
  await page.route(/\/v1\/sessions\/[^/]+\/lease$/, (route) => {
    if (route.request().method() === "POST") { taken = true; return route.fallback(); }
    if (!taken) return route.fulfill({ json: { held_by_me: false, controller_label: null } });
    return route.fallback();
  });
  await journey.stage("Send while this device does not hold the session", async () => {
    await journey.open();
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(send).toBeVisible();
    await composer.fill("Take it and send this.");
    const sent = hub.nextSend();
    await send.click();
    expect((await sent).text).toBe("Take it and send this.");
    expect(taken).toBe(true);
  });
  await journey.stage("Delivered: the take notice is gone", async () => {
    hub.deliverLatest(PELICAN);
    await expect(page.getByRole("log").getByText("Delivered")).toBeVisible();
    await expect(status).toBeHidden();
    await expect(page.getByText(/Taking control of/)).toHaveCount(0);
  });
});

test("HUB-J5 Send again without control takes it, replaces the settled message and settles Delivered (cas-cff2)", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const composer = page.getByRole("textbox", { name: "Your message" });
  const send = page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true });
  const status = page.locator("#message-status");
  let lapsed = false;
  let taken = false;
  await page.route(/\/v1\/sessions\/[^/]+\/lease$/, (route) => {
    if (route.request().method() === "POST") { if (lapsed) taken = true; return route.fallback(); }
    if (lapsed && !taken) return route.fulfill({ json: { held_by_me: false, controller_label: null } });
    return route.fallback();
  });
  await journey.stage("A message whose receipt never came settles as Not confirmed", async () => {
    await journey.open();
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(send).toBeVisible();
    await composer.fill("Did the Mac tests start?");
    const unreceipted = hub.nextSend();
    await send.click();
    expect((await unreceipted).text).toBe("Did the Mac tests start?");
    hub.supervisorSays(PELICAN, "Gate run 3 of 3 is going.");
    await expect(page.getByRole("log")).toContainText("Gate run 3 of 3 is going.");
    await page.clock.fastForward(5_000);
    hub.supervisorSays(PELICAN, "Tests are running on the Mac.");
    const bubble = page.locator('.conversation-turn[data-state="unconfirmed"]').filter({ hasText: "Did the Mac tests start?" });
    await expect(bubble).toHaveAttribute("data-settled", "true");
  });
  await journey.stage("The lease lapses; Send again takes control and the message goes once", async () => {
    lapsed = true;
    // The next lease read finds this device no longer in control.
    await page.clock.fastForward(30_000);
    await expect.poll(() => taken || page.evaluate(() => document.querySelector(".conversation-send-again") !== null)).toBe(true);
    const resent = hub.nextSend();
    await page.getByRole("button", { name: "Send this message again" }).click();
    expect((await resent).text).toBe("Did the Mac tests start?");
    expect(taken, "the send took control first").toBe(true);
    hub.deliverLatest(PELICAN);
    await expect(page.locator(".conversation-turn").filter({ hasText: "Did the Mac tests start?" }).locator(".conversation-delivered")).toHaveText("Delivered");
    await expect(page.getByRole("log").getByText("Did the Mac tests start?")).toHaveCount(1);
    await expect(status).toBeHidden();
  });
});
