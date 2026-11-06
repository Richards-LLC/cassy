# Planning — Gates, Templates, EPIC Sizing

## Planning Gates

Before work is assigned:

- **SRP** — split a task with more than one responsibility; "and" in a task description is a red flag.
- **Dependency ordering** — sequence tasks so no worker blocks on unfinished work.
- **Scope lock** — the brief freezes at assignment; link the plan section and its metric so workers can check plan fit without expanding scope.

## Trajectory Gate

Before finalizing EPIC scope, a multi-task plan or an architectural decision, assess trajectory, not just immediate correctness:

- **Scalability** — does it hold at 10x volume, users, code size or complexity? Name the breaking point.
- **Lock-in** — call out any one-way doors.
- **Production failure mode** — what breaks, how is it detected, how does on-call recover?
- **Six-month direction** — does this move toward or away from where the project is heading?
- **Known traps** — check memories and prior incidents for a pattern this repeats.

Surface a trajectory concern in-line; record nothing when there is none. Small decisions accumulate into architecture, so never skip this gate for them.

## Spec Requirements

Every task spec includes:

- **Acceptance criteria first** — what done looks like before how to build it.
- **Interface definition** — inputs, outputs and error states.
- **Layer boundary** — the files or modules the worker owns; a breach is a rejection.
- **Explicit non-goals** — what the task deliberately does not do.
- **Test guidance** — name the behaviour each test must catch, and the error path only when the change touches it. Ask for the [fewest tests](../../cas-tdd/SKILL.md#fewest-tests): never a scenario list, a test count or a fixture per case.
- **Evidence-source plan** — for report or investigation tasks, name the MCP, log, recording or task-record sources. Require live `.cas/cas.db` access only when safer surfaces are insufficient, and then read-only or from a copied snapshot.

State the symptom, failing surface and observable. Bad: `Improve issue intake (GH #221).` Good: `Codex workers receive a task but remain idle; instrument the wake path and prove a delivered message surfaces a turn (GH #224).`

EPIC subtasks shape this content with the Implementation Unit Template below.

## Implementation Unit Template

Use it for every EPIC child (`task_type=task` or `feature`) so a worker can parse the brief in five seconds. Standalone bugs, chores and spikes stay freeform; fields may be `N/A` or omitted.

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
**Execution note:** test-first | characterization-first (only behaviour that stays) | additive-only | value-only | (omit)
**Patterns to follow:** Reference existing code to mirror
**Test scenarios:** one per changed behaviour, e.g. empty input -> returns error Z
**Verification:** Observable outcomes when complete
```

Write decisions, not code: the Goal is one sentence the worker can restate, and Approach is 1–3 sentences of design choice, never pseudocode. Requirements are stable R-IDs from `docs/brainstorms/YYYY-MM-DD-<topic>-requirements.md`. Never use `characterization-first` on behaviour the plan replaces; use it only to protect what stays. `additive-only` refuses close on any modified file, and `value-only` permits edits to existing values only; omit the field for broader edits.

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

- **Agent-task fit** — the right lane for the job ([model-selection.md](model-selection.md)).
- **Context injection** — link the plan section and its metric in the brief; withhold what the task does not need.
- **Contract handoff** — a successful `task action=start` accepts the assignment; no prose ACK.

## Review Gates

You have rejection authority. Every rejection names the gate that failed, the code or file, and what must change: "SRP violation: `handle_request()` in `router.rs` handles both auth and routing — split it", not "SRP violation".

- **Fewest tests** — each changed behaviour has one test that fails without the change ([Fewest tests](../../cas-tdd/SKILL.md#fewest-tests)); they run once at the Phase 4 final gate. Near-duplicates, matrices, restated implementation, sleeps and retry-wrapped flakes are findings; read the line delta.
- **DRY, SRP and layer breaches** — sent back, never "clean up later"; work outside the declared boundary is rejected.
- **Interface and config compliance** — output matches the declared interface; no magic numbers that should be configurable.
- **Verify, don't trust** — read the diff and delivery evidence; worker self-reports are inputs, not verdicts.
- **Review cadence** — Every worker merge receives the canonical merge-time diff review in [workflow.md](workflow.md#required-merge-review-discipline): the diff against the spec and acceptance criteria, ownership, contracts, lane CI and receipts. Nothing is rebuilt at merge; Phase 4 runs the project's full final-tree assembly gate once, and a CI run of the full suite on the merged tree is that gate. Do not dispatch a separate review workflow or require worker-supplied review output.

## EPIC Sizing

5–12 subtasks per EPIC. Fewer usually means tasks are too coarse; more means split into phases, each its own EPIC with a clear handoff. Run 3–4 parallel workers for most EPICs, matched to independent file groups; beyond 4, coordination costs outweigh throughput.

| Pattern | Shape | Cassy fields |
|---|---|---|
| Chain | A → B → C | `dep_add id=B to_id=A dep_type=blocks` |
| Fan-out | A → {B, C, D} → E | B/C/D each `blocked_by=A`; E `blocked_by=B,C,D` |
| Independent | {A, B, C} | no `blocked_by` |

## Task Breakdown

Prefer tracer-bullet vertical slices (adapted from mattpocock/skills `to-tickets`, MIT © 2026 Matt Pocock): each task cuts a narrow, complete path through the affected layers, is demoable on its own and fits one fresh worker context. Give tasks only their genuine blocking edges.

For a mechanical, codebase-wide change that cannot land green as a slice, use **expand–contract**: add the new form beside the old, migrate call sites in blast-radius-sized batches blocked by the expansion, then remove the old form after every batch lands. Keep the dependencies in Cassy tasks.

- **Demo statements** — give a `demo_statement` only to user-facing work (`qa.user_facing_labels`), e.g. `demo_statement="User types a query and results filter live"`.
- **Spikes** — when the output is understanding, use `task_type=spike` with question-based acceptance criteria and a decision as the deliverable. Compare competing approaches in a spike before implementing one.
- **Gates** — a promotion, rollout, flag flip or sign-off you own is `task_type=gate`: start it, record the outcome with `task action=notes note_type=decision`, then close it without commit evidence.
- **Model lane** — tag each task's lane at breakdown; see [model-selection.md](model-selection.md). Search before planning: [workflow.md Phase 1](workflow.md#phase-1-plan).
