# User journeys

A journey is one end-to-end user flow, from a real entry point to the user's
goal, across feature boundaries. Walk journeys, not only the changed piece.

Contents: [registry](#the-registry), [impact](#pick-the-journeys-a-change-touches),
[journey rows](#journey-rows), [escape vocabulary](#escape-vocabulary),
[evidence](#run-them-and-keep-the-evidence), [scoring](#score-the-experience).

## The registry

The project's journeys live in `docs/qa/journeys.yaml` (`qa.registry.path`):
ids, status (`live`, `dark`, `dead`), entry, steps, owner globs, invariants
and traffic share. `cas qa journeys --check` validates it against tracked
files, `--status` reports curation debt, and `--seed` bootstraps a missing one.
A project without YAML may keep the legacy `docs/qa/journeys.md` catalog; edit
a journey's **Steps** together with its `test.step` titles, because
`scripts/journeys-for-diff.py --check` fails when the two drift.

Each delivery keeps the registry true for what it changed, and task park
refuses one that does not: a new surface file needs an owner, a bug fix needs
its case, a rename must not orphan a glob ([escapes](#escape-vocabulary)).

## Pick the journeys a change touches

Read the journey table in the QA task description: park computed it from the
delivered diff. Without one, run `cas qa impact --task <id>` (or
`cas qa impact <base>..<head>`; `--json` is the contract). Rows come live
first, then by traffic share, each with its invariants and an
`evidence_required` line naming the walk and the read-back. With no registry,
impact groups files by CODEMAP area: walk each area's user-facing entry. Copy
every impact warning (no registry, stale CODEMAP, missing overview) into the
ledger header. Add any journey that the `demo_statement` names. Each journey
row starts at its entry and ends at its goal.

## Journey rows

Run `cas qa ledger init --task <id>` to prefill predicted live journey IDs,
then compare them with the parked impact (`cas qa impact --task <id>`) for
this review round. Add missing journey IDs and walk each from entry to goal.
Prove dark routes stay unreachable and record dead routes honestly. Run
`cas qa ledger check <path> --task <id>` before recording the verdict.
G8 consumes the same typed rows and refuses a missing live journey once a
registry exists. The scaffold and check own the row grammar.

## Escape vocabulary

Park does not check the journey registry, CODEMAP or PRODUCT_OVERVIEW. Add a
bug case as `docs/qa/bug-families/<slug>.yaml` naming the task, own new surface
files that `cas qa impact` lists, and leave the shared docs for release prep.
<!-- cas-src:begin -->
The `journey-catalog` lane lint (`scripts/doc-fragments.py check`) refuses a
broken registry or bug family.
<!-- cas-src:end -->

| Gate | Refusal heading | Escape |
| --- | --- | --- |
| G8 | `QA RECORD REJECTED` | a `NOT EXERCISED: <reason>` row |
| G9 | `RELEASE NOTES BLOCKED`, `POST-RELEASE CHECK` | operator only: `POST-RELEASE OVERRIDE: <their words>` on the epic |

## Run them and keep the evidence

<!-- cas-src:begin -->
Workers run the project's journey suite, for example
`scripts/journey-eval.sh <artifacts_root>/<project-key>/<task-id>`. It resolves
the task target, selects affected journeys itself and runs four workers.
`--affected <base>` sets an explicit base. The producer owns the native ID
filter, including supplemental specs and parts. No selected IDs writes an
explicit empty receipt without launching a browser. Independent QA first
<!-- cas-src:else -->
Workers run the project's journey suite, selecting the journeys the diff
affects with the command its CLAUDE.md or AGENTS.md names. Independent QA first
<!-- cas-src:end -->
consumes matching worker receipts under [independent-pass.md](independent-pass.md).
Factory workers and QA cannot use `--full` or an unfiltered Playwright run.

Workers run only the tests covering the lines they touched, before park. The full suite runs once on the landing tree: in the merge queue, else CI, else one supervisor run per epic. QA walks impacted journeys once per release batch. Read receipts; never rerun a passing check.

Each journey leaves an evidence bundle in `<task-id>/journeys/<ID>/`, with the
`producer` field of its `bundle.json` set to `journey`.

<!-- cas-src:begin -->
For hub-web, the Playwright config never reuses a running server. Each
checkout gets its own default port pair in 20000–32767, and a port that is
already held fails the run loudly. Set `HUB_E2E_PORT`/`HUB_JOURNEY_PORT` to a
distinct pair in that range for runs in parallel from one checkout, and name
the ports in the evidence.
<!-- cas-src:end -->

The files are:

| File | What it holds |
|---|---|
| `trace.zip` | The trace, with DOM, aria and screen snapshots |
| `trace-actions.txt` | The trace's action list |
| `receipt.webm` | For `visual_change: true`, a screencast with one chapter per stage |
| `J01.png`, `J02.png`, … | The screen at the end of each stage |
| `final.aria.yml`, `final.aria.json` | Aria snapshots of the final screen |
| `result.json` | The stage titles and timings |

`<task-id>/journeys/JOURNEYS.md` summarises the run. Cite these paths in the
ledger. The run also writes `journey-receipt.json` (base/head SHAs, selection
IDs, per-ID counts); close and QA no longer require it; full-suite ownership follows the rule above. Keep the original failed run when
checking a single failing spec at one worker. Document-only rebinds follow
[the affected-journeys rules](evidence-bundle.md#affected-journeys).

## Score the experience

Pass/fail is not enough. Score each journey from 0 to 3 on five dimensions:

- a dead end
- confusing copy
- extra steps
- lost context
- waits

Route each score by the severity table in the contract. A journey that
cannot reach its goal is a defect task, never a note.

## Before a release

When a release ships a changed bundle, the release evaluation walks every
journey. An independent taste-lane evaluator scores the receipts, and the
release train blocks without a passing report. That procedure lives in the
contract and in `cas-cut-release`.
