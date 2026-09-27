import { describe, expect, it } from "vitest";
import { REFUSED_SEE_ABOVE, refusal, refusalSentence } from "./refusal";
import { outageRefusal } from "./connection-state-view";

describe("refusal (F6: plain reasons with a next step)", () => {
  it.each([
    ["forbidden", "This device isn't the one in control of the session.", "Take control, then retry."],
    ["authorization refused", "This device isn't the one in control of the session.", "Take control, then retry."],
    ["Permission refused", "This device isn't the one in control of the session.", "Take control, then retry."],
    ["The session no longer grants this device control.", "This device isn't the one in control of the session.", "Take control, then retry."],
    ["semantic message enqueue failed: in_reply_to notification 7 does not exist", "The question it answered is no longer open.", "Edit it and send it as a new message."],
    ["semantic message enqueue failed: in_reply_to notification 7 belongs to factory session a, not b", "The question it answered is no longer open.", "Edit it and send it as a new message."],
    ["semantic message enqueue failed: database is locked", "The supervisor's machine couldn't take the message.", "Retry in a moment."],
    ["authentication required", "This device's pairing is no longer accepted.", "Re-pair this device, then retry."],
    ["Session daemon stream closed", "The connection to the machine dropped.", "Retry once the session is live again."],
    // cas-a0e2: the hub's retryable upstream code (legacy socket: the code;
    // machine channel: its message) reads as a dropped connection, never as
    // a control refusal.
    ["upstream_unavailable", "The connection to the machine dropped.", "Retry once the session is live again."],
    ["The session's daemon connection is reconnecting, so the message was not sent. Retry once the session is live again.", "The connection to the machine dropped.", "Retry once the session is live again."],
    ["machine protocol error", "The hub didn't accept it.", "Retry; if it keeps happening, re-pair this device."],
    // cas-a355: a held send that expired says Not sent with Retry, never
    // "re-pair this device", whatever the machine is called.
    [outageRefusal("Atlas · Linux"), "The session didn't come back while it waited.", "Retry once the session is live again."],
    [outageRefusal("Repair bench · control lease"), "The session didn't come back while it waited.", "Retry once the session is live again."],
  ])("maps %j to a plain reason", (detail, reason, next) => {
    expect(refusal(detail)).toMatchObject({ reason, next });
  });
  it("names the control only for a control refusal, and never the absent header control (cas-3433)", () => {
    for (const detail of ["forbidden", "authorization refused", "lease expired", "This device is only observing"]) {
      expect(refusal(detail).action).toBe("take-control");
      expect(refusal(detail).next).not.toMatch(/header/i);
    }
    for (const detail of ["in_reply_to gone", "authentication required", "stream closed", "enqueue failed", "machine protocol error", undefined]) {
      expect(refusal(detail).action).toBeUndefined();
    }
  });
  it("never repeats the protocol code to the operator", () => {
    expect(refusalSentence("forbidden")).toBe("Not sent. This device isn't the one in control of the session. Take control, then retry.");
    expect(refusalSentence(undefined)).not.toMatch(/undefined|forbidden/);
  });
  it("points at the bubble instead of repeating its reason (cas-4d92)", () => {
    expect(REFUSED_SEE_ABOVE).toBe("Not sent — see the message above.");
    expect(REFUSED_SEE_ABOVE).not.toContain(refusal("forbidden").reason);
  });
});
