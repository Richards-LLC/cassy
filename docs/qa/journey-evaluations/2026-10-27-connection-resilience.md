# Commander connection resilience QA

The installed v3.82.0 build (`18a9e881687154a220228ee110d8b385cc98d05e`)
passed eight narrow real phone scenarios, including queue admission after hub
restart, offline/online, throttled 3G, frozen-page resume, daemon replacement,
and ending one of seven real sessions. The isolated stack used an inert
supervisor. Six outgoing messages each reached the real queue once; model
receipt and response remain unverified.

The selected HUB-J8/J11/J12/J16 browser sweep passed 30 tests and failed one
existing desktop churn precondition. Its one-worker control passed; the original
failure remains recorded. The separate real recovery walk passed restart replay,
a real sixty-second paused outage, and offline replay, then failed a stale
case-sensitive permission-help locator.

Portable evidence, committed to private Cloud storage:

- `art-9da53327`: final `LEDGER.md`, with 36 numbered journey invariant rows,
  fault scenarios, evidence labels, gaps and follow-up owners.
- `art-7a911161`: scrubbed traces, real server logs, queue readbacks, screenshots,
  reports and the artifact-only reproduction harness (15,875,897 bytes).

Filed findings: `cas-ccf14` (intermittent churn precondition), `cas-1ce3`
(real End session rejects CLI names shorter than three characters), and
`cas-c756` (permission-help locator wording). The Unix socket path limit is
already fixed by `cas-4927` for 3.83 and was not duplicated.

Actual Android radio handover, one-hour sleep/background aging, renderer discard,
and an isolated Cloud relay restart were not exercised. The ledger does not
claim the operator demo passed end to end. No confirmed match to the reported
Android send incident was established.

No product code changed. All owned registered services were stopped and private
disposable state was removed. Design reference: `hub-web/DESIGN.md@ce0d8adc9`.
