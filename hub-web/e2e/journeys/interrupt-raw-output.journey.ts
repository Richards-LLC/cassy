import type { Locator, Page } from "@playwright/test";
import { test, expect, journeyPart } from "./journey";
import { SCOPES, type HubDouble } from "./hub-double";
import { ATLAS, STUDIO, PELICAN } from "./world";

// cas-0546: Conversations is Commander's only surface. The supervisor's
// Interrupt and its read-only Raw output live in the conversation header.
// cas-f285: Raw output reads the daemon's rendered rows; no pane host or
// terminal surface stands behind the thread.

/** Tab, from wherever focus is, until `target` has it: a keyboard-only route. */
async function tabTo(page: Page, target: Locator, max = 60): Promise<number> {
  for (let presses = 0; presses <= max; presses++) {
    if (await target.evaluate((node) => node === document.activeElement)) return presses;
    const inTerminal = await page.evaluate(() => Boolean(document.activeElement?.closest(".pane-host, .terminal-mount, canvas")));
    expect(inTerminal, "Tab never stops inside a terminal").toBe(false);
    await page.keyboard.press("Tab");
  }
  throw new Error(`Tab did not reach ${target} in ${max} presses`);
}

const interrupts = (hub: HubDouble) => hub.frames.filter((frame) => frame.kind === "InterruptPane");

/** Only the thread shows: no pane host, terminal mount or canvas, and the pane's text is unspoken. */
async function expectOnlyTheThread(page: Page): Promise<void> {
  await expect(page.locator(".pane-host, .terminal-mount, canvas")).toHaveCount(0);
  await expect(page.locator("#conversation-stage > .conversation-thread-slot")).toBeVisible();
  await expect(page.locator(".conversation-thread-slot .conversation-reading.thread")).toBeVisible();
  expect(await page.locator("body").ariaSnapshot(), "the pane's text is not in the accessibility tree").not.toContain("The supervisor is ready.");
  // No Terminal view anywhere: not a button, a link, a palette command or a word on screen.
  await expect(page.getByRole("button", { name: /terminal/i, includeHidden: true })).toHaveCount(0);
  await expect(page.getByRole("link", { name: /terminal/i })).toHaveCount(0);
  await expect(page.getByText(/Terminal view/i)).toHaveCount(0);
}

/** Raw output opens by keyboard, reads the supervisor's pane, is read-only, and Escape returns focus. */
async function readRawOutput(page: Page, hub: HubDouble, sheet: boolean): Promise<void> {
  const raw = page.getByRole("button", { name: "Raw output", exact: true });
  await raw.focus();
  await expect(raw).toHaveAttribute("aria-expanded", "false");
  await page.keyboard.press("Enter");
  const drawer = page.getByRole("dialog", { name: "Raw output" });
  await expect(drawer).toBeVisible();
  await expect(raw).toHaveAttribute("aria-expanded", "true");
  await expect(drawer).toHaveAccessibleDescription("What the cas-src supervisor's terminal shows, as text. Read-only.");
  const transcript = drawer.getByRole("log", { name: "Raw output" });
  await expect(transcript).toContainText("The supervisor is ready.");
  // Focus moved into the drawer, and the drawer is the only thing on top.
  expect(await drawer.evaluate((dialog) => dialog.contains(document.activeElement))).toBe(true);
  // Read-only: nothing in it takes text, and typing in it sends nothing.
  await expect(drawer.locator("input, textarea, [contenteditable]:not([contenteditable=false])")).toHaveCount(0);
  await expect(drawer.getByRole("textbox")).toHaveCount(0);
  const sends = hub.sends.length;
  const before = hub.frames.length;
  await transcript.focus();
  await page.keyboard.type("q");
  await page.keyboard.press("Control+c");
  expect(hub.sends.length, "typing in Raw output sends no message").toBe(sends);
  expect(hub.frames.slice(before).filter((frame) => !["ScrollbackRequest", "ConversationHistoryRequest", "GetState"].includes(frame.kind)), "typing in Raw output reaches no pane").toEqual([]);
  const box = (await drawer.boundingBox())!;
  const viewport = page.viewportSize()!;
  if (sheet) {
    expect(Math.round(box.width), "a bottom sheet across the phone").toBe(viewport.width);
    expect(Math.round(box.y + box.height), "on the bottom edge").toBe(viewport.height);
  } else {
    expect(Math.round(box.x + box.width), "a drawer on the right edge").toBe(viewport.width);
    expect(box.width).toBeLessThan(viewport.width);
  }
  await page.keyboard.press("Escape");
  await expect(drawer).toBeHidden();
  await expect(raw).toBeFocused();
  await expect(raw).toHaveAttribute("aria-expanded", "false");
}

