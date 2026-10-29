// @vitest-environment jsdom
import { beforeAll, afterEach, expect, it, vi } from "vitest";
import { AccountsSheet, type AccountsHost } from "./accounts-panel";
import { accountCapacity, type AccountRow, type AccountOperation } from "./accounts-model";
const row: AccountRow = { key: "claude:spare", provider: "claude", name: "spare", display_name: "hello@example.test", login_state: "stale_login", disabled: false, duplicate_of: null, is_default: true, recommendation: null, windows: [], usage_observed_at: null, capabilities: { remove: null, disable: null, enable: "Already enabled", rename: null, set_default: null } };
beforeAll(() => {
  HTMLDialogElement.prototype.showModal = function() { this.setAttribute("open", ""); };
  HTMLDialogElement.prototype.close = function() { this.removeAttribute("open"); this.dispatchEvent(new Event("close")); };
});
afterEach(() => { document.body.innerHTML = ""; vi.restoreAllMocks(); });
const settle = async () => { await new Promise(resolve => setTimeout(resolve, 0)); };
function setup(manage = true, account = row) {
  const host: AccountsHost = {
    machines: () => [{ id: "linux", label: "Soundwave", scopes: manage ? ["machine-read", "factory-manage"] : ["machine-read"] }],
    list: vi.fn(async () => ({ accounts: [account] })), change: vi.fn(async () => {}), changed: vi.fn(), copy: vi.fn(async () => {}), returnFocus: vi.fn(),
  };
  const sheet = new AccountsSheet(host); sheet.open();
  return { sheet, host };
}
function click(selector: string) { document.querySelector<HTMLButtonElement>(selector)!.click(); }

it("lists state and capacity for read-only pairings and copies only a valid login command", async () => {
  const { host } = setup(false); await settle();
  expect(document.body.textContent).toContain("Stale login");
  expect(document.body.textContent).toContain("Usage unknown");
  expect(document.body.textContent).toContain("Read-only");
  expect(document.querySelector("[data-operation]")).toBeNull();
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
    if (operation === "remove") {
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
    input.dispatchEvent(new Event("input"));
    document.querySelector(".accounts-editor")!.dispatchEvent(new Event("submit", { cancelable: true }));
  }
  await settle();
  expect(host.change).toHaveBeenCalledWith("linux", expect.objectContaining({ operation, account: row.key }));
  expect(host.changed).toHaveBeenCalledWith("linux");
  expect(host.list).toHaveBeenCalledTimes(operation === "remove" ? 3 : 2);
  expect(document.body.textContent).toContain("Account updated");
  expect(document.querySelector<HTMLInputElement>(".accounts-editor input")!.value).toBe("");
  expect(document.querySelector(".accounts-editor h3")!.textContent).toBe("");
  expect(document.querySelector(".accounts-explanation")!.textContent).toBe("");
});

it("keeps other machines readable, retries failures, and refreshes after an uncertain mutation", async () => {
  const { host, sheet } = setup();
  vi.mocked(host.list).mockRejectedValueOnce(new Error("offline"));
  host.machines = () => [{ id: "linux", label: "Soundwave", scopes: ["machine-read", "factory-manage"] }, { id: "mac", label: "Prowl", scopes: ["machine-read"] }];
  sheet.open(); await settle();
  expect(document.body.textContent).toContain("Could not load accounts on Soundwave");
  expect(document.querySelector('section[data-machine="mac"]')!.textContent).toContain(row.display_name);
  click("[data-retry]"); await settle();
  vi.mocked(host.change).mockRejectedValue(new Error("partial change"));
  click('[data-operation="set_default"]'); await settle();
  expect(host.changed).toHaveBeenCalledWith("linux");
  expect(document.body.textContent).toContain("Could not verify the complete change");
});

it("formats reset and observation times locally in 12-hour form and clears expired capacity", () => {
  const now = new Date("2026-10-28T16:00:00Z").getTime();
  const fresh = accountCapacity([{ name: "weekly", used_percent: 38, resets_at: "2026-10-29T16:20:00Z" }], "2026-10-28T13:00:00Z", now)[0];
  expect(fresh).toContain("weekly 62% left"); expect(fresh).toContain("as of"); expect(fresh).toMatch(/AM|PM/);
  expect(accountCapacity([{ name: "weekly", used_percent: 38, resets_at: "2026-10-27T16:00:00Z" }], null, now)[0]).toContain("usage unknown");
});


it("shows the Codex alias and valid login help when identity metadata is unavailable", async () => {
  const { host } = setup(false, { ...row, key: "codex:work", provider: "codex", name: "work", display_name: "work", login_state: "unknown" });
  await settle();
  expect(document.querySelector(".accounts-identity strong")!.textContent).toBe("work");
  expect(document.body.textContent).toContain("codex:work");
  click("[data-login]"); await settle();
  expect(host.copy).toHaveBeenCalledWith("cas codex login work");
});
