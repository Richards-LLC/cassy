# User journeys

A journey is one end-to-end user flow, from a real entry point to the user's
goal, across feature boundaries. Walk journeys, not only the changed piece.
Projects that keep a catalog describe the contract in
`docs/qa/journey-evaluation.md`. Cassy's own is in cas-src.

## Keep the catalog current

- Every epic that changes a user-facing surface adds or updates the journeys
  it touches in `docs/qa/journeys.md`, in the same epic. Each journey records
  its steps, its expected experience and its edge paths, plus the spec that
  walks it. If the project has no catalog yet, create one with its critical
  journeys first.
- Edit a journey's **Steps** together with its `test.step` titles.
  `scripts/journeys-for-diff.py --check` fails when the two drift.

## Pick the journeys a change touches

- Where the project ships the helper, run
  `scripts/journeys-for-diff.py <base> [<head>]`. It prints JSON rows with
  `id`, `title`, `suite` and `reason`. Otherwise, match the diff against each
  journey's **Touches** globs by hand.
- Add any journey that the `demo_statement` names.
- Put each journey in the matrix as a row that starts at its **Entry** and
  ends at its **Goal**. Use the ledger labels and verdicts unchanged.

## Run them and keep the receipts

Run the project's journey suite, for example
`scripts/journey-eval.sh <artifacts_root>/<task-id> --grep <ID>`. Each
journey leaves an evidence bundle in `<task-id>/journeys/<ID>/`, with the
`producer` field of its `bundle.json` set to `journey`. Its files are:

| File | What it holds |
|---|---|
| `trace.zip` | The trace, with DOM, aria and screen snapshots |
| `trace-actions.txt` | The trace's action list |
| `receipt.webm` | A screencast with one chapter per stage |
| `J01.png`, `J02.png`, … | The screen at the end of each stage |
| `final.aria.yml`, `final.aria.json` | Aria snapshots of the final screen |
| `result.json` | The stage titles and timings |

`<task-id>/journeys/JOURNEYS.md` summarises the run. Cite these paths in the
ledger.

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
