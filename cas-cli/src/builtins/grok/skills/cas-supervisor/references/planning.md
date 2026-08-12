---
name: planning
description: Supervisor planning reference — gates, templates, EPIC sizing.
managed_by: cas
---

# Planning — Gates, Templates, EPIC Sizing

## Planning Gates

Before work is assigned:

- **SRP enforcement** — Split tasks with more than one responsibility; "and" in a task description is a red flag
- **Dependency ordering** — Sequence tasks so no worker blocks on unfinished work
- **Scope lock** — Task brief is frozen at assignment; workers cannot expand scope unilaterally

## Trajectory Gate

Before finalizing EPIC scope, multi-task plans, or architectural decisions, explicitly assess trajectory questions — not just immediate correctness:

- **Scalability** — does this approach hold up at 10x volume, users, code size, or complexity? Name the breaking point if there is one.
- **Lock-in** — does this commit us to a direction that's hard to reverse? Call out any one-way doors.
- **Production failure mode** — what breaks in production, how is it detected, and how does the on-call engineer recover?
- **Six-month direction** — given what we know about where the project is heading, does this move us toward or away from that destination?
- **Known traps** — check project memories and prior incidents for patterns this decision might repeat.

Surface the trajectory assessment in-line even when the answer is "no concerns" — the fact that you thought about it is part of the value. Do not skip this gate for "small" decisions that accumulate into architecture.

## Spec Requirements

Every task spec must include:

- **Acceptance criteria first** — Worker receives "what done looks like" before "how to build it"
- **Interface definition** — Inputs, outputs, and error states defined explicitly
- **Layer boundary** — Which files/modules the worker owns and must not touch outside of; boundary violation is a rejection condition
- **Explicit non-goals** — What the task deliberately does NOT do, stated to prevent scope creep
- **Test guidance** — Name the specific scenarios the worker must test, including at least one error path. Don't leave test design entirely to the worker.
- **Evidence-source plan** — For report/investigation/evidence tasks, explicitly name the MCP/log/recording/task-record sources the worker should use. Treat live `.cas/cas.db` access as an exception: only require it when safer surfaces are insufficient, and then specify read-only URI or copied-snapshot access.

**Task-description specificity (observed 2026-08-11):** Bad: `Improve issue intake (GH #221).`
Good: `Codex workers receive a task but remain idle; instrument the wake path and prove a delivered message surfaces a turn (GH #224).`
State the symptom, failing surface, and repro/observable; a title restatement leaves the worker to rediscover the assignment.

For EPIC subtasks specifically, shape the spec prose using the Implementation Unit Template below. `Spec Requirements` enumerates *what must be present*; the template specifies *how the prose is shaped*.

## Implementation Unit Template

Every EPIC subtask (`task_type=task` or `task_type=feature` that is a child of an EPIC) uses this template as the canonical shape of its `description` + companion fields. The goal is predictable structure a worker can parse in five seconds. Standalone bugs, chores, and spikes stay freeform — spike deliverables are *understanding*, not implementation, so the template does not fit.

Canonical template:

```markdown
- [ ] **Unit N: [Name]**

**Goal:** What this unit accomplishes
**Requirements:** R1, R2      # only when an EPIC brainstorm doc exists
**Dependencies:** None | Unit X | cas-<id>
**Files:**
  - Create: `path/to/new_file.rs`
  - Modify: `path/to/existing_file.rs`
  - Test: `path/to/test_file.rs`
**Approach:** Key design or sequencing decision
**Execution note:** test-first | characterization-first | additive-only | (omit)
**Patterns to follow:** Reference existing code to mirror
**Test scenarios:**
  - Happy path: input X -> expected Y
  - Edge case: empty input -> returns error Z
  - Error path: network failure -> retries 3x then fails
**Verification:** Observable outcomes when complete
```

Field purposes (write decisions, not code — "Approach" is 1–3 sentences of sequencing and design choice, not a diff sketch; "Files" lists paths only):

- **Goal** — one sentence the worker can restate back to you. If you can't state it in one sentence, the unit is too big.
- **Requirements** — stable R-IDs from the linked brainstorm doc at `docs/brainstorms/YYYY-MM-DD-<topic>-requirements.md`. Convention only, no new field. Omit when no brainstorm exists.
- **Dependencies** — hard blockers go in `blocked_by`; soft ordering or "after X lands" notes stay as prose.
- **Files** — the layer boundary. What the worker owns and must not touch outside of. Boundary violation is a rejection condition.
- **Approach** — the sequencing or design decision already made. Not a code sketch, not a pseudocode draft. If you find yourself writing pseudocode, you are doing the worker's job.
- **Execution note** — maps 1:1 to the task `execution_note` field. One of `test-first`, `characterization-first`, `additive-only`, or omitted. **Warning:** `additive-only` hard-blocks close on ANY file modification (M/D/R in git status). Only use for truly new-file-only tasks. If a task needs to edit existing files, do not set `additive-only`.
- **Patterns to follow** — pointer to existing code or a prior commit the worker should mirror. Reduces stylistic drift.
- **Test scenarios** — name the scenarios, including at least one error path. Don't leave test design entirely to the worker.
- **Verification** — observable outcome. What can be demonstrated when done. Maps to `demo_statement`.

