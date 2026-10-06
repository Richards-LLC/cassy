#!/usr/bin/env node
// Disposable account only. Pair with the ignored native live producer probe.
// The allowed hub origin serves this checkout's dist through browser routing;
// every operator API request goes to the deployed cloud without interception.
import assert from "node:assert/strict";
import { readFile, writeFile, lstat, mkdir } from "node:fs/promises";
import { resolve, extname, basename } from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";
import { chromium, expect } from "@playwright/test";

const cloud = "https://petra-stella-cloud.vercel.app";
const hub = "https://hub.petrastella.io";
const required = (name) => { assert(process.env[name], `missing ${name}`); return process.env[name]; };
assert.equal(required("CAS_E3DD_LIVE"), "1");
const root = resolve(required("CAS_E3DD_LIVE_ROOT"));
assert(basename(root).startsWith("cas-e3dd-live-"));
const keyFile = required("CAS_E3DD_API_KEY_FILE");
const meta = await lstat(keyFile);
assert(meta.isFile() && (meta.mode & 0o777) === 0o600, "API key must be a regular 0600 file");
const bearer = (await readFile(keyFile, "utf8")).trim();
const accountId = required("CAS_E3DD_ACCOUNT_ID");
const artifacts = resolve(required("CAS_E3DD_LIVE_ARTIFACTS"));
await mkdir(artifacts, { recursive: true });
const dist = fileURLToPath(new URL("../dist/", import.meta.url));
const head = execFileSync("git", ["rev-parse", "HEAD"], { cwd: dist, encoding: "utf8" }).trim();
async function authority(path, body) {
  const response = await fetch(`${cloud}${path}`, {
    method: body ? "POST" : "GET",
    headers: { Authorization: `Bearer ${bearer}`, ...(body ? { "Content-Type": "application/json" } : {}) },
    body: body ? JSON.stringify(body) : undefined,
    signal: AbortSignal.timeout(15_000),
  });
  assert(response.ok, `account request refused: ${response.status} ${path}`);
  return response.json();
}
const identity = await authority("/api/me");
assert.equal(identity.user_id, accountId);
assert.equal(identity.email?.split("@")[0], "cassy-e3dd-test");
async function waitFile(name, timeout = 60_000) {
  const until = Date.now() + timeout;
  for (;;) {
    try { return await readFile(resolve(root, name), "utf8"); }
    catch (error) { if (error.code !== "ENOENT") throw error; }
    assert(Date.now() < until, `native probe did not write ${name}`);
    await new Promise((done) => setTimeout(done, 1000));
  }
}
const ready = JSON.parse(await waitFile("ready.json"));
assert.equal(ready.account_id, accountId);
assert(ready.label.startsWith("cassy-e3dd-test-"));
const signal = (name) => writeFile(resolve(root, name), new Date().toISOString());
const browser = await chromium.launch({ channel: "chromium" });
const devices = [];
let latest;
let receipt = { head_sha: head, account_id: accountId, machine_id: ready.machine_id,
  build: "local committed dist served at allowed origin by browser routing",
  cloud: "deployed operator API; no protocol double or clock override", status: "NOT VERIFIED" };
