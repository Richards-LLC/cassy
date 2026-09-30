import type { Page } from "@playwright/test";
import { JOURNEY_NOW } from "./clock";
import type { ProtocolTime } from "./hub-double";

/** One clock for the page's deadlines AND the double's delayed wire replies.
 * Replies run on page.clock, not a second Node wall clock. Register all pending
 * replies before advancing, so an attach/refusal cannot miss its deadline. */
export class ProtocolClock implements ProtocolTime {
  private instant = JOURNEY_NOW;
  private origin = JOURNEY_NOW;
  private nextId = 0;
  private readonly callbacks = new Map<number, () => void>();
  private readonly registrations = new Set<Promise<unknown>>();

  constructor(private readonly page: Page) {}

  async start(): Promise<void> {
    // Seed the empty app first, then pause before navigating to paired machines.
    // The future instant lets pauseAt work even on a slow host.
    this.instant = JOURNEY_NOW + 60_000;
    await this.page.clock.pauseAt(this.instant);
    this.origin = this.instant - await this.page.evaluate(() => performance.now());
    await this.page.exposeBinding("__protocolTimer", (_source, id: number, elapsed: number) => {
      this.instant = this.origin + elapsed;
      const callback = this.callbacks.get(id);
      this.callbacks.delete(id);
      callback?.();
    });
  }

  now(): number { return this.instant; }

  async synchronize(): Promise<void> {
    this.instant = this.origin + await this.page.evaluate(() => performance.now());
  }

  delay(callback: () => void, ms: number): void {
    const id = ++this.nextId;
    this.callbacks.set(id, callback);
    const registration = this.page.evaluate(({ id, ms }) => {
      const wire = window as unknown as {
        __protocolTimer: (id: number, elapsed: number) => Promise<void>;
        __protocolReplies?: Set<Promise<void>>;
      };
      const replies = wire.__protocolReplies ??= new Set();
      window.setTimeout(() => {
        const reply = wire.__protocolTimer(id, performance.now());
        replies.add(reply);
        void reply.then(() => replies.delete(reply));
      }, ms);
    }, { id, ms });
    this.registrations.add(registration);
    void registration.finally(() => this.registrations.delete(registration));
  }

  async advance(ms: number): Promise<void> {
    await Promise.all(this.registrations);
    this.instant += ms;
    await this.page.clock.runFor(ms);
    await Promise.all(this.registrations);
    // runFor fires browser timers; await their binding acknowledgements too,
    // so deadline assertions cannot accidentally pass on later wall-clock IO.
    await this.page.evaluate(async () => {
      const wire = window as unknown as { __protocolReplies?: Set<Promise<void>> };
      await Promise.all(wire.__protocolReplies ?? []);
    });
    await this.synchronize();
  }
}
