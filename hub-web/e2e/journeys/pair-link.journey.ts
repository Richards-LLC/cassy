import { activate, phoneLayout, showConversationList } from "./responsive-goals";
import { test, expect, journeyPart } from "./journey";
import { ATLAS, STUDIO } from "./world";
import { SCOPES } from "./hub-double";
import type { Locator, Page } from "@playwright/test";

// A link opens the confirmation form, titled "Pair a machine".
const pairDialog = (page: Page) => page.getByRole("dialog", { name: "Pair a machine", exact: true });
const technicalSummary = (dialog: Locator) => dialog.getByText("Technical details", { exact: true });
// The <details> disclosure (role group) that holds the Technical details summary.
const technicalDetails = (page: Page, dialog: Locator) => dialog.getByRole("group").filter({ has: page.getByText("Technical details", { exact: true }) });
// One permission line in a machine's Fleet permissions region, found by its permission name.
const OPERATE = "Manage workers and tasks";
const MANAGE = "Stop and restart workers and sessions";
const permission = (page: Page, fleet: Locator, name: string) => fleet.getByTestId("fleet-permission").filter({ has: page.getByText(name, { exact: true }) });

// A first link, for another machine, that the operator opened earlier in the same tab.
const EARLIER_TOKEN = "Zq3VbXo8Zt1nA4wLr9cYp2KdJ6sHf0uEiMgTxBvNyRa";

// cas-d382 (fleet-operations brief S4): a link that grants factory:manage says
// so in plain words and the pairing holds it; a control pairing without it
// says so beside the command that adds it.
test("HUB-J2 pair a link that grants factory:manage, and see a pairing without it named (cas-d382)", journeyPart, async ({ page, journey }) => {
  const phone = await phoneLayout(page);
  const hub = await journey.hub({ machines: [ATLAS, STUDIO], paired: ["studio"], scopes: { studio: SCOPES } });
  const dialog = pairDialog(page);
  const MANAGE_TOKEN = "M3VbXo8Zt1nA4wLr9cYp2KdJ6sHf0uEiMgTxBvNyRaQ";
  await journey.stage("Open a link that grants factory:manage", async () => {
    await journey.open();
    await page.evaluate((hash) => { location.hash = hash; }, `pair=${MANAGE_TOKEN}&hub=atlas&hub_url=https%3A%2F%2Fatlas.test&machine=Atlas%20%C2%B7%20Linux&scopes=machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt,factory:manage`);
    await expect(dialog.getByRole("textbox", { name: "Machine's hub address", exact: true })).toBeVisible();
    // The capability lead; a withheld-scopes lead after it starts "This link does not let it".
    await expect(dialog.getByText(/^This browser will be able to:/)).toBeVisible();
    // Native disclosure scrolling is tracked separately in cas-207a.
    // Desktop retains the raw checkbox; both layouts verify visible grants
    // and the exact requested scopes at exchange.
    if (!phone) {
      await technicalSummary(dialog).click();
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
    // The footer's paired-machines button: "<n> paired machines <state>".
    await activate(page, page.getByRole("button", { name: /^2 paired machines / }));
    const machines = page.getByRole("dialog", { name: "Paired machines", exact: true });
    await expect(machines).toBeVisible();
    const atlas = machines.getByRole("region", { name: /^Fleet permissions on Atlas/ });
    const studio = machines.getByRole("region", { name: /^Fleet permissions on Studio/ });
    await expect(permission(page, atlas, MANAGE).getByTestId("fleet-permission-state")).toBeVisible();
    await expect(permission(page, atlas, MANAGE).getByText(MANAGE, { exact: true })).toBeVisible();
    // Studio was paired for control only: explain the missing permission
    // before its re-pair command, without a disabled fleet-action lookalike.
    const manage = permission(page, studio, MANAGE);
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
    // The name matches both "Allow managing workers" and its "Confirm: allow managing workers on …" press.
    const allow = permission(page, studio, OPERATE).getByRole("button", { name: "Allow managing workers" });
    await activate(page, allow);
    await expect(allow).toBeVisible();
    await activate(page, allow);
    await expect(permission(page, studio, OPERATE).getByTestId("fleet-permission-state")).toBeVisible();
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
      const dialog = pairDialog(page);
      // Scroll probe only: the form is the dialog's scroller.
      const form = dialog.locator("#pair-form");
      const name = dialog.getByRole("textbox", { name: "Your name (shown on the machine)" });
      const summary = technicalSummary(dialog);

      await journey.stage("Scroll the focused invitation form and tap Technical details", async () => {
        await page.goto(`./#pair=${EARLIER_TOKEN}&hub=studio&hub_url=https%3A%2F%2Fstudio.test&machine=Studio%20Mac&scopes=machine:read,session:read,pane:read`);
        await expect(dialog.getByRole("textbox", { name: /Machine name/ })).toHaveValue("Studio Mac");
        await expect(name).toBeFocused();
        await summary.scrollIntoViewIfNeeded();
        const scrolled = await form.evaluate((element) => element.scrollTop);
        expect(scrolled, "the disclosure sits below the form's first screen").toBeGreaterThan(0);

        await summary.tap();

        await expect(technicalDetails(page, dialog)).toHaveAttribute("open", "");
        // Let the tap's click task and the rebuild it released both run: on the
        // page clock, timers and frames included, not a fixed pause (cas-c4b2).
        await page.clock.runFor(100);
        await expect(technicalDetails(page, dialog)).toHaveAttribute("open", "");
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
