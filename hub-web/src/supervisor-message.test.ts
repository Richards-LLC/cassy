import { describe, expect, it } from "vitest";
import {
  planSupervisorSend,
  sendBlockTone,
  sendsOnEnter,
  supervisorMessage,
  supervisorTarget,
  type SupervisorSendContext,
} from "./supervisor-message";
import type { HubSession, Scope } from "./types";

const session = (supervisor: string): HubSession => ({
  name: "factory-live",
  supervisor,
  workers: ["worker-1"],
  liveness: "live",
});

const CONTROL_SCOPES: Scope[] = ["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt"];

const context = (overrides: Partial<SupervisorSendContext> = {}): SupervisorSendContext => ({
  text: "status?",
  machineLabel: "soundwave-linux",
  session: "factory-live",
  supervisor: "patient-lynx-59",
  daemonAttach: true,
  scopes: CONTROL_SCOPES,
  leaseHeldByMe: true,
  leaseControllerLabel: undefined,
  ...overrides,
});

describe("Cassy Cloud supervisor composer targeting", () => {
  it("targets the selected session supervisor exactly", () => {
    expect(supervisorTarget(session("patient-lynx-59"))).toBe("patient-lynx-59");
    expect(supervisorMessage("patient-lynx-59", "Please review the mobile state"))
      .toMatchObject({ SendMessage: { target: "patient-lynx-59", text: "Please review the mobile state" } });
  });

  it("includes a client reference when the sender needs a correlated refusal", () => {
    expect(supervisorMessage("patient-lynx-59", "Please review the mobile state", "send-42"))
      .toMatchObject({ SendMessage: { client_ref: "send-42" } });
  });

  it("carries in_reply_to for a reply to a supervisor ask, and omits it otherwise (cas-43f9)", () => {
    const frame = supervisorMessage("patient-lynx-59", "Yes, go ahead", "send-43", 52);
    expect(frame).toMatchObject({ SendMessage: { client_ref: "send-43", target: "patient-lynx-59", text: "Yes, go ahead", in_reply_to: 52 } });
    expect(JSON.parse(JSON.stringify(frame)).SendMessage.in_reply_to).toBe(52);
    expect(supervisorMessage("patient-lynx-59", "Plain", "send-44").SendMessage).not.toHaveProperty("in_reply_to");
  });

  it("does not invent a fallback target for a session without a supervisor", () => {
    expect(supervisorTarget(session("  "))).toBeUndefined();
  });
});

describe("Cassy Cloud composer send key", () => {
  const key = (overrides: Partial<Parameters<typeof sendsOnEnter>[0]> = {}) => ({
    key: "Enter",
    shiftKey: false,
    altKey: false,
    ctrlKey: false,
    metaKey: false,
    isComposing: false,
    ...overrides,
  });

  it("sends on a bare Enter", () => {
    expect(sendsOnEnter(key())).toBe(true);
  });

  it("leaves Shift+Enter as a newline so a message can have two lines", () => {
    expect(sendsOnEnter(key({ shiftKey: true }))).toBe(false);
  });

  it("ignores every other key", () => {
    expect(sendsOnEnter(key({ key: "a" }))).toBe(false);
    expect(sendsOnEnter(key({ key: "Escape" }))).toBe(false);
  });

  it("never sends the Enter that commits an IME composition", () => {
    // On a phone keyboard the first Enter of a Japanese or Pinyin composition
    // commits the candidate; sending there truncates the sentence being typed.
    expect(sendsOnEnter(key({ isComposing: true }))).toBe(false);
    expect(sendsOnEnter(key({ key: "Process" }))).toBe(false);
  });

  it("leaves chorded Enter to the browser and the shortcut layer", () => {
    expect(sendsOnEnter(key({ metaKey: true }))).toBe(false);
    expect(sendsOnEnter(key({ ctrlKey: true }))).toBe(false);
    expect(sendsOnEnter(key({ altKey: true }))).toBe(false);
  });
});

describe("Cassy Cloud supervisor send plan", () => {
  it("sends directly while this device holds the session lease", () => {
    expect(planSupervisorSend(context())).toEqual({ kind: "send" });
  });

  it("sends with message-only scope regardless of the pane controller", () => {
    for (const [leaseHeldByMe, leaseControllerLabel] of [[false, undefined], [false, "Daniel's phone"], [true, undefined]] as const) {
      expect(planSupervisorSend(context({ scopes: ["message-send"], leaseHeldByMe, leaseControllerLabel })))
        .toEqual({ kind: "send" });
    }
  });

  it("refuses an empty message instead of sending whitespace to the supervisor", () => {
    const plan = planSupervisorSend(context({ text: "   " }));
    expect(plan).toMatchObject({ kind: "blocked", block: "empty" });
  });

  it("tells a read-only device in plain words to ask the machine's owner for a link that can send", () => {
    // The default `cas hub pair` invitation grants read-only scopes, so a device
    // paired by the documented recipe cannot send at all. Silence there reads as
    // a broken Send button; a scope name or a CLI command is no better for the
    // person holding the phone, who does not run the machine (cas-b7c0).
    const plan = planSupervisorSend(context({
      scopes: ["machine-read", "session-read", "pane-read"],
      leaseHeldByMe: false,
    }));
    expect(plan).toMatchObject({ kind: "blocked", block: "missing-scope" });
    if (plan.kind !== "blocked") return;
    expect(plan.reason).toContain("Ask whoever runs soundwave-linux for a link that can send");
    // cas-a56c (QA F02): a fact about this pairing keeps its neutral tone when Send is tapped.
    expect(sendBlockTone(plan.block)).toBe("info");
    expect(sendBlockTone("no-supervisor")).toBe("error");
    expect(plan.reason).not.toMatch(/cas hub|--scopes|message:send/);
  });

  it("reports a device that can never send before it reports an empty draft", () => {
    // The Send button is rendered from this verdict while the composer is still
    // empty. Answering "type a message first" there hides the only fact that
    // matters: this device is refused whatever it types.
    const plan = planSupervisorSend(context({ text: "", scopes: ["machine-read", "session-read", "pane-read"] }));
    expect(plan).toMatchObject({ kind: "blocked", block: "missing-scope" });
    expect(planSupervisorSend(context({ text: "", session: undefined })))
      .toMatchObject({ kind: "blocked", block: "no-session" });
  });


  it("blocks a session with no supervisor and a hub without Cassy Cloud control", () => {
    expect(planSupervisorSend(context({ supervisor: undefined })))
      .toMatchObject({ kind: "blocked", block: "no-supervisor" });
    expect(planSupervisorSend(context({ daemonAttach: false })))
      .toMatchObject({ kind: "blocked", block: "unsupported-hub" });
    expect(planSupervisorSend(context({ session: undefined })))
      .toMatchObject({ kind: "blocked", block: "no-session" });
  });

  it("always states a reason a person can act on", () => {
    for (const plan of [
      planSupervisorSend(context({ text: "" })),
      planSupervisorSend(context({ session: undefined })),
      planSupervisorSend(context({ supervisor: undefined })),
      planSupervisorSend(context({ daemonAttach: false })),
      planSupervisorSend(context({ scopes: ["machine-read"] })),
    ]) {
      expect(plan.kind).toBe("blocked");
      if (plan.kind !== "blocked") continue;
      expect(plan.reason.length).toBeGreaterThan(20);
    }
  });
});
