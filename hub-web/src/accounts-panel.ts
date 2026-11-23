import { AccountUsageChecks, type CheckAccountUsage } from "./account-usage-check";
import { fillMeters, lowPickNote, meterMarkup, readings } from "./account-meters";
import "./accounts-panel.css";
import { escapeHtml } from "./cloud-brand";
import { accountLoginCommand } from "./launch-session";
import { accountTime, type AccountChange, type AccountOperation, type AccountRow, type AccountsCatalog, type GatewayInventory } from "./accounts-model";
import { FACTORY_MANAGE_SCOPE, scopeGrantCommand } from "./pairing-scopes";
import { HARNESS_NAME, providerAvatarMarkup } from "./provider-mark";
import type { Scope } from "./types";

export interface AccountsMachine { id: string; label: string; scopes: readonly Scope[] }
export interface AccountsHost {
  machines(): AccountsMachine[];
  list(machineId: string, signal: AbortSignal): Promise<AccountsCatalog>;
  /** The machine's home gateway; a hub without the route rejects and the section stays hidden. */
  gateway(machineId: string, signal: AbortSignal): Promise<GatewayInventory>;
  checkUsage: CheckAccountUsage;
  change(machineId: string, change: AccountChange): Promise<void>;
  changed(machineId: string): void;
  copy(text: string): Promise<void>;
  returnFocus(): void;
}
const ACTIONS: Record<AccountOperation, string> = { set_default: "Set default", rename: "Rename", disable: "Disable", enable: "Enable", remove: "Remove" };
const CLEANUP_WORD = "clean up";
/** Least capacity left across known windows; unknown sorts last. */
const headroom = (row: AccountRow) => Math.min(...readings(row.windows, row.usage_observed_at, Date.now(), row.usage_source).map(reading => "left" in reading ? reading.left : -1));
/** The account New session preselects for a provider: its recommendation, else the logged-in default, else the first logged-in one. */
function nextLaunch(rows: readonly AccountRow[]): AccountRow | undefined {
  const usable = rows.filter(row => row.login_state === "logged_in" && !row.disabled);
  return usable.find(row => row.recommendation) ?? usable.find(row => row.is_default) ?? usable[0];
}
const PROVIDERS = [["claude", "Claude"], ["codex", "Codex"], ["grok", "Grok"]] as const;
/** Removable stale logins and duplicates sort into one cleanup group per machine; protected accounts stay in their provider group. */
const needsCleanup = (row: AccountRow) => row.provider !== "grok" && row.capabilities.remove === null && (row.duplicate_of !== null || row.login_state === "stale_login");
/** The other account a duplicate copies, named for people: never its internal provider:key. */
function duplicateName(key: string, accounts: readonly AccountRow[], machine: string): string {
  const other = accounts.find(row => row.key === key);
  const name = other?.display_name ?? key.slice(key.indexOf(":") + 1);
  return `<strong>${escapeHtml(name)}</strong>${other?.location ? ` in <code>${escapeHtml(other.location)}</code>` : ""} on ${escapeHtml(machine)}`;
}
/** A profile named like another harness (a Claude profile called "grok") says whose it is. */
const harnessLookalike = (row: AccountRow) => PROVIDERS.some(([id]) => id !== row.provider && id === row.display_name.trim().toLowerCase());
/** Loading keeps the ledger's shape, so rows land without reflow. */
const SKELETON = `<p role="status" class="sr-only">Loading accounts…</p><ul class="accounts-ledger accounts-skeleton" aria-hidden="true">${"<li class=\"accounts-row\"><div class=\"accounts-who\"><span class=\"skeleton-dot\"></span><span class=\"skeleton-line\"></span></div><div class=\"accounts-cell\"><span class=\"skeleton-line\"></span></div><div class=\"accounts-cell\"><span class=\"skeleton-line\"></span></div></li>".repeat(3)}</ul>`;
const CONFIRM_VERB: Partial<Record<AccountOperation, string>> = { remove: "remove", disable: "disable" };

const HUB_STATE: Record<string, [string, string]> = { active: ["Active", "active"], disabled: ["Disabled", "idle"], unavailable: ["Unavailable", "blocked"] };

