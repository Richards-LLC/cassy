import "./accounts-panel.css";
import { escapeHtml } from "./cloud-brand";
import { accountLoginCommand } from "./launch-session";
import { accountCapacity, type AccountChange, type AccountOperation, type AccountRow, type AccountsCatalog } from "./accounts-model";
import type { Scope } from "./types";

export interface AccountsMachine { id: string; label: string; scopes: readonly Scope[] }
export interface AccountsHost {
  machines(): AccountsMachine[];
  list(machineId: string, signal: AbortSignal): Promise<AccountsCatalog>;
  change(machineId: string, change: AccountChange): Promise<void>;
  changed(machineId: string): void;
  copy(text: string): Promise<void>;
  returnFocus(): void;
}
const ACTIONS: Record<AccountOperation, string> = { remove: "Remove", disable: "Disable", enable: "Enable", rename: "Rename", set_default: "Set default" };

/** Body-mounted modal: shell redraws do not destroy edits or confirmations. */
export class AccountsSheet {
  private dialog?: HTMLDialogElement;
  private loads = new AbortController();
  private catalogs = new Map<string, AccountsCatalog>();
  private pending = false;
  private edit?: { machine: AccountsMachine; account: AccountRow; operation: AccountOperation };
  private generation = 0;
  private focusTarget?: { machine: string; account: string; operation: string };
  constructor(private readonly host: AccountsHost, private readonly doc: Document = document) {}

  open(): void {
    const dialog = this.ensureDialog();
    this.loads.abort(); this.loads = new AbortController(); this.generation++;
    this.clearEditor(); this.catalogs.clear();
    this.notice("");
    const machines = this.host.machines();
    this.$(".accounts-machines").innerHTML = machines.length ? machines.map(machine => `<section data-machine="${escapeHtml(machine.id)}" aria-label="${escapeHtml(machine.label)}"><h3>${escapeHtml(machine.label)}</h3><div class="accounts-machine-body"><p role="status">Loading accounts…</p></div></section>`).join("") : '<p>No paired machines. Pair a machine to see its accounts.</p>';
    if (!dialog.open) dialog.showModal();
    this.$<HTMLButtonElement>("[data-accounts-close]").focus();
    for (const machine of machines) void this.load(machine);
  }

  private ensureDialog(): HTMLDialogElement {
    if (this.dialog?.isConnected) return this.dialog;
    const dialog = this.doc.createElement("dialog");
    dialog.className = "accounts-sheet"; dialog.setAttribute("aria-labelledby", "accounts-title");
    dialog.innerHTML = `<header class="accounts-heading"><div><h2 id="accounts-title">Accounts</h2><p>Logins and capacity, on each machine.</p></div><button type="button" data-accounts-close aria-label="Close accounts">Close</button></header><p class="accounts-notice" role="status" aria-live="polite"></p><div class="accounts-machines"></div><form class="accounts-editor" hidden><h3></h3><p class="accounts-explanation"></p><label class="accounts-field"><span></span><input name="value" autocomplete="off" required></label><div class="dialog-actions"><button type="button" data-accounts-cancel>Cancel</button><button type="submit" class="primary">Confirm</button></div></form>`;
    this.doc.body.append(dialog); this.dialog = dialog;
    dialog.addEventListener("close", () => { this.loads.abort(); this.generation++; this.clearEditor(); this.host.returnFocus(); });
    dialog.addEventListener("click", event => {
      const button = (event.target as Element).closest<HTMLButtonElement>("button");
      if (!button) return;
      if (button.hasAttribute("data-accounts-close")) dialog.close();
      if (button.hasAttribute("data-accounts-cancel")) { this.clearEditor(); this.focusRow(); }
      const machine = this.host.machines().find(item => item.id === button.dataset.machine);
      if (button.hasAttribute("data-retry") && machine) void this.load(machine);
      const account = machine && this.catalogs.get(machine.id)?.accounts.find(row => row.key === button.dataset.account);
      if (account && machine && button.dataset.operation && !this.pending) this.begin(machine, account, button.dataset.operation as AccountOperation);
      if (account && machine && button.hasAttribute("data-login")) {
        if (account.provider === "grok") return;
        void this.host.copy(accountLoginCommand(account.provider, account.name)).then(() => this.notice("Login command copied. Run it on the named machine."), () => this.notice("Could not copy. Select the displayed command to copy it."));
      }
    });
    this.editor().addEventListener("submit", event => { event.preventDefault(); void this.submit(); });
    return dialog;
  }

