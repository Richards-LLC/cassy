import { describe, expect, it } from "vitest";
import { inboxThreads, projectInboxEvent } from "./projection";
import type { InboxEvent } from "./store";

let sequence = 0;
function event(plaintext: unknown, overrides: Partial<InboxEvent> = {}): InboxEvent {
  sequence += 1;
  return {
    accountId: "acct",
    feedGeneration: "1",
    sequence: String(sequence),
    eventId: `evt-${sequence}`,
    scope: "session",
    producerKind: "principal",
    hubId: "hub-1",
    projectId: "p1",
    sessionId: "s_x",
    keyEpoch: "1",
    digest: "sha256:x",
    storedAt: "2026-10-05T19:00:00Z",
    expiresAt: "2027-01-03T19:00:00Z",
    plaintext,
    verification: "verified",
    failure: null,
    acked: true,
    ...overrides,
  };
}

// The plaintext the hub drain seals (cas-cli operator_inbox/drain.rs
// `event_plaintext`) around an m263 frozen snapshot.
function hubTurn(snapshot: Record<string, unknown>) {
  return { type: "cas.operator.turn", v: 1, event_id: "e", session_name: "cas-src-quiet-hawk-74", snapshot };
}

describe("inbox projection", () => {
  it("maps a supervisor reply to the hub history's notification id", () => {
    const turn = projectInboxEvent(
      event(
        hubTurn({
          prompt_id: 812,
          source: "supervisor",
          target: "operator",
          prompt: "Merged.",
          summary: "merge",
          kind: "answer",
          created_at: "2026-10-05T18:59:00Z",
          factory_session: "cas-src-quiet-hawk-74",
          recipient_device_id: "phone",
          acknowledge_prompt_id: 811,
          attachments: [],
        }),
      ),
    );
    expect(turn).toMatchObject({
      kind: "reply",
      notificationId: 812,
      message: "Merged.",
      replyTo: 811,
      session: "cas-src-quiet-hawk-74",
      turnKind: "answer",
      at: "2026-10-05T18:59:00Z",
    });
  });

  it("maps an operator message and a queued offline command", () => {
    expect(
      projectInboxEvent(
        event(hubTurn({ prompt_id: 5, source: "commander:Alice@phone", target: "supervisor", prompt: "go", factory_session: "s", operator: { operator: "Alice", device_id: "phone", verified: true } })),
      ),
    ).toMatchObject({ kind: "message", notificationId: 5, text: "go", deviceId: "phone", operatorLabel: "Alice" });
    expect(
      projectInboxEvent(
        event({
          type: "cas.operator.turn",
          v: 1,
          event_id: "h",
          session_name: "s",
          snapshot: { source: "commander:dev-1", target: "supervisor", prompt: "later", factory_session: "s", command_id: "cmd-1", created_at: "t" },
        }),
      ),
    ).toMatchObject({ kind: "command", commandId: "cmd-1", text: "later", deviceId: "dev-1" });
  });

  it("never projects unverified rows, observer notices or unknown shapes", () => {
    expect(projectInboxEvent(event(hubTurn({ prompt_id: 1, target: "operator", factory_session: "s" }), { verification: "unverified", plaintext: null }))).toBeNull();
    expect(projectInboxEvent(event({ type: "psc.operator.machine_presence" }, { scope: "machine", producerKind: "cloud_observer" }))).toBeNull();
    expect(projectInboxEvent(event({ type: "something.else", v: 1 }))).toBeNull();
    expect(projectInboxEvent(event(hubTurn({ target: "operator", factory_session: "s" })))).toBeNull();
  });

  it("groups one machine's turns by session in feed order and drops duplicates", () => {
    const reply = hubTurn({ prompt_id: 9, target: "operator", prompt: "r", factory_session: "a" });
    const events = [
      event(hubTurn({ prompt_id: 7, target: "operator", prompt: "first", factory_session: "a" })),
      event(hubTurn({ prompt_id: 8, target: "operator", prompt: "other", factory_session: "b" })),
      event(reply),
      event(reply),
      event(hubTurn({ prompt_id: 10, target: "operator", prompt: "elsewhere", factory_session: "a" }), { hubId: "hub-2" }),
    ];
    const threads = inboxThreads(events, "hub-1");
    expect([...threads.keys()]).toEqual(["a", "b"]);
    expect(threads.get("a")!.map((turn) => (turn.kind === "reply" ? turn.notificationId : 0))).toEqual([7, 9]);
  });
});
