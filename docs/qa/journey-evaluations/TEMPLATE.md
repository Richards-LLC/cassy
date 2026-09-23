# Journey evaluation — hub-web/dist <tree8>

<!-- Copy to <date>-hub-web-<tree8>.md. The receipt lines are machine-read by
scripts/check-journey-evaluation.sh: keep their `key: value` shape. -->

## Receipt

- hub_web_dist: <40-hex output of `git rev-parse HEAD:hub-web/dist`>
- evaluated_commit: <release-candidate commit SHA>
- suite_run: <scripts/journey-eval.sh artifact dir> — <N> journeys, <N> PASS
- evaluator: <agent name> (taste lane; not an implementer of the release epic)
- label: real-bundle, protocol-double
- blocking_findings: <count of Blocking rows below>

## Scores

Scores are 0–3 for each dimension: 0 none, 1 minor, 2 noticeable, 3 blocking.
The rubric and severity routing are in `docs/qa/journey-evaluation.md`. The
Run column is copied from the suite. PASS means the journey reached its goal.

| ID | Run | Dead end | Copy | Steps | Context | Waits | Severity | Findings / tasks |
|---|---|---|---|---|---|---|---|---|
| HUB-J1 | PASS | 0 | 0 | 0 | 0 | 0 | Note | — |

## Findings

One entry per finding, most severe first. Each entry names:

- the journey and stage
- the receipt: `J03.png`, `receipt.webm @mm:ss` or `trace action N`
- what a user would experience
- the suggested fix
- the task id, or `report only` for a Note

## Stage timings

Paste the slowest stages from `journeys/JOURNEYS.md`, with a note on any wait that has
no visible progress.

## Not covered

List what the protocol double cannot prove for this release, taken from each
journey's **Gaps:** in `docs/qa/journeys.md`. Say whether a real-build proof
covered each gap for this release.