  private $<T extends HTMLElement = HTMLElement>(selector: string): T { return this.dialog!.querySelector<T>(selector)!; }
  private editor(): HTMLFormElement { return this.$<HTMLFormElement>(".accounts-editor"); }
  private clearEditor(): void {
    this.edit = undefined;
    const form = this.editor(); form.hidden = true;
    const input = form.elements.namedItem("value") as HTMLInputElement;
    input.value = ""; input.setCustomValidity(""); input.oninput = null;
    form.querySelector("h3")!.textContent = "";
    this.$(".accounts-explanation").textContent = "";
    this.$(".accounts-field span").textContent = "";
  }
  private notice(text: string): void { this.$(".accounts-notice").textContent = text; }
  private machineBody(id: string): HTMLElement | undefined {
    return [...this.dialog!.querySelectorAll<HTMLElement>("[data-machine]")].find(node => node.tagName === "SECTION" && node.dataset.machine === id)?.querySelector<HTMLElement>(".accounts-machine-body") ?? undefined;
  }

  private async load(machine: AccountsMachine): Promise<void> {
    const signal = this.loads.signal;
    const generation = this.generation;
    const body = this.machineBody(machine.id);
    if (!body) return;
    if (!machine.scopes.includes("machine-read")) { body.innerHTML = '<p>Account listing needs machine read access. Update this machine’s pairing.</p>'; return; }
    body.innerHTML = '<p role="status">Loading accounts…</p>';
    try {
      const catalog = await this.host.list(machine.id, signal);
      if (signal.aborted || generation !== this.generation) return;
      this.catalogs.set(machine.id, catalog);
      body.innerHTML = this.rows(machine, catalog);
    } catch {
      if (signal.aborted || generation !== this.generation) return;
      body.innerHTML = `<p role="alert">Could not load accounts on ${escapeHtml(machine.label)}.</p><button type="button" data-retry data-machine="${escapeHtml(machine.id)}">Try again</button>`;
    }
  }

  private rows(machine: AccountsMachine, catalog: AccountsCatalog): string {
    if (!catalog.accounts.length) return '<p>No accounts on this machine.</p>';
    const manage = machine.scopes.includes("factory-manage");
    return `${manage ? "" : '<p class="accounts-readonly">Read-only · Managing accounts needs factory manage access.</p>'}${["claude", "codex", "grok"].map(provider => {
      const accounts = catalog.accounts.filter(row => row.provider === provider);
      if (!accounts.length) return "";
      return `<h4 class="accounts-provider">${escapeHtml(provider)}</h4><ul class="accounts-ledger">${accounts.map(account => {
        const attrs = `data-machine="${escapeHtml(machine.id)}" data-account="${escapeHtml(account.key)}"`;
        const state = ({ logged_in: "Logged in", logged_out: "Logged out", never_logged_in: "Not logged in", stale_login: "Stale login", unknown: "Login unknown" })[account.login_state];
        const operations = (Object.keys(ACTIONS) as AccountOperation[]).filter(op => op !== (account.disabled ? "disable" : "enable"));
        const actions = manage && account.provider !== "grok" ? `<div class="accounts-actions" aria-label="Actions for ${escapeHtml(account.display_name)}">${operations.map(op => `<button type="button" ${attrs} data-operation="${op}"${this.pending || account.capabilities[op] !== null ? ` disabled title="${escapeHtml(account.capabilities[op] ?? "Unavailable")}"` : ""}>${ACTIONS[op]}</button>`).join("")}</div>` : "";
        const login = account.login_state !== "logged_in" && account.provider !== "grok" ? `<div class="accounts-login"><code>${escapeHtml(accountLoginCommand(account.provider, account.name))}</code><button type="button" ${attrs} data-login aria-label="Copy login command for ${escapeHtml(account.display_name)}">Copy login command</button></div>` : "";
        return `<li class="accounts-row"><div class="accounts-identity"><strong>${escapeHtml(account.display_name)}</strong><span class="accounts-alias">${escapeHtml(account.key)}</span><span>${state}${account.disabled ? " · Disabled" : ""}${account.duplicate_of ? ` · Duplicate of ${escapeHtml(account.duplicate_of)}` : ""}</span><span class="accounts-badges">${account.is_default ? '<span>Default</span>' : ""}${account.recommendation ? '<span title="' + escapeHtml("Based on current capacity and reset times.") + '">Recommended</span>' : ""}</span></div><div class="accounts-capacity">${accountCapacity(account.windows, account.usage_observed_at).map(text => `<p>${escapeHtml(text)}</p>`).join("")}</div>${account.provider === "grok" ? '<p class="accounts-readonly">Grok uses one account. Named account management and login status are unavailable.</p>' : ""}${login}${account.provider !== "grok" && account.capabilities.remove ? `<p class="accounts-readonly">${escapeHtml(account.capabilities.remove)}</p>` : ""}${account.capabilities.rename && account.capabilities.rename !== account.capabilities.remove ? `<p class="accounts-readonly">${escapeHtml(account.capabilities.rename)}</p>` : ""}${actions}</li>`;
      }).join("")}</ul>`;
    }).join("")}`;
  }

