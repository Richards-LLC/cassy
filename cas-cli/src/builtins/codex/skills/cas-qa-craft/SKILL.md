---
name: cas-qa-craft
description: Use when a factory task has a non-empty demo_statement and its observable user flow needs evidence from the real build before close.
managed_by: cas
---

# Demo-statement QA

Turn a non-empty task `demo_statement` into a capped exploration matrix and
prove it against the named build. This is an evidence pass, not a fixture test
and not a substitute for unit or integration tests. Time-box the whole pass to
**30 minutes**; an honest incomplete ledger beats a late, invented result.

## Procedure

1. Read the active task with `task action=show`; name the binary version or
   commit SHA and write a one-line scope sentence before exercising anything.
   If `demo_statement` is empty, do not invent a matrix or invoke this skill.
2. Build the exploration matrix with [references/matrix-builder.md](references/matrix-builder.md):
   derive the first row from the demo, then add **at least three unmentioned
   conditions**, at least one adjacent surface, and no replay cells after row
   one. Include empty, failure/timeout, revisit, resize/phone, or keyboard
   conditions as risk warrants. Cap the matrix at **8 cells**. Write each
   expected result in the user's words before running its cell.
3. Write the ledger to `~/.cas/artifacts/<task-id>/LEDGER.md` using
   [references/evidence-ledger.md](references/evidence-ledger.md). Drive every
   cell against the real build: Playwright using project/`cas-playwright-debug`
   conventions for web or hub surfaces, or the real binary for CLI. Register
   long-lived servers through `cas-servers`; never substitute fixtures.
4. Capture one screenshot or terminal capture per cell, and label every row
   `source-inferred`, `fixture`, `real-build`, or `eyewitness`. A label weaker
   than the cell needs is `NOT EXERCISED`, never `PASS`; never write “partial”.
   When the 30-minute box expires, mark every unrun cell `NOT EXERCISED`.
5. Grep the touched feature for `MIN_`, `MAX_`, `_MINUTES`, `_MS`, `_SECS`,
   `THRESHOLD`, `GRACE`, `DEBOUNCE`, and `RETRY`; record whether each constant is
   predictable from the user's visible contract. For terminal states, dump all
   visible text across surfaces and flag contradictory claims.
6. Record one task per defect found; do not patch from this QA pass. Add the
   ledger path, build revision, label split, and verdict counts to a task note
   and the `task action=close` reason. Stop registered servers before close.

## Boundaries

Keep this skill focused on user-flow evidence. The worked matrix is in
[references/exemplar.md](references/exemplar.md); `cas-playwright-debug` owns
framework-specific diagnosis, `cas-servers` owns process lifecycle, and the
task verifier owns judgment. Add automation-caused artifacts to the ledger's
honesty section. Do not modify the verifier to make a missing or failing
capture pass.
