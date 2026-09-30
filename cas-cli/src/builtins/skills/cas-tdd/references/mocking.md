---
metadata:
  managed_by: cas
---

# Boundary mocks and per-operation interfaces

Adapted from mattpocock/skills `tdd/mocking.md`, MIT © 2026 Matt Pocock.
See [LICENSE](../LICENSE) for the permission notice. Examples marked schematic
illustrate dependency design; they do not introduce production APIs.

## Contents

- [Choose the boundary](#choose-the-boundary)
- [Keep owned behavior real](#keep-owned-behavior-real)
- [Inject time and randomness without replacing behavior](#inject-time-and-randomness-without-replacing-behavior)
- [Inject typed per-operation ports](#inject-typed-per-operation-ports)
- [Framework testing seams](#framework-testing-seams)

## Choose the boundary

Mock only a system boundary when a real fixture is unsuitable. Keep the consuming
behavior real and assert its public result. Choose the fixture by the uncertainty
it controls:

| Boundary | Preferred fixture | When a substitute earns its place |
| --- | --- | --- |
| Network or external service | Local protocol server or the service's test environment | Vendor downtime, rate limits, authentication refusal or transport failure must be deterministic. |
| Time | Inject a clock/current instant; use the framework's timer seam for scheduling | Waiting for deadlines is slow or flaky. Restore fake timers after the test. |
| Randomness | Inject the random source or a documented seed | Jitter/token generation needs reproducible inputs; assert the outcome, not calls to the generator. |
| Filesystem | Temporary directory with real files | Permission, disk-full or I/O faults cannot be produced reliably by a disposable fixture. |
| Database | Real isolated test database with the actual schema | An unavailable external DB or a specific driver failure cannot be exercised reliably; keep schema/query tests on the real engine. |
| Git process | Temporary repository with real commits and refs | A process-launch failure is the subject; keep ancestry, dirty status and merge semantics on real Git. |

A mock's canned result does not prove the adapter serializes requests, handles
HTTP status codes, executes SQL correctly or interprets Git history. Give that
adapter its own boundary contract test. Avoid live paid or credential-dependent
services in the normal deterministic loop.

## Keep owned behavior real

Do not mock the function under test, private helpers, your own modules/classes or
internal collaborators just to verify call order/count. Do not replace the store
with a fake if persistence/retrieval is what the test promises. For Cassy examples,
use `open_task_store` with temporary SQLite and `GitOperations` with a real
repository; see [tests.md](tests.md).

A separately owned dependency can sit inside the process and still be a boundary.
Name its ownership and contract before substituting it. Passing every helper
through DI does not turn every helper into an external system. When a request
count is itself a contract (a billed external request or retry budget), assert it
alongside the caller-visible success/failure rather than private choreography.

## Inject time and randomness without replacing behavior

Hub-web exposes randomness in `backoffDelay(attempt, random)` and time in
`elapsedSeconds(snapshot, now)` in `hub-web/src/connection-state.ts`. Pass explicit
values so the real calculation runs without monkey-patching `Math.random` or
waiting for wall time:

```typescript
import { expect, it } from "vitest";
import { backoffDelay, elapsedSeconds } from "./connection-state";

it("reports seven elapsed seconds from the connection timestamp", () => {
  expect(elapsedSeconds({
    phase: "dialing", stage: "dialing", since: 10_000, attempt: 0,
    missedHeartbeats: 0, degraded: false,
  }, 17_900)).toBe(7);
});

it("applies the lower jitter bound to a four-second retry", () => {
  expect(backoffDelay(2, () => 0)).toBe(3_200);
});
```

For code that really schedules timers, Vitest fake timers are a framework seam:
advance the clock, assert the emitted state, and call `vi.useRealTimers()` during
cleanup. Assert an explicit deadline rather than sleep and hope the callback ran.
For process env/cwd, use the repository's shared `TestEnvGuard`; this is fixture
isolation, not a fake of production behavior.

## Inject typed per-operation ports

Pass an external dependency into the consuming behavior rather than construct a
credential-bearing vendor client inside it. Prefer SDK-style operations with
specific input/result types to a generic `fetch(url, options)` fake that branches
on URL/method. Each operation's substitute should return one typed response.
This schematic conversation port illustrates the shape:

```typescript
type ConversationPort = {
  getMessages(session: string): Promise<Array<{ id: string; text: string }>>;
  sendMessage(session: string, text: string): Promise<{ id: string }>;
};

async function sendAndRead(session: string, text: string, hub: ConversationPort) {
  const sent = await hub.sendMessage(session, text);
  const messages = await hub.getMessages(session);
  return messages.find(message => message.id === sent.id);
}

const hub: ConversationPort = {
  sendMessage: async () => ({ id: "message-1" }),
  getMessages: async () => [{ id: "message-1", text: "hello" }],
};
expect(await sendAndRead("fixture-session", "hello", hub))
  .toEqual({ id: "message-1", text: "hello" });
```

The consuming function is real; the substitute controls a separately owned
service's responses. Add empty/error cases as separate learned slices. Keep the
HTTP adapter responsible for URL/method details and test those details there.
Use a protocol server instead when exercising reconnects, stream ordering or
request encoding: canned per-operation responses cannot establish those promises.

## Framework testing seams

A DI container's provider override or a test harness's module builder is suitable
when it replaces an external or separately owned dependency and runs the real
consuming module through its public interface. Label which boundary is replaced
and which public contract is asserted. For example, override the external hub
provider, invoke the module's send operation, and assert the returned message or
refusal. Replacing its internal mapper and asserting `mapper.map` was called
remains implementation-coupled, even if a framework makes that override easy.