/**
 * The hub's own accounts, a sibling ledger on the same grid as the native one.
 * Nothing here is selectable; a native profile never pins one of these.
 * An older hub (no route) or an unknown answer draws nothing; a machine with
 * no hub configured says so in one quiet line.
 */
export function gatewaySection(machine: AccountsMachine, gateway: GatewayInventory | undefined): string {
  if (!gateway || !["off", "ready", "unavailable"].includes(gateway.status) || !Array.isArray(gateway.accounts)) return "";
  const name = escapeHtml(machine.label);
  if (gateway.status === "off") return `<section class="accounts-gateway" data-testid="accounts-gateway" data-gateway-status="off" aria-label="Home gateway on ${name}"><div class="accounts-group-head"><h4 class="accounts-provider">Home gateway</h4><span class="status-chip status-chip--idle">Off</span></div><p class="accounts-quiet">No hub is configured on ${name}. Workers run direct.</p></section>`;
  const ready = gateway.status === "ready";
  const where = gateway.host === "loopback" ? "on this machine" : gateway.host === "tailnet" ? "on the tailnet" : "";
  const reason = gateway.reason ? `<p class="accounts-flag" data-testid="gateway-reason">${escapeHtml(gateway.reason)}. Gateway-routed workers start direct until the hub is ready.</p>` : "";
  const team = gateway.team_accounts ? `<p class="accounts-quiet" data-testid="gateway-team">${gateway.team_accounts} Team account${gateway.team_accounts === 1 ? " is" : "s are"} on the hub and not listed; Cassy doesn't use Team.</p>` : "";
  const rows = gateway.inventory === "off" ? `<p class="accounts-quiet">Hub accounts aren't listed: set <code>factory.gateway.dashboard</code> and <code>factory.gateway.dashboard_key_file</code> on ${name}.</p>`
    : gateway.inventory === "unavailable" ? `<p class="accounts-flag" role="status">Couldn't read the hub's accounts${gateway.inventory_reason ? `: ${escapeHtml(gateway.inventory_reason)}` : ""}.</p>`
    : gateway.accounts.length ? `<div class="accounts-cols" aria-hidden="true"><span>Hub account</span><span>State</span><span>Requests${gateway.accounts[0]?.recent_minutes ? ` · last ${gateway.accounts[0].recent_minutes >= 120 ? `${Math.round(gateway.accounts[0].recent_minutes / 60)} h` : `${gateway.accounts[0].recent_minutes} min`}` : ""}</span></div><ul class="accounts-ledger">${gateway.accounts.map(account => {
      const [label, tone] = HUB_STATE[account.state] ?? ["State unknown", "open"];
      const provider = HARNESS_NAME[account.provider as keyof typeof HARNESS_NAME] ?? account.provider;
      return `<li class="accounts-row accounts-hub-row" data-testid="gateway-row" data-hub-state="${escapeHtml(account.state)}"><div class="accounts-who">${providerAvatarMarkup({ harness: account.provider, fallback: account.provider.slice(0, 2).toUpperCase(), size: "row" })}<div class="accounts-name"><strong title="${escapeHtml(account.label)}">${escapeHtml(account.label)}</strong><span class="accounts-sub">${escapeHtml(provider)}</span></div></div><div class="accounts-cell"><span class="status-chip status-chip--${tone}">${label}</span></div><div class="accounts-cell accounts-num"><span>${account.recent_requests} request${account.recent_requests === 1 ? "" : "s"}</span>${account.recent_failed ? `<span class="accounts-flag"> · ${account.recent_failed} failed</span>` : ""}</div></li>`;
    }).join("")}</ul>` : `<p class="accounts-quiet">No accounts on the hub.</p>`;
  return `<section class="accounts-gateway" data-testid="accounts-gateway" data-gateway-status="${gateway.status}" aria-label="Home gateway on ${name}"><div class="accounts-group-head"><h4 class="accounts-provider">Home gateway</h4><span class="status-chip status-chip--${ready ? "active" : "blocked"}">${ready ? "Ready" : "Not ready"}</span><span class="accounts-quiet">${where ? `${where} · ` : ""}gateway-routed workers draw on these; the hub picks per request · read-only</span></div>${reason}${rows}${team}</section>`;
}

