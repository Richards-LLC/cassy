/** Only the latest read for a session may publish status or failure. */
export class StatusRefresh {
  private readonly latest = new Map<string, symbol>();

  async request<T>(key: string, read: () => Promise<T>, received: (value: T) => void, failed: () => void): Promise<void> {
    const request = Symbol();
    this.latest.set(key, request);
    try {
      const value = await read();
      if (this.latest.get(key) === request) received(value);
    } catch {
      if (this.latest.get(key) === request) failed();
    } finally {
      // A stale response must not release a newer read's ownership.
      if (this.latest.get(key) === request) this.latest.delete(key);
    }
  }
}
