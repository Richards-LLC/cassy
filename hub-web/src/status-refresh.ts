/** Publish completed session reads in order without waiting for newer flights. */
export class StatusRefresh {
  private readonly sessions = new Map<string, { next: number; published: number; pending: number }>();

  async request<T>(key: string, read: () => Promise<T>, received: (value: T) => void, failed: () => void): Promise<void> {
    const state = this.sessions.get(key) ?? { next: 0, published: 0, pending: 0 };
    this.sessions.set(key, state);
    const request = ++state.next;
    state.pending++;
    const publish = () => {
      if (request < state.published) return false;
      state.published = request;
      return true;
    };
    try {
      const value = await read();
      if (publish()) received(value);
    } catch {
      if (publish()) failed();
    } finally {
      // Keep ordering until every overlapping read has settled, then release it.
      if (--state.pending === 0) this.sessions.delete(key);
    }
  }
}