/** Body-mounted modal: shell redraws do not destroy edits or confirmations. */
export class AccountsSheet {
  private dialog?: HTMLDialogElement;
  private loads = new AbortController();
  private catalogs = new Map<string, AccountsCatalog>();
  private gateways = new Map<string, GatewayInventory>();
  private pending = false;
  private edit?: { machine: AccountsMachine; account: AccountRow; operation: AccountOperation } | { machine: AccountsMachine; cleanup: AccountRow[] };
  /** One editor, moved into the card (or cleanup banner) it acts on. */
  private form!: HTMLFormElement;
  private generation = 0;
  private focusTarget?: { machine: string; account: string; operation: string };
  private readonly usageChecks: AccountUsageChecks;
  constructor(private readonly host: AccountsHost, private readonly doc: Document = document) {
    this.usageChecks = new AccountUsageChecks((...args) => host.checkUsage(...args), () => this.dialog, () => {
      for (const machine of host.machines()) {
        const body = this.machineBody(machine.id), catalog = this.catalogs.get(machine.id);
        if (body && catalog) this.render(machine, body, catalog);
      }
    });
  }

  open(): void {
    const dialog = this.ensureDialog();
    this.loads.abort(); this.loads = new AbortController(); this.generation++;
    this.usageChecks.reset(); this.clearEditor(); this.catalogs.clear(); this.gateways.clear();
    this.notice("");
    const machines = this.host.machines();
    this.$(".accounts-machines").innerHTML = machines.length ? machines.map(machine => `<section data-machine="${escapeHtml(machine.id)}" aria-label="${escapeHtml(machine.label)}"><h3 class="accounts-machine">${escapeHtml(machine.label)}</h3><div class="accounts-machine-body">${SKELETON}</div></section>`).join("") : '<p>No paired machines. Pair a machine to see its accounts.</p>';
    if (!dialog.open) dialog.showModal();
    this.$<HTMLButtonElement>("[data-accounts-close]").focus();
    for (const machine of machines) void this.load(machine);
  }

  private ensureDialog(): HTMLDialogElement {
    if (this.dialog?.isConnected) return this.dialog;
    const dialog = this.doc.createElement("dialog");
    dialog.className = "accounts-sheet"; dialog.setAttribute("aria-labelledby", "accounts-title");
    dialog.innerHTML = `<header class="accounts-heading"><div><h2 id="accounts-title">Accounts</h2><p>Logins, usage and where each one lives, on every paired machine.</p></div><button type="button" data-accounts-close aria-label="Close accounts">Close</button></header><p class="accounts-notice" role="status" aria-label="Accounts notice" aria-live="polite"></p><div class="accounts-machines"></div><form class="accounts-editor" data-testid="accounts-editor" novalidate hidden><h3></h3><p class="accounts-explanation"></p><label class="accounts-field"><span></span><input name="value" type="text" autocomplete="off" spellcheck="false"></label><div class="dialog-actions"><button type="button" data-accounts-cancel>Cancel</button><button type="submit" class="primary" data-accounts-confirm>Confirm</button></div></form>`;
    this.doc.body.append(dialog); this.dialog = dialog;
    this.form = dialog.querySelector<HTMLFormElement>(".accounts-editor")!;
    dialog.addEventListener("close", () => { this.usageChecks.reset(); this.loads.abort(); this.generation++; this.clearEditor(); this.host.returnFocus(); });
    dialog.addEventListener("click", event => {
      if (this.usageChecks.handle(event)) return;
      const button = (event.target as Element).closest<HTMLButtonElement>("button");
      if (!button) return;
      if (button.hasAttribute("data-accounts-close")) dialog.close();
      if (button.hasAttribute("data-accounts-cancel")) { this.clearEditor(); this.focusRow(); }
      const machine = this.host.machines().find(item => item.id === button.dataset.machine);
      if (button.hasAttribute("data-retry") && machine) void this.load(machine);
      if (button.dataset.grantCommand) void this.host.copy(button.dataset.grantCommand).then(() => this.notice("Command copied. Run it on the machine, then open the link it prints here."), () => this.notice("Could not copy. Select the displayed command to copy it."));
      const account = machine && this.catalogs.get(machine.id)?.accounts.find(row => row.key === button.dataset.account);
      if (account && machine && button.dataset.operation && !this.pending) this.begin(machine, account, button.dataset.operation as AccountOperation);
      if (machine && button.hasAttribute("data-cleanup") && !this.pending) this.beginCleanup(machine);
      if (account && machine && button.hasAttribute("data-login")) {
        if (account.provider === "grok") return;
        void this.host.copy(accountLoginCommand(account.provider, account.name)).then(() => this.notice("Login command copied. Run it on the named machine."), () => this.notice("Could not copy. Select the displayed command to copy it."));
      }
    });
    this.form.addEventListener("submit", event => { event.preventDefault(); void this.submit(); });
    this.form.addEventListener("input", () => this.syncConfirm());
    // Escape cancels the confirmation, not the whole sheet.
    this.form.addEventListener("keydown", event => {
      if (event.key !== "Escape" || !this.edit || this.pending) return;
      event.preventDefault(); event.stopPropagation();
      this.clearEditor(); this.focusRow();
    });
    return dialog;
  }

