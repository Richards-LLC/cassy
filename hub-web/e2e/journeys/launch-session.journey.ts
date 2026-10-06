import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { test, expect, journeyPart } from "./journey";
import { SCOPES, type LaunchWorld, type Machine } from "./hub-double";
import { PELICAN } from "./world";

/** Atlas with one live supervisor on cas-src; ledger-api and old-notes are idle projects. */
const ATLAS: Machine = {
  id: "atlas",
  label: "Atlas · Linux",
  sessions: [{ name: PELICAN, supervisor: PELICAN, project_dir: "/home/dev/cas-src", workers: [], liveness: "live" }],
};

const target = (id: string) => ({ kind: "project", id });

function atlasLaunch(): LaunchWorld {
  return {
    projects: [
      { id: "p-old", name: "old-notes", path: "/home/dev/old-notes", last_touched_at: "2026-03-01T09:00:00Z", touch_count: 2, running_session: null, target: target("p-old") },
      { id: "p-cas", name: "cas-src", path: "/home/dev/cas-src", last_touched_at: "2026-09-27T18:00:00Z", touch_count: 90, running_session: PELICAN, target: target("p-cas") },
      { id: "p-ledger", name: "ledger-api", path: "/home/dev/ledger-api", last_touched_at: "2026-09-28T08:00:00Z", touch_count: 12, running_session: null, target: target("p-ledger") },
    ],
    browse_roots: [{ id: "root-code", name: "code", path: "/home/dev/code" }],
    browse: {
      "root-code:": { path: "", entries: [{ name: "clients", path: "clients", launchable: false, project_id: null, target: null }] },
      "root-code:clients": { path: "clients", entries: [{ name: "acme-portal", path: "clients/acme-portal", launchable: true, project_id: null, target: { kind: "browse", root_id: "root-code", path: "clients/acme-portal" } }] },
    },
    refuse: { "p-old": { status: 422, error: "not_logged_in", detail: "claude: not logged in for profile main (run `claude /login`)" } },
    // cas-9666: every Claude account on the machine, one logged out, one a long address.
    profiles: {
      claude: { installed: true, profiles: [
        { name: "main", logged_in: true, is_default: true },
        { name: "support@petrastella.io", logged_in: true, is_default: false },
        { name: "customer-success-escalations@petrastella-international.example", logged_in: true, is_default: false },
        { name: "old@petrastella.io", logged_in: false, is_default: false },
      ] },
      codex: { installed: true, profiles: [{ name: "main", logged_in: true, is_default: true }] },
      grok: { installed: true, profiles: [] },
    },
    defaultCli: "claude",
    names: ["bright-heron-21", "quiet-fox-5"],
    bootPolls: 2,
  };
}

test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });

