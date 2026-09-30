import type { Page } from "@playwright/test";
import { JOURNEY_NOW } from "./clock";
import type { ProtocolTime } from "./hub-double";

/** One clock for browser deadlines AND delayed wire replies. The protocol's
 * fetch/sign/socket work settles between ticks: jumping past an HTTP deadline
 * before the test host can answer would manufacture an outage. */
export class ProtocolClock implements ProtocolTime {
  private instant = JOURNEY_NOW;
  private origin = JOURNEY_NOW;
  private nextId = 0;
  private readonly callbacks = new Map<number, () => void>();
  private readonly registrations = new Set<Promise<unknown>>();

  constructor(private readonly page: Page) {}

  async start(): Promise<void> {
    // Seed the empty app first, then pause before navigating to paired machines.
    this.instant = JOURNEY_NOW + 60_000;
    await this.page.clock.pauseAt(this.instant);
    this.origin = this.instant - await this.page.evaluate(() => performance.now());
    await this.page.exposeBinding("__protocolMoment", (_source, elapsed: number) => {
      this.instant = this.origin + elapsed;
    });
    await this.page.exposeBinding("__protocolTimer", (_source, id: number, elapsed: number) => {
      this.instant = this.origin + elapsed;
      const callback = this.callbacks.get(id);
      this.callbacks.delete(id);
      callback?.();
    });
    await this.page.addInitScript(() => {
      const wire = window as unknown as {
        __protocolMoment: (elapsed: number) => Promise<void>;
        __protocolIdle: () => Promise<void>;
      };
      const pending = new Set<Promise<unknown>>();
      function observe<T>(work: Promise<T>): Promise<T> {
        pending.add(work);
        void work.then(() => pending.delete(work), () => pending.delete(work));
        return work;
      }
      wire.__protocolIdle = async () => {
        while (pending.size) await Promise.allSettled([...pending]);
      };
      const fetch = window.fetch.bind(window);
      window.fetch = (...args) => observe((async () => {
        // Publish the browser's time BEFORE the double receives a signed request.
        await wire.__protocolMoment(performance.now());
        return fetch(...args);
      })());
      // fetch resolves at headers; await body decoding too, including cloned
      // reasoned 401s, before advancing the next authentication deadline.
      const json = Response.prototype.json;
      Response.prototype.json = function () { return observe(json.call(this)); };
      const sign = crypto.subtle.sign.bind(crypto.subtle);
      crypto.subtle.sign = (...args) => observe(sign(...args));
      const Socket = window.WebSocket;
      window.WebSocket = class extends Socket {
        constructor(url: string | URL, protocols?: string | string[]) {
          super(url, protocols);
          observe(new Promise<void>(resolve => {
            for (const event of ["open", "error", "close"]) this.addEventListener(event, () => resolve(), { once: true });
          }));
          // A binding round trip acknowledges each received frame after its
          // synchronous app listeners, without inspecting the connection's fields.
          this.addEventListener("message", () => { observe(wire.__protocolMoment(performance.now())); });
        }
      };
    });
  }

  now(): number { return this.instant; }

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
    // Two seconds is below the protocol's shortest HTTP/state timeout (3 s).
    // These are virtual ticks, each followed by IO acknowledgements, not sleeps.
    while (ms > 0) {
      const tick = Math.min(ms, 2_000);
      await Promise.all(this.registrations);
      await this.page.clock.runFor(tick);
      const elapsed = await this.page.evaluate(async () => {
        const wire = window as unknown as {
          __protocolIdle: () => Promise<void>;
          __protocolReplies?: Set<Promise<void>>;
        };
        await Promise.all(wire.__protocolReplies ?? []);
        await wire.__protocolIdle();
        return performance.now();
      });
      await Promise.all(this.registrations);
      this.instant = this.origin + elapsed;
      ms -= tick;
    }
  }
}
