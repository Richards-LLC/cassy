import { escapeHtml } from "./cloud-brand";
import type { AccountWindow } from "./accounts-model";

export interface CheckedAccountUsage {
  checked_at: string; observed_at: string | null; source: string;
  windows: AccountWindow[]; reason: string | null;
}
export type CheckAccountUsage = (machine: string, account: string, signal: AbortSignal) => Promise<CheckedAccountUsage>;
type CheckState = { pending?: boolean; result?: CheckedAccountUsage; error?: string };

function usageFailure(error: unknown): string {
  if (typeof error === "object" && error !== null && "status" in error && error.status === 429) return "Usage check is rate-limited. Try again shortly.";
  if (!(error instanceof Error)) return "Could not check usage. Try again.";
  const detail = (error as { detail?: unknown }).detail;
  return typeof detail === "string" ? detail : error.message;
}

/** One owner for explicit checks in either sheet; never changes account selection. */
export class AccountUsageChecks {
  private states = new Map<string, CheckState>();
  private controller = new AbortController();
  constructor(private request: CheckAccountUsage, private root: () => HTMLElement | undefined, private completed: () => void) {}
  reset(): void { this.controller.abort(); this.controller = new AbortController(); this.states.clear(); }
  private key(machine: string, account: string): string { return JSON.stringify([machine, account]); }
  hasResult(machine: string, account: string): boolean { return Boolean(this.states.get(this.key(machine, account))?.result); }
  /** The latest explicit check for this account; callers draw its windows as meters. */
  result(machine: string, account: string): CheckedAccountUsage | undefined { return this.states.get(this.key(machine, account))?.result; }
  /**
   * The ↻ control and its live status. It never selects or changes an account.
   * The `menu` variant is the same check as a ⋯ menu item, for rows too
   * narrow for an icon beside the name; only the icon owns the live status.
   */
  markup(machine: string, account: string, variant: "icon" | "menu" = "icon"): string {
    const state = this.states.get(this.key(machine, account)) ?? {};
    const status = state.pending ? "Refreshing usage" : state.error ? state.error : state.result ? `Usage refreshed${state.result.reason ? `. ${state.result.reason}` : ""}` : "";
    if (variant === "menu") return `<span class="account-usage-check account-usage-menu" data-usage-variant="menu" data-usage-machine="${escapeHtml(machine)}" data-usage-account="${escapeHtml(account)}"><button type="button" data-check-usage${state.pending ? ' aria-busy="true" disabled' : ""}>${state.pending ? "Refreshing usage…" : "Refresh usage"}</button>${state.error ? `<span class="account-usage-result is-error">${escapeHtml(state.error)}</span>` : ""}</span>`;
    return `<span class="account-usage-check" data-usage-machine="${escapeHtml(machine)}" data-usage-account="${escapeHtml(account)}"><button type="button" class="account-refresh" data-testid="account-refresh" data-check-usage${state.pending ? ' aria-busy="true" disabled' : ""} aria-label="Check usage for ${escapeHtml(account)}" title="Refresh usage"><svg viewBox="0 0 16 16" aria-hidden="true" focusable="false"><path d="M13.5 8a5.5 5.5 0 1 1-1.6-3.9M13.5 2.5v3h-3" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg></button><span class="account-usage-result${state.error ? " is-error" : ""}" data-testid="account-usage-status" role="status" aria-label="Usage for ${escapeHtml(account)}" aria-live="polite">${escapeHtml(status)}</span></span>`;
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
      if (holder.dataset.usageMachine === machine && holder.dataset.usageAccount === account) holder.outerHTML = this.markup(machine, account, holder.dataset.usageVariant === "menu" ? "menu" : "icon");
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
      this.states.set(key, { result: old?.result, error: usageFailure(error) });
    }
    this.paint(machine, account);
    for (const holder of this.root()?.querySelectorAll<HTMLElement>("[data-usage-machine]") ?? []) {
      if (holder.dataset.usageMachine === machine && holder.dataset.usageAccount === account && holder.getClientRects().length) {
        holder.querySelector<HTMLButtonElement>("button")?.focus({ preventScroll: true });
        holder.querySelector<HTMLElement>(".account-usage-result")?.scrollIntoView?.({ block: "center", behavior: "instant" });
      }
    }
  }
}
