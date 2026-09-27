/**
 * Questions the operator dismissed stay dismissed across a reload
 * (cas-16eed): a history page replays the same ask, and without this it would
 * pin itself again. Kept per thread (machine:session), newest last, capped so
 * the store cannot grow without bound.
 */
export const DISMISSED_ASKS_STORAGE_KEY = "cas.commander.dismissed-asks.v1";
/** Dismissed ids kept per thread; older ones are long out of any history page. */
export const DISMISSED_ASKS_PER_THREAD = 50;
/** Threads kept; the least recently touched drop first. */
export const DISMISSED_ASKS_THREADS = 40;

export interface DismissedAsksStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

type Store = Record<string, number[]>;

function read(storage: DismissedAsksStorage | undefined): Store {
  if (!storage) return {};
  try {
    const parsed: unknown = JSON.parse(storage.getItem(DISMISSED_ASKS_STORAGE_KEY) ?? "{}");
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const store: Store = {};
    for (const [thread, ids] of Object.entries(parsed as Record<string, unknown>)) {
      if (Array.isArray(ids)) store[thread] = ids.filter((id): id is number => Number.isSafeInteger(id));
    }
    return store;
  } catch {
    return {};
  }
}

/** The ask ids dismissed in this thread. */
export function loadDismissedAsks(storage: DismissedAsksStorage | undefined, thread: string): number[] {
  return read(storage)[thread] ?? [];
}

/** Record this thread's dismissed ask ids (the whole set, as the history holds it). */
export function saveDismissedAsks(storage: DismissedAsksStorage | undefined, thread: string, ids: readonly number[]): void {
  if (!storage) return;
  const store = read(storage);
  delete store[thread];
  if (ids.length) store[thread] = [...new Set(ids)].slice(-DISMISSED_ASKS_PER_THREAD);
  // Insertion order is recency: the thread just written is last.
  const threads = Object.keys(store);
  for (const stale of threads.slice(0, Math.max(0, threads.length - DISMISSED_ASKS_THREADS))) delete store[stale];
  try { storage.setItem(DISMISSED_ASKS_STORAGE_KEY, JSON.stringify(store)); } catch { /* storage full or blocked: dismissal still holds for this visit */ }
}