EPIC subtasks only — standalone bugs/chores/spikes stay freeform. Fields can be `N/A` or omitted when not applicable.

| Template field | Maps to |
|---|---|
| Unit N name | `title` |
| Goal | first paragraph of `description` |
| Requirements | prose bullet in `description` (convention) |
| Dependencies | `blocked_by` (hard) or `description` prose (soft) |
| Files | `description` prose block |
| Approach | `design` field |
| Execution note | `execution_note` field |
| Patterns to follow | `description` prose |
| Test scenarios | `acceptance_criteria` field |
| Verification | `demo_statement` field |

## Assignment Checks

- **Agent-task fit** — Right capability for the job; no generalist on specialist work
- **Context injection** — Send only needed context; withhold irrelevant info to prevent scope bleed
- **Contract handoff** — Worker acknowledges acceptance criteria before starting

## Review Gates

Supervisor has rejection authority. Work is sent back with specific, actionable reasons.

- **Tests exist and pass** — No untested code ships
- **Failure paths tested** — Test suite covers error states and edge cases, not just happy path
- **DRY violation scan** — Duplication flagged and sent back; "clean up later" is not accepted
- **SRP violation scan** — Multi-responsibility modules or functions are sent back
- **Layer breach** — Work outside declared boundary is automatic rejection
- **Interface compliance** — Output matches the declared interface exactly; surprises are rejected
- **Config compliance** — No magic numbers or hardcoded values that should be configurable
- **Test quality** — Tests must verify behavior, not just pass
- **Flag obvious SOLID violations** — with specifics; don't rubber-stamp "SOLID compliance verified"
- **Verify, don't trust** — Read the actual diff or run tests yourself before accepting. Worker self-reports are inputs, not verdicts.
- **Rejection format** — Every rejection names: (1) which gate failed, (2) the specific code/file, (3) what needs to change. "SRP violation" alone is not actionable; "SRP violation: `handle_request()` in `router.rs` handles both auth and routing — split into two functions" is.
- **Review cadence** — Workers do not run `/cas-code-review` at close. Supervisors run a lightweight per-merge gate at cherry-pick time, then one full multi-persona `/cas-code-review` against the assembled EPIC diff in Phase 4. Do not dispatch workers with "run `/cas-code-review` on the diff" instructions; that's the legacy `owner = "worker"` path and only applies if a project has opted in via `.cas/config.toml`. Your manual per-merge review should focus on architectural fit, scope compliance, obvious defects, and domain knowledge.

## Ongoing Discipline

- **Pattern consistency** — New work matches established conventions; deviations require explicit justification
- **Debt tagging** — Log deliberate shortcuts with reason and remediation plan; unlogged shortcuts are violations
- **Search before planning** — Use `cas__search` for semantic search across memories and tasks, `cas__memory` for storing learnings. Always search before creating new work to avoid duplicating prior solutions or contradicting past decisions.

## EPIC Sizing

5–12 subtasks per EPIC is the sweet spot. Below 5 usually means the tasks are too coarse (split further). Above 12, consider splitting into phases — each phase is its own EPIC with a clear handoff boundary.

Practical worker limits: 3–4 parallel workers for most EPICs. Beyond 4, coordination overhead (merge conflicts, sync messages, context injection) dominates over throughput gains. Match worker count to independent file groups, not task count.

**Dependency patterns:**

| Pattern | Shape | When to use | CAS fields |
|---|---|---|---|
| Chain | A → B → C | Sequential work where each step needs the prior output | `dep_add id=B to_id=A dep_type=blocks` |
| Fan-out | A → {B, C, D} → E | Independent tasks after a shared setup, converging at a gate | B/C/D each `blocked_by=A`; E `blocked_by=B,C,D` |
| Independent | {A, B, C} | No dependencies — maximum parallelism | No `blocked_by` needed |

Fan-out is the most common pattern for EPICs with 3+ workers: one setup/spike task fans out into parallel implementation tasks, then a final integration task gates on all of them.

## Task Breakdown Guidelines

When breaking an epic into subtasks, apply these patterns:

**Demo statements** — Every subtask must have a `demo_statement` describing what can be demonstrated when complete. Example: `demo_statement="User types a query and results filter live"`. If a task has no demo-able output, it may be a horizontal slice — restructure it into a vertical slice that delivers observable value.

**Spikes** — If a task's primary output is understanding (not code), create it as a spike: `task_type=spike`. Spikes have question-based acceptance criteria (e.g., "Which auth library fits our constraints?") and produce a decision or recommendation, not implementation.

**Gates** — If a task represents a promotion, rollout, flag flip, or sign-off decision owned by the supervisor, create it as a gate: `task_type=gate`. Start the gate yourself, record the outcome with `task action=notes note_type=decision`, then close it without borrowing commit evidence. An open gate can block downstream tasks through `blocked_by` like any other task.

**Fit checks** — When multiple approaches exist, create a spike first to compare options. Document the comparison in the spec's `design_notes` before committing to an approach. This prevents wasted implementation effort on the wrong path.

**Model tier** — Decide each task's worker tier while breaking down: default standard; tag deviations with `labels="tier:light"` / `"tier:heavy"` / `"tier:frontier"` and note non-obvious rationale in `design`. Tier rubric, spawn mix, and escalation workflow in [model-selection.md](model-selection.md).
