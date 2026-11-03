import { escapeHtml } from "./cloud-brand";
import { accountTime, type AccountWindow } from "./accounts-model";

export interface CheckedAccountUsage {
  checked_at: string; observed_at: string | null; source: string;
  windows: AccountWindow[]; reason: string | null;
}
export type CheckAccountUsage = (machine: string, account: string, signal: AbortSignal) => Promise<CheckedAccountUsage>;
type CheckState = { pending?: boolean; result?: CheckedAccountUsage; error?: string };

/** One owner for explicit checks in either sheet; never changes account selection. */
export class AccountUsageChecks {
  private states = new Map<string, CheckState>();
  private controller = new AbortController();
  constructor(private request: CheckAccountUsage, private root: () => HTMLElement | undefined, private completed: () => void) {}
  reset(): void { this.controller.abort(); this.controller = new AbortController(); this.states.clear(); }
  private key(machine: string, account: string): string { return JSON.stringify([machine, account]); }
  hasResult(machine: string, account: string): boolean { return Boolean(this.states.get(this.key(machine, account))?.result); }
  markup(machine: string, account: string): string {
    const state = this.states.get(this.key(machine, account)) ?? {};
    const result = state.result;
    const windows = result ? ["5h", "weekly"].map(name => {
      const window = result.windows.find(window => window.name === name);
      const amount = window?.used_percent;
      return `<p>${name} ${amount === null || amount === undefined || !Number.isFinite(amount) || amount < 0 || amount > 100 ? "usage unknown" : `${Math.round(amount)}% used`}${window?.resets_at ? ` · resets ${escapeHtml(accountTime(window.resets_at))}` : " · reset time unknown"}</p>`;
    }).join("") : "";
    const detail = result ? `${windows}<p>Checked at ${escapeHtml(accountTime(result.checked_at))}</p><p>Usage as of ${result.observed_at ? escapeHtml(accountTime(result.observed_at)) : "unknown"} · ${escapeHtml(result.source)}</p>${result.reason ? `<p>${escapeHtml(result.reason)}</p>` : ""}` : "";
    return `<div class="account-usage-check" data-usage-machine="${escapeHtml(machine)}" data-usage-account="${escapeHtml(account)}"><button type="button" data-check-usage${state.pending ? " disabled" : ""} aria-label="Check usage for ${escapeHtml(account)}">${state.pending ? "Checking usage…" : "Check usage"}</button><div class="account-usage-result" role="status" aria-live="polite">${detail}${state.error ? `<p>${escapeHtml(state.error)}</p>` : ""}</div></div>`;
  }
  handle(event: Event): boolean {
    const button = (event.target as Element).closest("[data-check-usage]");
    const holder = button?.closest<HTMLElement>("[data-usage-machine]");
    if (!holder) return false;
    event.preventDefault(); event.stopPropagation();
    void this.check(holder.dataset.usageMachine!, holder.dataset.usageAccount!);
    return true;
  }
  private paint(machine: string, account: string): void {
    for (const holder of this.root()?.querySelectorAll<HTMLElement>("[data-usage-machine]") ?? []) {
      if (holder.dataset.usageMachine === machine && holder.dataset.usageAccount === account) holder.outerHTML = this.markup(machine, account);
    }
  }
  private async check(machine: string, account: string): Promise<void> {
    const key = this.key(machine, account), old = this.states.get(key);
    if (old?.pending) return;
    const signal = this.controller.signal;
    this.states.set(key, { result: old?.result, pending: true }); this.paint(machine, account);
    try {
      const result = await this.request(machine, account, signal);
      if (signal.aborted) return;
      this.states.set(key, { result }); this.completed();
    } catch (error) {
      if (signal.aborted) return;
      this.states.set(key, { result: old?.result, error: error instanceof Error ? (typeof (error as { detail?: unknown }).detail === "string" ? (error as Error & { detail: string }).detail : error.message) : "Could not check usage. Try again." });
    }
    this.paint(machine, account);
    for (const holder of this.root()?.querySelectorAll<HTMLElement>("[data-usage-machine]") ?? []) {
      if (holder.dataset.usageMachine === machine && holder.dataset.usageAccount === account) holder.querySelector<HTMLButtonElement>("button")?.focus();
    }
  }
}
