import { test, expect, journeyPart } from "./journey";
import type { Page } from "@playwright/test";
import { ATLAS } from "./world";
import type { HubDouble } from "./hub-double";
import { showConversationList } from "./responsive-goals";
import { mkdir, writeFile, readFile } from "node:fs/promises";
import { join } from "node:path";

async function pair(page: Page, token: string): Promise<void> {
  if (await page.locator("#paired-machines-dialog").isVisible()) await page.getByRole("button", { name: "Close paired machines", exact: true }).click();
  await page.evaluate((hash) => { location.hash = hash; }, `pair=${token}&hub=atlas&hub_url=https%3A%2F%2Fatlas.test&machine=Atlas&scopes=machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt`);
  const dialog = page.locator("#pair-dialog");
  await expect(dialog).toBeVisible();
  await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Operator");
  await dialog.getByRole("button", { name: "Pair", exact: true }).click();
  await expect(dialog).toBeHidden();
}
async function inventory(page: Page): Promise<void> {
  if (!await page.locator("#paired-machines-dialog").isVisible()) {
    await showConversationList(page);
    await page.locator("#paired-machines-toggle").click();
  }
  await page.getByRole("button", { name: "Browser installations on Atlas", exact: true }).click();
  await expect(page.locator(".installation-inventory").getByText("This browser's access.", { exact: false })).toBeVisible();
}

/** Protocol evidence remains available, but never leads the default sheet. */
async function expectGeneration(page: Page, generation: number): Promise<void> {
  const technical = page.locator(".installation-inventory-row details");
  await technical.locator("summary").click();
  await expect(technical.locator("div").filter({ has: page.getByText("Credential generation", { exact: true }) }).locator("dd")).toHaveText(String(generation));
  await technical.locator("summary").click();
  await expect(technical).not.toHaveAttribute("open", "");
}

// cas-f7c5: HUB-J2's installation journey ran as one test of five stages,
// about 54 s idle against a 60 s budget, so a loaded host timed it out. Every
// action is traced with screen snapshots, so time goes with the number of
// actions; each part now starts from its own one-pairing setup and carries
// its own budget. The assertions are the original stages' own.

/** One pairing from the link: the installation every later stage acts on. */
async function pairedInstallation(page: Page, hub: HubDouble): Promise<string> {
  await pair(page, "1".repeat(43));
  expect(hub.installations.size).toBe(1);
  expect([...hub.installations.values()][0]!.credential_generation).toBe(1);
  return [...hub.installations.keys()][0]!;
}

test("HUB-J2 possession-proven repairs update one browser installation (cas-5e53)", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS] });
  await journey.open();
  let id: string;
  await journey.stage("M01 five repairs update one browser installation", async () => {
    for (let n = 1; n <= 5; n++) {
      await pair(page, String(n).repeat(43));
      expect(hub.installations.size).toBe(1);
      expect([...hub.installations.values()][0]!.credential_generation).toBe(n);
    }
    id = [...hub.installations.keys()][0]!;
    await inventory(page);
    await expect(page.locator(".installation-inventory-row")).toHaveCount(1);
    await expectGeneration(page, 5);
    await expect(page.locator(".installation-inventory-row")).toContainText("Not in an operator inbox");
    await page.locator(".installation-inventory").getByRole("button", { name: "Close", exact: true }).click();
  });
  expect(id!).toBeTruthy();
});

test("HUB-J2 another IndexedDB tab adopts the accepted repair (cas-5e53)", journeyPart, async ({ page, context, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS] });
  await journey.open();
  await pairedInstallation(page, hub);
  const peer = await context.newPage();
  // Force stale-request adoption rather than letting BroadcastChannel update
  // the peer first. Both clients still share the real IDB and real Web Lock.
  await peer.addInitScript(() => { Object.defineProperty(window, "BroadcastChannel", { value: undefined }); });
  await hub.install(peer);
  await peer.goto(page.url());
  await expect(peer.locator("#paired-machines-toggle")).toBeAttached();
  await journey.stage("M02 another open tab adopts the accepted credential on refusal", async () => {
    await pair(page, "2".repeat(43));
    await inventory(peer);
    await expectGeneration(peer, 2);
    expect(hub.staleInstallationRefusals.length).toBeGreaterThan(0);
    expect(hub.installations.size).toBe(1);
    await peer.locator(".installation-inventory").getByRole("button", { name: "Close", exact: true }).click();
    await peer.close();
  });
});

