# Communication SLO measurement (cas-7e24)

The store computes one session projection. Doctor and session-end persistence
consume it; the release gate executes the same SQL against a checked-in factory
session in the existing fast/integration stage. There is no new release stage.

| Metric | Source and denominator | Target |
| --- | --- | --- |
| False abandon | `xs.sql`: abandoned rows with an addressed recipient's strong surfacing receipt; `recipient_ended_unread` is a separate disposition | 0 |
| Operator relay receipts | Enrolled `operator_cloud_outbox` rows for the session's snapshots with a `stored`/`duplicate` receipt / all enrolled rows (cas-5c12); unenrolled local snapshots are excluded and counted separately | 100% |
| Duplicate body reads | `dups.py`: extra rendered bodies per notification id and recipient; additive observation counts at store surface/poll boundaries | 0 |
| Director nudges | Typed `director` / `worker_stall_nudge` queue rows divided by worker-hours | at most 1/hour |
| Gated without outcome | `gate.sql`: rows with wake declines and no surface, acknowledgement, transport or terminal disposition | 0 |

A transport claim, handoff, pointer, enqueue, deferred wake or mere later agent
activity does not prove a body was seen. Only `hook_surfaced`, `inbox_poll` and
transcript-backed `observed_wake` qualify. A deliberate hook replay after an
unsettled poll counts as another body; repeated observation of the same wake
is idempotent bookkeeping. Different recipients of a broadcast are independent.
These counts measure CAS store-observed surfaces, not arbitrary harness copies
or a person's cognition. Historical sessions predating the observation epoch
show UNKNOWN for duplicate bodies instead of a fabricated PASS.

Worker-hours sum each worker incarnation's registration-to-observation interval,
ending at retirement or the last heartbeat for shutdown/dead/stale workers. Retirement identities are retained in
`agent_shutdown` events, so deleting the live registry row does not remove its
exposure. Duplicate live/retired representations of the same incarnation count
once. Only the enrolled cloud lane owes a relay receipt: the local
`operator_delivery_outbox` is unenrolled by schema and nothing in production
writes its `relay_receipt`. A cloud row is written with its snapshot while a
binding is active and deleted only with that snapshot, so a snapshot without one
was never enrolled. A session with only unenrolled snapshots reports N/A with the
`cas hub operator bind` enrollment step; a pending, parked or expired enrolled
row still breaches. A receipt proves the cloud retained the event, not that a
person or model handled it. Session-end summaries saved before this split keep
their recorded values and are labelled with the legacy denominator.
No operator events is labelled 100% (0/0, no events); a nonzero nudge count
with zero measurable exposure breaches. Ordinary active gated messages are
unresolved at the time of measurement, and can resolve on the next surface.

The source review is
`docs/reports/2026-10-14-agent-communication-review/communication-review.md`.
Original `xs.sql`, `gate.sql` and `dups.py` are copied as delivery provenance;
the 1.7 GB production snapshot stays read-only outside the checkout/artifacts.

## Release row

`python3 scripts/check-communication-slos.py --json` runs without building Rust,
opening only an in-memory SQLite fixture. Production SQL and fixture inserts
are also exercised by the store tests against initialized production tables.
`python3 scripts/test-check-communication-slos.py` includes an independent breach
for every SLO, receipt semantics, coverage and worker-retirement controls.
The row fails closed on schema/query errors and on incomplete body coverage.