test("HUB-J13 start a new session from Commander", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], launch: { atlas: atlasLaunch() } });
  const sheet = page.getByRole("dialog", { name: "New session" });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const back = page.getByRole("button", { name: "‹ Conversations", exact: true });

  await journey.stage("A paired controller enables launch from Commander", async () => {
    await journey.open();
    await expect(list.getByRole("button", { name: /cas-src/ })).toBeVisible();
    // cas-865c: New session is named by its goal before the permission too; it
    // opens the sheet's grant view, which explains what is asked.
    const toggle = page.locator("#new-session-toggle");
    await expect(toggle).toHaveText("+ New session");
    await expect(toggle).toHaveAttribute("data-launch-grant", "true");
    await toggle.tap();
    await expect(sheet).toBeVisible();
    await expect(sheet.getByRole("button", { name: "Allow starting sessions on Atlas · Linux" })).toBeVisible();
    await expect(sheet.getByRole("button", { name: "Allow starting sessions on Atlas · Linux" })).toBeFocused();
    await sheet.getByRole("button", { name: "Allow starting sessions on Atlas · Linux" }).tap();
    // cas-e123: this single deliberate Allow grants the named machine.
    // A second consent would leave the project picker hidden.
    await expect(sheet.getByRole("button", { name: /^Allow starting sessions/ })).toHaveCount(0);
    await expect(sheet.getByRole("radio", { name: /ledger-api/ })).toBeVisible();
    await expect(sheet.getByRole("searchbox", { name: "Filter projects" })).toBeFocused();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "no sideways scrolling").toBe(true);
    await sheet.getByRole("button", { name: "Cancel", exact: true }).tap();
    await expect(sheet).toBeHidden();
  });

  await journey.stage("The granted scope stays available", async () => {
    await hub.seedPaired();
    await page.reload();
    await expect(page.getByRole("button", { name: "New session", exact: true })).toBeVisible();
    await expect(page.locator("#new-session-toggle")).not.toHaveAttribute("data-launch-grant", "true");
  });

  await journey.stage("Open New session and find the project", async () => {
    await page.getByRole("button", { name: "New session", exact: true }).tap();
    await expect(sheet).toBeVisible();
    // One machine: no machine picker, and the most recent project leads.
    await expect(sheet.getByRole("combobox", { name: "Machine" })).toBeHidden();
    await expect(sheet.locator("#launch-panel-known .launch-row-name")).toHaveText(["ledger-api", "cas-src", "old-notes"]);
    // The running project offers Attach, not a second session.
    await expect(sheet.getByRole("button", { name: `Attach to cas-src (${PELICAN})` })).toBeVisible();
    await expect(sheet.getByRole("tab", { name: "Browse" })).toBeVisible();
    await sheet.getByRole("searchbox", { name: "Filter projects" }).fill("ledger");
    await expect(sheet.locator("#launch-panel-known .launch-row-name")).toHaveText(["ledger-api"]);
    await sheet.getByRole("radio", { name: /ledger-api/ }).check();
    await expect(sheet.getByRole("radio", { name: "Claude Default" })).toBeChecked();
    await expect(sheet.getByText("Start ledger-api with Claude (main) on Atlas · Linux.")).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "no sideways scrolling").toBe(true);
  });

  await journey.stage("Choose the account", async () => {
    const accounts = sheet.getByRole("radiogroup", { name: "Claude accounts" });
    // The machine default is preselected and says so.
    await expect(accounts.getByRole("radio", { name: /^main Default/ })).toBeChecked();
    // A logged-out account is listed but can't be picked, and names its fix.
    await expect(accounts.getByRole("radio", { name: /old@petrastella\.io/ })).toBeDisabled();
    await expect(sheet.locator(".launch-account-out code")).toHaveText("cas claude login old@petrastella.io");
    await expect(sheet.getByRole("button", { name: "Copy login command for old@petrastella.io" })).toBeVisible();
    // A long address wraps inside its row.
    const long = sheet.locator(".launch-account-row").filter({ hasText: "customer-success-escalations" });
    expect(await long.evaluate((row) => row.scrollWidth <= row.clientWidth + 1), "the long account name wraps").toBe(true);
    // Grok runs without an account: the step goes away.
    await sheet.getByRole("radio", { name: "Grok" }).check();
    await expect(sheet.locator(".launch-account")).toBeHidden();
    await sheet.getByRole("radio", { name: /^Claude/ }).check();
    await accounts.getByRole("radio", { name: "support@petrastella.io" }).check();
    await expect(sheet.getByText("Start ledger-api with Claude (support@petrastella.io) on Atlas · Linux.")).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "no sideways scrolling").toBe(true);
  });

  await journey.stage("Start it and land on its supervisor", async () => {
    await sheet.getByRole("button", { name: "Start", exact: true }).tap();
    await expect(sheet.getByRole("status")).toContainText("Starting ledger-api with Claude (support@petrastella.io) on Atlas · Linux…");
    // cas-cee5 (journey F34): the starting line names the project, never the
    // generated codename the operator never chose.
    await expect(sheet.locator(".launch-progress-step")).toHaveText("Waiting for the ledger-api supervisor to come up on Atlas · Linux.");
    await expect(sheet.getByRole("status")).not.toContainText("bright-heron-21");
    await expect(sheet).toBeHidden({ timeout: 15_000 });
    await expect(page.getByRole("button", { name: "Send to the ledger-api supervisor", exact: true })).toBeVisible();
    await expect(page.locator(".conversation-heading")).toContainText("ledger-api");
    expect(hub.launches.at(-1)?.body).toEqual({ target: { kind: "project", id: "p-ledger" }, supervisor_cli: "claude", profile: "support@petrastella.io" });
  });

  await journey.stage("The session outlives the tab", async () => {
    // Closing the tab and coming back: the machine still runs it, and the
    // conversation reopens where the operator left it.
    await page.reload();
    await expect(page.getByRole("button", { name: "Send to the ledger-api supervisor", exact: true })).toBeVisible();
    await back.tap();
    await expect(list.getByRole("button", { name: /ledger-api/ })).toBeVisible();
  });

  await journey.stage("A running project attaches instead of starting again", async () => {
    await page.getByRole("button", { name: "New session", exact: true }).tap();
    await sheet.getByRole("button", { name: `Attach to cas-src (${PELICAN})` }).tap();
    await expect(sheet).toBeHidden();
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
    await back.tap();
  });

  await journey.stage("A launch refused by the machine says why", async () => {
    await page.getByRole("button", { name: "New session", exact: true }).tap();
    await sheet.getByRole("radio", { name: /old-notes/ }).check();
    await sheet.getByRole("button", { name: "Start", exact: true }).tap();
    const alert = sheet.getByRole("alert");
    await expect(alert).toContainText("The Claude account main isn't logged in on Atlas · Linux.");
    // cas-cee5 (journey F32): the login command is code with Copy, as a
    // logged-out account row shows it, not prose to retype.
    await expect(alert).toContainText("Run this on Atlas · Linux, or pick another account, then start again.");
    await expect(alert.locator(".launch-error-command code")).toHaveText("cas claude login main");
    await alert.getByRole("button", { name: "Copy cas claude login main", exact: true }).tap();
    await expect(alert.getByRole("button", { name: "Copy cas claude login main", exact: true })).toHaveText("Copied");
    await sheet.getByText("The machine's message").tap();
    await expect(sheet.getByText("claude: not logged in for profile main")).toBeVisible();
    await sheet.getByRole("button", { name: "Back", exact: true }).tap();
    await expect(sheet.getByRole("radio", { name: /old-notes/ })).toBeChecked();
  });

  await journey.stage("Browse a launch folder and start a repository in it", async () => {
    await sheet.getByRole("tab", { name: "Browse" }).tap();
    await sheet.getByRole("button", { name: /clients/ }).tap();
    await expect(sheet.locator(".launch-crumbs")).toContainText("clients");
    await sheet.getByRole("radio", { name: /acme-portal/ }).check();
    await sheet.getByRole("button", { name: "Start", exact: true }).tap();
    await expect(sheet).toBeHidden({ timeout: 15_000 });
    await expect(page.getByRole("button", { name: "Send to the acme-portal supervisor", exact: true })).toBeVisible();
    expect(hub.launches.at(-1)?.body).toEqual({ target: { kind: "browse", root_id: "root-code", path: "clients/acme-portal" }, supervisor_cli: "claude", profile: "main" });
  });

  await journey.stage("The grant command reads as whole tokens on a phone, with Supervisor and Workers explained", async () => {
    // cas-cee5 (journey F31): a read-only pairing cannot allow launch from
    // here, so the sheet shows the command to run on the machine. It wraps
    // only between whole tokens at 390px ("pane:i / nput" was the defect).
    hub.setScopes("atlas", ["machine-read", "session-read"]);
    await hub.seedPaired();
    await page.reload();
    await page.locator('#new-session-toggle[data-launch-grant="true"]').tap();
    const code = sheet.locator(".launch-grant-command code");
    await expect(code).toContainText("session:launch");
    const splits = await code.evaluate((node) => {
      const chars: Array<{ char: string; top: number }> = [];
      const walker = document.createTreeWalker(node, NodeFilter.SHOW_TEXT);
      const range = document.createRange();
      for (let text = walker.nextNode(); text; text = walker.nextNode()) {
        const value = text.textContent ?? "";
        for (let index = 0; index < value.length; index += 1) {
          range.setStart(text, index); range.setEnd(text, index + 1);
          chars.push({ char: value[index]!, top: Math.round(range.getClientRects()[0]?.top ?? 0) });
        }
      }
      return chars.flatMap((item, index) => index > 0 && item.top !== chars[index - 1]!.top && chars[index - 1]!.char !== " " && chars[index - 1]!.char !== "," && item.char !== " " ? [`${chars[index - 1]!.char}|${item.char}`] : []);
    });
    expect(splits, "the command breaks only at spaces and after commas").toEqual([]);
    await expect(sheet.locator(".launch-grant-note")).toHaveText("The link re-pairs this browser with the machine: what it can do now is kept, and starting sessions is added.");
    await sheet.getByRole("button", { name: "Close", exact: true }).tap();
    hub.setScopes("atlas", [...SCOPES, "session-launch"]);
    await hub.seedPaired();
    await page.reload();
    // cas-cee5 (journey F33): the hints say what each field changes, and the
    // Workers placeholder agrees with its hint.
    await page.getByRole("button", { name: "New session", exact: true }).tap();
    await sheet.getByRole("tab", { name: "Known projects" }).tap();
    await sheet.getByRole("radio", { name: /old-notes/ }).check();
    await expect(sheet.locator("#launch-cli-hint")).toHaveText("Which assistant runs the supervisor.");
    await expect(sheet.locator("input[name=launch-workers]")).toHaveAttribute("placeholder", "None");
    await expect(sheet.locator("#launch-workers-hint")).toHaveText("Up to 16. None starts the supervisor alone.");
    await sheet.getByRole("button", { name: "Cancel", exact: true }).tap();
  });

  await journey.stage("A long machine name fits the phone consent", async () => {
    const originalLabel = ATLAS.label;
    ATLAS.label = "soundwave — a very long personal workstation name with several extra words and anunbrokentailthatneedstowrap";
    hub.setScopes("atlas", [...SCOPES]);
    await hub.seedPaired();
    await page.reload();
    await page.locator('#new-session-toggle[data-launch-grant="true"]').tap();
    const allow = sheet.getByRole("button", { name: `Allow starting sessions on ${ATLAS.label}` });
    await expect(allow).toBeVisible();
    await expect(sheet.getByRole("button", { name: "Close", exact: true })).toBeVisible();
    expect(await allow.evaluate((button) => button.getBoundingClientRect().right <= innerWidth), "the long button fits the viewport").toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "no sideways scroll").toBe(true);
    // cas-84a2: on a loaded queue runner the tap's touch dispatch never came
    // back while the sheet was still settling after the reload. Tap only once
    // every finite animation on the page has finished (the sheet's own state,
    // not a wall-clock wait), and give the tap its own deadline so a stall
    // names the tap instead of eating the whole test timeout.
    await page.evaluate(() => Promise.all(document.getAnimations()
      .filter((animation) => animation.effect?.getComputedTiming().iterations !== Infinity)
      .map((animation) => animation.finished.catch(() => undefined))));
    await allow.tap({ timeout: 15_000 });
    // Earlier in this journey ledger-api was launched: the project picker
    // now offers its live supervisor, rather than an idle-project radio.
    await expect(sheet.getByRole("button", { name: "Attach to ledger-api (bright-heron-21)", exact: true })).toBeVisible();
    await expect(sheet.getByRole("searchbox", { name: "Filter projects" })).toBeFocused();
    await expect(sheet.getByRole("button", { name: /^Allow starting sessions/ })).toHaveCount(0);
    await sheet.getByRole("button", { name: "Cancel", exact: true }).tap();
    ATLAS.label = originalLabel;
  });
});

