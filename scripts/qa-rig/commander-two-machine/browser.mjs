#!/usr/bin/env node
// The rig's saved Commander browser: one persistent Chromium profile under
// $RIG/browser-profile that stays paired to both rig hubs. See README.md.
//
//   node browser.mjs pair <invitation-url>...   open each one-time invitation and pair
//   node browser.mjs walk <commander-url>       drive Commander journeys, then reload so
//                                               every live machine takes the usage counts
//   node browser.mjs open <commander-url>       print the paired conversation rows
//
// OUT=<dir> receives screenshots and walk.json. Needs `npm ci` in hub-web.
import { mkdir, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const { chromium } = await import(resolve(here, "../../../hub-web/node_modules/playwright/index.mjs"));
const rig = process.env.RIG ?? join(homedir(), ".cas/qa-rig/commander-two-machine");
const out = resolve(process.env.OUT ?? join(rig, "captures"));
const [command, ...args] = process.argv.slice(2);
const steps = [];
const log = (step, ok, observed) => { steps.push({ step, ok, observed }); console.log(`${ok ? "PASS" : "FAIL"} ${step}: ${observed}`); };
const pause = (ms) => new Promise((done) => setTimeout(done, ms));

await mkdir(out, { recursive: true });
const context = await chromium.launchPersistentContext(join(rig, "browser-profile"), {
  headless: true,
  viewport: { width: 1280, height: 800 },
  permissions: ["local-network-access"],
  args: ["--disable-dev-shm-usage"],
});
for (const existing of context.pages()) existing.on("crash", () => console.error("renderer crashed"));
// The rig's inert supervisors go quiet; show their sessions rather than hide them as dormant.
await context.addInitScript(() => localStorage.setItem("cas-commander:dormant", "1"));
const page = context.pages()[0] ?? (await context.newPage());
const shot = (name) => page.screenshot({ path: join(out, `${name}.png`) });
const rows = () => page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /project on/ });

try {
  if (command === "pair") {
    for (const [index, url] of args.entries()) {
      await page.goto(url);
      const dialog = page.locator("#pair-dialog");
      await dialog.waitFor({ state: "visible", timeout: 30_000 });
      await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Commander rig");
      await dialog.getByRole("button", { name: "Pair", exact: true }).click();
      await dialog.waitFor({ state: "hidden", timeout: 60_000 });
      await shot(`pair-${index + 1}`);
      log(`pair invitation ${index + 1}`, true, "pairing dialog closed");
    }
  } else if (command === "walk" || command === "open") {
    await page.goto(args[0]);
    await rows().first().waitFor({ timeout: 60_000 });
    await pause(3_000);
    const labels = await rows().allInnerTexts();
    log("rows", true, `${labels.length} conversation rows: ${labels.map((l) => l.replace(/\s+/g, " ").slice(0, 80)).join(" | ")}`);
    await shot(`${command}-01-list`);
    if (command === "walk") {
      // HUB-J3, then HUB-J8: open a conversation on each machine and back.
      for (const index of [0, 1, 0]) { await rows().nth(index).click(); await pause(2_000); }
      log("conversations", true, "opened row 1, row 2, row 1");
      await shot("walk-02-conversation");
      // HUB-J18: Raw output.
      await page.locator("#conversation-raw-output").click({ timeout: 10_000 });
      await pause(1_500); await shot("walk-03-raw-output");
      await page.keyboard.press("Escape");
      log("raw output", true, "Raw output opened");
      // HUB-J10: Appearance · Dark (then back to Light, a second change).
      for (const scheme of ["dark", "light"]) {
        await page.locator("#command-palette-toggle").click({ timeout: 10_000 });
        await page.locator(`[data-palette-scheme="${scheme}"]`).click({ timeout: 10_000 });
        await pause(800);
        if (scheme === "dark") await shot("walk-04-dark");
      }
      log("appearance", true, `scheme now ${await page.evaluate(() => document.documentElement.dataset.scheme)}`);
      // HUB-J19: Operator inbox.
      await page.locator("#inbox-toggle").click({ timeout: 10_000 });
      await pause(1_500); await shot("walk-05-inbox");
      await page.keyboard.press("Escape");
      log("inbox", true, "Operator inbox opened");
      log("counts before reload", true, await page.evaluate(() => localStorage.getItem("commander.usage.v1")));
      // Each machine's return to live flushes the days it has not accepted.
      await page.reload();
      await rows().first().waitFor({ timeout: 60_000 });
      await pause(10_000);
      await shot("walk-06-after-reload");
      log("counts after reload", true, await page.evaluate(() => localStorage.getItem("commander.usage.v1")));
    }
  } else {
    console.error("usage: browser.mjs pair <invitation-url>... | walk <commander-url> | open <commander-url>");
    process.exitCode = 2;
  }
} catch (error) {
  log(command ?? "browser", false, String(error?.message ?? error).slice(0, 400));
  await shot(`${command}-failure`).catch(() => {});
  process.exitCode = 1;
} finally {
  await writeFile(join(out, `${command}.json`), JSON.stringify(steps, null, 2));
  await context.close();
}