test("HUB-J2 cancelling a staged repair restores access on reload (cas-5e53)", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS] });
  await journey.open();
  const id = await pairedInstallation(page, hub);
  await journey.stage("M03 cancelling a staged repair restores access on reload", async () => {
    if (await page.locator("#paired-machines-dialog").isVisible()) await page.getByRole("button", { name: "Close paired machines", exact: true }).click();
    let release!: () => void;
    const held = new Promise<void>((resolve) => { release = resolve; });
    let arrived!: () => void;
    const waiting = new Promise<void>((resolve) => { arrived = resolve; });
    await page.route("https://atlas.test/v1/auth/pairing/commit", async (route) => { arrived(); await held; await route.fallback(); });
    await page.evaluate((hash) => { location.hash = hash; }, `pair=${"7".repeat(43)}&hub=atlas&hub_url=https%3A%2F%2Fatlas.test&machine=Atlas&scopes=machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt`);
    const dialog = page.locator("#pair-dialog");
    await expect(dialog).toBeVisible();
    await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Operator");
    await dialog.getByRole("button", { name: "Pair", exact: true }).click();
    await waiting;
    await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
    release();
    await expect(dialog).toBeHidden();
    await page.unroute("https://atlas.test/v1/auth/pairing/commit");
    expect(hub.installations.get(id!)!.credential_generation).toBe(1);
    await page.reload();
    await inventory(page);
    await expectGeneration(page, 1);
    await page.locator(".installation-inventory").getByRole("button", { name: "Close", exact: true }).click();
  });
});

