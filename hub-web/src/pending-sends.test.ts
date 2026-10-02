// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { ConversationHistory, RESTORED_MATCH_WINDOW_MS } from "./conversation-history";
import { ConversationView } from "./conversation-view";

const at = (hh: number, mm: number) => new Date(2026, 9, 2, hh, mm).getTime();

/** A thread with one message of each unsettled kind, plus a delivered one. */
function unsettledThread(): ConversationHistory {
  const history = new ConversationHistory();
  history.submit("delivered", "sup", "Already delivered", at(9, 0), undefined, "pelican");
  history.acknowledge({ client_ref: "delivered", notification_id: 7, target: "sup", stamped: true });
  history.hold("held", "sup", "Waiting for the network", at(9, 1), 7, "pelican");
  history.submit("in-flight", "sup", "On the wire, no receipt yet", at(9, 2), undefined, "pelican");
  history.submit("unconfirmed", "sup", "Receipt never came", at(9, 3), undefined, "pelican");
  history.unconfirmInFlight(at(9, 4));
  history.submit("refused", "sup", "Refused by the hub", at(9, 6), undefined, "pelican");
  history.reject("refused", "Not sent: lost connection to Atlas · Linux. Your message is kept; send it again when it's back.");
  return history;
}

describe("unsettled messages across a reload (cas-e7b1)", () => {
  it("keeps held, unreceipted, unconfirmed and not-sent messages, never delivered, replaced or dismissed ones", () => {
    const history = new ConversationHistory();
    history.submit("delivered", "sup", "Delivered", at(9, 0));
    history.acknowledge({ client_ref: "delivered", notification_id: 7, target: "sup", stamped: true });
    history.hold("held", "sup", "Held", at(9, 1), 7, "pelican");
    history.submit("sending", "sup", "Sending", at(9, 2));
    history.submit("refused", "sup", "Refused", at(9, 3));
    history.reject("refused", "Not sent: no");
    history.submit("replaced", "sup", "Replaced", at(9, 4));
    history.reject("replaced", "Not sent: no");
    history.retireRefused("replaced");
    history.submit("dismissed", "sup", "Dismissed", at(9, 5));
    history.reject("dismissed", "Not sent: no");
    history.dismissSend("dismissed");
    expect(history.pendingSends()).toEqual([
      { id: "held", target: "sup", text: "Held", state: "held", at: at(9, 1), replyTo: 7, session: "pelican" },
      { id: "sending", target: "sup", text: "Sending", state: "sending", at: at(9, 2), sentAt: at(9, 2) },
      { id: "refused", target: "sup", text: "Refused", state: "error", at: at(9, 3), sentAt: at(9, 3), error: "Not sent: no" },
    ]);
  });

  it("restores each message honestly: a held one still waits and a sent one without a receipt is Not confirmed, never Sending… or Delivered", () => {
    const stored = unsettledThread().pendingSends();
    expect(stored.map((send) => [send.id, send.state])).toEqual([["held", "held"], ["in-flight", "unconfirmed"], ["unconfirmed", "unconfirmed"], ["refused", "error"]]);
    // A reload while one was still on the wire.
    stored[1] = { ...stored[1]!, state: "sending" };

    const reloaded = new ConversationHistory();
    const held = reloaded.restorePending(stored, at(9, 30));
    expect(held.map((send) => send.id), "only the held message is queued to go out").toEqual(["held"]);
    expect(reloaded.restorePending(stored, at(9, 31)), "restoring twice adds nothing").toEqual([]);
    expect(reloaded.events).toHaveLength(4);

    const view = new ConversationView(document, reloaded, { supervisor: "sup", retryMessage: vi.fn() });
    document.body.replaceChildren(view.element);
    view.update();
    const line = (text: string) => [...view.element.querySelectorAll<HTMLElement>(".bub")].find((bubble) => bubble.textContent?.includes(text))?.querySelector(".conversation-delivery")?.textContent;
    expect(line("Waiting for the network")).toBe("Waiting for the connection — sends when it's back");
    // Two in a row read as one notice on the second (cas-b00c); neither claims it arrived.
    expect(line("On the wire, no receipt yet")).toBeUndefined();
    expect(line("Receipt never came")).toContain("2 messages not confirmed");
    expect(line("Refused by the hub")).toContain("Not sent");
    expect(view.element.textContent).not.toMatch(/Sending…|Delivered/);
    // The held message still counts as pending (its session stays listed), and nothing claims the supervisor has it.
    expect(reloaded.hasPending()).toBe(true);
    expect(reloaded.awaitingReply()).toBe(false);
  });

  it("a held message goes out once: released, it is on the wire; a later reload brings it back Not confirmed, not held", () => {
    const history = new ConversationHistory();
    history.restorePending([{ id: "held", target: "sup", text: "Go when you can", state: "held", at: at(9, 0), heldAt: at(9, 0) }], at(9, 1));
    expect(history.release("held", at(9, 2))).toBe(true);
    const afterSend = history.pendingSends();
    expect(afterSend).toEqual([{ id: "held", target: "sup", text: "Go when you can", state: "sending", at: at(9, 0), sentAt: at(9, 2) }]);
    const reloaded = new ConversationHistory();
    expect(reloaded.restorePending(afterSend, at(9, 3)), "it is not queued to be sent a second time").toEqual([]);
    expect(reloaded.events[0]?.kind === "send" && reloaded.events[0].value.state).toBe("unconfirmed");
    // Its receipt settles it: nothing is left to keep.
    history.acknowledge({ client_ref: "held", notification_id: 9, target: "sup", stamped: true });
    expect(history.pendingSends()).toEqual([]);
  });

  it("a restored Not confirmed message the machine's history shows did arrive becomes that row, not a second copy", () => {
    const history = new ConversationHistory();
    history.restorePending([
      { id: "a", target: "sup", text: "Did this land?", state: "sending", at: at(9, 0), sentAt: at(9, 0) },
      { id: "b", target: "sup", text: "yes", state: "unconfirmed", at: at(9, 5), sentAt: at(9, 5) },
    ], at(9, 6));
    history.hydrateSend({ notification_id: 21, target: "sup", text: "Did this land?", state: "acknowledged", stamped: true, device_id: "d", at: new Date(at(9, 0) + 2_000).toISOString() });
    // An identical earlier "yes", from well before this one went out, is a different message.
    history.hydrateSend({ notification_id: 3, target: "sup", text: "yes", state: "acknowledged", stamped: true, device_id: "d", at: new Date(at(9, 5) - RESTORED_MATCH_WINDOW_MS - 1).toISOString() });
    const sends = history.events.flatMap((event) => event.kind === "send" ? [event.value] : []);
    expect(sends.filter((send) => send.text === "Did this land?")).toEqual([expect.objectContaining({ id: "a", notificationId: 21, state: "acknowledged" })]);
    expect(sends.filter((send) => send.text === "yes").map((send) => [send.id, send.state])).toEqual([["history:3", "acknowledged"], ["b", "unconfirmed"]]);
    expect(history.pendingSends().map((send) => send.id)).toEqual(["b"]);
  });
});