  private $<T extends HTMLElement = HTMLElement>(selector: string): T { return this.dialog!.querySelector<T>(selector)!; }
  private editor(): HTMLFormElement { return this.form; }
  private input(): HTMLInputElement { return this.form.elements.namedItem("value") as HTMLInputElement; }
  private clearEditor(): void {
    this.edit = undefined;
    const form = this.editor(); form.hidden = true;
    this.input().value = "";
    form.querySelector("h3")!.textContent = "";
    form.querySelector(".accounts-explanation")!.textContent = "";
    form.querySelector(".accounts-field span")!.textContent = "";
    this.placeEditor();
  }
  /** The typed confirmation a removal or disable needs; rename needs any new alias. */
  private confirmationMatches(): boolean {
    const value = this.input().value;
    if (!this.edit) return false;
    if ("cleanup" in this.edit) return value === CLEANUP_WORD;
    return this.edit.operation === "rename" ? Boolean(value.trim()) : value === this.edit.account.key;
  }
  private syncConfirm(): void {
    this.form.querySelector<HTMLButtonElement>("[data-accounts-confirm]")!.disabled = this.pending || !this.confirmationMatches();
  }
  /** Keep the editor inside the card, or right under the cleanup banner, it acts on, across redraws. */
  private placeEditor(): void {
    const form = this.form, dialog = this.dialog!;
    const focused = form.contains(this.doc.activeElement);
    let anchor: Element | null | undefined;
    let banner: Element | null | undefined;
    if (this.edit) {
      const body = this.machineBody(this.edit.machine.id);
      // The banner is a flex row and the dialog form a zero-intrinsic-width grid (styles.css `dialog form`), so inside it the form collapses to a sliver: sit beside it.
      if ("cleanup" in this.edit) banner = body?.querySelector(".accounts-cleanup-banner");
      else {
        const key = this.edit.account.key;
        anchor = [...body?.querySelectorAll<HTMLElement>("[data-account-key]") ?? []].find(node => node.dataset.accountKey === key);
      }
    }
    if (banner) { if (form.previousElementSibling !== banner) banner.after(form); }
    else if (anchor) { if (form.parentElement !== anchor) anchor.append(form); }
    else if (form.parentElement !== dialog) dialog.append(form);
    if (focused && !form.contains(this.doc.activeElement)) this.input().focus();
  }
  private render(machine: AccountsMachine, body: HTMLElement, catalog: AccountsCatalog): void {
    body.innerHTML = this.rows(machine, catalog); fillMeters(body); this.placeEditor();
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
    body.innerHTML = SKELETON;
    void this.host.gateway(machine.id, signal).then(inventory => {
      if (signal.aborted || generation !== this.generation) return;
      this.gateways.set(machine.id, inventory);
      const catalog = this.catalogs.get(machine.id), current = this.machineBody(machine.id);
      if (catalog && current) this.render(machine, current, catalog);
    }, () => undefined);
    try {
      const catalog = await this.host.list(machine.id, signal);
      if (signal.aborted || generation !== this.generation) return;
      this.catalogs.set(machine.id, catalog);
      this.render(machine, body, catalog);
    } catch {
      if (signal.aborted || generation !== this.generation) return;
      body.innerHTML = `<p role="alert">Could not load accounts on ${escapeHtml(machine.label)}.</p><button type="button" data-retry data-machine="${escapeHtml(machine.id)}">Try again</button>`;
    }
  }