// cas-0e14 (journey F30): New session follows the machine's connection. A
// machine that is reconnecting says so in the sheet, in the banner's words,
// instead of failing a project load; once it is back its projects load.
test("HUB-J13 New session says a reconnecting machine is reconnecting, then loads once it's back (cas-0e14)", journeyPart, async ({ page, journey }) => {
  test.setTimeout(120_000);
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], scopes: { atlas: [...SCOPES, "session-launch"] }, launch: { atlas: atlasLaunch() } });
  const sheet = page.getByRole("dialog", { name: "New session" });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });

  await journey.stage("The machine drops while I am about to start a session", async () => {
    await journey.open();
    await expect(page.getByRole("button", { name: "New session", exact: true })).toBeVisible();
    await hub.down("atlas", { sockets: "close" });
    await expect(list.getByRole("button", { name: /cas-src/ })).toContainText("Reconnecting", { timeout: 20_000 });
  });

  await journey.stage("New session says it is reconnecting and offers nothing that can only fail", async () => {
    await page.getByRole("button", { name: "New session", exact: true }).tap();
    await expect(sheet).toBeVisible();
    await expect(sheet.locator('[data-launch-list="known"]')).toHaveText("Lost connection to Atlas · Linux. Reconnecting… Its projects load once it's back.");
    await expect(sheet.getByText(/Couldn't load/)).toHaveCount(0);
    await expect(sheet.getByRole("button", { name: "Start", exact: true })).toHaveAttribute("aria-disabled", "true");
    expect(hub.launches).toEqual([]);
  });

  await journey.stage("Back online, its projects load in the same sheet", async () => {
    await hub.up("atlas");
    await expect(sheet.getByRole("radio", { name: /ledger-api/ })).toBeVisible({ timeout: 30_000 });
    await expect(sheet.locator(".launch-offline")).toHaveCount(0);
    await sheet.getByRole("radio", { name: /ledger-api/ }).tap();
    await expect(sheet.getByRole("button", { name: "Start", exact: true })).toHaveAttribute("aria-disabled", "false");
  });
});

