import { describe, expect, it } from "vitest";
import type { AttachSnapshot, ConnectionSnapshot } from "./connection-state";
import { machineConnection, sessionConnection } from "./session-connection";

const machine = (phase: ConnectionSnapshot["phase"], extra: Partial<ConnectionSnapshot> = {}): ConnectionSnapshot => ({ phase, stage: phase === "live" ? "live" : "dialing", since: 0, attempt: 0, missedHeartbeats: 0, degraded: false, ...extra });
const attach = (phase: AttachSnapshot["phase"], extra: Partial<AttachSnapshot> = {}): AttachSnapshot => ({ session: "s", phase, stage: phase === "live" ? "live" : "attaching", since: 0, attempt: 1, missedHeartbeats: 0, degraded: false, ...extra });

describe("sessionConnection (cas-a447: one state for header, row and footer)", () => {
  it("is the machine's state while the machine is not live", () => {
    const down = machine("backoff");
    expect(sessionConnection(down, attach("live"), true)).toBe(down);
    expect(sessionConnection(undefined, attach("failed"), true)).toBeUndefined();
  });
  it("is the machine's state while the session socket is live or idle", () => {
    const live = machine("live");
    expect(sessionConnection(live, attach("live"), true)).toBe(live);
    expect(sessionConnection(live, attach("idle"), true)).toBe(live);
    expect(sessionConnection(live, undefined, false)).toBe(live);
  });
  it("reads as reconnecting when a session that was live drops, whatever stage the retry is in", () => {
    for (const phase of ["failed", "dialing", "attaching", "auth"] as const) {
      expect(sessionConnection(machine("live"), attach(phase), true)?.phase).toBe("backoff");
    }
  });
  it("keeps a first connect as connecting and a hopeless failure as failed", () => {
    expect(sessionConnection(machine("live"), attach("attaching"), false)?.phase).toBe("attaching");
    expect(sessionConnection(machine("live"), attach("failed", { fatal: true }), true)?.phase).toBe("failed");
    expect(sessionConnection(machine("live"), attach("failed", { authFailure: "revoked" }), true)?.authFailure).toBe("revoked");
  });
});

describe("machineConnection (the footer)", () => {
  it("shows the first attached session that is not live, else the machine", () => {
    const live = machine("live");
    expect(machineConnection(live, [])).toBe(live);
    expect(machineConnection(live, [{ attach: attach("live"), wasLive: true }])).toBe(live);
    expect(machineConnection(live, [{ attach: attach("live"), wasLive: true }, { attach: attach("failed"), wasLive: true }])?.phase).toBe("backoff");
    const down = machine("failed");
    expect(machineConnection(down, [{ attach: attach("live"), wasLive: true }])).toBe(down);
  });
});
