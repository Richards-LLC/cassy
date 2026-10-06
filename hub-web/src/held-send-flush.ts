/** Serialize durable dispatch and retain recovery callbacks during a batch. */
export class HeldSendFlushes {
  private readonly active = new Map<string, { again: boolean; work: () => Promise<void> }>();

  async run(key: string, work: () => Promise<void>): Promise<void> {
    const current = this.active.get(key);
    if (current) {
      current.again = true;
      current.work = work;
      return;
    }
    const entry = { again: false, work };
    this.active.set(key, entry);
    try {
      // A definite no-write may be settling after the last live callback.
      // Run that requested work once more; the journal still owns permission
      // to write, so an uncertain or revoked claim is never replayed here.
      do {
        entry.again = false;
        await entry.work();
      } while (entry.again);
    } finally {
      this.active.delete(key);
    }
  }
}