test("HUB-J18 interrupt or read the supervisor from its conversation", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS, STUDIO], paired: ["atlas", "studio"], scopes: { atlas: [...SCOPES, "hub-admin"] } });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const interrupt = page.getByRole("button", { name: "Interrupt the cas-src supervisor", exact: true });
  const toast = page.locator("#toast");

  await journey.stage("Open a conversation; only the thread shows", async () => {
    await journey.open();
    await list.getByRole("button", { name: /cas-src/ }).and(list.locator(".conversation-row")).click();
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
    await expect(interrupt).toBeVisible();
    await expect(interrupt).toHaveText("Interrupt");
    await expect(page.getByRole("button", { name: "Raw output", exact: true })).toBeVisible();
    for (const action of [interrupt, page.getByRole("button", { name: "Raw output", exact: true })]) await expect(action).not.toHaveAttribute("aria-disabled", "true");
    await expectOnlyTheThread(page);
    // Nor does the palette offer one.
    await page.getByRole("button", { name: "Appearance & commands" }).click();
    await expect(page.locator("#command-palette")).toBeVisible();
    await expect(page.locator("#command-palette").getByRole("button", { name: /terminal/i, includeHidden: true })).toHaveCount(0);
    await page.keyboard.press("Escape");
    await expect(page.locator("#command-palette")).toBeHidden();
  });

  await journey.stage("Interrupt the supervisor from the keyboard", async () => {
    await page.getByRole("searchbox", { name: "Search conversations" }).focus();
    // cas-d043 G02/H11: the first stop after the search skips to the
    // conversation, so Interrupt is three keys away, not 12 Tabs; every stop
    // on the way shows its ring.
    await page.keyboard.press("Tab");
    const skip = page.getByRole("button", { name: "Skip to the conversation", exact: true });
    await expect(skip).toBeFocused();
    await expect(skip).toBeVisible();
    // Each ring is polled as a whole condition, not read once (cas-c4b2).
    const ring = (locator: Locator) => locator.evaluate((node) => { const style = getComputedStyle(node); return style.outlineStyle !== "none" && parseFloat(style.outlineWidth) > 0; });
    await expect.poll(() => ring(skip), { message: "the skip link shows its focus ring" }).toBe(true);
    await page.keyboard.press("Enter");
    const raw = page.getByRole("button", { name: "Raw output", exact: true });
    await expect(raw).toBeFocused();
    await expect.poll(() => ring(raw), { message: "Raw output shows its focus ring" }).toBe(true);
    expect(await tabTo(page, interrupt)).toBe(1);
    await expect.poll(() => ring(interrupt), { message: "Interrupt shows its focus ring" }).toBe(true);
    await expect(interrupt).toBeFocused();
    await page.keyboard.press("Enter");
    await hub.waitFor(() => interrupts(hub).length === 1);
    expect(interrupts(hub)[0]).toEqual({ machine: "atlas", session: PELICAN, kind: "InterruptPane", body: { pane_id: "supervisor" } });
    await expect(toast).toHaveText("Interrupted the cas-src supervisor.");
    await expect(interrupt).toBeFocused();
    expect(hub.leaseTakes.filter((take) => take.force), "this browser already held control: nothing forced").toEqual([]);
  });

  await journey.stage("Take control from another device to interrupt", async () => {
    // Studio iPad takes control of the session; reopening the conversation
    // reads who holds it.
    hub.holdLease(PELICAN, "Studio iPad");
    await list.getByRole("button", { name: /gabber-studio/ }).and(list.locator(".conversation-row")).click();
    await expect(page.locator(".conversation-identity h1")).toHaveText("gabber-studio");
    // cas-97d58 F11: the cas-src toast belongs to cas-src; it does not stay up
    // (or stay in the accessibility tree) under gabber-studio's own Interrupt.
    await expect(page.getByText("Interrupted the cas-src supervisor.")).toHaveCount(0);
    await list.getByRole("button", { name: /cas-src/ }).and(list.locator(".conversation-row")).click();
    await expect(page.locator(".conversation-identity h1")).toHaveText("cas-src");
    // The reopened conversation has read who holds control before the press:
    // with the holder still unknown, the take is not forced (queue run
    // 38742699142, a slow lease read on a shard's first test).
    await expect(page.locator("#conversation-control")).toHaveText(" · Studio iPad in control");
    await expect(interrupt).not.toHaveAttribute("aria-disabled", "true");
    await interrupt.click();
    // Taking it over is never silent: this hub-admin pairing forces the
    // take, and the toast names the device control was taken from.
    // pin: the confirmation must name the device taken over and the interrupted supervisor.
    await expect(toast).toHaveText("Took control from Studio iPad. Interrupted the cas-src supervisor.");
    expect(hub.leaseTakes.filter((take) => take.force)).toEqual([{ machine: "atlas", session: PELICAN, force: true, status: 200 }]);
    expect(interrupts(hub)).toHaveLength(2);
  });

  await journey.stage("Read the raw output from the keyboard", async () => {
    const released = page.waitForResponse((response) => response.request().method() === "DELETE" && new URL(response.url()).pathname === `/v1/sessions/${PELICAN}/lease`);
    await readRawOutput(page, hub, false);
    expect((await released).status()).toBe(204);
    await expectOnlyTheThread(page);
  });

  await journey.stage("Leaving a conversation releases its input control", async () => {
    await interrupt.click();
    await hub.waitFor(() => interrupts(hub).length === 3);
    const released = page.waitForRequest((request) => request.method() === "DELETE" && new URL(request.url()).pathname === `/v1/sessions/${PELICAN}/lease`);
    await list.getByRole("button", { name: /gabber-studio/ }).and(list.locator(".conversation-row")).click();
    await released;
    await expect(page.locator(".conversation-identity h1")).toHaveText("gabber-studio");
  });

  await journey.stage("A take that finishes after leaving cannot keep the old conversation", async () => {
    await list.getByRole("button", { name: /cas-src/ }).and(list.locator(".conversation-row")).click();
    let resume!: () => void;
    let started!: () => void;
    const waiting = new Promise<void>((resolve) => { started = resolve; });
    const held = new Promise<void>((resolve) => { resume = resolve; });
    const path = `**/v1/sessions/${PELICAN}/lease`;
    await page.route(path, async (route) => {
      if (route.request().method() === "POST") { started(); await held; }
      await route.fallback();
    });
    try {
      await interrupt.click();
      await waiting;
      const released = page.waitForRequest((request) => request.method() === "DELETE" && new URL(request.url()).pathname === `/v1/sessions/${PELICAN}/lease`);
      await list.getByRole("button", { name: /gabber-studio/ }).and(list.locator(".conversation-row")).click();
      resume();
      await released;
      expect(interrupts(hub), "the abandoned take must not interrupt the old supervisor").toHaveLength(3);
    } finally {
      resume();
      await page.unroute(path);
    }
  });
});

