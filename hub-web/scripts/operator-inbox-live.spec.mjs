// Real deployed Cloud, real CLI enrollment/outbox/admission, two independent
// Chromium profiles. Only committed Commander assets are routed locally.
import { test, expect } from "@playwright/test";
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { join, extname } from "node:path";
import { liveOptions, COMMANDER_ORIGIN, CLOUD_ORIGIN, commanderAssetPath } from "./operator-inbox-live-safety.mjs";

const options = liveOptions(process.env);
const qa = join(options.artifacts, "qa");
const control = `http://127.0.0.1:${process.env.CAS_2077_CONTROL_PORT ?? 29935}`;
const dist = fileURLToPath(new URL("../dist/", import.meta.url));
const head = execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim();
async function native(path, body = {}) {
  const response = await fetch(`${control}${path}`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body), signal: AbortSignal.timeout(90_000) });
  const value = await response.json();
  expect(response.ok, value.error ?? `native ${path}`).toBe(true);
  return value;
}
async function storage(page) {
  // Read-only metadata; never export grants, private CryptoKeys or ciphertext.
  return page.evaluate(async () => {
    const opened = indexedDB.open("cas-operator-inbox-v1");
    const db = await new Promise((ok, fail) => { opened.onsuccess = () => ok(opened.result); opened.onerror = () => fail(opened.error); });
    const names = ["identity", "events", "epochs", "commands", "cursors"];
    const tx = db.transaction(names, "readonly");
    const get = (name) => new Promise((ok, fail) => { const req = tx.objectStore(name).getAll(); req.onsuccess = () => ok(req.result); req.onerror = () => fail(req.error); });
    const [identities, events, epochs, commands, cursors] = await Promise.all(names.map(get));
    const identity = identities.find((x) => x.deviceId);
    db.close();
    return {
      deviceId: identity?.deviceId ?? null,
      grantGeneration: identity?.grant.generation ?? null,
      identityCount: identities.length,
      epochs: epochs.map((x) => x.epoch),
      events: events.map((x) => ({ eventId: x.eventId, hubId: x.hubId, epoch: x.keyEpoch, verification: x.verification, acked: x.acked })),
      commands: commands.map((x) => ({ commandId: x.commandId, historyEventId: x.historyEventId, state: x.state })),
      cursors: cursors.map((x) => ({ cursor: x.cursor, syncedCursor: x.syncedCursor })),
    };
  });
}