// cas-e123: the consent is the only permission decision, across input and
// display modes. Keep cancellation and read-only invitations explicit.
const consentCells = [
  { id: "M01", width: 1280, dark: false },
  { id: "M02", width: 390, dark: false, long: true },
  { id: "M03", width: 1280, dark: true, cancel: true },
  { id: "M04", width: 390, dark: true, cancel: true },
  { id: "M05", width: 390, dark: false, forced: true },
  { id: "M06", width: 1280, dark: false, contrast: true },
  { id: "M07", width: 390, dark: false, motion: true },
  { id: "M08", width: 1280, dark: false, readonly: true },
];
for (const cell of consentCells) {
  test(`HUB-J13 one launch consent ${cell.id} (cas-e123)`, journeyPart, async ({ page, journey }) => {
    await page.setViewportSize({ width: cell.width, height: 844 });
    await page.emulateMedia({ colorScheme: cell.dark ? "dark" : "light", forcedColors: cell.forced ? "active" : "none", contrast: cell.contrast ? "more" : "no-preference", reducedMotion: cell.motion ? "reduce" : "no-preference" });
    const machine = { ...ATLAS, label: cell.long ? "soundwave — a very long personal workstation name with several extra words and anunbrokentailthatneedstowrap" : ATLAS.label };
    const scopes = cell.readonly ? ["machine-read", "session-read", "pane-read"] : [...SCOPES];
    const hub = await journey.hub({ machines: [machine], paired: ["atlas"], scopes: { atlas: scopes }, launch: { atlas: atlasLaunch() } });
    const sheet = page.getByRole("dialog", { name: "New session" });
    await journey.open();
    await journey.stage(`${cell.id} the machine permission is readable before consent`, async () => {
      await page.getByRole("button", { name: "New session", exact: true }).click();
      await expect(sheet).toBeVisible();
      await expect(sheet.locator(".launch-grant .launch-lead")).toContainText(machine.label);
      expect(hub.scopesFor("atlas")).not.toContain("session-launch");
      const allow = sheet.getByRole("button", { name: `Allow starting sessions on ${machine.label}` });
      if (cell.readonly) {
        await expect(allow).toHaveCount(0);
        await expect(sheet.locator(".launch-grant-command")).toContainText("session:launch");
      } else {
        await expect(allow).toBeFocused();
        expect(await allow.evaluate((el) => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
      }
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      if (process.env.DELIVERY_QA && cell.id === "M01") {
        mkdirSync(process.env.DELIVERY_QA, { recursive: true });
        writeFileSync(join(process.env.DELIVERY_QA, "consent.html"), `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>${readFileSync("dist/app.css", "utf8")}</style></head><body>${await sheet.evaluate(node => node.outerHTML)}</body></html>`);
      }
    });
    await journey.stage(`${cell.id} cancel keeps permission, or one Allow opens projects`, async () => {
      if (cell.readonly) {
        await sheet.getByRole("button", { name: "Close", exact: true }).click();
        await expect(sheet).toBeHidden();
        expect(hub.scopesFor("atlas")).not.toContain("session-launch");
        return;
      }
      if (cell.cancel) {
        if (cell.width === 390) await page.keyboard.press("Escape");
        else await sheet.getByRole("button", { name: "Close", exact: true }).click();
        await expect(sheet).toBeHidden();
        expect(hub.scopesFor("atlas")).not.toContain("session-launch");
        await page.getByRole("button", { name: "New session", exact: true }).click();
      }
      const allow = sheet.getByRole("button", { name: `Allow starting sessions on ${machine.label}` });
      await expect(allow).toBeFocused();
      await page.keyboard.press("Enter");
      const search = sheet.getByRole("searchbox", { name: "Filter projects" });
      await expect(search).toBeFocused();
      await expect(sheet.getByRole("radio", { name: /ledger-api/ })).toBeVisible();
      await expect(sheet.getByRole("button", { name: /^Allow starting sessions/ })).toHaveCount(0);
      await expect(search).toMatchAriaSnapshot('- searchbox "Filter projects"');
      expect(hub.scopesFor("atlas")).toContain("session-launch");
      expect(hub.launches).toEqual([]);
      if (process.env.DELIVERY_QA && cell.id === "M01") writeFileSync(join(process.env.DELIVERY_QA, "projects.html"), `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>${readFileSync("dist/app.css", "utf8")}</style></head><body>${await sheet.evaluate(node => node.outerHTML)}</body></html>`);
    });
  });
}
