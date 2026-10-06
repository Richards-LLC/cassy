import { activate, phoneLayout, phonePairLink, showConversationList } from "./responsive-goals";
import { test, expect, expectWholeFocusRing, journeyPart } from "./journey";
import { ATLAS, PELICAN, STUDIO } from "./world";
import { SCOPES } from "./hub-double";

// A one-time invitation as `cas hub pair` prints it: 43 base64url characters.
const TOKEN = "q3VbXo8Zt1nA4wLr9cYp2KdJ6sHf0uEiMgTxBvNyRaQ";
// A first link, for another machine, that the operator opened earlier in the same tab.
const EARLIER_TOKEN = "Zq3VbXo8Zt1nA4wLr9cYp2KdJ6sHf0uEiMgTxBvNyRa";

test("HUB-J2 pair a machine from a cas hub pair link", async ({ page, journey }) => {
  if (await phoneLayout(page)) { await phonePairLink(page, journey, TOKEN, EARLIER_TOKEN); return; }
  const hub = await journey.hub({ machines: [ATLAS, STUDIO] });
  const dialog = page.locator("#pair-dialog");

  await journey.stage("Open the link the machine printed", async () => {
    // An earlier link for another machine is already open, the name typed.
    await page.goto(`./#pair=${EARLIER_TOKEN}&hub=studio&hub_url=https%3A%2F%2Fstudio.test&machine=Studio%20Mac&scopes=machine:read,session:read,pane:read`);
    await expect(dialog.getByRole("textbox", { name: /Machine name/ })).toHaveValue("Studio Mac");
    // A fresh link opens on the operator's name with the dialog at its top (QA F02),
    // drawing the standard focus ring whole rather than two bars (cas-b2e4 F03).
    await expectWholeFocusRing(dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }));
    await expect(dialog.getByRole("heading", { name: "Pair a machine" })).toBeInViewport({ ratio: 1 });
    // cas-b52d (journey F26): what the link withholds, in plain words, and the
    // command that grants it, with its Copy, are on screen without scrolling
    // inside the dialog, beside what this browser will be able to do. That is
    // the dialog as it opens: once Technical details is opened, the operator
    // keeps their place there rather than jumping back up (cas-207a).
    await expect(dialog.getByRole("button", { name: "Copy command" })).toBeInViewport({ ratio: 1 });
    const withheld = dialog.locator(".pair-withheld");
    await expect(withheld).toContainText("This link does not let it: Type, send messages and interrupt");
    await expect(withheld).toBeInViewport({ ratio: 1 });
    await expect(dialog.locator(".pair-withheld-command code")).toHaveText(/^cas hub pair --origin \S+ --scopes machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt$/);
    await expect(dialog.locator(".pair-withheld-command code")).toBeInViewport({ ratio: 1 });
    // A read-only invitation cannot offer a control grant in the real bundle.
    await dialog.getByText("Technical details").click();
    for (const scope of ["machine:read", "session:read", "pane:read"]) {
      const checkbox = dialog.getByRole("checkbox", { name: scope, exact: true });
      await expect(checkbox).toBeEnabled();
      await expect(checkbox).toBeChecked();
    }
    for (const scope of ["pane:input", "message:send", "pane:interrupt"]) {
      const checkbox = dialog.getByRole("checkbox", { name: `${scope} not granted by this invitation`, exact: true });
      await expect(checkbox).toBeDisabled();
      await expect(checkbox).not.toBeChecked();
    }
    await expect(dialog.locator("#pair-copy")).toHaveAttribute("data-pair-command", /--scopes machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt$/);
    await dialog.getByText("Technical details").click();
    await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Daniel");
    // The link carries the machine's hub address and name, as `cas hub pair`
    // prints it, and arrives in the tab that is already open (hashchange).
    await page.evaluate((hash) => { location.hash = hash; }, `pair=${TOKEN}&hub=atlas&hub_url=https%3A%2F%2Fatlas.test&machine=Atlas%20%C2%B7%20Linux&scopes=machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt`);
    await expect(dialog.getByText("One-time invitation ready. Check the machine, then add your name.")).toBeVisible();
    expect(new URL(page.url()).hash, "the secret leaves the address bar at once").toBe("");
  });

  await journey.stage("Confirm the machine", async () => {
    // Address and machine name arrive filled in and editable; only the operator's name is left.
    await expect(dialog.getByRole("textbox", { name: /Machine's hub address/ })).toHaveValue("https://atlas.test");
    await expect(dialog.getByRole("textbox", { name: /Machine's hub address/ })).toBeEditable();
    await expect(dialog.getByRole("textbox", { name: /Machine name/ })).toHaveValue("Atlas · Linux");
    // The name typed for the earlier link is kept; only the machine changed.
    await expect(dialog.getByRole("textbox", { name: "Your name (shown on the machine)" })).toHaveValue("Daniel");
    // The dialog opens at its top and every field is visible without scrolling it (QA F02, F4).
    await expect(dialog.getByRole("heading", { name: "Pair a machine" })).toBeInViewport({ ratio: 1 });
    const fields = dialog.locator("input:visible");
    for (let index = 0; index < await fields.count(); index += 1) await expect(fields.nth(index)).toBeInViewport({ ratio: 1 });
    // The address guidance waits behind a disclosure; its page-origin shortcut is an ordinary button.
    await expect(dialog.getByText("Where do I find this?")).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Use this page's address" })).toBeHidden();
    await dialog.getByText("Where do I find this?").click();
    await expect(dialog.getByRole("button", { name: "Use this page's address" })).toBeVisible();
    // The stage ends back in the name field, drawing its whole focus ring.
    await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).focus();
    await expectWholeFocusRing(dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }));
  });

  await journey.stage("Check the technical details", async () => {
    // The raw scope boxes wait under Technical details.
    await dialog.getByText("Where do I find this?").click();
    await expect(dialog.getByRole("checkbox", { name: "message:send" })).toBeHidden();
    await dialog.getByText("Technical details").click();
    // Its terms read in sentence case, not as shouting eyebrows (cas-b2e4 F02).
    const term = dialog.locator("details.pair-technical dt").first();
    await expect(term).toHaveText("Cassy Cloud origin");
    expect(await term.evaluate((el) => getComputedStyle(el).textTransform)).toBe("none");
    await expect(dialog.getByRole("checkbox", { name: "message:send" })).toBeChecked();
    await dialog.locator("details.pair-technical").scrollIntoViewIfNeeded();
  });

  await journey.stage("Pair", async () => {
    await dialog.getByRole("button", { name: "Pair", exact: true }).click();
    await expect(dialog).toBeHidden();
    expect(hub.exchanges).toHaveLength(1);
    expect(hub.exchanges[0]).toMatchObject({ token: TOKEN, hub_id: "atlas", operator_label: "Daniel" });
    // The new token goes to the new link's machine, never the earlier one (QA F01).
    expect(hub.exchangeOrigins).toEqual(["https://atlas.test"]);
  });

  await journey.stage("Reach the supervisor", async () => {
    const row = page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ });
    await expect(row).toBeVisible({ timeout: 15_000 });
    await row.click();
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
    await expect(page.locator("#toast")).toHaveText(/connected/);
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
    await expect(dialog.getByText("One-time invitation ready. Check the machine, then add your name.")).toBeVisible();
    await expect(dialog.locator(".pair-lead").first()).toHaveText("This browser will be able to: See its sessions and raw output · Type, send messages and interrupt · Stop and restart workers and sessions");
    // Native disclosure scrolling is tracked separately in cas-207a.
    // Desktop retains the raw checkbox; both layouts verify visible grants
    // and the exact requested scopes at exchange.
    if (!phone) {
      await dialog.getByText("Technical details").click();
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
    await expect(atlas.locator('[data-permission="manage"] .fleet-permission-state')).toHaveText("Allowed");
    await expect(atlas.locator('[data-permission="manage"] .fleet-permission-name')).toHaveText("Stop and restart workers and sessions");
    // Studio was paired for control only: Stop and restart reads unavailable,
    // in words, with the command that adds it to that pairing.
    const manage = studio.locator('[data-permission="manage"]');
    await expect(manage.locator(".fleet-permission-state")).toHaveText("Not allowed on this pairing");
    await expect(manage.getByRole("button", { name: "Stop and restart" })).toHaveAttribute("aria-disabled", "true");
    await expect(manage.locator("code")).toHaveText(/^cas hub pair --origin \S+ --scopes machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt,factory:manage$/);
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
    await expect(studio.locator('[data-permission="operate"] .fleet-permission-allow')).toHaveText("Confirm: allow managing workers on Studio Mac · macOS");
    await activate(page, studio.locator('[data-permission="operate"] .fleet-permission-allow'));
    await expect(studio.locator('[data-permission="operate"] .fleet-permission-state')).toHaveText("Allowed");
    // Stop and restart still is not: it is never allowed from this browser.
    await expect(manage.locator(".fleet-permission-state")).toHaveText("Not allowed on this pairing");
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
      const summary = dialog.getByText("Technical details");

      await journey.stage("Scroll the focused invitation form and tap Technical details", async () => {
        await page.goto(`./#pair=${EARLIER_TOKEN}&hub=studio&hub_url=https%3A%2F%2Fstudio.test&machine=Studio%20Mac&scopes=machine:read,session:read,pane:read`);
        await expect(dialog.getByRole("textbox", { name: /Machine name/ })).toHaveValue("Studio Mac");
        await expect(name).toBeFocused();
        await summary.scrollIntoViewIfNeeded();
        const scrolled = await form.evaluate((element) => element.scrollTop);
        expect(scrolled, "the disclosure sits below the form's first screen").toBeGreaterThan(0);

        await summary.tap();

        await expect(dialog.locator("details.pair-technical")).toHaveAttribute("open", "");
        // Let the tap's click task and the rebuild it released both run.
        await page.evaluate(() => new Promise((resolve) => setTimeout(() => requestAnimationFrame(() => resolve(null)), 50)));
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
