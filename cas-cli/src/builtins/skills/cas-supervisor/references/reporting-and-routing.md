## Reporting style

- **Facts, not narration.** Report assignments, verdicts, and merge state; omit process recaps and preambles.
- **Brevity never trims evidence.** Preserve findings, rejection reasons, measurements, merge receipts, causal chains, hedges, and failed approaches.
- **In the pane, shape beats compression.** Answer first; use bullets or a small table. Don't recap the message, restate the board, or close with a summary.
- **Check a capability before you claim it.** Before telling the operator, a product owner or release notes that the product does something, confirm it in the system of record: the code on the deployed SHA, live configuration or data. A plan, task title or memory is not proof, and a wrong claim gets repeated to customers.

## Release train

Runtime releases use only skills/cas-cut-release/SKILL.md; it owns the mechanical gate, merge queue, publish receipt, Slack POSTED block, and host verification. The Slack transport is skills/violet/SKILL.md — the default for every harness, so route a worker to it rather than taking its draft back by hand.

Before/after screenshots come from each visual delivery's QA bundle, at `~/.cas/artifacts/<project-key>/<task-id>/qa/pairs/` ([layout](../../cas-qa-craft/references/evidence-bundle.md#beforeafter-pairs)). When you route release notes or a report, name the task ids whose `bundle.json` records `pairs_status: complete` or `partial`; the drafter shows only their paired shots.

## Post-release check

A downstream product release is not done at deploy, and its release notes wait for this check. Pages returning 200 never count: a broken flow still serves its pages. Runtime releases use `cas-cut-release` host verification instead.

- **Configured** (`[qa.post_release] command` in `.cas/config.toml`): run `cas qa post-release start --epic <id> --sha <released-sha>` as the first step after the merge lands, with `cas serve` running. A merge-request reminder starts it if you forget. `cas qa post-release status --epic <id>` reports `pending`, `clean` or `failed`. Release-notes posts and epic close refuse until the check is `clean`; merge, deploy and tag never wait.
- **Not configured:** for about 30 minutes after deploy, run the project's invariant queries against production every ~15 minutes, and read back at least one real customer flow end to end from the system of record. Record each run on the epic.

A failed check is an incident, not a note: file the bug, and the fix-forward release starts its own check on its SHA. Only the operator can waive a pending or failed check, with their own words in an epic decision note starting `POST-RELEASE OVERRIDE:` (six hours, each use logged).

## Upkeep refusals and escapes

Task park refuses a delivery that leaves the journey registry, CODEMAP or PRODUCT_OVERVIEW untrue for the paths it changed (G1–G3, G5, G6). A worker fixes the problem in its branch or records the gate's escape token as a decision note ([vocabulary](../../cas-qa-craft/references/journeys.md#escape-vocabulary)). At review, read each `UPKEEP ESCAPE` note against the diff and request changes when its reason does not hold; epic close prints the epic's escape count. `supervisor_override=true` with a reason waives any of these gates and is logged: use it for a gate defect, not to skip upkeep. Stages (`qa.registry.enforce`, `upkeep.codemap`, `upkeep.product_overview`) are operator config; `UPKEEP OBSERVE` notes show what `enforce` would refuse.

## Cross-team routing

Route every bug through the issue-repository registry; [filing-cas-bugs.md](filing-cas-bugs.md) has the destinations, filing steps, and receipt policy.

## Publication

A deliverable (a report, a file, any client-bound output) is not shared before its verification passes. While the epic has open verification tasks, do not post it through Violet, not even with a "final check still running" caveat: a retraction costs more than the wait. The PreToolUse gate enforces this. It refuses a `violet_post` of `kind: "file"`, or one marked `deliverable: true`, with `verification_pending`, and lists the open tasks. Only the operator can authorize an earlier share: record their own words on the epic as a decision note starting `PUBLICATION OVERRIDE:`. The override holds for six hours, and every post it lets through is logged on the epic.
