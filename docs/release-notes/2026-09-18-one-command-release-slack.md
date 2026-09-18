# One-command release — source-on-main Slack announcement draft

Channel: `#cas-internal` (`C0B44GUKDK2`)

Deploy target: Source on main. This is a source-merge announcement, not a runtime release.

Publication status: DRAFT. Do not post this draft.

Review basis: the actual merged epic tip at `aedd9a1a`. The four fenced blocks below are the complete postable text, in order: User top-level, User reply, Dev top-level, Dev reply.

## User top-level

```text
*Source on main — User — Cassy*
Was: asking Cassy for a release meant coordinating several steps. → Now: one command runs the train and names the blocker when it needs an answer.
```

## User reply

```text
• *One command* — Was: a release required hand-running each step. → Now: `scripts/release-train.sh <version> <release-worktree> --cut` owns the train.

• *Named blockers* — Was: a failed step left the next action unclear. → Now: the train names the blocker and prints the exact `--cut --resume` retry.

• *Receipts* — Was: manual detours were hard to count. → Now: the run records how many interventions happened and which stages blocked.

• *Self-healing assembly* — Was: changes to Main after integration could stop assembly. → Now: assembly repairs the stale base once before retrying.

• *Handoffs* — Was: it was hard to tell when a release moved between checks and publishing. → Now: the run keeps those hand-off timings with the release evidence.
```

## Dev top-level

```text
*Source on main — Dev — Cassy*
Was: release orchestration depended on separate commands and implicit state. → Now: one train records every stage and its evidence from preflight through host update.
```

## Dev reply

```text
• *Canonical cut train* — Was: release stages were invoked as separate commands. → Now: `--cut` dispatches `preflight, assemble, prep, ledger, gate, pr-body, pipeline, publish, post-publication, announce, report, receipts, host-update` in order.

• *Durable ledger* — Was: completed work and safe resume boundaries were implicit. → Now: each stage writes a SHA receipt, and `--resume` skips only validated ancestor-safe stages; the ledger is the last prep step.

• *Self-healing assembly* — Was: stale `origin/main` could refuse assembly, and recovery could inherit the supervisor identity. → Now: assemble invokes bounded base-only recovery once with identity scrubbed before sweep children run (GH #901).

• *Docs stages* — Was: prep, announcement, and receipt work lived outside the train, with auth setup left implicit. → Now: prep, announce, and receipts run in the train, while announce resolves proxy-configured auth.

• *Intervention latency* — Was: manual blockers and hand-off delays were missing from release evidence. → Now: the latency receipt records `INTERVENTIONS`, `BLOCKERS`, `GREEN_TO_PIPELINE_SECS`, `MERGED_TO_PUBLISHER_SECS`, and the hand-off epochs.

• *End-to-end proof* — Was: cut seams and recovery behavior could be checked separately. → Now: the combined fixture covers cut/resume, stale-base healing, docs receipts, and intervention counts in 148 passing tests.

• *Operator procedure* — Was: release guidance left supervisors coordinating separate commands. → Now: `cas-cut-release` centers one `--cut`, named `--resume`, `--status`, and the receipt checklist.
```