  private rows(machine: AccountsMachine, catalog: AccountsCatalog): string {
    const gateway = gatewaySection(machine, this.gateways.get(machine.id));
    if (!catalog.accounts.length) return `<p class="accounts-quiet">No accounts on this machine.</p>${gateway}`;
    const manage = machine.scopes.includes("factory-manage");
    const cleanup = catalog.accounts.filter(needsCleanup);
    const observed = catalog.accounts.map(row => row.usage_observed_at).filter((at): at is string => Boolean(at)).sort().at(-1);
    const why = manage ? "" : "Needs manage access on this machine";
    const source = catalog.quota_source;
    const from = source?.state === "hub" ? " · usage from the hub, one number per login on every machine" : "";
    const unreachable = source?.state === "unreachable" ? `<p class="accounts-flag" data-testid="accounts-hub-unreachable" role="status">Hub unreachable${source.reason ? `: ${escapeHtml(source.reason)}` : ""}. Usage is hidden until it answers, rather than showing this machine's own read.</p>` : "";
    const summary = `<div class="accounts-summary" data-quota-source="${escapeHtml(source?.state ?? "off")}"><p class="accounts-quiet">${catalog.accounts.length} account${catalog.accounts.length === 1 ? "" : "s"}${observed && source?.state !== "unreachable" ? ` · usage read ${escapeHtml(accountTime(observed))}` : ""}${from}</p>${unreachable}${cleanup.length ? `<div class="accounts-cleanup-banner" data-testid="accounts-cleanup-banner" role="note"><p><strong>${cleanup.length} to clean up</strong> · stale logins and duplicates</p><button type="button" data-cleanup data-machine="${escapeHtml(machine.id)}"${why ? ` disabled title="${escapeHtml(why)}"` : ""}>Clean up</button></div>` : ""}</div>`;
    const groups = PROVIDERS.map(([provider, label]) => {
      const accounts = catalog.accounts.filter(row => row.provider === provider).sort((a, b) => Number(needsCleanup(a)) - Number(needsCleanup(b)) || headroom(b) - headroom(a));
      if (!accounts.length) return "";
      const next = nextLaunch(accounts);
      return `<section class="accounts-provider-group" aria-label="${label} on ${escapeHtml(machine.label)}"><h4 class="accounts-provider">${HARNESS_NAME[provider]} · ${accounts.length}</h4><ul class="accounts-ledger">${accounts.map(account => this.card(machine, account, manage, catalog.accounts, account === next)).join("")}</ul></section>`;
    }).join("");
    const cols = `<div class="accounts-cols" aria-hidden="true"><span>Account</span><span>5-hour window</span><span>Weekly window</span></div>`;
    return `${summary}${cols}${groups}${gateway}${manage ? "" : this.grantCallout(machine)}`;
  }

  /** DESIGN.md: read-only pairings see the actions and exactly how to unlock them. */
  private grantCallout(machine: AccountsMachine): string {
    const command = scopeGrantCommand(this.doc.defaultView?.location.origin ?? "", machine.scopes, FACTORY_MANAGE_SCOPE);
    return `<div class="accounts-grant" data-testid="accounts-grant" role="note"><p><strong>Read-only on ${escapeHtml(machine.label)}.</strong> Removing, disabling, renaming and choosing the default need manage access. Run this on ${escapeHtml(machine.label)}, then open the link it prints here:</p><div class="accounts-login"><code>${escapeHtml(command)}</code><button type="button" data-grant-command="${escapeHtml(command)}">Copy command</button></div></div>`;
  }

