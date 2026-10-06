/** Serialize a conversation's durable dispatch work. */
export class HeldSendFlushes {
  private readonly active = new Set<string>();

  async run(key: string, work: () => Promise<void>): Promise<void> {
    if (this.active.has(key)) return;
    this.active.add(key);
    try {
      await work();
    } finally {
      this.active.delete(key);
    }
  }
}
