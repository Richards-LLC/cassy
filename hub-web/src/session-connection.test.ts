import { describe, expect, it } from "vitest";
import type { AttachSnapshot, ConnectionSnapshot } from "./connection-state";
import { firstAttachRetry, machineConnection, sessionConnection } from "./session-connection";

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
  it("keeps the fatal machine verdict instead of stale attach retries (cas-99d7)", () => {
    const stopped = machine("failed", { fatal: true, reason: "unsupported browser" });
    expect(machineConnection(stopped, [{ attach: attach("backoff"), wasLive: true }])).toBe(stopped);
    expect(machineConnection(stopped, [{ attach: attach("live"), wasLive: true }])).toBe(stopped);
  });
  it("shows the first attached session that is not live, else the machine", () => {
    const live = machine("live");
    expect(machineConnection(live, [])).toBe(live);
    expect(machineConnection(live, [{ attach: attach("live"), wasLive: true }])).toBe(live);
    // cas-d15c: a live session reconnecting on a connected machine is that
    // session's state; a failure retrying cannot fix still counts.
    expect(machineConnection(live, [{ attach: attach("live"), wasLive: true }, { attach: attach("backoff", { sessionOnly: true }), wasLive: true }])).toBe(live);
    // A socket that simply dropped, or a failure retrying cannot fix, still counts.
    expect(machineConnection(live, [{ attach: attach("live"), wasLive: true }, { attach: attach("failed"), wasLive: true }])?.phase).toBe("backoff");
    expect(machineConnection(live, [{ attach: attach("live"), wasLive: true }, { attach: attach("failed", { fatal: true, sessionOnly: true }), wasLive: true }])?.phase).toBe("failed");
    const down = machine("failed");
    expect(machineConnection(down, [{ attach: attach("live"), wasLive: true }])).toBe(down);
  });
  it("does not count a conversation opening for the first time against its machine (journey F3)", () => {
    const live = machine("live");
    for (const phase of ["resolving", "dialing", "auth", "attaching"] as const) {
      expect(machineConnection(live, [{ attach: attach(phase, { stage: phase === "resolving" ? "resolving" : phase }), wasLive: false }])).toBe(live);
    }
    // A first attach failing a second time and a pairing loss still count.
    expect(machineConnection(live, [{ attach: attach("failed", { attempt: 1 }), wasLive: false }])?.phase).toBe("failed");
    expect(machineConnection(live, [{ attach: attach("backoff", { attempt: 2 }), wasLive: false }])?.phase).toBe("backoff");
    expect(machineConnection(live, [{ attach: attach("auth", { authFailure: "revoked" }), wasLive: false }])?.authFailure).toBe("revoked");
    expect(machineConnection(live, [{ attach: attach("attaching"), wasLive: true }])?.phase).toBe("backoff");
    // An opening conversation beside a dropped one: the drop still shows.
    expect(machineConnection(live, [{ attach: attach("attaching"), wasLive: false }, { attach: attach("dialing"), wasLive: true }])?.phase).toBe("backoff");
    // A stream the hub closed on a connected machine does not (cas-d15c).
    expect(machineConnection(live, [{ attach: attach("attaching", { sessionOnly: true }), wasLive: true }])).toBe(live);
  });
});

describe("a never-live conversation's first retry (cas-28df)", () => {
  const live = machine("live");
  it("is the first failure and its scheduled retry, never a second failure, a drop, or a hopeless one", () => {
    expect(firstAttachRetry(attach("failed", { attempt: 0 }), false)).toBe(true);
    expect(firstAttachRetry(attach("backoff", { attempt: 1 }), false)).toBe(true);
    expect(firstAttachRetry(attach("failed", { attempt: 1 }), false)).toBe(false);
    expect(firstAttachRetry(attach("backoff", { attempt: 2 }), false)).toBe(false);
    expect(firstAttachRetry(attach("backoff", { attempt: 1 }), true)).toBe(false);
    expect(firstAttachRetry(attach("failed", { attempt: 0, fatal: true }), false)).toBe(false);
    expect(firstAttachRetry(attach("failed", { attempt: 0, authFailure: "revoked" }), false)).toBe(false);
    expect(firstAttachRetry(attach("attaching", { attempt: 1 }), false)).toBe(false);
    expect(firstAttachRetry(undefined, false)).toBe(false);
  });
  it("keeps the footer Connected: the machine's hub is up", () => {
    expect(machineConnection(live, [{ attach: attach("failed", { attempt: 0 }), wasLive: false }])).toBe(live);
    expect(machineConnection(live, [{ attach: attach("backoff", { attempt: 1 }), wasLive: false }])).toBe(live);
  });
  it("reads as connecting in the header and the row, not reconnecting", () => {
    const state = sessionConnection(live, attach("backoff", { attempt: 1, stage: "attaching", reason: "no session state within 3s" }), false);
    expect(state?.phase).toBe("attaching");
    expect(state?.reason).toBe("no session state within 3s");
    expect(sessionConnection(live, attach("backoff", { attempt: 2 }), false)?.phase).toBe("backoff");
  });
});


describe("responding attaches during event recovery (cas-49cc)", () => {
  it("keeps the conversation Live only with fresh responding transport evidence", () => {
    const retrying = machine("backoff", { nextRetryAt: 10_000, reason: "event stream closed" });
    const speaking = attach("live");
    expect(sessionConnection(retrying, speaking, true, true)).toBe(speaking);
    expect(sessionConnection(retrying, speaking, true, false)).toBe(retrying);
    expect(sessionConnection(retrying, attach("attaching"), true, true)).toBe(retrying);
    expect(sessionConnection(retrying, undefined, true, true)).toBe(retrying);
  });
  it("keeps auth, fatal, permission and degraded machine verdicts authoritative", () => {
    for (const extra of [{ fatal: true }, { authFailure: "revoked" as const }, { networkAccessHelp: "Allow local network access" }, { degraded: true }]) {
      const blocked = machine("failed", extra);
      expect(sessionConnection(blocked, attach("live"), true, true)).toBe(blocked);
      expect(machineConnection(blocked, [{ attach: attach("live"), wasLive: true, responding: true }])).toBe(blocked);
    }
    const stopped = machine("idle");
    expect(sessionConnection(stopped, attach("live"), true, true)).toBe(stopped);
  });
  it("keeps the footer connected while a peer retries beside a responding attach", () => {
    const retrying = machine("backoff");
    const speaking = attach("live");
    expect(machineConnection(retrying, [
      { attach: speaking, wasLive: true, responding: true },
      { attach: attach("backoff", { sessionOnly: true }), wasLive: true, responding: false },
    ])).toBe(speaking);
    expect(machineConnection(retrying, [{ attach: speaking, wasLive: true, responding: false }])).toBe(retrying);
  });
});
