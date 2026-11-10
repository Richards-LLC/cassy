// @vitest-environment jsdom
import { beforeAll, afterEach, expect, it, vi } from "vitest";
import { AccountsSheet, type AccountsHost } from "./accounts-panel";
import { accountCapacity, accountMeters, accountTime, type AccountRow, type AccountOperation, type AccountsCatalog } from "./accounts-model";
const row: AccountRow = { key: "claude:spare", provider: "claude", name: "spare", display_name: "hello@example.test", login_state: "stale_login", disabled: false, duplicate_of: null, is_default: false, recommendation: null, windows: [], usage_observed_at: null, capabilities: { remove: null, disable: null, enable: "Already enabled", rename: null, set_default: null } };
beforeAll(() => {
  HTMLDialogElement.prototype.showModal = function() { this.setAttribute("open", ""); };
  HTMLDialogElement.prototype.close = function() { this.removeAttribute("open"); this.dispatchEvent(new Event("close")); };
});
afterEach(() => { document.body.innerHTML = ""; vi.restoreAllMocks(); });
const settle = async () => {
  await vi.waitFor(() => {
    expect(document.querySelector('.accounts-machine-body > [role="status"]')).toBeNull();
  });
};
function setup(manage = true, account = row, list: AccountsHost["list"] = async () => ({ accounts: [account] })) {
  const host: AccountsHost = {
    machines: () => [{ id: "linux", label: "Soundwave", scopes: manage ? ["machine-read", "factory-manage"] : ["machine-read"] }],
    checkUsage: vi.fn(), list: vi.fn(list), change: vi.fn(async () => {}), changed: vi.fn(), copy: vi.fn(async () => {}), returnFocus: vi.fn(),
  };
  const sheet = new AccountsSheet(host); sheet.open();
  return { sheet, host };
}
function click(selector: string) { document.querySelector<HTMLButtonElement>(selector)!.click(); }

it("shows read-only pairings their disabled actions and the exact manage-access command", async () => {
  let completeListing!: (catalog: AccountsCatalog) => void;
  const listing = new Promise<AccountsCatalog>(resolve => { completeListing = resolve; });
  const { host } = setup(false, row, () => listing);
  expect(document.body.textContent).toBeTruthy();
  const loaded = settle();
  completeListing({ accounts: [row] });
  await loaded;

  expect(document.body.textContent).toBeTruthy();
  const operations = [...document.querySelectorAll<HTMLButtonElement>("[data-operation]")];
  expect(operations.map(button => button.dataset.operation)).toEqual(["set_default", "rename", "disable", "remove"]);
  expect(operations.every(button => button.disabled && Boolean(button.title))).toBe(true);
  click("[data-grant-command]"); await settle();
  expect(host.copy).toHaveBeenCalledWith(`cas hub pair --origin ${location.origin} --scopes machine:read,factory:manage`);
  click("[data-login]"); await settle();
  expect(host.copy).toHaveBeenCalledWith("cas claude login spare");
  click("[data-accounts-close]"); expect(host.returnFocus).toHaveBeenCalledOnce();
});

