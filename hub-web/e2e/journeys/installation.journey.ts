import { test, expect, journeyPart } from "./journey";
import type { Page } from "@playwright/test";
import { ATLAS } from "./world";
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
  if (!await page.locator("#paired-machines-dialog").isVisible()) await page.locator("#paired-machines-toggle").click();
  await page.getByRole("button", { name: "Browser installations on Atlas", exact: true }).click();
  await expect(page.locator(".installation-inventory").getByText("This browser's access.", { exact: false })).toBeVisible();
}

test("HUB-J2 possession-proven repairs, actual IndexedDB tabs, cancellation and inventory (cas-5e53)", journeyPart, async ({ page, context, journey }) => {
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
    await expect(page.locator(".installation-inventory-row")).toContainText("Generation 5");
    await expect(page.locator(".installation-inventory-row")).toContainText("Un-enrolled");
    await page.locator(".installation-inventory").getByRole("button", { name: "Close", exact: true }).click();
  });

  const peer = await context.newPage();
  // Force stale-request adoption rather than letting BroadcastChannel update
  // the peer first. Both clients still share the real IDB and real Web Lock.
  await peer.addInitScript(() => { Object.defineProperty(window, "BroadcastChannel", { value: undefined }); });
  await hub.install(peer);
  await peer.goto(page.url());
  await expect(peer.locator("#paired-machines-toggle")).toBeAttached();
  await journey.stage("M02 another open tab adopts the accepted credential on refusal", async () => {
    await pair(page, "6".repeat(43));
    await inventory(peer);
    await expect(peer.locator(".installation-inventory-row")).toContainText("Generation 6");
    expect(hub.staleInstallationRefusals.length).toBeGreaterThan(0);
    expect(hub.installations.size).toBe(1);
    await peer.locator(".installation-inventory").getByRole("button", { name: "Close", exact: true }).click();
    await peer.close();
  });

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
    expect(hub.installations.get(id!)!.credential_generation).toBe(6);
    await page.reload();
    await inventory(page);
    await expect(page.locator(".installation-inventory-row")).toContainText("Generation 6");
    await page.locator(".installation-inventory").getByRole("button", { name: "Close", exact: true }).click();
  });

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
          await expect(page.locator(".installation-inventory-row code")).toHaveText(id!);
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
        await expect(page.locator(".installation-inventory-row code")).toHaveText(id!);
        await page.screenshot({ path: join(qa, `a11y-${name}.png`) });
      }
      await page.emulateMedia({ forcedColors: null, reducedMotion: null, contrast: null });
    }
    await expect(page.locator(".installation-inventory-row code")).toHaveText(id!);
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
    await page.reload();
    await expect(page.getByText("Pair a machine to start your first conversation.")).toBeVisible();
    await expect(page.locator("body")).toMatchAriaSnapshot(`
      - complementary "Supervisor conversations":
        - text: Cassy Cloud
        - button "Pair a machine"
        - button "Appearance & commands"
        - heading "Conversations" [level=1]
        - paragraph: Your projects. Your supervisors.
        - navigation "Choose a supervisor"
        - status
        - text: Pair a machine to start your first conversation.
        - button "0 paired machines Not paired"
    `);
  });
});

test("HUB-J2 two expired tabs share one refresh before the next repair (cas-5e53)", journeyPart, async ({ page, context, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS] });
  await journey.open();
  await pair(page, "8".repeat(43));
  hub.expireInstallation([...hub.installations.keys()][0]!);
  const url = page.url();
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
  const peer = await context.newPage(); await hub.install(peer);
  await journey.stage("M01 expired tabs serialize their refresh through real Web Locks and IndexedDB", async () => {
    await Promise.all([page.goto(url), peer.goto(url)]);
    await inventory(page); await inventory(peer);
    await expect(page.locator(".installation-inventory-row")).toContainText("Generation 2");
    await expect(peer.locator(".installation-inventory-row")).toContainText("Generation 2");
    expect(hub.installationRefreshes).toBe(1);
    expect(hub.installations.size).toBe(1);
  });
  await journey.stage("M02 the next repair uses the accepted refresh generation", async () => {
    await page.locator(".installation-inventory").getByRole("button", { name: "Close", exact: true }).click();
    await pair(page, "9".repeat(43));
    expect((hub.exchanges.at(-1)!.installation as { expected_generation: number }).expected_generation).toBe(2);
    expect([...hub.installations.values()][0]!.credential_generation).toBe(3);
    await inventory(page);
    await expect(page.locator(".installation-inventory-row")).toContainText("Generation 3");
  });
  await peer.close();
});
