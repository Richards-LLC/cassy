# Cassy - Product Overview
>
> Auto-generated domain snapshot. Regenerate with /project-overview when the domain model drifts.

## Pitch

Cassy is a local-first multi-agent coding factory and persistent context system for developers working in Git repositories.
The `cas` binary runs a supervisor with isolated workers; `cas serve` gives coding agents durable project context through MCP.
Tasks, memories, rules, skills and repository knowledge live in one local store; Cloud sync, the machine hub and the Commander browser app are optional.
Every delivery is checked before it merges: tests, the journeys a change touches, documentation upkeep and, after release, a live post-release check.

## Personas

- **Operator** - the repository maintainer: starts factory sessions, picks the work, reviews delivery evidence, approves releases and can answer from Commander.
- **Supervisor agent** - plans epics, assigns one task at a time to workers, merges pushed deliveries once CI is green, and coordinates with supervisors on other projects or machines.
- **Worker agent** - executes one assigned task in its own worktree, runs the targeted tests, and pushes a committed result for review.
- **QA reviewer** - walks the journeys a delivery touches on the real build and records an approve or reject verdict in a QA ledger.

## Core Concepts

- **Factory session** - a live supervisor and its worker fleet, coordinated through the terminal UI.
- **Epic / Task** - an epic is a larger outcome built from tasks; a task is a prioritized work item with a lease, notes, an owner and close requirements.
- **Epic split** - two supervisors divide one epic's tasks; each side's claim is refused to the other until the split is accepted, cancelled or expires.
- **Worktree** - a separate Git checkout and branch that isolates a worker's changes; under disk pressure, build caches left by removed checkouts are reclaimed oldest first.
- **Delivery branch** - the pushed `factory/<name>` branch a task is proven and merged from; a supervisor `rebind_branch` approves a replacement branch after an audit of the old tip.
- **Close gate** - the evidence, test-receipt and merge checks that decide whether a delivery may close; each merge records its CI admission as green, advisory or override.
- **Merge on green** - `READY TO MERGE` fires for any owned, pushed delivery with green CI, even before park or a merge request; the merge requests verification at the merge commit, so approval closes the task in one hop and rejection reopens it to the same worker. An override never closes automatically.
- **EPIC READY** - a deduplicated action sent to the epic's live owner once every child task, including those of an accepted split, is closed.
- **Journey registry** - a project's `docs/qa/journeys.yaml` (drafted by `cas qa journeys --seed`): each user journey's owner files, specs, surfaces and live, dark or dead status. Cassy's Commander registry preserves import and CSS selector coverage, with enforcement declared in the tracked registry.
- **Journey impact** - which journeys a diff or delivery touches, the evidence each needs, and the files no journey owns.
- **Upkeep gates (G1-G9)** - park refuses until the journey registry, regression case, CODEMAP and this overview are updated or a reasoned decision is recorded; release notes wait for the post-release check.
- **Acknowledged delivery** - a message counts as delivered only when the recipient's session receives it; idle recipients get bounded wakes and the sender is told after five minutes.
- **Peer messaging** - supervisors reach each other by name, project, `project@machine` or `supervisor:<project>`; a message to an ended session goes to its live successor.
- **Worker isolation** - on Linux, workers cannot read operator credential stores such as Neon and Vercel, and see only the connectors an operator grants a dedicated worker account.
- **Memory, Rule, Skill** - durable facts, versioned instructions that sync into agent harnesses, and procedural playbooks agents load for a kind of work.
- **Knowledge page** - a distilled, source-linked page about the current repository; the code index connects symbols to that context.

## Primary Journeys

1. The operator launches `cas`, reviews the active epic and staffs workers for its ready tasks.
2. A worker starts its lease, commits in its worktree, runs targeted tests and pushes; the supervisor merges on green, and the verifier's verdict at the merge commit closes or reopens the task.
3. A QA reviewer runs `cas qa impact` on a delivery, walks the touched journeys and records the verdict in the QA ledger.
4. A supervisor messages a supervisor on another project or machine, sees RECEIVED or PENDING, or splits an epic with that peer.
5. After a release merges, `cas serve` runs the post-release check, `cas release report` builds the branded release report, and Violet posts the release notes to Slack; `cas violet channels` and `cas doctor` check the configured channels.
6. An operator prepares a client-branded SOW delivery report with before/after evidence for each requested change.
7. An agent calls `cas serve` tools to find tasks, memories, rules, skills and knowledge while it works.
8. The operator invokes the builtin `finish` skill (`/finish` in Claude Code) to clean up session work and save a handoff and learnings; updates distribute it across harnesses while preserving personal overrides.

## Authoritative Sources

- Product pitch and user-facing surfaces: `README.md`.
- Coordination and context-system boundaries: `cas-cli/docs/ARCHITECTURE.md`.
- Task lifecycle and close states: `crates/cas-types/src/task.rs`.
- Agent roles and session responsibilities: `crates/cas-types/src/agent.rs`.
- Memory, rule and skill records: `crates/cas-types/src/entry.rs`, `crates/cas-types/src/rule.rs`, `crates/cas-types/src/skill.rs`.
- Adaptive QA: `cas-cli/src/qa_registry.rs`, `cas-cli/src/qa_impact.rs`, `cas-cli/src/qa_post_release.rs`, `docs/qa/impact.md`, `docs/qa/docs-upkeep.md`.
- CLI and MCP surfaces: `cas-cli/src/cli/` and `cas-cli/src/mcp/tools/`.
- Commander browser app: `hub-web/`.
- Release and delivery reports: `cas-cli/src/builtins/skills/cas-release-report/`, `cas-cli/src/builtins/skills/cas-sow-delivery-report/`.
- Session wrap-up: `cas-cli/src/builtins/skills/finish/SKILL.md`.
- Release history: `CHANGELOG.md`.

<!-- cas:project-overview
# Product paths the G6 park gate and overview freshness watch. Keep when regenerating.
domain_paths = ["crates/cas-types/src/task.rs", "crates/cas-types/src/agent.rs"]
watch_dirs = ["cas-cli/src/cli", "cas-cli/src/mcp/tools", "cas-cli/src/builtins/skills", "crates/cas-types/src", "hub-web/src"]
-->
