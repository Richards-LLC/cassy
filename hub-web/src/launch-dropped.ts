/**
 * Machines whose code re-pair dropped the start-sessions permission
 * (cas-0e14 F29), kept across a reload (cas-093d F01). Without it, New
 * session forgot after a reload why starting sessions needed allowing again
 * and fell back to the generic lead. One small localStorage entry of machine
 * ids; cleared per machine when starting sessions is allowed again or the
 * machine is removed from this browser.
 */
export type LaunchDroppedStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;

export const LAUNCH_DROPPED_KEY = "cas-commander-launch-dropped:v1";
/** Far more machines than any browser pairs; a bound for a corrupted entry. */
const MAX_MACHINES = 64;

export function loadLaunchDropped(storage: LaunchDroppedStorage | undefined): Set<string> {
  let raw: string | null = null;
  try { raw = storage?.getItem(LAUNCH_DROPPED_KEY) ?? null; } catch { return new Set(); }
  if (!raw) return new Set();
  let parsed: unknown;
  try { parsed = JSON.parse(raw); } catch { return new Set(); }
  if (!Array.isArray(parsed)) return new Set();
  return new Set(parsed.filter((id): id is string => typeof id === "string" && id.length > 0 && id.length <= 200).slice(0, MAX_MACHINES));
}

export function saveLaunchDropped(storage: LaunchDroppedStorage | undefined, machines: ReadonlySet<string>): void {
  if (!storage) return;
  try {
    if (machines.size) storage.setItem(LAUNCH_DROPPED_KEY, JSON.stringify([...machines].slice(0, MAX_MACHINES)));
    else storage.removeItem(LAUNCH_DROPPED_KEY);
  } catch { /* full or denied: the in-memory state still holds for this page */ }
}