test("cas-2077 deployed Cloud: reply upgrade, stopped hub replay, one offline admission, revoke and rotated epoch", async ({ browser, page, context }) => {
  await mkdir(qa, { recursive: true });
  const secrets = [], receipt = { head_sha: head, cloud: CLOUD_ORIGIN, candidate_origin: COMMANDER_ORIGIN, asset_routing: "local committed dist only; Cloud API never intercepted", clock: "real wall time", status: "NOT VERIFIED", cells: [], reply_upgrade: [] };
  let other;
  const capture = async (reader, cell, expected) => {
    await reader.screenshot({ path: join(qa, `${cell}.png`) });
    receipt.cells.push({ id: cell, expected, result: "PASS", label: "real-build" });
    await writeFile(join(options.artifacts, "live-receipt.json"), JSON.stringify(receipt, null, 2));
  };
  const observe = (reader) => {
    reader.on("response", async (response) => {
      if (!response.url().startsWith(`${CLOUD_ORIGIN}/api/operator/`)) return;
      try {
        const value = await response.json();
        // These stay in the private disposable root, consumed by trace scrub.
        const collect = (object) => {
          if (!object || typeof object !== "object") return;
          for (const [key, entry] of Object.entries(object)) {
            if (/^(poll_secret|user_code|grant|jwt|token|access_token)$/.test(key) && typeof entry === "string") secrets.push(entry);
            else if (typeof entry === "object") collect(entry);
          }
        };
        collect(value);
        const path = new URL(response.url()).pathname;
        if (path === "/api/operator/grants/me") receipt.reply_upgrade.push({ path, status: response.status(), features: value.features, grant_id: value.grant?.grant_id, generation: value.grant?.grant_generation, origin: value.grant?.origin ?? null });
        else if (path.startsWith("/api/operator/reply-grant-requests")) receipt.reply_upgrade.push({ path, method: response.request().method(), status: response.status(), request: value.request });
      } catch {}
    });
  };
  const routeAssets = async (profile) => profile.route(`${COMMANDER_ORIGIN}/**`, async (route) => {
    const target = commanderAssetPath(dist, route.request().url());
    if (!target) { await route.fulfill({ status: 404, body: "" }); return; }
    try { await route.fulfill({ contentType: ({ ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml" })[extname(target)] ?? "application/octet-stream", body: await readFile(target) }); }
    catch { await route.fulfill({ status: 404, body: "" }); }
  });
  const enroll = async (reader, suffix, reply, manage = false) => {
    observe(reader);
    await reader.goto(`${COMMANDER_ORIGIN}/commander/`);
    await reader.getByRole("button", { name: "Operator inbox", exact: true }).click();
    const inbox = reader.getByRole("dialog", { name: "Operator inbox" });
    await inbox.getByLabel("Name this browser").fill(`${receipt.label}-${suffix}`);
    await inbox.getByLabel("Name this browser").press("Enter");
    await expect(inbox.getByLabel("Sign-in code")).toHaveText(/^[A-HJ-NP-Z2-9]{4}-[A-HJ-NP-Z2-9]{4}$/);
    const code = (await inbox.getByLabel("Sign-in code").textContent()).trim(); secrets.push(code);
    await native("/approve", { code, reply, manage });
    await expect(inbox.getByRole("button", { name: "Sign out of the inbox", exact: true })).toBeVisible();
    await expect.poll(async () => (await storage(reader)).deviceId).not.toBe(null);
    return inbox;
  };
  const openThread = async (inbox) => inbox.getByRole("button", { name: new RegExp(`${receipt.label} ·`) }).click();
  try {
    Object.assign(receipt, await native("/prepare"));
    const messages = ["Release started.", "Cloud contract ready.", "Ready for your go."].map((x) => `${receipt.label}: ${x}`);
    await routeAssets(context); await page.setViewportSize({ width: 1280, height: 800 });
    const desk = await enroll(page, "desk", false, true);
    receipt.older_device_id = (await storage(page)).deviceId;
    for (const message of messages) await native("/queue", { message });
    await openThread(desk);
    for (const message of messages) await expect(desk.getByText(message, { exact: true })).toBeVisible();
    receipt.drained = await native("/evidence");
    expect(receipt.drained.outbox).toHaveLength(3);
    expect(receipt.drained.outbox.every((x) => x.receipt_kind === "stored" || x.receipt_kind === "already_stored")).toBe(true);
    // Older profile is offline before the phone exists. The hub was never
    // needed for enrollment/replay; explicitly start then stop it after drain.
    await native("/start"); receipt.stopped = await native("/stop");
    await context.setOffline(true);
    other = await browser.newContext({ viewport: { width: 390, height: 844 } });
    await routeAssets(other);
    const phone = await other.newPage(), mobile = await enroll(phone, "phone", false);
    receipt.new_device_id = (await storage(phone)).deviceId;
    await openThread(mobile);
    for (const message of messages) { await expect(mobile.getByText(message, { exact: true })).toBeVisible(); await expect(mobile.getByText(message, { exact: true })).toHaveCount(1); }
    await capture(phone, "M01", "A new phone recovers retained supervisor messages while the hub and older browser are off.");

    const beforeUpgrade = await storage(phone);
    await expect(mobile.getByRole("button", { name: "Queue reply", exact: true })).toHaveCount(0);
    await mobile.getByRole("button", { name: "Ask to reply", exact: true }).press("Enter");
    await expect(mobile.getByText("Waiting for the account owner to allow replies. Your messages stay here.", { exact: true })).toBeVisible();
    await capture(phone, "A01", "The read-only phone asks without signing out; pending grants no reply permission.");
    await context.setOffline(false);
    await page.reload(); await page.getByRole("button", { name: "Operator inbox", exact: true }).click();
    await desk.getByRole("region", { name: "Reply requests", exact: true }).getByRole("button", { name: new RegExp(`^Allow ${receipt.label}-phone`) }).press("Enter");
    await expect(desk.getByText(`Replying allowed for ${receipt.label}-phone.`, { exact: true })).toBeVisible();
    await expect(mobile.getByRole("button", { name: "Queue reply", exact: true })).toBeVisible();
    const afterUpgrade = await storage(phone);
    expect(afterUpgrade.deviceId).toBe(beforeUpgrade.deviceId);
    expect(BigInt(afterUpgrade.grantGeneration)).toBe(BigInt(beforeUpgrade.grantGeneration) + 1n);
    expect(afterUpgrade.epochs).toEqual(beforeUpgrade.epochs);
    for (const message of messages) await expect(mobile.getByText(message, { exact: true })).toHaveCount(1);
    await capture(page, "A02", "The independent owner allows the same device and one generation increment; retained keys and history survive.");

    const reply = `${receipt.label}: Go. Cut the release.`;
    await mobile.getByLabel(/Reply — .* gets it when it’s back/).fill(reply);
    await mobile.getByRole("button", { name: "Queue reply", exact: true }).click();
    await expect(mobile.locator('[data-state="pending_machine"]')).toBeVisible();
    receipt.offline = await storage(phone);
    expect(receipt.offline.commands).toHaveLength(1);
    receipt.command_id = receipt.offline.commands[0].commandId;
    expect((await native("/evidence")).admissions).toHaveLength(0);
    await capture(phone, "M02", "My reply waits for the stopped machine; no native command is admitted yet.");

    await context.setOffline(false);
    await page.reload(); await page.getByRole("button", { name: "Operator inbox", exact: true }).click(); await openThread(desk);
    for (const message of [...messages, reply]) await expect(desk.getByText(message, { exact: true })).toHaveCount(1);
    await expect(desk.getByText(/can read this conversation but not reply/)).toBeVisible();
    await expect.poll(async () => (await storage(page)).events.filter((x) => x.hubId === receipt.hub_id).every((x) => x.acked)).toBe(true);
    await capture(page, "M03", "The read-only desktop receives each message and queued reply once after revisiting.");

    await native("/start");
    await expect(mobile.locator('[data-state="accepted"]')).toBeVisible({ timeout: 60_000 });
    receipt.admitted = await native("/evidence");
    expect(receipt.admitted.admissions).toHaveLength(1);
    expect(receipt.admitted.prompts).toHaveLength(1);
    expect(receipt.admitted.admissions[0]).toMatchObject({ command_id: receipt.command_id, device_id: receipt.new_device_id });
    expect(receipt.admitted.admissions[0].receipt_sent_at).not.toBe(null);
    await native("/stop"); await native("/start");
    await expect(mobile.locator('[data-state="accepted"]')).toBeVisible();
    // Give the restarted real 15s drain a tick before asserting dedupe.
    // Real time is required for the native 15s drain and Cloud PoP expiry.
    // A fixed observation window avoids recording deliberate failed Expect
    // attempts while waiting; this does not replace an asynchronous UI wait.
    await page.waitForTimeout(16_000);
    receipt.restarted = await native("/evidence");
    expect(receipt.restarted.admissions).toHaveLength(1);
    expect(receipt.restarted.prompts).toHaveLength(1);
    expect(receipt.restarted.admissions[0]).toEqual(receipt.admitted.admissions[0]);
    await capture(phone, "M04", "The returning machine receives my reply once, including after a hub restart.");

    const refusal = page.waitForResponse(async (response) => response.url().startsWith(`${CLOUD_ORIGIN}/api/operator/`) && response.status() === 401 && (await response.json()).error === "grant_revoked", { timeout: 30_000 });
    receipt.revocation = await native("/revoke", { id: receipt.older_device_id });
    const refused = await refusal;
    receipt.revoked_request = { status: refused.status(), error: (await refused.json()).error };
    await expect(desk.getByText("This device was signed out of the operator inbox.", { exact: true })).toBeVisible();
    await expect.poll(async () => { const value = await storage(page); return { identity: value.identityCount, keys: value.epochs.length, events: value.events.length }; }).toEqual({ identity: 0, keys: 0, events: 0 });
    await capture(page, "M05", "A revoked browser loses access on its next request and removes its local keys and history.");

    const rotated = `${receipt.label}: Post-rotation message.`;
    await native("/queue", { message: rotated });
    await expect(mobile.getByText(rotated, { exact: true })).toBeVisible();
    receipt.rotated = await native("/evidence");
    const oldEpoch = receipt.drained.outbox[0].key_epoch;
    const next = receipt.rotated.outbox.find((x) => !receipt.drained.outbox.some((before) => before.event_id === x.event_id));
    expect(next?.key_epoch).toBeTruthy(); expect(next.key_epoch).not.toBe(oldEpoch);
    const phoneState = await storage(phone);
    expect(phoneState.events.find((x) => x.eventId === next.event_id)).toMatchObject({ epoch: next.key_epoch, verification: "verified" });
    receipt.epoch_rotation = { from: oldEpoch, to: next.key_epoch, event_id: next.event_id };
    await phone.reload(); await phone.getByRole("button", { name: "Operator inbox", exact: true }).click(); await openThread(mobile);
    for (const message of [...messages, reply, rotated]) await expect(mobile.getByText(message, { exact: true })).toHaveCount(1);
    await expect(mobile.locator('[data-state="accepted"]')).toBeVisible();
    // pin: reviewed accessible inbox heading and sign-out controls after replay.
    await expect(mobile).toMatchAriaSnapshot(`
      - dialog "Operator inbox":
        - heading "Operator inbox" [level=2]
        - button "Close the operator inbox"
        - button "Sign out of the inbox"
    `);
    await capture(phone, "M06", "The remaining phone reads a message encrypted under the new epoch and keeps one copy after reload.");
    await writeFile(join(qa, "final.aria.yml"), await mobile.ariaSnapshot());
    await writeFile(join(qa, "final.aria.json"), JSON.stringify(await mobile.ariaSnapshotJSON(), null, 2));
    for (const size of [{ name: "desktop", width: 1280, height: 800 }, { name: "phone", width: 390, height: 844 }]) {
      for (const scheme of ["light", "dark"]) {
        await phone.setViewportSize(size); await phone.emulateMedia({ colorScheme: scheme });
        await phone.evaluate((value) => { document.documentElement.dataset.scheme = value; }, scheme);
        await expect(mobile.getByText(rotated, { exact: true })).toBeVisible();
        await expect.poll(() => phone.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
        await phone.screenshot({ path: join(qa, `inbox-${scheme}-${size.name}.png`) });
      }
    }
    // Static local render for strict visual QA; no account credential/state.
    const files = await (await import("node:fs/promises")).readdir(dist);
    const css = (await Promise.all(files.filter((x) => x.endsWith(".css")).map((x) => readFile(join(dist, x), "utf8")))).join("\n");
    await writeFile(join(qa, "inbox.html"), `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Live inbox</title><style>${css}</style></head><body>${await mobile.evaluate((node) => node.outerHTML)}</body></html>`);
    receipt.status = "PASS";
  } catch (error) {
    receipt.failure = String(error).split("\n")[0];
    throw error;
  } finally {
    await writeFile(join(options.root, "trace-secrets.json"), JSON.stringify(secrets), { mode: 0o600 }).catch(() => {});
    if (other) await other.close();
    receipt.cleanup = await native("/cleanup");
    await writeFile(join(options.artifacts, "live-receipt.json"), `${JSON.stringify(receipt, null, 2)}\n`);
  }
});
