import { test, expect, journeyPart } from "./journey";
import type { Locator, Page } from "@playwright/test";
import { ATLAS } from "./world";
import type { HubDouble } from "./hub-double";
import { showConversationList } from "./responsive-goals";
import { mkdir, writeFile, readFile } from "node:fs/promises";
import { join } from "node:path";

const pairDialog = (page: Page) => page.getByRole("dialog", { name: "Pair a machine", exact: true });
const pairedMachinesDialog = (page: Page) => page.getByRole("dialog", { name: "Paired machines", exact: true });
// The footer opener reads "<machine> <state>", or "Paired machines Loading…" before the catalog.
const pairedMachinesToggle = (page: Page) => page.getByRole("complementary", { name: "Supervisor conversations" })
  .getByRole("button", { name: /^(Atlas\b|Paired machines )/ });
const inventorySheet = (page: Page) => page.getByRole("dialog", { name: / browser installations$/ });
const inventoryRows = (page: Page) => inventorySheet(page).getByTestId("installation-inventory-row");
const inventoryClose = (page: Page) => inventorySheet(page).getByRole("button", { name: "Close", exact: true });
/** A row's "Technical details" disclosure and its toggle. */
const technicalDetails = (row: Locator) => row.getByRole("group");
const technicalToggle = (technical: Locator) => technical.getByText("Technical details", { exact: true });
const conversationRows = (page: Page) => page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /^\S+ on / });

async function pair(page: Page, token: string): Promise<void> {
  if (await pairedMachinesDialog(page).isVisible()) await page.getByRole("button", { name: "Close paired machines", exact: true }).click();
  await page.evaluate((hash) => { location.hash = hash; }, `pair=${token}&hub=atlas&hub_url=https%3A%2F%2Fatlas.test&machine=Atlas&scopes=machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt`);
  const dialog = pairDialog(page);
  await expect(dialog).toBeVisible();
  await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Operator");
  await dialog.getByRole("button", { name: "Pair", exact: true }).click();
  await expect(dialog).toBeHidden();
}
async function inventory(page: Page): Promise<void> {
  if (!await pairedMachinesDialog(page).isVisible()) {
    await showConversationList(page);
    await pairedMachinesToggle(page).click();
  }
  await page.getByRole("button", { name: "Browser installations on Atlas", exact: true }).click();
  await expect(inventorySheet(page).getByRole("status")).toBeVisible();
}

/** Protocol evidence remains available, but never leads the default sheet. */
async function expectGeneration(page: Page, generation: number): Promise<void> {
  const technical = technicalDetails(inventoryRows(page));
  await technicalToggle(technical).click();
  await expect(technical.getByTestId("credential-generation")).toHaveText(String(generation));
  await technicalToggle(technical).click();
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
    await expect(inventoryRows(page)).toHaveCount(1);
    await expectGeneration(page, 5);
    await expect(inventoryRows(page)).toBeVisible();
    await inventoryClose(page).click();
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
  await expect(pairedMachinesToggle(peer)).toBeAttached();
  await journey.stage("M02 another open tab adopts the accepted credential on refusal", async () => {
    await pair(page, "2".repeat(43));
    await inventory(peer);
    await expectGeneration(peer, 2);
    expect(hub.staleInstallationRefusals.length).toBeGreaterThan(0);
    expect(hub.installations.size).toBe(1);
    await inventoryClose(peer).click();
    await peer.close();
  });
});