test("HUB-J18 Interrupt recovers when the first lease read is still pending", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], scopes: { atlas: [...SCOPES, "hub-admin"] } });
  hub.holdLease(PELICAN, "Studio iPad");
  const leaseRead = hub.holdNextLeaseRead(PELICAN);
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const interrupt = page.getByRole("button", { name: "Interrupt the cas-src supervisor", exact: true });
  const toast = page.locator("#toast");

  try {
    await journey.open();
    await list.getByRole("button", { name: /cas-src/ }).and(list.locator(".conversation-row")).click();
    await expect(interrupt).toBeVisible();
    await leaseRead.started;

    await interrupt.click();
    await hub.waitFor(() => hub.leaseTakes.length >= 1);
    expect(hub.leaseTakes[0]).toMatchObject({ session: PELICAN, force: false, status: 409 });
    leaseRead.release();

    await expect(toast).toHaveText("Took control from Studio iPad. Interrupted the cas-src supervisor.");
    expect(hub.leaseTakes).toEqual([
      { machine: "atlas", session: PELICAN, force: false, status: 409 },
      { machine: "atlas", session: PELICAN, force: true, status: 200 },
    ]);
    expect(interrupts(hub)).toHaveLength(1);
  } finally {
    leaseRead.release();
  }
});

for (const [width, colorScheme] of [[390, "light"], [390, "dark"], [1280, "dark"]] as const) {
  test.describe(`${width} ${colorScheme}`, () => {
    test.use({ viewport: { width, height: width === 390 ? 844 : 800 }, colorScheme });
    test(`HUB-J18 ${width} ${colorScheme}: interrupt and read the raw output by keyboard`, journeyPart, async ({ page, journey }) => {
      const phone = width === 390;
      // The light phone pairing may not force a take: it is told who holds control instead.
      const admin = !(phone && colorScheme === "light");
      const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], scopes: { atlas: admin ? [...SCOPES, "hub-admin"] : SCOPES.filter((scope) => scope !== "message-send") } });
      const interrupt = page.getByRole("button", { name: "Interrupt the cas-src supervisor", exact: true });
      const raw = page.getByRole("button", { name: "Raw output", exact: true });
      const toast = page.locator("#toast");

      await journey.stage(`Open the conversation at ${width} in ${colorScheme}`, async () => {
        await journey.open();
        await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
        await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
        await expectOnlyTheThread(page);
        if (!admin) {
          const send = page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true });
          const reason = page.locator("#message-status");
          await expect(send).toHaveAttribute("aria-disabled", "true");
          await expect(send).toHaveAttribute("aria-describedby", "message-status");
          await expect(reason).toBeVisible();
          await expect(reason).toContainText("Ask whoever runs Atlas");
          await expect(send).toHaveAccessibleDescription(await reason.innerText());
        }
        // Both stay whole in the header at every width: Interrupt keeps its
        // word, Raw output its icon under its full name, each a full target.
        await expect(interrupt.locator(".action-label")).toBeVisible();
        for (const action of [interrupt, raw]) {
          await expect(action).toBeInViewport({ ratio: 1 });
          const box = (await action.boundingBox())!;
          expect(box.height, "a full-size target").toBeGreaterThanOrEqual(phone ? 44 : 36);
        }
        // cas-d043 G02: with the identity on its own row, a phone has room
        // for Raw output's word too; the bare ">_" glyph read as nothing.
        await expect(raw.locator(".action-label")).toBeVisible();
        await expect(raw.locator(".action-label")).toHaveText("Raw output");
        await expect(raw).toHaveAccessibleName("Raw output");
        // The two read apart in either scheme: Interrupt in the critical tone.
        const colours = await Promise.all([interrupt, raw].map((action) => action.evaluate((node) => getComputedStyle(node).color)));
        expect(colours[0], "Interrupt is drawn in its own tone").not.toBe(colours[1]);
      });

      await journey.stage(`Interrupt by keyboard at ${width} in ${colorScheme}`, async () => {
        if (phone) await page.locator("#conversation-back").focus();
        else await page.getByRole("searchbox", { name: "Search conversations" }).focus();
        await tabTo(page, interrupt);
        await page.keyboard.press("Enter");
        await hub.waitFor(() => interrupts(hub).length === 1);
        expect(interrupts(hub)[0]).toMatchObject({ machine: "atlas", session: PELICAN, body: { pane_id: "supervisor" } });
        await expect(toast).toHaveText("Interrupted the cas-src supervisor.");
        await expect(interrupt).toBeFocused();
      });

      await journey.stage(`Another device in control at ${width} in ${colorScheme}`, async () => {
        hub.holdLease(PELICAN, "Studio iPad");
        // The page re-reads who holds control when it reloads.
        await page.reload();
        const back = page.getByRole("button", { name: "‹ Conversations", exact: true });
        // Phone startup restores the conversation asynchronously. Wait for
        // Back instead of sampling visibility before the phone shell is ready.
        if (phone) await back.click();
        await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
        await expect(interrupt).toBeVisible();
        // cas-d043 G02: the header names the device in control before any take.
        await expect(page.locator("#conversation-control")).toHaveText(" · Studio iPad in control");
        await expect(page.locator("#conversation-control")).toBeVisible();
        await interrupt.focus();
        await page.keyboard.press("Enter");
        if (admin) {
          await expect(toast).toHaveText("Took control from Studio iPad. Interrupted the cas-src supervisor.");
          await hub.waitFor(() => interrupts(hub).length === 2);
        } else {
          await expect(toast).toHaveText("Studio iPad is in control of this session. Interrupt works once it releases control, or from a pairing with administrator access, which can take over.");
          expect(interrupts(hub), "nothing interrupted while another device holds control").toHaveLength(1);
        }
        expect(hub.leaseTakes.filter((take) => take.status !== 200 || take.force).at(-1)).toMatchObject({ session: PELICAN, force: admin, status: admin ? 200 : 409 });
      });

      await journey.stage(`Read the raw output at ${width} in ${colorScheme}`, async () => {
        await readRawOutput(page, hub, phone);
        await expectOnlyTheThread(page);
      });
      if (phone && admin) await journey.stage("Back to Conversations releases the input lease", async () => {
        await interrupt.click();
        await hub.waitFor(() => interrupts(hub).length === 3);
        const released = page.waitForRequest((request) => request.method() === "DELETE" && new URL(request.url()).pathname === `/v1/sessions/${PELICAN}/lease`);
        await page.locator("#conversation-back").click();
        await released;
        await expect(page.getByRole("navigation", { name: "Choose a supervisor" })).toBeVisible();
      });
    });
  });
}