  private begin(machine: AccountsMachine, account: AccountRow, operation: AccountOperation): void {
    if (!machine.scopes.includes("factory-manage") || account.capabilities[operation] !== null) return;
    this.focusTarget = { machine: machine.id, account: account.key, operation };
    if (operation === "enable" || operation === "set_default") { void this.mutate(machine, { operation, account: account.key }); return; }
    this.edit = { machine, account, operation };
    const form = this.editor(); form.hidden = false;
    form.querySelector("h3")!.textContent = `${ACTIONS[operation]} ${account.display_name}`;
    const rename = operation === "rename";
    this.$(".accounts-explanation").textContent = operation === "remove" ? `Permanently remove ${account.key} on ${machine.label}. Shared base items stay intact. Active and main accounts are protected.` : operation === "disable" ? `Exclude ${account.key} from automatic selection. The login stays available for manual selection.` : `Choose a new alias for ${account.key} on ${machine.label}. Active and main accounts are protected.`;
    this.$(".accounts-field span").textContent = rename ? "New account alias" : `Type ${account.key} to confirm`;
    const input = form.elements.namedItem("value") as HTMLInputElement;
    input.value = rename ? account.name : ""; input.setCustomValidity(""); input.oninput = () => input.setCustomValidity("");
    input.focus(); input.select();
  }

  private async submit(): Promise<void> {
    if (!this.edit || this.pending) return;
    const { machine, account, operation } = this.edit;
    const input = this.editor().elements.namedItem("value") as HTMLInputElement;
    if (operation !== "rename" && input.value !== account.key) { input.setCustomValidity(`Type ${account.key} exactly.`); input.reportValidity(); return; }
    if (!input.value.trim()) return;
    await this.mutate(machine, { operation, account: account.key, ...(operation === "rename" ? { new_name: input.value } : { confirmation: input.value }) });
  }

  private async mutate(machine: AccountsMachine, change: AccountChange): Promise<void> {
    this.pending = true;
    const generation = this.generation;
    this.dialog!.querySelectorAll<HTMLButtonElement>("[data-operation], .accounts-editor button").forEach(button => button.disabled = true);
    this.notice("Changing account…");
    let success = false;
    try { await this.host.change(machine.id, change); success = true; }
    catch { /* A refusal may follow a partial disk change: always invalidate. */ }
    this.host.changed(machine.id);
    this.pending = false;
    this.editor().querySelectorAll<HTMLButtonElement>("button").forEach(button => button.disabled = false);
    if (generation !== this.generation || !this.dialog?.open) {
      if (this.dialog?.open) for (const current of this.host.machines()) void this.load(current);
      return;
    }
    this.clearEditor();
    this.editor().querySelectorAll<HTMLButtonElement>("button").forEach(button => button.disabled = false);
    this.notice(success ? "Account updated." : "Could not verify the complete change. The list has been refreshed; check the account on the machine before trying again.");
    // Re-render all rows to restore their capability-derived disabled states.
    for (const [id, catalog] of this.catalogs) {
      const current = this.host.machines().find(item => item.id === id);
      if (current) { const body = this.machineBody(id); if (body) body.innerHTML = this.rows(current, catalog); }
    }
    await this.load(machine); this.focusRow();
  }
  private focusRow(): void {
    const target = [...this.dialog!.querySelectorAll<HTMLButtonElement>("[data-operation]")].find(button => !button.disabled && button.dataset.machine === this.focusTarget?.machine && button.dataset.account === this.focusTarget?.account && button.dataset.operation === this.focusTarget?.operation);
    (target ?? this.$<HTMLButtonElement>("[data-accounts-close]")).focus();
  }
}