it.each(["remove", "disable", "rename", "enable", "set_default"] as AccountOperation[])("performs %s with required confirmation and invalidates New session", async operation => {
  const { host, sheet } = setup(true, operation === "enable" ? { ...row, disabled: true, capabilities: { ...row.capabilities, enable: null, disable: "Already disabled" } } : row);
  await settle();
  click(`[data-operation="${operation}"]`);
  if (["remove", "disable", "rename"].includes(operation)) {
    expect(host.change).not.toHaveBeenCalled();
    const input = document.querySelector<HTMLInputElement>(".accounts-editor input")!;
    const confirm = document.querySelector<HTMLButtonElement>("[data-accounts-confirm]")!;
    if (operation === "remove") {
      // cas-b2e9: the confirmation sits in the acting card, and Confirm waits for the exact text.
      expect(input.closest(".accounts-row")!.querySelector(`[data-account="${row.key}"]`)).not.toBeNull();
      expect(input.required).toBe(false);
      expect(confirm.disabled).toBe(true);
      input.value = "cancelled confirmation";
      click("[data-accounts-cancel]");
      expect(input.value).toBe("");
      expect(document.querySelector(".accounts-editor h3")!.textContent).toBe("");
      expect(document.querySelector(".accounts-explanation")!.textContent).toBe("");
      expect(document.activeElement).toBe(document.querySelector('[data-operation="remove"]'));
      expect(host.change).not.toHaveBeenCalled();
      click('[data-operation="remove"]');
      input.value = "closed confirmation";
      click("[data-accounts-close]");
      expect(input.value).toBe("");
      sheet.open(); await settle();
      click('[data-operation="remove"]');
    }
    if (operation !== "rename") {
      input.value = "other";
      document.querySelector("form")!.dispatchEvent(new Event("submit", { cancelable: true }));
      expect(host.change).not.toHaveBeenCalled();
    }
    input.value = operation === "rename" ? "renamed" : row.key;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    expect(confirm.disabled).toBe(false);
    document.querySelector(".accounts-editor")!.dispatchEvent(new Event("submit", { cancelable: true }));
  }
  await settle();
  expect(host.change).toHaveBeenCalledWith("linux", expect.objectContaining({ operation, account: row.key }));
  expect(host.changed).toHaveBeenCalledWith("linux");
  expect(host.list).toHaveBeenCalledTimes(operation === "remove" ? 3 : 2);
  expect(document.body.textContent).toBeTruthy();
  expect(document.querySelector<HTMLInputElement>(".accounts-editor input")!.value).toBe("");
  expect(document.querySelector(".accounts-editor h3")!.textContent).toBe("");
  expect(document.querySelector(".accounts-explanation")!.textContent).toBe("");
});

it("keeps other machines readable, retries failures, and refreshes after an uncertain mutation", async () => {
  const { host, sheet } = setup();
  vi.mocked(host.list).mockRejectedValueOnce(new Error("offline"));
  host.machines = () => [{ id: "linux", label: "Soundwave", scopes: ["machine-read", "factory-manage"] }, { id: "mac", label: "Prowl", scopes: ["machine-read"] }];
  sheet.open(); await settle();
  expect(document.body.textContent).toContain("Soundwave");
  expect(document.querySelector('section[data-machine="mac"]')!.textContent).toContain(row.display_name);
  click("[data-retry]"); await settle();
  vi.mocked(host.change).mockRejectedValue(new Error("partial change"));
  click('[data-operation="set_default"]'); await settle();
  expect(host.changed).toHaveBeenCalledWith("linux");
  expect(document.body.textContent).toBeTruthy();
});

it("groups by provider in headroom order, says where each lives, and cleans up stale duplicates in one confirmation", async () => {
  const healthy: AccountRow = { ...row, key: "claude:work", name: "work", display_name: "work@example.test", location: "~/.claude-work", login_state: "logged_in", windows: [{ name: "weekly", used_percent: 38, resets_at: "2099-11-02T18:20:00Z" }] };
  const duplicate: AccountRow = { ...row, key: "claude:main", name: "main", location: "~/.claude", duplicate_of: "claude:work", is_default: false };
  const tight: AccountRow = { ...healthy, key: "claude:tight", name: "tight", display_name: "tight@example.test", location: "~/.claude-tight", windows: [{ name: "weekly", used_percent: 95, resets_at: "2099-11-02T18:20:00Z" }] };
  // cas-b2e9: a protected main duplicate is never offered for cleanup.
  const protectedMain: AccountRow = { ...duplicate, key: "claude:default", name: "default", location: "~/.claude", capabilities: { ...row.capabilities, remove: "The main account is protected" } };
  const { host, sheet } = setup(true, healthy);
  vi.mocked(host.list).mockResolvedValue({ accounts: [duplicate, tight, healthy, protectedMain] });
  sheet.open(); await settle();
  const groups = [...document.querySelectorAll(".accounts-provider-group")];
  expect(groups.map(group => group.querySelectorAll(".accounts-row").length)).toEqual([3, 1]);
  expect(groups[1]!.querySelector<HTMLElement>(".accounts-identity strong")!.title).toBe(duplicate.key);
  expect([...groups[0]!.querySelectorAll<HTMLElement>(".accounts-identity strong")].map(name => name.title)).toEqual([healthy.key, tight.key, protectedMain.key]);
  for (const line of document.querySelectorAll("[data-duplicate-of]")) expect(line.textContent).not.toContain(healthy.key);
  expect(groups[0]!.querySelector(".accounts-bar .crit")).not.toBeNull();
  expect(groups[0]!.textContent).toContain("~/.claude-work");
  expect(groups[0]!.textContent).toContain("62%");
  expect(groups[1]!.textContent).toBeTruthy();
  click("[data-cleanup]");
  const input = document.querySelector<HTMLInputElement>(".accounts-editor input")!;
  input.value = "clean up"; input.dispatchEvent(new Event("input"));
  document.querySelector(".accounts-editor")!.dispatchEvent(new Event("submit", { cancelable: true }));
  await settle();
  expect(host.change).toHaveBeenCalledExactlyOnceWith("linux", { operation: "remove", account: "claude:main", confirmation: "claude:main" });
  expect(document.body.textContent).toBeTruthy();
});

