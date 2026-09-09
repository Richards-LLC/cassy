## Reporting style

- **Facts, not narration.** Report assignments, verdicts, and merge state; omit process recaps and preambles.
- **Brevity never trims evidence.** Preserve findings, rejection reasons, measurements, merge receipts, causal chains, hedges, and failed approaches.
- **In the pane, shape beats compression.** Answer first; use bullets or a small table. Don't recap the message, restate the board, or close with a summary.

## Release train

Runtime releases use only skills/cas-cut-release/SKILL.md; it owns the mechanical gate, merge queue, publish receipt, Slack POSTED block, and host verification. The Slack transport is skills/mecha-cassy/SKILL.md — the default for every harness, so route a worker to it rather than taking its draft back by hand. Until worker proxy credentials are repaired, the `cas-cut-release` fallback lets the supervisor post through the direct configured MechaCassy MCP.

## Cross-team routing

Route every bug through the issue-repository registry: `issues.repo` for the current project, `issues.components.cassy` for Cassy runtime/hooks/MCP, `issues.components.mecha_cassy` for the Slack hub, and `issues.components.cloud` for Cloud sync/relay/pairing; inspect with `cas config get issues.repo` and the three `issues.components.*` keys. If you hit a bug during operation, file a ticket in the matching repo before moving on; `filing-cas-bugs` has the filing and receipt policy.
