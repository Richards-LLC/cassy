# Commander connection resilience QA

The installed v3.82.0 build (`18a9e881687154a220228ee110d8b385cc98d05e`)
fails the connection-recovery truthfulness check. Messages shown as Delivered
revert to Sending after hub restart or reconnect. After daemon replacement,
the header is Live while the new send remains Sending and an attention badge
retains “Daemon connection lost” and “Session ended.”

The corrected ledger has **70 cells: 19 PASS, 15 FAIL, 36 NOT EXERCISED**.
Labels: 33 real-build, 32 fixture, 5 source-inferred. R02, R03 and R06 are FAIL.
The original harness reported eight successful narrow assertions because it
checked queue admission and reconnection, without checking receipt monotonicity
or obsolete attention items. That result does not establish a passing QA walk.

Six outgoing messages each reached the real queue once, but recorded rows stayed
gated with zero delivery attempts and no transport delivery timestamp. Candidate
causes are `cas-bf20` (an operator row does not wake an idle supervisor) and
`cas-3f40` (live acceptance and persisted history project different states).
The capture omitted `last_pending_detail`, so it cannot separate those causes.
The findings were sent to both workers; completed model delivery is unverified.

Current portable evidence, committed to private Cloud storage:

- `art-7dea4bf7`: corrected `LEDGER.md`, with the required eight columns,
  matching totals, constants review, contradictions and explicit gaps.
- `art-61d4f477`: corrected evidence archive, including the failed short-name
  DELETE trace and the previously omitted no-pane-input refusal screenshot.

These supersede `art-9da53327` and `art-7a911161`. The original selected fixture
sweep remains 30 passed/1 failed, with its passing one-worker control preserved.
The real recovery walk passed restart replay, a sixty-second paused outage and
offline replay, then failed the permission-help locator. Existing findings are
`cas-ccf14`, `cas-1ce3`, `cas-c756`, and the known Unix socket fix `cas-4927`.

The supervisor transferred Android radio switching, long-idle aging and discard
to `cas-5142` for its final epic-tip run with a consuming supervisor. This report
claims no fix-tip rerun or physical-machine/Cloud-relay restart coverage.

This correction reviews saved evidence; it changes no product code and runs no
new browser. All owned services and disposable state were cleaned up before the
original delivery. Design reference: `hub-web/DESIGN.md@ce0d8adc9`.
