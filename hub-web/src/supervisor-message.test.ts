import { describe, expect, it } from "vitest";
import {
  composerFocusWinner,
  planSupervisorSend,
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
  commanderOrigin: "https://hub.example",
  ...overrides,
});

describe("Cassy Commander supervisor composer targeting", () => {
  it("targets the selected session supervisor exactly", () => {
    expect(supervisorTarget(session("patient-lynx-59"))).toBe("patient-lynx-59");
    expect(supervisorMessage("patient-lynx-59", "Please review the mobile state"))
      .toMatchObject({ SendMessage: { target: "patient-lynx-59", text: "Please review the mobile state" } });
  });

  it("does not invent a fallback target for a session without a supervisor", () => {
    expect(supervisorTarget(session("  "))).toBeUndefined();
  });
});

describe("Cassy Commander composer send key", () => {
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

describe("Cassy Commander supervisor send plan", () => {
  it("sends directly while this device holds the session lease", () => {
    expect(planSupervisorSend(context())).toEqual({ kind: "send" });
  });

  it("takes control first when nobody controls the session, and says so", () => {
    const plan = planSupervisorSend(context({ leaseHeldByMe: false }));
    expect(plan.kind).toBe("take-control-then-send");
    if (plan.kind !== "take-control-then-send") return;
    expect(plan.notice).toContain("control");
  });

  it("refuses an empty message instead of sending whitespace to the supervisor", () => {
    const plan = planSupervisorSend(context({ text: "   " }));
    expect(plan).toMatchObject({ kind: "blocked", block: "empty" });
  });

  it("names the missing message:send scope and the exact re-pairing command", () => {
    // The default `cas hub pair` invitation grants read-only scopes, so a device
    // paired by the documented recipe cannot send at all. Silence there reads as
    // a broken Send button.
    const plan = planSupervisorSend(context({
      scopes: ["machine-read", "session-read", "pane-read"],
      leaseHeldByMe: false,
    }));
    expect(plan).toMatchObject({ kind: "blocked", block: "missing-scope" });
    if (plan.kind !== "blocked") return;
    expect(plan.reason).toContain("message:send");
    expect(plan.reason).toContain("cas hub pair --origin https://hub.example");
    expect(plan.reason).toContain("soundwave-linux");
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

  it("names the operator holding control instead of failing silently", () => {
    const plan = planSupervisorSend(context({ leaseHeldByMe: false, leaseControllerLabel: "Daniel's phone" }));
    expect(plan).toMatchObject({ kind: "blocked", block: "controlled-elsewhere" });
    if (plan.kind !== "blocked") return;
    expect(plan.reason).toContain("Daniel's phone");
  });

  it("blocks a session with no supervisor and a hub without Cassy Commander control", () => {
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
      planSupervisorSend(context({ leaseHeldByMe: false, leaseControllerLabel: "Studio Mac" })),
    ]) {
      expect(plan.kind).toBe("blocked");
      if (plan.kind !== "blocked") continue;
      expect(plan.reason.length).toBeGreaterThan(20);
    }
  });
});

describe("Cassy Commander focus arbitration after a render", () => {
  it("keeps the composer focused when both the terminal and the composer were focused", () => {
    // Every render replaces app.innerHTML, so both restores race. A terminal
    // that wins swallows the rest of the sentence being typed.
    expect(composerFocusWinner({ composerWasFocused: true, terminalWasFocused: true })).toBe("composer");
  });

  it("restores the terminal only when the composer was not being typed into", () => {
    expect(composerFocusWinner({ composerWasFocused: false, terminalWasFocused: true })).toBe("terminal");
    expect(composerFocusWinner({ composerWasFocused: true, terminalWasFocused: false })).toBe("composer");
    expect(composerFocusWinner({ composerWasFocused: false, terminalWasFocused: false })).toBe("none");
  });
});