  /** One ledger row: identity and actions, then the two windows on the shared grid. */
  private card(machine: AccountsMachine, account: AccountRow, manage: boolean, accounts: readonly AccountRow[], next: boolean): string {
    const attrs = `data-machine="${escapeHtml(machine.id)}" data-account="${escapeHtml(account.key)}"`;
    const grok = account.provider === "grok";
    const checked = this.usageChecks.result(machine.id, account.key);
    const lookalike = harnessLookalike(account) ? ` · ${HARNESS_NAME[account.provider]} profile` : "";
    const flag = account.duplicate_of ? `<span class="accounts-flag" data-duplicate-of>Duplicate of ${duplicateName(account.duplicate_of, accounts, machine.label)}</span>` : account.login_state === "stale_login" ? `<span class="accounts-flag">Stale login</span>` : "";
    const shown = readings(checked?.windows ?? account.windows, checked ? checked.observed_at : account.usage_observed_at, Date.now(), checked ? undefined : account.usage_source);
    const low = next && account.recommendation ? lowPickNote(shown) : undefined;
    const nextTag = next ? `<span class="accounts-next" data-testid="account-next" title="${escapeHtml(account.recommendation ?? "Default profile")}">Next launch</span><span class="accounts-sub">${escapeHtml(account.recommendation ?? "default profile")}</span>` : "";
    const lowTag = low ? `<span class="accounts-flag accounts-why" data-testid="account-low-pick">${escapeHtml(low)}</span>` : "";
    // With a hub configured, a number that isn't the hub's says whose it is.
    const local = (checked && account.usage_source) || account.usage_source === "local" ? `<span class="accounts-sub" data-testid="account-local-read">${escapeHtml(machine.label)}'s own read</span>` : "";
    const sub = [nextTag, lowTag, flag, local, !next && account.is_default ? `<span class="accounts-sub">default</span>` : "", account.disabled ? `<span class="accounts-sub">disabled</span>` : "", account.location ? `<code class="accounts-sub" title="${escapeHtml(account.location)} on ${escapeHtml(machine.label)}">${escapeHtml(account.location)}</code>` : ""].filter(Boolean).join("");
    const operations = (Object.keys(ACTIONS) as AccountOperation[]).filter(op => op !== (account.disabled ? "disable" : "enable"));
    const button = (op: AccountOperation) => {
      const reason = !manage ? "Needs manage access on this machine" : account.capabilities[op];
      return `<button type="button" ${attrs} data-operation="${op}"${this.pending || reason !== null ? ` disabled title="${escapeHtml(reason ?? "Unavailable")}"` : ""}>${ACTIONS[op]}</button>`;
    };
    const reasons = [...new Set([account.capabilities.remove, account.capabilities.rename].filter(Boolean))].join(" · ");
    const actions = grok ? "" : `<div class="accounts-actions" role="group" aria-label="Actions for ${escapeHtml(account.display_name)}">${this.usageChecks.markup(machine.id, account.key)}<details class="accounts-more" data-testid="account-actions"><summary aria-label="More actions for ${escapeHtml(account.display_name)}" title="More actions">⋯</summary><div class="accounts-menu">${operations.map(button).join("")}${reasons ? `<p class="accounts-quiet" data-testid="accounts-readonly">${escapeHtml(reasons)}</p>` : ""}</div></details></div>`;
    const loggedIn = account.login_state === "logged_in";
    const cells = grok ? `<div class="accounts-cell accounts-span"><p class="accounts-quiet" data-testid="accounts-readonly">Grok uses one account and doesn't report usage.</p></div>`
      : !loggedIn && !checked ? `<div class="accounts-cell accounts-span"><p class="accounts-quiet">Not read · ${account.login_state === "stale_login" ? "stale login" : "logged out"}</p><div class="accounts-login"><code>${escapeHtml(accountLoginCommand(account.provider, account.name))}</code><button type="button" ${attrs} data-login aria-label="Copy login command for ${escapeHtml(account.display_name)}">Copy</button></div></div>`
      : shown.map(reading => `<div class="accounts-cell" data-col="${reading.slot}">${meterMarkup(reading, { labelled: true })}</div>`).join("");
    return `<li class="accounts-row${needsCleanup(account) ? " needs-cleanup" : ""}${next ? " is-next" : ""}" data-testid="account-row" data-account-key="${escapeHtml(account.key)}" data-login-state="${escapeHtml(account.login_state)}"><div class="accounts-who">${providerAvatarMarkup({ harness: account.provider, fallback: account.provider.slice(0, 2).toUpperCase(), size: "row" })}<div class="accounts-name"><strong title="${escapeHtml(account.key)}">${escapeHtml(account.display_name)}</strong><span class="sr-only"> (${escapeHtml(account.key)})${lookalike}</span><span class="accounts-subline">${sub}</span></div>${actions}</div>${cells}</li>`;
  }

