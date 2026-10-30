/** How long a batch waits before re-attempting a write its socket could not take (cas-5dbc). */
export const HELD_SEND_RETRY_MS = 1_000;

/** Serialize durable dispatch and retain recovery callbacks during a batch. */
export class HeldSendFlushes {
  private readonly active = new Map<string, { again: boolean; work: () => Promise<void | "waiting"> }>();
  private readonly retries = new Map<string, ReturnType<typeof setTimeout>>();

  constructor(private readonly retryMs = HELD_SEND_RETRY_MS) {}

  /**
   * Run `work` for `key`, once more for each call that arrives meanwhile. A
   * batch that ends "waiting" (the socket could not take the write: a
   * definite no-write) is re-attempted after `retryMs`. It no longer depends
   * on a later live callback, which may never come once the header already
   * says Live (cas-5dbc). The work itself decides whether anything is still
   * held and whether the session is up.
   */
  async run(key: string, work: () => Promise<void | "waiting">): Promise<void> {
    const current = this.active.get(key);
    if (current) {
      current.again = true;
      current.work = work;
      return;
    }
    const entry: { again: boolean; work: () => Promise<void | "waiting"> } = { again: false, work };
    this.active.set(key, entry);
    let result: void | "waiting";
    try {
      // A definite no-write may be settling after the last live callback.
      // Run that requested work once more; the journal still owns permission
      // to write, so an uncertain or revoked claim is never replayed here.
      do {
        entry.again = false;
        result = await entry.work();
      } while (entry.again);
    } finally {
      this.active.delete(key);
    }
    if (result === "waiting" && !this.retries.has(key)) {
      const latest = entry.work;
      this.retries.set(key, setTimeout(() => {
        this.retries.delete(key);
        void this.run(key, latest);
      }, this.retryMs));
    }
  }
}

/**
 * Dispatch a conversation's held sends in order, each once. `dispatch`
 * returns false to stop the batch, or the work to run once the entry has left
 * the queue.
 */
export async function drainHeld<T>(queue: T[], dispatch: (entry: T) => Promise<false | (() => Promise<void> | void)>): Promise<void> {
  while (queue.length) {
    const entry = queue[0]!;
    const settle = await dispatch(entry);
    if (!settle) return;
    queue.shift();
    await settle();
  }
}
