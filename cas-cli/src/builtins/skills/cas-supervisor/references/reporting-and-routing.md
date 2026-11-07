## Reporting style

- **Facts, not narration.** Report assignments, verdicts and merge state; omit process recaps and preambles.
- **Brevity never trims evidence.** Keep findings, rejection reasons, measurements, merge receipts, causal chains, hedges and failed approaches.
- **In the pane, shape beats compression.** Answer first, then the visual form the content earns ([reply contract](operator-reply.md)). Don't recap the message, restate the board, or close with a summary.
- **Voice belongs to the operator's instructions.** The operator's user-level instructions (CLAUDE.md or AGENTS.md) govern voice and style; these rules govern content and length.
- **Check a capability before you claim it.** Before telling the operator, a product owner or release notes that the product does something, confirm it on the deployed SHA, live configuration or data. A plan, task title or memory is not proof, and a wrong claim reaches customers.

## Release train

The supervisor writes release notes; workers never draft them, and release prep only merges, writes the changelog and regenerates the codemap.

<!-- cas-src:begin -->
Runtime releases use only skills/cas-cut-release/SKILL.md; it owns the mechanical gate, merge queue, publish receipt, Slack POSTED block and host verification. Slack goes through skills/violet/SKILL.md on every harness; route a worker to it rather than taking its draft back by hand.
<!-- cas-src:else -->
Slack goes through skills/violet/SKILL.md on every harness; route a worker to it rather than taking its draft back by hand.
<!-- cas-src:end -->

To show the operator a visual delivery, put its screenshots inline in the reply ([showing a UI delivery](operator-reply.md#showing-a-ui-delivery)). Before/after screenshots come from each visual delivery's QA bundle under `[factory] artifacts_root/<project-key>/<task-id>/qa/pairs/` ([layout](../../cas-qa-craft/references/evidence-bundle.md#beforeafter-pairs)). When routing release notes or a report, name the task ids whose `bundle.json` records `pairs_status: complete` or `partial`.

## Post-release check

A downstream product release is not done at deploy, and its release notes wait for this check. Pages returning 200 never count: a broken flow still serves its pages. Runtime releases use `cas-cut-release` host verification instead.

- **Configured** (`[qa.post_release] command` in `.cas/config.toml`): run `cas qa post-release start --epic <id> --sha <released-sha>` as the first step after the merge lands, with `cas serve` running. `cas qa post-release status --epic <id>` reports `pending`, `clean` or `failed`; release-notes posts and epic close refuse until it is `clean`.
- **Not configured:** run the project's invariant queries against production at deploy and on two `remind_delay_secs=900` deadline reminders, and read back at least one real customer flow end to end from the system of record. Record each run on the epic.

A failed check is an incident: file the bug, and the fix-forward release runs its own check. Only the operator can waive a pending or failed check, in their own words in an epic decision note starting `POST-RELEASE OVERRIDE:`.

## Upkeep refusals and escapes

Task park refuses a delivery that leaves the journey registry, CODEMAP or PRODUCT_OVERVIEW untrue for the paths it changed. The worker fixes it or records the gate's escape token as a decision note ([vocabulary](../../cas-qa-craft/references/journeys.md#escape-vocabulary)). At review, read each `UPKEEP ESCAPE` note against the diff and request changes when its reason does not hold. `supervisor_override=true` with a reason waives these gates: use it for a gate defect, never to skip upkeep.

## Cross-team routing

Route every bug through the issue-repository registry in [filing-cas-bugs.md](filing-cas-bugs.md).

## Publication

A deliverable (a report, a file, any client-bound output) is not shared before its verification passes, not even with a "final check still running" caveat: a retraction costs more than the wait. The PreToolUse gate refuses such a `violet_post` while verification is pending. Only the operator can authorize an earlier share, in their own words on the epic as a decision note starting `PUBLICATION OVERRIDE:`.
