# Cassy - Product Overview
>
> Auto-generated domain snapshot. Regenerate with /project-overview when the domain model drifts.

## Pitch

Cassy is a local-first multi-agent coding factory and persistent context system for developers working in Git repositories.
The `cas` binary runs a supervisor with isolated workers; `cas serve` gives coding agents durable project context through MCP.
Task coordination, memories, rules, skills, and repository knowledge live in one local system; Cloud sync is optional.

## Personas

- **Repository maintainer** - starts a factory session, chooses work, reviews delivery evidence, and merges accepted branches.
- **Supervisor agent** - plans epics, assigns one task at a time to workers, coordinates review and integration.
- **Worker agent** - executes an assigned task in its own worktree, records progress, and parks a committed result for review.

## Core Concepts

- **Factory session** - a live supervisor and worker fleet coordinated through the terminal UI.
- **Supervisor** - the session role that plans work, assigns tasks, reviews deliveries, and manages integration.
- **Worker** - an agent assigned a bounded task and isolated checkout.
- **Epic** - a larger outcome represented by related tasks and dependencies.
- **Task** - a prioritized work item with lifecycle status, notes, ownership, and close requirements.
- **Lease** - the time-bounded assignment that gives one worker responsibility for a task.
- **Worktree** - a separate Git checkout and branch used to isolate a worker's changes.
- **Close gate** - evidence and merge-state checks that determine whether a delivery can close.
- **Memory entry** - a durable fact, preference, context, or observation retrieved in later sessions.
- **Rule** - a versioned instruction that can become proven and sync into agent harnesses.
- **Skill** - a procedural playbook loaded by an agent for a particular kind of work.
- **Knowledge page** - a distilled, source-linked page about the current repository.
- **Code index** - searchable files and symbols that connect code to project context.

## Primary Journeys

1. A maintainer launches `cas`, reviews the active epic, and chooses which repository work to staff.
2. A supervisor creates tasks and dependencies, then assigns each ready task to a worker.
3. A worker starts its lease, commits changes in its worktree, and parks the branch with evidence for review.
4. An agent calls `cas serve` tools to find tasks, memories, rules, skills, and knowledge while working.
5. A maintainer builds or searches repository knowledge and carries the resulting context into later sessions.

## Authoritative Sources

- Product pitch and user-facing surfaces: `README.md`.
- Coordination and context-system boundaries: `cas-cli/docs/ARCHITECTURE.md`.
- Task lifecycle and close states: `crates/cas-types/src/task.rs`.
- Agent roles and session responsibilities: `crates/cas-types/src/agent.rs`.
- Memory, rule, and skill records: `crates/cas-types/src/entry.rs`, `crates/cas-types/src/rule.rs`, `crates/cas-types/src/skill.rs`.
- CLI and MCP surfaces: `cas-cli/src/cli/` and `cas-cli/src/mcp/tools/`.
