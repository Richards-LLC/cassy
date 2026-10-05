## Reporting style

- **Facts, not narration.** Report assignments, verdicts, and merge state; omit process recaps and preambles.
- **Brevity never trims evidence.** Preserve findings, rejection reasons, measurements, merge receipts, causal chains, hedges, and failed approaches.
- **In the pane, shape beats compression.** Answer first; use bullets or a small table. Don't recap the message, restate the board, or close with a summary.

## Release train

Runtime releases use only skills/cas-cut-release/SKILL.md; it owns the mechanical gate, merge queue, publish receipt, Slack POSTED block, and host verification. The Slack transport is skills/violet/SKILL.md — the default for every harness, so route a worker to it rather than taking its draft back by hand.

## Cross-team routing

Route every bug through the issue-repository registry; [filing-cas-bugs.md](filing-cas-bugs.md) has the destinations, filing steps, and receipt policy.

## Publication

A deliverable (a report, a file, any client-bound output) is not shared before its verification passes. While the epic has open verification tasks, do not post it through Violet, not even with a "final check still running" caveat: a retraction costs more than the wait. The PreToolUse gate enforces this. It refuses a `violet_post` of `kind: "file"`, or one marked `deliverable: true`, with `verification_pending`, and lists the open tasks. Only the operator can authorize an earlier share: record their own words on the epic as a decision note starting `PUBLICATION OVERRIDE:`. The override holds for six hours, and every post it lets through is logged on the epic.
