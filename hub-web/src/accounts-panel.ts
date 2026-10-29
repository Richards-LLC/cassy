import "./accounts-panel.css";
import { escapeHtml } from "./cloud-brand";
import { accountLoginCommand } from "./launch-session";
import { accountMeters, accountTime, type AccountChange, type AccountOperation, type AccountRow, type AccountsCatalog } from "./accounts-model";
import { FACTORY_MANAGE_SCOPE, scopeGrantCommand } from "./pairing-scopes";
import { HARNESS_NAME, providerAvatarMarkup } from "./provider-mark";
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
const ACTIONS: Record<AccountOperation, string> = { set_default: "Set default", rename: "Rename", disable: "Disable", enable: "Enable", remove: "Remove" };
const PROVIDERS = [["claude", "Claude"], ["codex", "Codex"], ["grok", "Grok"]] as const;
const LOGIN: Record<AccountRow["login_state"], string> = { logged_in: "Logged in", logged_out: "Logged out", never_logged_in: "Not logged in", stale_login: "Stale login", unknown: "Login unknown" };
/** Bar widths go through CSSOM: the hub's style-src refuses inline style attributes. */
function fillBars(root: HTMLElement): void {
  for (const bar of root.querySelectorAll<HTMLElement>(".accounts-bar > [data-left]")) bar.style.inlineSize = `${Number(bar.dataset.left) || 0}%`;
}
/** Stale logins and duplicates sort into one cleanup group per machine. */
const needsCleanup = (row: AccountRow) => row.provider !== "grok" && (row.duplicate_of !== null || row.login_state === "stale_login");

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
    this.$(".accounts-machines").innerHTML = machines.length ? machines.map(machine => `<section data-machine="${escapeHtml(machine.id)}" aria-label="${escapeHtml(machine.label)}"><h3 class="accounts-machine">${escapeHtml(machine.label)}</h3><div class="accounts-machine-body"><p role="status">Loading accounts…</p></div></section>`).join("") : '<p>No paired machines. Pair a machine to see its accounts.</p>';
    if (!dialog.open) dialog.showModal();
    this.$<HTMLButtonElement>("[data-accounts-close]").focus();
    for (const machine of machines) void this.load(machine);
  }

  private ensureDialog(): HTMLDialogElement {
    if (this.dialog?.isConnected) return this.dialog;
    const dialog = this.doc.createElement("dialog");
    dialog.className = "accounts-sheet"; dialog.setAttribute("aria-labelledby", "accounts-title");
    dialog.innerHTML = `<header class="accounts-heading"><div><h2 id="accounts-title">Accounts</h2><p>Logins, usage and where each one lives, on every paired machine.</p></div><button type="button" data-accounts-close aria-label="Close accounts">Close</button></header><p class="accounts-notice" role="status" aria-live="polite"></p><div class="accounts-machines"></div><form class="accounts-editor" hidden><h3></h3><p class="accounts-explanation"></p><label class="accounts-field"><span></span><input name="value" autocomplete="off" required></label><div class="dialog-actions"><button type="button" data-accounts-cancel>Cancel</button><button type="submit" class="primary">Confirm</button></div></form>`;
    this.doc.body.append(dialog); this.dialog = dialog;
    dialog.addEventListener("close", () => { this.loads.abort(); this.generation++; this.clearEditor(); this.host.returnFocus(); });
    dialog.addEventListener("click", event => {
      const button = (event.target as Element).closest<HTMLButtonElement>("button");
      if (!button) return;
      if (button.hasAttribute("data-accounts-close")) dialog.close();
      if (button.hasAttribute("data-accounts-cancel")) { this.clearEditor(); this.focusRow(); }
      const machine = this.host.machines().find(item => item.id === button.dataset.machine);
      if (button.hasAttribute("data-retry") && machine) void this.load(machine);
      if (button.dataset.grantCommand) void this.host.copy(button.dataset.grantCommand).then(() => this.notice("Command copied. Run it on the machine, then open the link it prints here."), () => this.notice("Could not copy. Select the displayed command to copy it."));
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
      body.innerHTML = this.rows(machine, catalog); fillBars(body);
    } catch {
      if (signal.aborted || generation !== this.generation) return;
      body.innerHTML = `<p role="alert">Could not load accounts on ${escapeHtml(machine.label)}.</p><button type="button" data-retry data-machine="${escapeHtml(machine.id)}">Try again</button>`;
    }
  }

  private rows(machine: AccountsMachine, catalog: AccountsCatalog): string {
    if (!catalog.accounts.length) return '<p class="accounts-empty">No accounts on this machine.</p>';
    const manage = machine.scopes.includes("factory-manage");
    const cleanup = catalog.accounts.filter(needsCleanup);
    const summary = `${catalog.accounts.length} account${catalog.accounts.length === 1 ? "" : "s"}${cleanup.length ? ` · ${cleanup.length} to clean up` : ""}`;
    const groups = PROVIDERS.map(([provider, label]) => {
      const accounts = catalog.accounts.filter(row => row.provider === provider && !needsCleanup(row));
      return accounts.length ? `<section class="accounts-provider-group" aria-label="${label} on ${escapeHtml(machine.label)}"><h4 class="accounts-provider">${label} · ${accounts.length}</h4><ul class="accounts-ledger">${accounts.map(account => this.card(machine, account, manage)).join("")}</ul></section>` : "";
    }).join("");
    const stale = cleanup.length ? `<section class="accounts-provider-group accounts-cleanup" aria-label="Needs cleanup on ${escapeHtml(machine.label)}"><h4 class="accounts-provider">Needs cleanup · ${cleanup.length}</h4><p class="accounts-cleanup-lead">Stale logins and duplicates of another account. Remove the copy you no longer use; the account it duplicates stays.</p><ul class="accounts-ledger">${cleanup.map(account => this.card(machine, account, manage)).join("")}</ul></section>` : "";
    return `<p class="accounts-machine-summary">${summary}</p>${manage ? "" : this.grantCallout(machine)}${groups}${stale}`;
  }

  /** DESIGN.md: read-only pairings see the actions and exactly how to unlock them. */
  private grantCallout(machine: AccountsMachine): string {
    const command = scopeGrantCommand(this.doc.defaultView?.location.origin ?? "", machine.scopes, FACTORY_MANAGE_SCOPE);
    return `<div class="accounts-grant" role="note"><p><strong>Read-only on ${escapeHtml(machine.label)}.</strong> Removing, disabling, renaming and choosing the default need manage access. Run this on ${escapeHtml(machine.label)}, then open the link it prints here:</p><div class="accounts-login"><code>${escapeHtml(command)}</code><button type="button" data-grant-command="${escapeHtml(command)}">Copy command</button></div></div>`;
  }

  private card(machine: AccountsMachine, account: AccountRow, manage: boolean): string {
    const attrs = `data-machine="${escapeHtml(machine.id)}" data-account="${escapeHtml(account.key)}"`;
    const grok = account.provider === "grok";
    const state = LOGIN[account.login_state];
    const tone = account.disabled ? "idle" : account.login_state === "logged_in" ? "active" : account.login_state === "stale_login" ? "blocked" : "open";
    const badges = `${account.is_default ? '<span class="accounts-badge">Default</span>' : ""}${account.recommendation ? `<span class="accounts-badge" title="${escapeHtml(account.recommendation)}">Recommended</span>` : ""}`;
    const where = `${account.location ? `Lives in <code>${escapeHtml(account.location)}</code> on ` : "On "}${escapeHtml(machine.label)} · used by ${HARNESS_NAME[account.provider]}`;
    const meters = accountMeters(account.windows);
    const usage = meters.length
      ? `<div class="accounts-usage">${meters.map(meter => `<div class="accounts-meter"><span class="accounts-meter-name">${escapeHtml(meter.name)}</span><span class="accounts-bar" aria-hidden="true"><span data-left="${meter.left ?? 0}"${meter.left !== null && meter.left < 20 ? ' class="low"' : ""}></span></span><span class="accounts-meter-text">${meter.left === null ? "usage unknown" : `${meter.left}% left`}${meter.reset ? ` <span class="accounts-reset">· ${meter.resetPast ? "reset" : "resets"} ${escapeHtml(meter.reset)}</span>` : ""}</span></div>`).join("")}${account.usage_observed_at ? `<p class="accounts-observed">As of ${escapeHtml(accountTime(account.usage_observed_at))}</p>` : ""}</div>`
      : `<p class="accounts-observed">Usage unknown${grok ? "" : account.login_state === "logged_in" ? " · not read yet on this machine" : " · log in to read it"}</p>`;
    const cleanup = account.duplicate_of ? `<p class="accounts-cleanup-why">Duplicate of <code>${escapeHtml(account.duplicate_of)}</code>${account.login_state === "stale_login" ? " with a stale login" : ""}.</p>` : account.login_state === "stale_login" ? '<p class="accounts-cleanup-why">Stale login: log in again, or remove it.</p>' : "";
    const login = account.login_state !== "logged_in" && !grok ? `<div class="accounts-login"><code>${escapeHtml(accountLoginCommand(account.provider, account.name))}</code><button type="button" ${attrs} data-login aria-label="Copy login command for ${escapeHtml(account.display_name)}">Copy login command</button></div>` : "";
    const reasons = grok ? '<p class="accounts-readonly">Grok uses one account. Named account management and login status are unavailable.</p>'
      : [...new Set([account.capabilities.remove, account.capabilities.rename].filter(Boolean))].map(reason => `<p class="accounts-readonly">${escapeHtml(reason!)}</p>`).join("");
    const operations = (Object.keys(ACTIONS) as AccountOperation[]).filter(op => op !== (account.disabled ? "disable" : "enable"));
    const actions = grok ? "" : `<div class="accounts-actions" role="group" aria-label="Actions for ${escapeHtml(account.display_name)}">${operations.map(op => {
      const reason = !manage ? "Needs manage access on this machine" : account.capabilities[op];
      return `<button type="button" ${attrs} data-operation="${op}"${this.pending || reason !== null ? ` disabled title="${escapeHtml(reason ?? "Unavailable")}"` : ""}>${ACTIONS[op]}</button>`;
    }).join("")}</div>`;
    return `<li class="accounts-row${needsCleanup(account) ? " needs-cleanup" : ""}">${providerAvatarMarkup({ harness: account.provider, fallback: account.provider.slice(0, 2).toUpperCase(), size: "lane" })}<div class="accounts-card-body"><div class="accounts-identity"><strong>${escapeHtml(account.display_name)}</strong><span class="accounts-badges">${badges}</span></div><p class="accounts-alias">${escapeHtml(account.key)}</p><p class="accounts-where">${where}</p><p class="accounts-state"><span class="status-chip status-chip--${tone}">${state}${account.disabled ? " · Disabled" : ""}</span></p>${cleanup}${usage}${reasons}${login}${actions}</div></li>`;
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
      if (current) { const body = this.machineBody(id); if (body) { body.innerHTML = this.rows(current, catalog); fillBars(body); } }
    }
    await this.load(machine); this.focusRow();
  }
  private focusRow(): void {
    const target = [...this.dialog!.querySelectorAll<HTMLButtonElement>("[data-operation]")].find(button => !button.disabled && button.dataset.machine === this.focusTarget?.machine && button.dataset.account === this.focusTarget?.account && button.dataset.operation === this.focusTarget?.operation);
    (target ?? this.$<HTMLButtonElement>("[data-accounts-close]")).focus();
  }
}
