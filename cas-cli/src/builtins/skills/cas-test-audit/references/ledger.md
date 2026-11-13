# Ledger format

One markdown file in the task artifact directory. Use the shape below.

## Header

State at the top: "Phase 1 changed no tests", the project and code revision, the CI run ids and how timing was read, the dead-code and tool commands run, and the method.

## Baseline table

| layer | measured cost per run | where it sits |
|---|---|---|

One row per layer and one for the whole run (runner minutes, wall minutes), with the critical path named.

## Summary by bucket

| bucket | findings (rows / tests) | strongest evidence | detector precision (held / flagged) | measured CI time removed per run | projected saving per run |
|---|---|---|---|---|---|

Measured and projected stay in separate columns. End with totals if every proposal is applied, labelling projections.

## Per-bucket row tables

| id | item (file:line) | verdict | tests exercising it | evidence | proposed fix | covering test | confidence |
|---|---|---|---|---|---|---|---|

- `evidence` quotes both sides for a duplicate, cites the retiring commit for a superseded test, and names the tool for a dead one.
- `covering test` names the test that still protects the behaviour, or `none needed: dead` / `none needed: superseded`.
- Low-confidence and `noisy, not proposed` rows are listed but carry no deletion.

## Flake history and detector precision

Keep every historical observation, including tests now repaired or deleted. List native test identity, SHA/tree, attempts, fail/pass execution, run/issue URL or Cassy note, current disposition and uncertainty. Separate a verified named same-source flip from an aggregate workflow flip, ejection, issue report or skip-list signal. State the thirty-day window, collection caps and unavailable sources.

Record held/sample for up to ten rows per detector. Keep noisy and low-confidence inventory separate from approved fix/delete proposals. Publish both machine-readable findings and the hand review. Counts are finding rows, not unique flaky tests or measured failure rates. Report `unmeasured` when CI cost was not collected; never infer a saving from sleep literals.

## Side findings

Anything real found along the way (tests that cannot fail, a possible network dependency, unimported files) that is not a cut.

## Decisions the operator must make

Numbered, each with a recommendation. At least: approve the deletion batch, confirm implementation pins are removed while accessibility contracts stay, and pick which speed-ups to take first. Phase 2 starts only after approval.
