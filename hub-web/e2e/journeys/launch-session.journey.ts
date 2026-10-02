import { test, expect } from "./journey";
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
    await expect(page.getByRole("button", { name: "New session", exact: true })).toHaveCount(0);
    await page.getByRole("button", { name: "Allow new sessions" }).tap();
    await expect(sheet).toBeVisible();
    await expect(sheet.getByRole("button", { name: "Allow starting sessions on Atlas · Linux" })).toBeVisible();
    await expect(sheet.getByRole("button", { name: "Allow starting sessions on Atlas · Linux" })).toBeFocused();
    await sheet.getByRole("button", { name: "Allow starting sessions on Atlas · Linux" }).tap();
    await expect(sheet.getByText("Allow “Start new sessions” on Atlas · Linux?")).toBeVisible();
    await expect(sheet.getByRole("button", { name: "Allow starting sessions", exact: true })).toBeFocused();
    await sheet.getByRole("button", { name: "Allow starting sessions", exact: true }).tap();
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
    await expect(page.getByRole("button", { name: "Allow new sessions" })).toHaveCount(0);
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
    await expect(alert).toContainText("Run cas claude login main on Atlas · Linux, or pick another account, then start again.");
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

  await journey.stage("A long machine name fits the phone consent", async () => {
    const originalLabel = ATLAS.label;
    ATLAS.label = "soundwave — a very long personal workstation name with several extra words and anunbrokentailthatneedstowrap";
    hub.setScopes("atlas", [...SCOPES]);
    await hub.seedPaired();
    await page.reload();
    await page.getByRole("button", { name: "Allow new sessions" }).tap();
    const allow = sheet.getByRole("button", { name: `Allow starting sessions on ${ATLAS.label}` });
    await expect(allow).toBeVisible();
    await expect(sheet.getByRole("button", { name: "Close", exact: true })).toBeVisible();
    expect(await allow.evaluate((button) => button.getBoundingClientRect().right <= innerWidth), "the long button fits the viewport").toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "no sideways scroll").toBe(true);
    await allow.tap();
    await expect(sheet.getByText(`Allow “Start new sessions” on ${ATLAS.label}?`)).toBeVisible();
    await expect(sheet.getByRole("button", { name: "Allow starting sessions", exact: true })).toBeFocused();
    ATLAS.label = originalLabel;
  });
});
