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
Prove dark routes stay unreachable and record dead routes honestly. Once a
registry exists, `qa_record` refuses a ledger missing a live journey's row; the
scaffold and `cas qa ledger check` own the row grammar.

## Escape vocabulary

Park does not check the journey registry, CODEMAP or PRODUCT_OVERVIEW; each
delivery still keeps the registry true for what it changed. Add a bug case as
`docs/qa/bug-families/<slug>.yaml` naming the task, own new surface files that
`cas qa impact` lists, and leave the shared docs for release prep.
<!-- cas-src:begin -->
The `journey-catalog` lane lint (`scripts/doc-fragments.py check`) refuses a
broken registry or bug family.
<!-- cas-src:end -->

| Gate | Refusal heading | Escape |
| --- | --- | --- |
| G-118 (`qa_record`) | `QA RECORD REJECTED` | a `NOT EXERCISED: <reason>` row |
| G-120 (post-release) | `RELEASE NOTES BLOCKED`, `POST-RELEASE CHECK` | operator only: `POST-RELEASE OVERRIDE: <their words>` on the epic |

## Run them and keep the evidence

Release batch QA runs the affected journeys through the project's established runner.
<!-- cas-src:begin -->
Use `scripts/journey-eval.sh <task-artifact-dir>`; its source selection and native ID filter remain authoritative.
<!-- cas-src:end -->
Consume matching receipts before running missing checks. Workers may iterate a
named failing spec while preserving the original failure.

Each journey leaves an evidence bundle in `<task-id>/journeys/<ID>/`, with the
`producer` field of its `bundle.json` set to `journey`.

<!-- cas-src:begin -->
For hub-web, the Playwright config never reuses a running server. Each
checkout gets its own default port pair in 20000–32767, and a port that is
already held fails the run. Set `HUB_E2E_PORT`/`HUB_JOURNEY_PORT` to a
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

`<task-id>/journeys/JOURNEYS.md` summarises the run; cite these paths in the
ledger. Document-only rebinds follow
[the affected-journeys rules](evidence-bundle.md#affected-journeys).

## Score the experience

Pass/fail is not enough. Score each journey from 0 to 3 on five dimensions: a
dead end, confusing copy, extra steps, lost context, and waits. Route the
scores by severity:

| Severity | When |
| --- | --- |
| Blocking | any dimension scores 3, or the journey cannot reach its goal |
| High | any 2 on a core journey |
| Normal | any other 2, or a 1 with a concrete fix |
| Note | everything else |

A journey that cannot reach its goal is a defect task, never a note.