try {
  async function enroll(width, suffix, manage) {
    const context = await browser.newContext({ viewport: { width, height: width === 390 ? 844 : 800 } });
    // Fulfill ALL requests to the hub host locally. No request reaches a real
    // hub, and no installed user's browser profile or enrollment is opened.
    await context.route(`${hub}/**`, async (route) => {
      const path = new URL(route.request().url()).pathname;
      if (!path.startsWith("/commander/")) { await route.fulfill({ status: 404, body: "" }); return; }
      const relative = path.slice("/commander/".length) || "index.html";
      const target = resolve(dist, relative);
      assert(target.startsWith(dist), "invalid local bundle path");
      const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml" };
      await route.fulfill({ contentType: types[extname(target)] ?? "application/octet-stream", body: await readFile(target) });
    });
    const page = await context.newPage();
    // Capture the exact completed device ID before any later UI assertion can
    // fail, so cleanup does not depend on another inventory request succeeding.
    let completion;
    page.on("response", async (response) => {
      if (!/\/api\/operator\/enrollments\/[^/]+\/complete$/.test(response.url()) || !response.ok()) return;
      completion = response.json().then((body) => {
        assert(body.device_id, "completion omitted the test device id");
        devices.push(body.device_id);
      });
      await completion;
    });
    page.on("response", async (response) => {
      if (response.url() !== `${cloud}/api/operator/machine-presence` || !response.ok()) return;
      try { const snapshot = await response.json(); latest = snapshot.machines?.find((machine) => machine.machine_id === ready.machine_id) ?? latest; }
      catch { /* assertions below require the actual cloud snapshot */ }
    });
    await page.goto(`${hub}/commander/`);
    await page.getByRole("button", { name: "Operator inbox", exact: true }).click();
    const inbox = page.getByRole("dialog", { name: "Operator inbox" });
    const label = `${ready.label}-${suffix}`;
    await inbox.getByLabel("Name this browser").fill(label);
    await inbox.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(inbox.getByLabel("Sign-in code")).toHaveText(/^[A-HJ-NP-Z2-9]{4}-[A-HJ-NP-Z2-9]{4}$/);
    const code = (await inbox.getByLabel("Sign-in code").textContent()).trim();
    await authority("/api/operator/enrollments/approve", { wire_version: 1, user_code: code, decision: "approve",
      capabilities: ["feed:read", ...(manage ? ["account:manage"] : [])], scopes: [],
      consent: { custody: "cloud_account_permission", not_end_to_end_acknowledged: true, retention_days: 90 } });
    await expect(inbox.locator(`[data-machine-presence="${ready.machine_id}"]`)).toBeVisible({ timeout: 30_000 });
    assert(completion, "test enrollment did not complete");
    await completion;
    const principals = await authority("/api/operator/principals");
    const device = principals.devices.find((entry) => entry.label === label);
    assert(device?.device_id, "new test browser missing from account principals");
    assert(devices.includes(device.device_id), "inventory did not match the completed test device");
    return { context, page, inbox, row: inbox.locator(`[data-machine-presence="${ready.machine_id}"]`) };
  }
  const phone = await enroll(390, "phone", true);
  const desk = await enroll(1280, "desk", false);
  await expect(phone.row.getByText("Monitoring is off", { exact: true })).toBeVisible();
  await phone.row.locator("summary").press("Enter");
  await phone.row.getByRole("button", { name: `Enable alerts for ${ready.label}`, exact: true }).press("Enter");
  await expect(phone.row.getByText("Waiting for the first report", { exact: true })).toBeVisible();
  await signal("start");
  await expect(phone.row.getByText("Reporting to Cassy Cloud", { exact: true })).toBeVisible({ timeout: 90_000 });
  await phone.page.screenshot({ path: resolve(artifacts, "live-reporting-phone.png") });
  await signal("stop");
  receipt.producer_stopped_at = (await waitFile("producer-stopped", 30_000)).trim();
  const notices = desk.inbox.getByRole("list", { name: "Machine alert history" });
  await expect(notices.getByText(`${ready.label} unreachable`, { exact: true })).toBeVisible({ timeout: 330_000 });
  await expect(notices.getByText(`${ready.label} unreachable`, { exact: true })).toHaveCount(1);
  assert(latest?.open_outage && latest.last_report_at, "cloud outage snapshot missing");
  receipt.last_report_at = latest.last_report_at;
  receipt.outage_opened_at = latest.open_outage.opened_at;
  receipt.outage_event_id = latest.open_outage.unobserved_event_id;
  receipt.detected_after_last_report_ms = Date.parse(receipt.outage_opened_at) - Date.parse(receipt.last_report_at);
  assert(receipt.detected_after_last_report_ms >= 240_000 && receipt.detected_after_last_report_ms <= 300_000, "watchdog missed the four-to-five-minute server bound");
  await desk.page.screenshot({ path: resolve(artifacts, "live-outage-desk.png") });
  await desk.page.reload();
  await desk.page.getByRole("button", { name: "Operator inbox", exact: true }).click();
  await expect(notices.locator(`[data-presence-event="${receipt.outage_event_id}"]`)).toHaveCount(1);
  await expect(desk.inbox.getByRole("button", { name: "Sign in", exact: true })).toHaveCount(0);
  await signal("restart");
  const recovered = notices.getByText(`${ready.label} recovered`, { exact: true });
  await expect(recovered).toBeVisible({ timeout: 90_000 });
  await expect(recovered).toHaveCount(1);
  await expect(notices.locator(`[data-ref-event="${receipt.outage_event_id}"]`)).toHaveCount(1);
  await expect(notices.getByText(`${ready.label} unreachable`, { exact: true })).toHaveCount(1);
  await expect(desk.inbox.locator(".operator-inbox-bubble")).toHaveCount(0);
  await desk.page.screenshot({ path: resolve(artifacts, "live-recovery-desk.png") });
  receipt.status = "PASS";
} finally {
  // Unblock all native waits even if browser proof fails. The native probe
  // revokes its own machine; this driver revokes only the two IDs it created.
  for (const name of ["start", "stop", "restart", "finish"]) await signal(name);
  const failures = [];
  for (const id of devices) {
    try { await authority(`/api/operator/devices/${id}/revoke`, { wire_version: 1 }); }
    catch { failures.push(id); }
  }
  receipt.device_ids = devices;
  receipt.cleanup_failed_device_ids = failures;
  await browser.close();
  await writeFile(resolve(artifacts, "live-receipt.json"), `${JSON.stringify(receipt, null, 2)}\n`);
  assert.equal(failures.length, 0, "test device revocation failed; see receipt ids");
}
console.log(`live presence: ${receipt.status}; ${receipt.detected_after_last_report_ms} ms; artifacts ${artifacts}`);
