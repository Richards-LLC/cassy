/** Structured capacity stays numeric until the operator's browser formats it. */
export interface AccountWindow { name: string; used_percent: number | null; resets_at: string | null }
export type AccountOperation = "remove" | "disable" | "enable" | "rename" | "set_default";
export interface AccountRow {
  key: string; provider: "claude" | "codex" | "grok"; name: string; display_name: string;
  /** Home-relative profile folder ("~/.claude-work"); older hubs omit it. */
  location?: string;
  login_state: "logged_in" | "logged_out" | "never_logged_in" | "stale_login" | "unknown";
  disabled: boolean; duplicate_of: string | null; is_default: boolean; recommendation: string | null;
  /** Why automatic selection skipped this login, in the selector's words; older hubs omit it. */
  exclusion?: string | null;
  windows: AccountWindow[]; usage_observed_at: string | null;
  /** `hub`: the one shared number for this login; `local`: this machine's own read; `hub_unreachable`: none shown. Absent: no hub configured. */
  usage_source?: UsageSource;
  capabilities: Record<AccountOperation, string | null>;
}
export type UsageSource = "hub" | "local" | "hub_unreachable";
/** Where this machine's capacity numbers come from. Older hubs omit it. */
export interface QuotaSource { state: "off" | "hub" | "unreachable"; reason?: string }
export interface AccountsCatalog { accounts: AccountRow[]; quota_source?: QuotaSource }
/** One account on the machine's home gateway hub (GET /v1/accounts/gateway). Read-only; Team plans never appear. */
export interface HubAccount { provider: string; label: string; state: "active" | "disabled" | "unavailable"; recent_requests: number; recent_failed: number; recent_minutes?: number }
export interface GatewayInventory {
  status: "off" | "ready" | "unavailable"; reason?: string; host?: "loopback" | "tailnet";
  inventory: "ok" | "off" | "unavailable"; inventory_reason?: string;
  accounts: HubAccount[]; team_accounts: number;
}
export interface AccountChange { operation: AccountOperation; account: string; confirmation?: string; new_name?: string }

export function accountTime(instant: string): string {
  const date = new Date(instant);
  if (!Number.isFinite(date.getTime())) return "unknown time";
  // Account checks use Eastern 12-hour time with the generic zone ("1:10 PM ET", never EDT/EST).
  return new Intl.DateTimeFormat("en-US", { timeZone: "America/New_York", month: "short", day: "numeric", hour: "numeric", minute: "2-digit", hour12: true, timeZoneName: "shortGeneric" }).format(date);
}

