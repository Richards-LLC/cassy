import { activate, phoneLayout, showConversationList } from "./responsive-goals";
import { test, expect, journeyPart } from "./journey";
import { ATLAS, STUDIO } from "./world";
import { SCOPES } from "./hub-double";

// A first link, for another machine, that the operator opened earlier in the same tab.
const EARLIER_TOKEN = "Zq3VbXo8Zt1nA4wLr9cYp2KdJ6sHf0uEiMgTxBvNyRa";

// cas-d382 (fleet-operations brief S4): a link that grants factory:manage says
// so in plain words and the pairing holds it; a control pairing without it
// says so beside the command that adds it.
test("HUB-J2 pair a link that grants factory:manage, and see a pairing without it named (cas-d382)", journeyPart, async ({ page, journey }) => {
  const phone = await phoneLayout(page);
  const hub = await journey.hub({ machines: [ATLAS, STUDIO], paired: ["studio"], scopes: { studio: SCOPES } });
  const dialog = page.locator("#pair-dialog");
  const MANAGE_TOKEN = "M3VbXo8Zt1nA4wLr9cYp2KdJ6sHf0uEiMgTxBvNyRaQ";
  await journey.stage("Open a link that grants factory:manage", async () => {
    await journey.open();
    await page.evaluate((hash) => { location.hash = hash; }, `pair=${MANAGE_TOKEN}&hub=atlas&hub_url=https%3A%2F%2Fatlas.test&machine=Atlas%20%C2%B7%20Linux&scopes=machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt,factory:manage`);
    await expect(dialog.locator('input[name="url"]')).toBeVisible();
    await expect(dialog.locator(".pair-lead").first()).toBeVisible();
    // Native disclosure scrolling is tracked separately in cas-207a.
    // Desktop retains the raw checkbox; both layouts verify visible grants
    // and the exact requested scopes at exchange.
    if (!phone) {
      await dialog.locator(".pair-technical > summary").click();
      await expect(dialog.getByRole("checkbox", { name: "factory:manage", exact: true })).toBeChecked();
    }
    await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Daniel");
  });
  await journey.stage("Pair, and the pairing holds factory:manage", async () => {
    await activate(page, dialog.getByRole("button", { name: "Pair", exact: true }));
    await expect(dialog).toBeHidden();
    expect(hub.exchanges).toHaveLength(1);
    expect(hub.exchanges[0]?.requested_scopes).toEqual(["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt", "factory-manage"]);
  });
  await journey.stage("Paired machines names each pairing's fleet permissions", async () => {
    await showConversationList(page);
    await activate(page, page.locator("#paired-machines-toggle"));
    const machines = page.locator("#paired-machines-dialog");
    await expect(machines).toBeVisible();
    const atlas = machines.locator('[data-machine-id="atlas"] .paired-machine-fleet');
    const studio = machines.locator('[data-machine-id="studio"] .paired-machine-fleet');
    await expect(atlas.locator('[data-permission="manage"] .fleet-permission-state')).toBeVisible();
    await expect(atlas.locator('[data-permission="manage"] .fleet-permission-name')).toBeVisible();
    // Studio was paired for control only: explain the missing permission
    // before its re-pair command, without a disabled fleet-action lookalike.
    const manage = studio.locator('[data-permission="manage"]');
    // The command is reachable by keyboard and copies exactly.
    const copy = manage.getByRole("button", { name: "Copy command" });
    if (phone) await activate(page, copy);
    else {
      await copy.focus();
      await page.keyboard.press("Enter");
    }
    expect(await page.evaluate(() => navigator.clipboard.readText())).toMatch(/--scopes machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt,factory:manage$/);
    // A control pairing may allow managing workers itself, once: the first
    // press says what it allows, the second grants it.
    const allow = studio.locator('[data-permission="operate"]').getByRole("button", { name: "Allow managing workers" });
    await activate(page, allow);
    await expect(studio.locator('[data-permission="operate"] .fleet-permission-allow')).toBeVisible();
    await activate(page, studio.locator('[data-permission="operate"] .fleet-permission-allow'));
    await expect(studio.locator('[data-permission="operate"] .fleet-permission-state')).toBeVisible();
  });
});

// cas-207a: on a touch screen the finger lifts (pointerup) before the tap's
// mousedown and click. A rebuild owed while the name field had focus used to
// run between them, replacing the form under the finger: the disclosure
// stayed shut and the form jumped back to its top, on the name field.
for (const scheme of ["light", "dark"] as const) {
  test.describe(`phone touch ${scheme}`, () => {
    test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true, colorScheme: scheme });

    test(`HUB-J2 a tap opens Technical details on a scrolled invitation form, phone ${scheme} (cas-207a)`, journeyPart, async ({ page, journey }) => {
      await journey.hub({ machines: [ATLAS, STUDIO] });
      const dialog = page.locator("#pair-dialog");
      const form = dialog.locator("#pair-form");
      const name = dialog.getByRole("textbox", { name: "Your name (shown on the machine)" });
      const summary = dialog.locator(".pair-technical > summary");

      await journey.stage("Scroll the focused invitation form and tap Technical details", async () => {
        await page.goto(`./#pair=${EARLIER_TOKEN}&hub=studio&hub_url=https%3A%2F%2Fstudio.test&machine=Studio%20Mac&scopes=machine:read,session:read,pane:read`);
        await expect(dialog.getByRole("textbox", { name: /Machine name/ })).toHaveValue("Studio Mac");
        await expect(name).toBeFocused();
        await summary.scrollIntoViewIfNeeded();
        const scrolled = await form.evaluate((element) => element.scrollTop);
        expect(scrolled, "the disclosure sits below the form's first screen").toBeGreaterThan(0);

        await summary.tap();

        await expect(dialog.locator("details.pair-technical")).toHaveAttribute("open", "");
        // Let the tap's click task and the rebuild it released both run: on the
        // page clock, timers and frames included, not a fixed pause (cas-c4b2).
        await page.clock.runFor(100);
        await expect(dialog.locator("details.pair-technical")).toHaveAttribute("open", "");
        for (const scope of ["machine:read", "session:read", "pane:read"]) {
          await expect(dialog.getByRole("checkbox", { name: scope, exact: true })).toBeChecked();
        }
        await expect(dialog.getByRole("checkbox", { name: "message:send not granted by this invitation", exact: true })).toBeDisabled();
        // The operator keeps their place: the form did not jump to its top.
        expect(await form.evaluate((element) => element.scrollTop), "form scroll after the tap").toBeGreaterThanOrEqual(scrolled);
        await expect(summary).toBeInViewport();
        await expect(name).not.toBeFocused();
        expect(new URL(page.url()).hash, "the secret leaves the address bar").toBe("");
      });
    });
  });
}