test("HUB-J2 the installation inventory on keyboard and phone, and exact-device revocation (cas-5e53)", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS] });
  await journey.open();
  const id = await pairedInstallation(page, hub);
  await journey.stage("M04 keyboard and phone inventory keep identity and actions reachable", async () => {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.emulateMedia({ colorScheme: "dark" });
    await inventory(page);
    if (process.env.INSTALLATION_QA) {
      const qa = process.env.INSTALLATION_QA;
      await mkdir(qa, { recursive: true });
      for (const [size, width, height] of [["desktop", 1280, 800], ["phone", 390, 844]] as const) {
        for (const colorScheme of ["light", "dark"] as const) {
          await page.setViewportSize({ width, height }); await page.emulateMedia({ colorScheme });
          await expect(page.locator(".installation-inventory-row h3")).toContainText("This browser");
          expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
          await page.screenshot({ path: join(qa, `inventory-${colorScheme}-${size}.png`) });
        }
      }
      const html = await page.locator(".installation-inventory").evaluate((dialog) => dialog.outerHTML);
      const css = await readFile("dist/app.css", "utf8");
      await writeFile(join(qa, "inventory.html"), `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>${css}\ndialog{position:fixed;inset:0;margin:auto}</style></head><body>${html}</body></html>`);
      for (const [name, query, media] of [["forced-colors", "(forced-colors: active)", { forcedColors: "active" }], ["reduced-motion", "(prefers-reduced-motion: reduce)", { reducedMotion: "reduce" }], ["contrast-more", "(prefers-contrast: more)", { contrast: "more" }]] as const) {
        await page.emulateMedia({ forcedColors: null, reducedMotion: null, contrast: null });
        await page.emulateMedia(media);
        expect(await page.evaluate((query) => matchMedia(query).matches, query)).toBe(true);
        await expect(page.locator(".installation-inventory-row h3")).toContainText("This browser");
        await page.screenshot({ path: join(qa, `a11y-${name}.png`) });
      }
      await page.emulateMedia({ forcedColors: null, reducedMotion: null, contrast: null });
    }
    await expect(page.locator(".installation-inventory-row h3")).toContainText("This browser");
    const technical = page.locator(".installation-inventory-row details");
    await technical.locator("summary").click();
    await expect(technical.locator("code")).toHaveText(id!);
    await technical.locator("summary").click();
    const close = page.locator(".installation-inventory").getByRole("button", { name: "Close", exact: true });
    await close.focus();
    await expect(close).toBeFocused();
    await expect(close).toBeInViewport();
    await close.press("Escape");
    await expect(page.locator(".installation-inventory")).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Browser installations on Atlas", exact: true })).toBeFocused();
  });

  await journey.stage("M05 explicit exact-device revocation removes this browser access", async () => {
    await inventory(page);
    page.once("dialog", async (dialog) => { expect(dialog.message()).toContain(id!); await dialog.accept(); });
    await page.locator(".installation-inventory").getByRole("button", { name: "Revoke this browser's access", exact: true }).click();
    await expect(page.locator(".installation-inventory")).toHaveCount(0);
    expect(hub.installations.get(id!)!.revoked_at).not.toBeNull();
    // cas-d043 G09: the page says what happened and lands on the next step.
    await expect(page.locator("#toast")).toHaveText("This browser's access to Atlas was revoked.");
    await expect(page.locator(":focus")).toHaveAccessibleName("Pair a machine");
    await page.reload();
    await expect(page.getByText("Pair a machine to start your first conversation.")).toBeVisible();
    await expect(page.locator("body")).toMatchAriaSnapshot(`
      - complementary "Supervisor conversations":
        - button "Cassy Cloud apps": Cassy Cloud
        - button "Pair a machine"
        - button "Appearance & commands"
        - heading "Conversations" [level=1]
        - paragraph: Your projects. Your supervisors.
        - navigation "Choose a supervisor"
        - status
        - text: Pair a machine to start your first conversation. Got a pairing link from whoever runs it? Open it on this device.
        - button "0 paired machines Not paired"
    `);
  });
});

test("HUB-J2 two expired tabs share one refresh before the next repair (cas-5e53)", journeyPart, async ({ page, context, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS] });
  await journey.open();
  await pair(page, "8".repeat(43));
  const url = page.url();
  // cas-d06f: expire on the hub only once no tab holds a live connection.
  // Expiring first let the still-connected page see 401 expired and start a
  // refresh that the hub applied but the page never persisted (aborted by the
  // navigation below, or overwritten by this IndexedDB edit), so both tabs
  // later presented a replaced credential and read as revoked.
  await page.evaluate(async () => {
    const db = await new Promise<IDBDatabase>((resolve, reject) => { const r = indexedDB.open("cas-commander-v1", 2); r.onsuccess = () => resolve(r.result); r.onerror = () => reject(r.error); });
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction("machines", "readwrite"); const store = tx.objectStore("machines"); const read = store.get("atlas");
      read.onsuccess = () => {
        const machine = read.result.pairingInstall?.candidate ?? read.result;
        store.put({ ...machine, expiresAt: "2000-01-01T00:00:00Z" });
      };
      tx.oncomplete = () => resolve(); tx.onerror = () => reject(tx.error);
    }); db.close();
  });
  await page.goto("about:blank");
  hub.expireInstallation([...hub.installations.keys()][0]!);
  expect(hub.installationRefreshes, "no refresh may start before both tabs open").toBe(0);
  const peer = await context.newPage(); await hub.install(peer);
  await journey.stage("M01 expired tabs serialize their refresh through real Web Locks and IndexedDB", async () => {
    await Promise.all([page.goto(url), peer.goto(url)]);
    // cas-f7c5: the two tabs are independent pages; read both at once.
    await Promise.all([inventory(page), inventory(peer)]);
    await Promise.all([expectGeneration(page, 2), expectGeneration(peer, 2)]);
    expect(hub.installationRefreshes).toBe(1);
    expect(hub.installations.size).toBe(1);
  });
  await journey.stage("M02 the next repair uses the accepted refresh generation", async () => {
    await page.locator(".installation-inventory").getByRole("button", { name: "Close", exact: true }).click();
    await pair(page, "9".repeat(43));
    expect((hub.exchanges.at(-1)!.installation as { expected_generation: number }).expected_generation).toBe(2);
    expect([...hub.installations.values()][0]!.credential_generation).toBe(3);
    await inventory(page);
    await expectGeneration(page, 3);
  });
  await peer.close();
});