test("HUB-J2 cancelling a staged repair restores access on reload (cas-5e53)", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS] });
  await journey.open();
  const id = await pairedInstallation(page, hub);
  await journey.stage("M03 cancelling a staged repair restores access on reload", async () => {
    if (await pairedMachinesDialog(page).isVisible()) await page.getByRole("button", { name: "Close paired machines", exact: true }).click();
    let release!: () => void;
    const held = new Promise<void>((resolve) => { release = resolve; });
    let arrived!: () => void;
    const waiting = new Promise<void>((resolve) => { arrived = resolve; });
    await page.route("https://atlas.test/v1/auth/pairing/commit", async (route) => { arrived(); await held; await route.fallback(); });
    await page.evaluate((hash) => { location.hash = hash; }, `pair=${"7".repeat(43)}&hub=atlas&hub_url=https%3A%2F%2Fatlas.test&machine=Atlas&scopes=machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt`);
    const dialog = pairDialog(page);
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
    await inventoryClose(page).click();
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
          await expect(inventoryRows(page).getByRole("heading", { level: 3 })).toBeVisible();
          expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
          await page.screenshot({ path: join(qa, `inventory-${colorScheme}-${size}.png`) });
        }
      }
      const html = await inventorySheet(page).evaluate((dialog) => dialog.outerHTML);
      const css = await readFile("dist/app.css", "utf8");
      await writeFile(join(qa, "inventory.html"), `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>${css}\ndialog{position:fixed;inset:0;margin:auto}</style></head><body>${html}</body></html>`);
      for (const [name, query, media] of [["forced-colors", "(forced-colors: active)", { forcedColors: "active" }], ["reduced-motion", "(prefers-reduced-motion: reduce)", { reducedMotion: "reduce" }], ["contrast-more", "(prefers-contrast: more)", { contrast: "more" }]] as const) {
        await page.emulateMedia({ forcedColors: null, reducedMotion: null, contrast: null });
        await page.emulateMedia(media);
        expect(await page.evaluate((query) => matchMedia(query).matches, query)).toBe(true);

        await page.screenshot({ path: join(qa, `a11y-${name}.png`) });
      }
      await page.emulateMedia({ forcedColors: null, reducedMotion: null, contrast: null });
    }

    const technical = technicalDetails(inventoryRows(page));
    await technicalToggle(technical).click();
    await expect(technical.getByRole("code")).toHaveText(id!);
    await technicalToggle(technical).click();
    const close = inventoryClose(page);
    await close.focus();
    await expect(close).toBeFocused();
    await expect(close).toBeInViewport();
    await close.press("Escape");
    await expect(inventorySheet(page)).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Browser installations on Atlas", exact: true })).toBeFocused();
  });

  await journey.stage("M05 explicit exact-device revocation removes this browser access", async () => {
    await inventory(page);
    page.once("dialog", async (dialog) => { expect(dialog.message()).toContain(id!); await dialog.accept(); });
    await inventorySheet(page).getByRole("button", { name: "Revoke this browser's access", exact: true }).click();
    await expect(inventorySheet(page)).toHaveCount(0);
    expect(hub.installations.get(id!)!.revoked_at).not.toBeNull();
    // cas-d043 G09: the page says what happened and lands on the next step.
    await expect(page.getByRole("status", { name: "Notice" })).toBeVisible();
    await expect(page.locator(":focus")).toHaveAccessibleName(/\S/);
    await page.reload();
    await expect(page.getByTestId("conversation-empty")).toBeVisible();
    await expect(page.getByTestId("conversation-empty")).toHaveAttribute("data-state", "text");
    await expect(conversationRows(page)).toHaveCount(0);
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
    await inventoryClose(page).click();
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
    const dialog = pairDialog(page);
    await expect(dialog).toBeVisible();
    const admin = dialog.getByRole("checkbox", { name: /See and revoke other browsers on Atlas/ });
    await expect(admin).toBeVisible();
    await expect(admin, "admin is never pre-ticked: it needs explicit consent").not.toBeChecked();
    // cas-d043 G08: plain words naming the machine, not the raw "hub:admin";
    // the summary claims the power only once the box is ticked.
    await expect(dialog.getByRole("group", { name: "Hub administration", exact: true })).toContainText("Atlas");
    await expect(dialog.getByTestId("pair-summary-admin")).toBeHidden();
    await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Operator");
  });

  for (const size of [{ width: 1280, height: 800 }, { width: 390, height: 844 }]) {
    for (const scheme of ["light", "dark"] as const) {
      await journey.stage(`The consent reads whole at ${size.width}px in ${scheme}`, async () => {
        await page.setViewportSize(size);
        await page.emulateMedia({ colorScheme: scheme });
        const consent = pairDialog(page).getByRole("group", { name: "Hub administration", exact: true });
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
  const row = inventoryRows(page);
  await journey.stage("M01 a newly paired browser is not in an operator inbox", async () => {
    await pair(page, "e".repeat(43));
    await inventory(page);
    await expect(row).toBeVisible();
    await inventoryClose(page).click();
  });
  await journey.stage("M02 after the hub verifies its enrollment, the row names the operator inbox", async () => {
    const id = [...hub.installations.keys()][0]!;
    hub.accountEnrollments.set(id, { state: "enrolled", account_id: "acct-1", relay_device_id: "dev-relay-1", grant_generation: "2", feed_generation: "1", epoch: "4", verified_at: "2026-10-05T21:00:00Z" });
    await inventory(page);

    await expect(row, "no account or relay ID is shown as if it were a name").not.toContainText("acct-1");
    const technical = technicalDetails(row);
    await technicalToggle(technical).click();
    await expect(technical.getByTestId("epoch")).toHaveText("4");
    await technicalToggle(technical).click();
  });
});
