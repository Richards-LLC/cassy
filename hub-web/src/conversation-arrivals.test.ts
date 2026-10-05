import { describe, expect, it } from "vitest";
import { ConversationHistory } from "./conversation-history";
import { arrivalStore } from "./conversation-store";
import { activityTime } from "./conversation-list";
import { shownTimes } from "./thread-model";
import type { OperatorReply } from "./types";

const MINUTE = 60_000;
const noon = new Date(2026, 8, 30, 12).getTime();
const session = "patient-pelican-9";
const key = `atlas:${session}`;
const reply = (id: number, parent: number): OperatorReply => ({ notification_id: id, reply_to: parent, message: `answer ${id}`, summary: "", device_id: "fixture", kind: "answer", attachments: [] });
const message = (id: number, at: number): Parameters<ConversationHistory["hydrateSend"]>[0] => ({ notification_id: id, target: "supervisor", text: `message ${id}`, state: "acknowledged", stamped: true, device_id: "fixture", session, at: new Date(at).toISOString() });
const stampedReply = (id: number, parent: number, at: number): Parameters<ConversationHistory["hydrateReply"]>[0] => ({ ...reply(id, parent), session, at: new Date(at).toISOString() });

describe("observed turn times survive repeated reloads (cas-940f)", () => {
  it.each(["steady five-minute lead", "machine clock corrected backward"])("keeps both exchanges and row age with %s", (clock) => {
    const values = new Map<string, string>();
    const storage = { getItem: (name: string) => values.get(name) ?? null, setItem: (name: string, value: string) => { values.set(name, value); }, removeItem: (name: string) => { values.delete(name); } };
    const fresh = () => { const history = new ConversationHistory(); history.currentSession = session; const record = arrivalStore(storage).load().get(key); if (record) history.seedArrivals(record); return history; };
    const save = (history: ConversationHistory) => arrivalStore(storage).save(key, history.arrivalsRecord());
    const exchange = (history: ConversationHistory, id: number, at: number) => {
      history.submit(`c${id}`, "supervisor", `message ${id}`, at, undefined, session);
      history.acknowledge({ client_ref: `c${id}`, notification_id: id, target: "supervisor", stamped: true });
      history.receive(reply(id + 1, id), at, session);
      save(history);
    };
    const times = (history: ConversationHistory, now: number) => shownTimes(history.events, now).map(time => ({ at: time.at, marked: time.clockAhead }));
    const first = fresh();
    exchange(first, 1, noon);
    const reloaded = fresh();
    reloaded.hydrateSend(message(1, noon + 5 * MINUTE), noon + 3 * MINUTE);
    reloaded.hydrateReply(stampedReply(2, 1, noon + 5 * MINUTE), noon + 3 * MINUTE);
    expect(reloaded.machineLead()).toBe(5 * MINUTE);
    exchange(reloaded, 3, noon + 8 * MINUTE);
    const expected = times(reloaded, noon + 8 * MINUTE);
    expect(expected).toEqual([{ at: noon, marked: false }, { at: noon, marked: false }, { at: noon + 8 * MINUTE, marked: false }, { at: noon + 8 * MINUTE, marked: true }]);
    expect(arrivalStore(storage).load().get(key)?.at).toMatchObject({ "s:3": noon + 8 * MINUTE, "r:4": noon + 8 * MINUTE });
    const laterStamp = noon + (clock === "steady five-minute lead" ? 13 : 5) * MINUTE;
    // Match the real history page: hydrate all sends before all replies.
    for (const minute of [9, 14, 30]) {
      const again = fresh();
      const now = noon + minute * MINUTE;
      again.hydrateSend(message(1, noon + 5 * MINUTE), now);
      again.hydrateSend(message(3, laterStamp), now);
      again.hydrateReply(stampedReply(2, 1, noon + 5 * MINUTE), now);
      again.hydrateReply(stampedReply(4, 3, laterStamp), now);
      expect(times(again, now)).toEqual(expected);
      expect(activityTime(again.lastActivityAt()!, now).short).toBe(`${minute - 8}m`);
      save(again);
      expect(arrivalStore(storage).load().get(key)?.at).toMatchObject({ "s:3": noon + 8 * MINUTE, "r:4": noon + 8 * MINUTE });
    }
  });

  it("stores the time actually shown for old history, not the later hydration time", () => {
    const first = new ConversationHistory();
    first.hydrateSend(message(1, noon), noon + 20 * MINUTE);
    first.hydrateReply(stampedReply(2, 1, noon + MINUTE), noon + 20 * MINUTE);
    const expected = shownTimes(first.events, noon + 20 * MINUTE);
    const again = new ConversationHistory();
    again.seedArrivals(first.arrivalsRecord());
    again.hydrateSend(message(1, noon), noon + 30 * MINUTE);
    again.hydrateReply(stampedReply(2, 1, noon + MINUTE), noon + 30 * MINUTE);
    expect(shownTimes(again.events, noon + 30 * MINUTE)).toEqual(expected);
    expect(again.lastActivityAt()).toBe(noon + MINUTE);
    expect(first.arrivalsRecord().at).toEqual({ "s:1": noon, "r:2": noon + MINUTE });
  });
});
