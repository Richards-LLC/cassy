# GH #238: protected-main merge evaluation

## Verdict

Rule-suite history refutes a sustained ruleset PUT-propagation lag. GitHub
accepted merge `7309077c` with `required_status_checks=pass` before that
merge's own required checks began; its first parent had the three required
successful checks. Parent-derived merge evaluation is strongly supported but
not safely confirmed; GH #238 remains open with a GitHub Support draft.

| Evidence | Source | Meaning |
| --- | --- | --- |
| `8e91de67 -> 7309077c` passed at 18:33:17Z | Rule suite `3641766868` | Active ruleset `20698019` passed required checks. |
| Novel push failed at 18:34:18Z: `3 of 3 required status checks are expected` | Rule suite `3641780823` | Same ruleset was enforcing one minute later. |
| First parent completed full suite, doctests, macOS successfully | Checks API for `8e91de67` | Supports parent-derived evaluation. |
| Merge's required checks began 18:33:22Z | Checks API for `7309077c` | They did not explain the 18:33:17Z evaluation. |

## Reasoning, limit, and next action

The adjacent rejected novel push rules out sustained propagation lag. A controlled
scratch reproduction cannot safely produce the three trusted Actions contexts
without changing live workflow/ruleset posture or forging checks. GitHub Support
must confirm whether direct-pushed merge commits inherit first-parent statuses.
Keep GH #238 open; the Support draft is in its issue comment.

## Provenance

Extracted 2026-08-11 via `gh api /repos/pippenz/cas/rulesets/rule-suites`,
per-suite detail, and commit `check-runs` endpoints. Issue finding:
https://github.com/pippenz/cas/issues/238#issuecomment-5257699547
