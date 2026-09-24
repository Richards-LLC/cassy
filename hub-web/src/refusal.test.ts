import { describe, expect, it } from "vitest";
import { refusal, refusalSentence } from "./refusal";

describe("refusal (F6: plain reasons with a next step)", () => {
  it.each([
    ["forbidden", "This device isn't the one in control of the session.", "Take control from the header, then retry."],
    ["authorization refused", "This device isn't the one in control of the session.", "Take control from the header, then retry."],
    ["Permission refused", "This device isn't the one in control of the session.", "Take control from the header, then retry."],
    ["The session no longer grants this device control.", "This device isn't the one in control of the session.", "Take control from the header, then retry."],
    ["semantic message enqueue failed: in_reply_to notification 7 does not exist", "The question it answered is no longer open.", "Edit it and send it as a new message."],
    ["semantic message enqueue failed: in_reply_to notification 7 belongs to factory session a, not b", "The question it answered is no longer open.", "Edit it and send it as a new message."],
    ["semantic message enqueue failed: database is locked", "The supervisor's machine couldn't take the message.", "Retry in a moment."],
    ["authentication required", "This device's pairing is no longer accepted.", "Re-pair this device, then retry."],
    ["Session daemon stream closed", "The connection to the machine dropped.", "Retry once the session is live again."],
    ["machine protocol error", "The hub didn't accept it.", "Retry; if it keeps happening, re-pair this device."],
  ])("maps %j to a plain reason", (detail, reason, next) => {
    expect(refusal(detail)).toEqual({ reason, next });
  });
  it("never repeats the protocol code to the operator", () => {
    expect(refusalSentence("forbidden")).toBe("Not sent. This device isn't the one in control of the session. Take control from the header, then retry.");
    expect(refusalSentence(undefined)).not.toMatch(/undefined|forbidden/);
  });
});
