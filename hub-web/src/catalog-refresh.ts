/** One event-driven catalog flight, with a guaranteed trailing refresh. */
export class CoalescedRefresh {
  private running?: Promise<void>;
  private pending = false;
  private nextStart = 0;

  constructor(
    private readonly refresh: () => Promise<unknown>,
    private readonly failed: (error: unknown) => void,
    private readonly intervalMs = 1_000,
  ) {}

  request(): Promise<void> {
    this.pending = true;
    // Publish the flight before invoking refresh, including reentrant callers.
    this.running ??= Promise.resolve().then(() => this.drain());
    return this.running;
  }

  private async drain(): Promise<void> {
    try {
      do {
        const wait = this.nextStart - Date.now();
        if (wait > 0) await new Promise<void>(resolve => setTimeout(resolve, wait));
        this.pending = false;
        this.nextStart = Date.now() + this.intervalMs;
        try { await this.refresh(); }
        catch (error) { this.failed(error); }
      } while (this.pending);
    } finally {
      // Clear synchronously with the final pending check so no event is lost
      // between the drain resolving and a separate promise-finally callback.
      this.running = undefined;
    }
  }
}
