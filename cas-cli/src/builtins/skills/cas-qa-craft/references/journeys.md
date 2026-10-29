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

Upkeep gates refuse at park, before the merge request. Fix the named problem
in the same branch, or record the escape as
`task action=notes id=<task> note_type=decision notes="<token> <reason>"` and
retry close. The token starts the note and the reason must be non-empty.

| Gate | Refusal heading | Escape token |
| --- | --- | --- |
| G1 | `JOURNEY REGISTRY BROKEN BY THIS DELIVERY` | `registry unchanged:` |
| G2 | `UNOWNED USER-FACING FILE` | `no journey impact:` (or own the file: owner glob, owner group or `ignore`) |
| G3 | `BUG FIX WITHOUT A CASE` | `no case:` (or add a `bug_families` entry whose `tasks` names the task) |
| G5 | `CODEMAP DRIFT` | `codemap unchanged:` |
| G6 | `PRODUCT_OVERVIEW DRIFT` | `overview unchanged:` |
| G8 | `QA RECORD REJECTED` | a `NOT EXERCISED: <reason>` row |
| G9 | `RELEASE NOTES BLOCKED`, `POST-RELEASE CHECK` | operator only: `POST-RELEASE OVERRIDE: <their words>` on the epic |

Cassy records the outcome as notes that start `UPKEEP ESCAPE G1|G2|G3|G5|G6:
<token> <reason>`, or `UPKEEP OBSERVE <gate>:` for a would-be refusal when the
gate's stage is `observe` (`qa.registry.enforce`, `upkeep.codemap`,
`upkeep.product_overview`). Those notes are accounting; quoting a token there
waives nothing. The QA task lists the delivery's escapes: an independent
reviewer challenges a reason that does not hold.

## Run them and keep the evidence

Workers run the project's journey suite, for example
`scripts/journey-eval.sh <artifacts_root>/<project-key>/<task-id>`. It resolves
the task target, selects affected journeys itself and runs four workers.
`--affected <base>` sets an explicit base. The producer owns the native ID
filter, including supplemental specs and parts. No selected IDs writes an
explicit empty receipt without launching a browser. Independent QA first
consumes matching worker receipts under [independent-pass.md](independent-pass.md).
Factory workers and QA cannot use `--full` or an unfiltered Playwright run.

With a merge queue, the queue owns the full suite; without one, the full suite must be green on the exact SHA reaching the default branch, via CI check-runs on that SHA (preferred) or the supervisor's integrated run recorded on the epic; workers run targeted tests only. Read existing receipts; each check runs once.

Each journey leaves an evidence bundle in `<task-id>/journeys/<ID>/`, with the
`producer` field of its `bundle.json` set to `journey`.

For hub-web, the Playwright config never reuses a running server. Each
checkout gets its own default port pair in 20000–32767, and a port that is
already held fails the run loudly. Set `HUB_E2E_PORT`/`HUB_JOURNEY_PORT` to a
distinct pair in that range for runs in parallel from one checkout, and name
the ports in the evidence.

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