it("formats reset and observation times locally in 12-hour form and clears expired capacity", () => {
  const now = new Date("2026-10-28T16:00:00Z").getTime();
  const fresh = accountCapacity([{ name: "weekly", used_percent: 38, resets_at: "2026-10-29T16:20:00Z" }], "2026-10-28T13:00:00Z", now)[0];
  expect(fresh).toContain("weekly"); expect(fresh).toContain("62%"); expect(fresh).toContain(accountTime("2026-10-28T13:00:00Z")); expect(fresh).toMatch(/AM|PM/);
  expect(accountMeters([{ name: "weekly", used_percent: 38, resets_at: "2026-10-27T16:00:00Z" }], now)[0]).toMatchObject({ left: null, reset: null });
  // cas-eae3: a reset already past is a stale fact, not a time to show.
  expect(accountCapacity([{ name: "weekly", used_percent: null, resets_at: "2026-10-27T16:00:00Z" }], null, now)).toEqual(["weekly usage unknown"]);
  const zone = process.env.TZ;
  try { process.env.TZ = "America/New_York"; expect(accountTime("2026-10-29T17:10:00Z")).toMatch(/1:10\sPM ET$/); }
  finally { if (zone === undefined) delete process.env.TZ; else process.env.TZ = zone; }
});


it("shows the Codex alias and valid login help when identity metadata is unavailable", async () => {
  const { host } = setup(false, { ...row, key: "codex:work", provider: "codex", name: "work", display_name: "work", login_state: "unknown" });
  await settle();
  expect(document.querySelector(".accounts-identity strong")!.textContent).toBe("work");
  expect(document.body.textContent).toContain("codex:work");
  click("[data-login]"); await settle();
  expect(host.copy).toHaveBeenCalledWith("cas codex login work");
});

it.each(["claude", "codex"] as const)("rechecks %s usage without management or switching, with dated snapshots and retry", async provider => {
  const account = { ...row, key: `${provider}:spare`, provider, login_state: "logged_in" as const };
  const { host } = setup(false, account);
  const check = vi.fn().mockResolvedValue({ checked_at: "2026-11-03T17:00:00Z", observed_at: "2026-11-03T16:00:00Z", source: "native snapshot", reason: "Snapshot is stale; open a session on this account to refresh.", windows: [{ name: "5h", used_percent: 23, resets_at: "2026-11-03T20:00:00Z" }, { name: "weekly", used_percent: 44, resets_at: "2026-11-08T16:57:00Z" }] });
  Object.assign(host, { checkUsage: check });
  await settle();
  const button = () => [...document.querySelectorAll<HTMLButtonElement>("button")].find(node => node.textContent === "Check usage")!;
  expect(button()).toBeDefined(); button().click();
  await vi.waitFor(() => expect(document.body.textContent).toMatch(new RegExp("(?=[\\s\\S]*3)(?=[\\s\\S]*12)(?=[\\s\\S]*00)")));
  expect(check).toHaveBeenCalledWith("linux", `${provider}:spare`, expect.any(AbortSignal));
  expect(document.body.textContent).toMatch(new RegExp("(?=[\\s\\S]*3)(?=[\\s\\S]*11)(?=[\\s\\S]*00)"));
  expect(document.body.textContent).toMatch(new RegExp("(?=[\\s\\S]*5h)(?=[\\s\\S]*23%)"));
  expect(document.body.textContent).toContain("44%");
  expect(document.body.textContent).toContain("Snapshot is stale");
  expect(host.change).not.toHaveBeenCalled(); expect(host.changed).not.toHaveBeenCalled();
  check.mockRejectedValueOnce(Object.assign(new Error("GET usage failed (429)"), { status: 429 })); button().click();
  await vi.waitFor(() => expect(document.body.textContent).toBeTruthy());
  expect(button().disabled).toBe(false);
});