  private begin(machine: AccountsMachine, account: AccountRow, operation: AccountOperation): void {
    if (!machine.scopes.includes("factory-manage") || account.capabilities[operation] !== null) return;
    this.focusTarget = { machine: machine.id, account: account.key, operation };
    if (operation === "enable" || operation === "set_default") { void this.mutate(machine, [{ operation, account: account.key }]); return; }
    this.edit = { machine, account, operation };
    const form = this.editor(); form.hidden = false;
    form.querySelector("h3")!.textContent = `${ACTIONS[operation]} ${account.display_name}`;
    const rename = operation === "rename";
    form.querySelector(".accounts-explanation")!.textContent = operation === "remove" ? `Permanently remove ${account.key} on ${machine.label}. Shared base items stay intact. Active and main accounts are protected.` : operation === "disable" ? `Exclude ${account.key} from automatic selection. The login stays available for manual selection.` : `Choose a new alias for ${account.key} on ${machine.label}. Active and main accounts are protected.`;
    form.querySelector(".accounts-field span")!.textContent = rename ? "New account alias" : `Type ${account.key} to ${CONFIRM_VERB[operation] ?? "confirm"} it`;
    const input = this.input();
    input.value = rename ? account.name : "";
    this.placeEditor(); this.syncConfirm();
    input.focus(); input.select();
  }

  /** One confirmation removes every stale or duplicate copy this pairing may remove. */
  private beginCleanup(machine: AccountsMachine): void {
    const cleanup = (this.catalogs.get(machine.id)?.accounts ?? []).filter(row => needsCleanup(row) && row.capabilities.remove === null);
    if (!machine.scopes.includes("factory-manage") || !cleanup.length) return;
    this.focusTarget = undefined;
    this.edit = { machine, cleanup };
    const form = this.editor(); form.hidden = false;
    form.querySelector("h3")!.textContent = `Clean up ${cleanup.length} account${cleanup.length === 1 ? "" : "s"} on ${machine.label}`;
    form.querySelector(".accounts-explanation")!.textContent = `Permanently remove ${cleanup.map(row => row.display_name).join(", ")}. The accounts they duplicate stay; shared base items stay intact.`;
    form.querySelector(".accounts-field span")!.textContent = `Type ${CLEANUP_WORD} to remove them`;
    this.input().value = "";
    this.placeEditor(); this.syncConfirm();
    this.input().focus();
  }

  private async submit(): Promise<void> {
    if (!this.edit || this.pending || !this.confirmationMatches()) return;
    const input = this.input();
    if ("cleanup" in this.edit) {
      await this.mutate(this.edit.machine, this.edit.cleanup.map(row => ({ operation: "remove" as const, account: row.key, confirmation: row.key })));
      return;
    }
    const { machine, account, operation } = this.edit;
    await this.mutate(machine, [{ operation, account: account.key, ...(operation === "rename" ? { new_name: input.value } : { confirmation: input.value }) }]);
  }

  private async mutate(machine: AccountsMachine, changes: readonly AccountChange[]): Promise<void> {
    this.pending = true;
    const generation = this.generation;
    this.dialog!.querySelectorAll<HTMLButtonElement>("[data-operation], .accounts-editor button").forEach(button => button.disabled = true);
    this.notice("Changing account…");
    let success = false;
    try { for (const change of changes) await this.host.change(machine.id, change); success = true; }
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
    this.notice(success ? (changes.length > 1 ? `Removed ${changes.length} accounts.` : "Account updated.") : "Could not verify the complete change. The list has been refreshed; check the account on the machine before trying again.");
    // Re-render all rows to restore their capability-derived disabled states.
    for (const [id, catalog] of this.catalogs) {
      const current = this.host.machines().find(item => item.id === id);
      if (current) { const body = this.machineBody(id); if (body) this.render(current, body, catalog); }
    }
    await this.load(machine); this.focusRow();
  }
  private focusRow(): void {
    const target = [...this.dialog!.querySelectorAll<HTMLButtonElement>("[data-operation]")].find(button => !button.disabled && button.dataset.machine === this.focusTarget?.machine && button.dataset.account === this.focusTarget?.account && button.dataset.operation === this.focusTarget?.operation);
    const menu = target?.closest("details");
    if (menu) menu.open = true;
    (target ?? this.$<HTMLButtonElement>("[data-accounts-close]")).focus();
  }
}