/** An ordinary or admin pairing link for Atlas (cas-5e53 F08). */
const link = (token: string, scopes: string) => `pair=${token}&hub=atlas&hub_url=https%3A%2F%2Fatlas.test&machine=Atlas&scopes=${scopes}`;
const ordinary = "machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt";

// cas-5e53 independent QA F08: an ordinary admin invitation (`cas hub pair
// --scopes …,hub:admin`) must let the operator reach and revoke ANOTHER, old
// installation through the real pairing entry path, with hub:admin held only
// by explicit consent.
test("HUB-J2 an admin invitation offers hub:admin unticked, and its consent reads whole at every size (cas-5e53 F08)", journeyPart, async ({ page, journey }) => {
  await journey.hub({ machines: [ATLAS] });
  await journey.open();
  await journey.stage("The admin invitation offers hub:admin unticked, with what it allows", async () => {
    await page.evaluate((hash) => { location.hash = hash; }, link("a".repeat(43), `${ordinary},hub:admin`));
    const dialog = page.locator("#pair-dialog");
    await expect(dialog).toBeVisible();
    const admin = dialog.getByRole("checkbox", { name: /See and revoke other browsers on Atlas/ });
    await expect(admin).toBeVisible();
    await expect(admin, "admin is never pre-ticked: it needs explicit consent").not.toBeChecked();
    // cas-d043 G08: plain words naming the machine, not the raw "hub:admin";
    // the summary claims the power only once the box is ticked.
    await expect(dialog.locator(".pair-admin-consent label.scope")).toContainText("See and revoke other browsers on Atlas");
    await expect(dialog.locator(".pair-summary")).not.toContainText("See and revoke", { useInnerText: true });
    await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Operator");
  });

  for (const size of [{ width: 1280, height: 800 }, { width: 390, height: 844 }]) {
    for (const scheme of ["light", "dark"] as const) {
      await journey.stage(`The consent reads whole at ${size.width}px in ${scheme}`, async () => {
        await page.setViewportSize(size);
        await page.emulateMedia({ colorScheme: scheme });
        const consent = page.locator("#pair-dialog .pair-admin-consent");
        await consent.scrollIntoViewIfNeeded();
        await expect(consent).toBeInViewport();
        expect(await consent.evaluate((element) => element.scrollWidth <= element.clientWidth), "the consent note wraps inside its box").toBe(true);
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "no sideways scroll").toBe(true);
      });
    }
  }
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.emulateMedia({ colorScheme: "light" });

});

