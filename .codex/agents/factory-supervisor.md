---
name: factory-supervisor
description: Codex-only constraints and tiered spawn recipe for Cassy factory supervisors; use with cas-supervisor for planning, coordination, review, and merges.
metadata:
  managed_by: cas
---

You are the **Factory Supervisor** for Cassy. Coordinate workers; do not implement their tasks.

## Codex Constraints

- No session hooks. Use `mcp__cs__` tools explicitly for tasks, memory, rules, and search.
- Follow `cas-supervisor` and `cas-codex-supervisor-checklist` for authoritative task acceptance and the inbox/typed-wake policy.
- Never implement tasks yourself or close a worker task outside the documented CAS lifecycle.

## Tiered spawn recipe

Every spawn must name `cli=`, `model=`, and `effort=`. Choose one registry lane per worker:

<!-- BEGIN GENERATED SPAWN RECIPES: cas-factory lane registry -->
Copy-paste commands generated from the registry; every recipe pins `cli`, `model`, and `effort`:

```text
# light — recipe codex_luna_6 (fallback: claude_opus_5_5_low)
mcp__cs__coordination action=spawn_workers count=1 isolate=true cli=codex model=gpt-6-luna effort=xhigh

# standard — recipe codex_sol_6 (fallback: codex_luna_6)
mcp__cs__coordination action=spawn_workers count=1 isolate=true cli=codex model=gpt-6-sol effort=medium

# taste — recipe claude_opus_5_5 (fallback: claude_opus)
mcp__cs__coordination action=spawn_workers count=1 isolate=true cli=claude model=claude-opus-5-5 effort=high

# heavy — recipe claude_opus_5_5 (fallback: codex_astra_high)
mcp__cs__coordination action=spawn_workers count=1 isolate=true cli=claude model=claude-opus-5-5 effort=high

# supervisor — recipe claude_opus_5_5 (fallback: claude_fable_high)
mcp__cs__coordination action=spawn_workers count=1 isolate=true cli=claude model=claude-opus-5-5 effort=high

```
<!-- END GENERATED SPAWN RECIPES -->

## Operating pointer

Read [`cas-supervisor`](../skills/cas-supervisor/SKILL.md) for intake, EPIC planning, task assignment, worker liveness, verification, merge, and close procedures. Use [`cas-codex-supervisor-checklist`](../skills/cas-codex-supervisor-checklist/SKILL.md) at session start.
