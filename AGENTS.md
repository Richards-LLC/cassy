<!-- CAS:BEGIN - This section is managed by CAS. Do not edit manually. -->
## Cassy: tasks, memory and context

Track work and knowledge in Cassy rather than in harness-local todo lists; Cassy tasks and memories persist across sessions.
Cassy's MCP tools are `task`, `memory` and `search`, named with your harness's prefix: `mcp__cas__` in Claude Code, `mcp__cs__` in Codex, `cas__` in Grok, `cas_` in OpenCode. Call them directly.

- `task`: action=create, start, close, ready.
- `memory`: action=remember.
- `search`: action=search.

Bug routing: `cas config get issues.repo` names this project's tracker, and `issues.components.{cassy,violet,cloud}` name the Cassy, Violet and Cloud trackers. File an operational bug in the matching tracker before moving on; in the Cassy source repo itself, a Cassy bug becomes a task there. If `issues.repo` is unset, record the bug as a task note.
Release notes: if docs/release-notes/RUBRIC.md exists, follow it for every merge to `staging` or `main`, using the `cas-release-notes` skill.
<!-- CAS:END -->

# Cassy source repository (cas-src)

This file is canonical for every harness; `CLAUDE.md` imports it.

## Build and test

Only the supervisor builds Rust: once per epic, at assembly. Factory workers edit, commit and park code without running `cargo`, `rustc`, `scripts/run-scoped-tests.sh` or `make test*` (a PreToolUse hook enforces this in Claude Code and Codex). Build commands, the assembly proof, worker build caches, the CI-load policy and build profiles are in [cas-cli/docs/CONTRIBUTING.md](cas-cli/docs/CONTRIBUTING.md#build-assembly-and-ci-policy). Build profiles must keep `panic = "unwind"`; a compile-time guard in `cas-cli/src/lib.rs` enforces it.

Minimum supported Rust version: **1.88** (edition 2024).

## Architecture and contributing

- Module layout, crate purposes, store traits, CasCore, hook scoring: [cas-cli/docs/ARCHITECTURE.md](cas-cli/docs/ARCHITECTURE.md).
- Adding CLI commands, MCP tools, migrations, testing setup, skill/rule sync, releasing: [cas-cli/docs/CONTRIBUTING.md](cas-cli/docs/CONTRIBUTING.md).
- Codebase navigation map: [.claude/CODEMAP.md](.claude/CODEMAP.md).

## Hooks and verification

CAS installs its hooks for Claude Code (`.claude/settings.json`) and Codex (`.codex/hooks.json`), including the PreToolUse guard that denies Rust builds to factory workers. Follow the factory worker lifecycle and let the supervisor own verification and review.

## Don't assume — always verify

When diagnosing a bug or reasoning about behavior, verify the claim against the actual code or data before acting on it. Trace the real path, read the real handler, and confirm the symptom maps to the line you think it does. Do not propose, implement, or ship a fix on a plausible but unconfirmed theory. A diagnosis is done only when you can point at concrete evidence: the file:line, the test output, or the reproduced behavior. Environment details the user gives (OS, terminal, hardware) are clues to verify, not facts to wave away. This applies to root-cause analysis, "this already works", "that's the harness, not us", and every other confident assertion.

## CAS system bugs are in-repo fixes

This repository is the CAS source. When a bug is reported in the verifier, hooks, factory orchestration, MCP dispatch, the task-verifier agent, worker prompts, or built-in skills, whichever downstream project surfaced it, the fix lands here as a Rust or Markdown change through a task assigned to a worker. Do not file it with a team lead, do not report it upstream, and do not treat CAS as an external dependency: other projects consume CAS, they do not modify it. If you want to escalate a CAS bug, create the fix task in this repository instead.