test("HUB-J2 an admin invitation, explicitly consented, revokes another browser installation (cas-5e53 F08)", journeyPart, async ({ page, browser, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS] });
  await journey.open();
  // The old installation is a genuinely separate browser (its own storage and
  // key), paired the ordinary way, never seeded into the double.
  const elsewhere = await browser.newContext({ baseURL: new URL(page.url()).origin });
  const old = await elsewhere.newPage();
  await hub.install(old);
  await old.goto(page.url());
  let oldId = "";
  await journey.stage("An old installation pairs from another browser", async () => {
    await old.evaluate((hash) => { location.hash = hash; }, link("o".repeat(43), ordinary));
    const dialog = old.locator("#pair-dialog");
    await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Operator");
    await dialog.getByRole("textbox", { name: "Name for this browser" }).fill("Old laptop");
    await dialog.getByRole("button", { name: "Pair", exact: true }).click();
    await expect(dialog).toBeHidden();
    expect(hub.installations.size).toBe(1);
    oldId = [...hub.installations.keys()][0]!;
  });

  // cas-f7c5: the invitation is opened as the part above shows it; its
  // offer and its rendering at every size are asserted there.
  await page.evaluate((hash) => { location.hash = hash; }, link("a".repeat(43), `${ordinary},hub:admin`));
  await expect(page.locator("#pair-dialog")).toBeVisible();
  await page.locator("#pair-dialog").getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Operator");

  await journey.stage("Consent is given from the keyboard", async () => {
    const dialog = page.locator("#pair-dialog");
    const admin = dialog.getByRole("checkbox", { name: /See and revoke other browsers on Atlas/ });
    // Keyboard consent: focus the box and press Space.
    await admin.focus();
    await page.keyboard.press("Space");
    await expect(admin).toBeChecked();
    await expect(dialog.locator(".pair-summary")).toContainText("See and revoke other browsers on Atlas", { useInnerText: true });
  });

  await journey.stage("Pairing with that consent grants hub:admin", async () => {
    const dialog = page.locator("#pair-dialog");
    await dialog.getByRole("button", { name: "Pair", exact: true }).click();
    await expect(dialog).toBeHidden();
    const mine = [...hub.installations.values()].find((row) => row.device_id !== oldId)!;
    expect(mine.scopes, "the consented admin scope was granted").toContain("hub-admin");
  });

  await journey.stage("The inventory lists the other installation and revokes it by exact ID", async () => {
    if (!await page.locator("#paired-machines-dialog").isVisible()) {
      await showConversationList(page);
      await page.locator("#paired-machines-toggle").click();
    }
    await page.getByRole("button", { name: "Browser installations on Atlas", exact: true }).click();
    const inventory = page.locator(".installation-inventory");
    await expect(inventory.getByText("2 installations.", { exact: false })).toBeVisible();
    const row = inventory.locator(".installation-inventory-row", { hasText: oldId });
    await expect(row).toContainText("Old laptop");
    page.once("dialog", async (dialog) => { expect(dialog.message()).toContain(oldId); await dialog.accept(); });
    await row.getByRole("button", { name: "Revoke this installation", exact: true }).click();
    await expect(row.getByRole("button", { name: "Revoked", exact: true })).toBeDisabled();
    await expect(inventory.getByRole("status")).toHaveText("Revoked Old laptop. Its live connections are closing.");
    expect(hub.installations.get(oldId)!.revoked_at, "the other installation is revoked").not.toBeNull();
    const mine = [...hub.installations.values()].find((r) => r.device_id !== oldId)!;
    expect(mine.revoked_at, "this browser keeps its access").toBeNull();
    await inventory.getByRole("button", { name: "Close", exact: true }).click();
  });

  await journey.stage("The revoked browser is refused; this one still lists the machine", async () => {
    await old.reload();
    await expect(old.getByText(/needs pairing|was revoked|no longer paired/i).filter({ visible: true }).first()).toBeVisible({ timeout: 15_000 });
    await expect(page.locator("#paired-machines-toggle")).toContainText("Atlas");
    await elsewhere.close();
  });
});

/**
 * cas-4634: the hub records an installation as enrolled only after verifying
 * the cloud's enrollment assertion (hub/auth/account.rs). The inventory then
 * says so in plain words, on a desktop and a phone, light and dark.
 * With QA_ARTIFACTS set, the part that renders the row at every size also
 * writes the cas-qa-craft polish evidence: a standalone snapshot of the
 * dialog and the three a11y modes.
 */
