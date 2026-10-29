# Async fixture completion audit

Three existing fixture cases now observe completion instead of waiting for a
fixed event-loop turn or 5 ms. No production code, assertion, retry policy or
timeout changed, and no test cases were added.

| Candidate | Finding and disposition | Evidence |
| --- | --- | --- |
| Stopped SSE stream | Its decoded reader stays locked until cancellation and the consumer's `finally` finish. The restart case now holds real reader cancellation, observes that lock, releases cancellation, then waits for the lock to clear before restarting. | With cancellation held, the old one-turn wait fails the drain assertion: expected unlocked, received locked. Existing case passes with actual drain observation. |
| End-session keyboard focus | The original mock's zero-delay timer ordinarily precedes its 5 ms assertion timer. That is safe only for that mock chain. The existing case now holds the hub response, retains the Ending focus assertion, releases the response, then observes focus on a conversation row before retaining the exact next/previous row assertions. | The held-response control fails the old timer wait: expected the next row's key, received undefined. The repaired case passes. |
| Accounts sheet | Its original host mocks return already-resolved promises, so one turn sufficed for those mocks. The shared fixture now waits until account loading and mutation progress end. The existing read-only case holds its listing, verifies Loading accounts, releases the catalog and retains every access/action assertion. | Whole accounts module passes with controlled listing completion. Copy mocks remain immediately resolved. |
| Screenshot settling | Cleared for its existing bounded held-frame contract: the intentional real-time fallback drains native evaluations without advancing a paused protocol clock. Running clocks still use two frames when delivered within that bound. It is not a universal application-async completion barrier. No helper or screenshot-state assertion changed. | Existing frozen, running and page-closure controls: 60/60 native Chromium passes at one worker, 20 independent repeats each, no retries. |
| Historical WebCrypto siblings | The previous fixes are already present: timer advancement starts the retry, then `vi.waitFor` observes replacement socket creation. The slow-signing case holds the real native digest before releasing it. No migration needed. | Current connection-lifecycle module passes, including the held-digest control. Historical 4/200 and 137/200 failures remain preserved in the sweep evidence. |
| Queue-completion fixtures | The previous fixes are already present: the writer starts held, is released explicitly, and the fixture observes `queue.peek(10).is_empty()` before using the dequeued completion. Writer-entry `ran()` is not its completion signal. No Rust changes. | `queue_control_tests.rs::dequeued_fixture` and the preserved cas-c4b3 same-head CI failure/pass evidence. Fresh Rust execution is not claimed. |

The controlled failures are delayed-completion controls, not newly measured
natural flake rates. They do not establish that the original immediate mocks
were failing in normal runs.

Executed on the source change: the complete connection-lifecycle,
session-thread-identity and accounts-panel modules passed 97/97 tests. The
same focus module failed 1/26 before the focus repair; the named stream case
failed before replacing its elapsed wait. Native screenshot controls passed
60/60 with their existing held/running/closed-page assertions. Final typecheck,
full Vitest and affected-input receipts are attached to the task delivery.

No browser product, journey source, catalog or runner input changed. Affected
journeys are selected by the producer; an empty selection is expected for
this test/documentation change. Native hub and Rust assembly are outside this
audit's execution claims.

design: DESIGN.md@b43940e43
