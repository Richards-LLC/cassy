/** Structured capacity stays numeric until the operator's browser formats it. */
export interface AccountWindow { name: string; used_percent: number | null; resets_at: string | null }
export type AccountOperation = "remove" | "disable" | "enable" | "rename" | "set_default";
export interface AccountRow {
  key: string; provider: "claude" | "codex" | "grok"; name: string; display_name: string;
  login_state: "logged_in" | "logged_out" | "never_logged_in" | "stale_login" | "unknown";
  disabled: boolean; duplicate_of: string | null; is_default: boolean; recommendation: string | null;
  windows: AccountWindow[]; usage_observed_at: string | null;
  capabilities: Record<AccountOperation, string | null>;
}
export interface AccountsCatalog { accounts: AccountRow[] }
export interface AccountChange { operation: AccountOperation; account: string; confirmation?: string; new_name?: string }

export function accountTime(instant: string): string {
  const date = new Date(instant);
  if (!Number.isFinite(date.getTime())) return "unknown time";
  return new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit", hour12: true, timeZoneName: "short" }).format(date);
}

export function accountCapacity(windows: readonly AccountWindow[], observed?: string | null, now = Date.now()): string[] {
  if (!windows.length) return ["Usage unknown"];
  return windows.map(window => {
    const reset = window.resets_at ? new Date(window.resets_at).getTime() : undefined;
    const known = window.used_percent !== null && Number.isFinite(window.used_percent) && window.used_percent >= 0 && window.used_percent <= 100 && (reset === undefined || (Number.isFinite(reset) && reset > now));
    const amount = known ? `${Math.round(100 - window.used_percent!)}% left` : "usage unknown";
    const resetCopy = reset !== undefined && Number.isFinite(reset) ? ` · ${reset > now ? "resets" : "reset"} ${accountTime(window.resets_at!)}` : "";
    return `${window.name} ${amount}${resetCopy}${observed ? ` · as of ${accountTime(observed)}` : ""}`;
  });
}