test("HUB-J2 an installation the hub verified reads as in the operator inbox (cas-4634)", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS] });
  await journey.open();
  const row = page.locator(".installation-inventory-row");
  await journey.stage("M01 a newly paired browser is not in an operator inbox", async () => {
    await pair(page, "e".repeat(43));
    await inventory(page);
    await expect(row).toContainText("AccountNot in an operator inbox");
    await page.locator(".installation-inventory").getByRole("button", { name: "Close", exact: true }).click();
  });
  await journey.stage("M02 after the hub verifies its enrollment, the row names the operator inbox", async () => {
    const id = [...hub.installations.keys()][0]!;
    hub.accountEnrollments.set(id, { state: "enrolled", account_id: "acct-1", relay_device_id: "dev-relay-1", grant_generation: "2", feed_generation: "1", epoch: "4", verified_at: "2026-10-05T21:00:00Z" });
    await inventory(page);
    await expect(row).toContainText("AccountIn your operator inbox");
    await expect(row, "no account or relay ID is shown as if it were a name").not.toContainText("acct-1");
    const technical = row.locator("details");
    await technical.locator("summary").click();
    await expect(technical.locator("div").filter({ has: page.getByText("Account key epoch", { exact: true }) }).locator("dd")).toHaveText("4");
    await technical.locator("summary").click();
  });
});

test("HUB-J2 a verified installation's inbox row reads whole on desktop and phone, light and dark (cas-4634)", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS] });
  await journey.open();
  const qa = process.env.QA_ARTIFACTS;
  const row = page.locator(".installation-inventory-row");
  // cas-f7c5: the enrolled row, as the part above reaches it and asserts it.
  await pair(page, "e".repeat(43));
  const id = [...hub.installations.keys()][0]!;
  hub.accountEnrollments.set(id, { state: "enrolled", account_id: "acct-1", relay_device_id: "dev-relay-1", grant_generation: "2", feed_generation: "1", epoch: "4", verified_at: "2026-10-05T21:00:00Z" });
  await inventory(page);
  await expect(row).toContainText("AccountIn your operator inbox");
  for (const size of [{ name: "desktop", width: 1280, height: 800 }, { name: "phone", width: 390, height: 844 }]) {
    for (const scheme of ["light", "dark"] as const) {
      await journey.stage(`M03 the enrolled row reads whole at ${size.width}px in ${scheme}`, async () => {
        await page.setViewportSize(size);
        await page.emulateMedia({ colorScheme: scheme });
        await page.evaluate((value) => { document.documentElement.dataset.scheme = value; }, scheme);
        const account = row.locator(".installation-summary dd").last();
        await expect(account).toHaveText("In your operator inbox");
        await account.scrollIntoViewIfNeeded();
        await expect(account).toBeInViewport();
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "no sideways scroll").toBe(true);
        if (qa) await page.screenshot({ path: `${qa}/enrolled-${scheme}-${size.name}.png` });
      });
    }
  }
  if (qa) {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.emulateMedia({ colorScheme: "light" });
    const { readFile, writeFile } = await import("node:fs/promises");
    const css = await readFile(new URL("../../dist/app.css", import.meta.url), "utf8");
    const dialog = await page.locator(".installation-inventory").evaluate((node) => node.outerHTML);
    await writeFile(`${qa}/enrolled.html`, `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Browser installations</title><style>${css}</style></head><body>${dialog}</body></html>`);
    const modes: Array<[string, string, Parameters<typeof page.emulateMedia>[0]]> = [
      ["forced-colors", "(forced-colors: active)", { forcedColors: "active", reducedMotion: null, contrast: null }],
      ["reduced-motion", "(prefers-reduced-motion: reduce)", { forcedColors: null, reducedMotion: "reduce", contrast: null }],
      ["contrast-more", "(prefers-contrast: more)", { forcedColors: null, reducedMotion: null, contrast: "more" }],
    ];
    for (const [name, query, media] of modes) {
      await page.emulateMedia(media);
      expect(await page.evaluate((q) => matchMedia(q).matches, query)).toBe(true);
      await expect(row.locator(".installation-summary dd").last()).toHaveText("In your operator inbox");
      await page.screenshot({ path: `${qa}/a11y-${name}.png` });
    }
  }
});
