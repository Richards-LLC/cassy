/**
 * Dormant sessions are metadata records whose supervisor is no longer live.
 * They stay hidden in the normal Commander view, while an explicit recovery
 * preference can request them from the hub catalog.
 */
export const DORMANT_STORAGE_KEY = "cas-commander:dormant";
export const DORMANT_QUERY_PARAM = "dormant";

export interface DormantVisibilityStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

const ENABLED = new Set(["1", "true", "yes", "on"]);

function enabledFlag(value: string | null | undefined): boolean | undefined {
  if (value === null || value === undefined) return undefined;
  return ENABLED.has(value.trim().toLowerCase());
}

/** Route state wins so a shared recovery URL is deterministic. */
export function dormantRevealed(search: string, storage: DormantVisibilityStorage | undefined): boolean {
  const fromRoute = enabledFlag(new URLSearchParams(search).get(DORMANT_QUERY_PARAM));
  if (fromRoute !== undefined) return fromRoute;
  return enabledFlag(storage?.getItem(DORMANT_STORAGE_KEY)) ?? false;
}

/** Persist only the non-default state. */
export function saveDormantRevealed(storage: DormantVisibilityStorage | undefined, revealed: boolean): void {
  if (!storage) return;
  if (revealed) storage.setItem(DORMANT_STORAGE_KEY, "1");
  else storage.removeItem(DORMANT_STORAGE_KEY);
}

/** Preserve unrelated route state while changing dormant visibility. */
export function dormantRoute(search: string, revealed: boolean): string {
  const params = new URLSearchParams(search);
  if (revealed) params.set(DORMANT_QUERY_PARAM, "1");
  else params.delete(DORMANT_QUERY_PARAM);
  const query = params.toString();
  return query ? `?${query}` : "";
}

export function dormantCommandLabel(revealed: boolean): { title: string; hint: string } {
  return revealed
    ? { title: "Dormant · Shown", hint: "Hide sessions without a live supervisor" }
    : { title: "Dormant · Hidden", hint: "Show sessions for recovery" };
}
