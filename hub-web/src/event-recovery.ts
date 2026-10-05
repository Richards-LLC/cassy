/** One catalog request in flight, one trailing request for any burst. */
export class CoalescedRefresh {
  private running?: Promise<void>;
  private pending = false;
  constructor(private readonly refresh: () => Promise<unknown>, private readonly failed: (error: unknown) => void) {}
  request(): Promise<void> {
    this.pending = true;
    if (!this.running) this.running = this.drain().finally(() => { this.running = undefined; });
    return this.running;
  }
  private async drain(): Promise<void> {
    do {
      this.pending = false;
      try { await this.refresh(); } catch (error) { this.failed(error); }
    } while (this.pending);
  }
}

/** Revisions are upserts; replayed events never regress an enriched version.
 * A stream epoch is a hub-process identity, not an account or credential. */
export class EventRecovery {
  private epoch?: string;
  private high = 0;
  private revisions = new Map<number, number>();
  begin(epoch: string, oldest: number, latest: number): "epoch_changed" | "retention_gap" | undefined {
    const changed = this.epoch !== undefined && this.epoch !== epoch;
    const gap = this.high > 0 && oldest > this.high + 1;
    if (changed || latest < this.high) { this.high = 0; this.revisions.clear(); }
    this.epoch = epoch;
    return changed ? "epoch_changed" : gap ? "retention_gap" : undefined;
  }
  accept(sequence: number, revision: number): { deliver: boolean; gap: boolean } {
    const known = this.revisions.get(sequence);
    if (known !== undefined && revision <= known) return { deliver: false, gap: false };
    const gap = this.high > 0 && sequence > this.high + 1;
    // Older unknown replay rows may be valid retained history. Keep the latest
    // 1024 ids and accept revisions of known rows regardless of the high water.
    if (known === undefined && sequence < this.high - 1024) return { deliver: false, gap: false };
    this.revisions.set(sequence, revision);
    this.high = Math.max(this.high, sequence);
    while (this.revisions.size > 1024) this.revisions.delete(this.revisions.keys().next().value!);
    return { deliver: true, gap };
  }
}
