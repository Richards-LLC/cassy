/** Commit session status at the async read boundary. */
export class StatusRefresh {
  async request<T>(key: string, read: () => Promise<T>, received: (value: T) => void, failed: () => void): Promise<void> {
    try { received(await read()); }
    catch { failed(); }
  }
}
