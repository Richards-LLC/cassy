---
date: 2026-04-09
topic: cas-worker-extensions
---

# cas-worker Extensions (Execution Methodology)

## Problem Frame

The `cas-worker` skill at `cas-cli/src/builtins/skills/cas-worker.md` defines how factory workers execute assigned tasks — start/show/close lifecycle, pre-close self-verification, blockers, communication. It is already substantive (~190 lines) and was extended today with a VERIFICATION_JAIL_BLOCKED recovery section via cas-3b8d.

The compound engineering roadmap §3.1, §3.2, §3.3 specify three additional execution-methodology extensions that the cas-6103 audit confirmed were never built: execution posture signals on tasks, a system-wide test check before close, and simplify-as-you-go mid-EPIC. Prior design was captured in the closed (but unshipped) task cas-3ea7 — "Quick wins: execution posture signals, system-wide test check, simplify-as-you-go". That description is directionally sound but leaves key product decisions unmade: where `execution_note` lives on the task schema, how strictly it is enforced at close, and what the system-wide test check actually requires the worker to do.

This brainstorm resolves those decisions so a follow-up Phase 1 EPIC can ship subsystem F as the smallest Phase 1 deliverable — adding one task-schema field, one Rust migration, three skill-content sections, and a mechanical `additive-only` enforcement check.

Operational validation on PRs (roadmap §2.3) is **dropped** from this subsystem — Cassy is distributed via homebrew and direct main commits, not a deploy pipeline, so post-deploy monitoring requirements do not map cleanly.

## Requirements

**Execution Posture Signal**
- **R1.** A new `execution_note` field is added to the `mcp__cas__task` schema and the underlying SQLite tasks table via Rust migration. Field is nullable (most tasks will not have one), type string, constrained at the MCP tool layer to an enum: `test-first`, `characterization-first`, `additive-only`, or null. Supervisors set the field on `action=create` or `action=update`; workers read it via `action=show`.
- **R2.** The `cas-worker` skill (and codex mirror) documents all three postures with a short paragraph each explaining what the worker should do differently:
  - `test-first` — write a failing test before implementation; the close-time self-verification should confirm at least one new test file exists in the diff.
  - `characterization-first` — write tests that capture the current behavior of the code being modified before any modification; useful for risky refactors. Not mechanically enforceable; the test-verifier agent inspects for appropriate evidence at close time.
  - `additive-only` — new files only, no modifications to existing files. The close gate hard-fails if the diff contains any modified-file entries.
- **R3.** Default is null (unset). A null `execution_note` means "use your judgment" — no posture guidance applies, no additional enforcement fires.

**Close-Time Enforcement**
- **R4.** Hard-enforce `additive-only`: on `task.close`, if the task has `execution_note = additive-only` and `git diff --cached --name-status` (or the equivalent for the task's staged work) reports any line starting with `M` or `D`, the close fails with a clear error message identifying the modified files. Rename-only changes (`R`) are treated as modifications and fail the gate. This check runs inside the worker execution flow (pre-close self-verification) and also in the Rust close_ops path as a backstop.
- **R5.** `test-first` is soft-enforced via task-verifier. The verifier agent sees the posture field in task context and, if set to `test-first`, rejects the close with advisory feedback when the diff contains no new test files. This is a task-verifier content update, not a Rust close_ops change.
- **R6.** `characterization-first` is context-only — the posture is passed to task-verifier, but the verifier does not mechanically check git history for tests-before-impl ordering. It inspects the worker's notes + committed evidence using normal judgment. If the verifier believes characterization tests are missing, it rejects with feedback.

**System-Wide Test Check**
- **R7.** A new item is added to the existing "Pre-Close Self-Verification (REQUIRED before closing)" checklist in `cas-worker` skill:
  > 6. **System-wide test check** — for every non-trivial change, trace 2 levels out from the edited code (callers, observers, middleware, hook subscribers). For each touched boundary, confirm integration tests exist with real objects (not mocks) covering that boundary, and run those integration tests as part of pre-close validation. Skip for: pure additive helpers with no callers yet, pure styling changes, pure documentation changes.
- **R8.** The skip list for R7 is worker-judgment — no hard gate. The worker decides "is this a pure additive helper / styling / doc change?" and skips the check if so. The item is framed as a mandatory checklist entry workers must either execute or explicitly justify skipping in task notes.
- **R9.** "Run those integration tests" in R7 means actually executing them — `cargo test <touched-crate>::<integration-test-name>` or equivalent — not just confirming their existence. The existing "4. Tests pass" checklist item requires a full `cargo test`, which already covers this in practice for most single-crate changes; R7 exists to force workers to identify the cross-file integration tests that matter *and* to run them, rather than trusting that a full test suite pass covers them.

**Simplify-As-You-Go**
- **R10.** The `cas-worker` skill instructs workers to invoke the existing `simplify` skill on their own recent work after closing their **third** task in the same EPIC, then again after the 6th, 9th, etc. The counter is per-worker-per-EPIC — resets when the worker moves to a different EPIC.
- **R11.** Simplify scope is the worker's own committed work within the current EPIC — `git log --author=<worker> <epic-branch>` style, plus any staged but uncommitted work. Not cross-worker. Not cross-EPIC.
- **R12.** If an EPIC contains fewer than 3 tasks assigned to a single worker, simplify-as-you-go never fires for that worker in that EPIC. That is intentional — the trigger exists to catch pattern accumulation, and <3 tasks is below the accumulation threshold.
- **R13.** The worker does not need a persistent counter in the task database. The count is derived at close time by querying `mcp__cas__task action=list assignee=<self> epic=<current-epic> status=closed` and checking whether `count % 3 == 0` after incrementing by one for the task being closed. Stateless, queryable.

**Distribution**
- **R14.** All skill-content changes (R2, R7, R10) update `cas-cli/src/builtins/skills/cas-worker.md` and `cas-cli/src/builtins/codex/skills/cas-worker.md` atomically. Content is kept in sync; stylistic differences between claude and codex variants are preserved where they already exist.
- **R15.** The new `execution_note` field propagates through the full Rust stack: SQLite column (new migration), Task struct in cas-core, `mcp__cas__task` parameter, `action=show` output, and display in the TUI task panel (if present).

## Success Criteria

- **Primary:** Workers demonstrably adapt their approach when `execution_note` is set. Verified in practice: a test-first task shows a new test file in the diff; an additive-only task cannot be closed with a modified-file in the diff; a characterization-first task has characterization tests per verifier review.
- The hard `additive-only` gate fires correctly on a deliberately crafted test case where a worker tries to close a task with modified files.
- Simplify-as-you-go runs automatically and visibly every 3rd task close in an EPIC, producing simplify output as a task note or commit, not silently.
- System-wide test check shows up in the pre-close checklist and workers execute it (visible in task notes or pre-close output).
- Zero regression in current worker behavior for tasks with null `execution_note` — the feature is purely additive at the task level.

## Scope Boundaries

- **Not shipping:** operational validation on PRs (roadmap §2.3). Dropped entirely for cas-src. Revisit only if Cassy deploys to a production environment that needs monitoring hooks.
- **Not shipping:** enforcement for `characterization-first` via git history inspection. Git ordering is too fragile (amended commits, squashes, rebases) and the check is left to task-verifier judgment.
- **Not shipping:** UI/TUI enhancements beyond surfacing the new field in `action=show` output. No new filters, no posture-based task list views.
- **Not shipping:** auto-population of `execution_note` based on task content (e.g., "refactoring tasks should be characterization-first"). Supervisors set it explicitly when they want the signal.
- **Not changing:** existing `mcp__cas__task` actions or parameters beyond adding the new field.
- **Not changing:** the existing pre-close self-verification checklist structure. R7 adds a new item (#6) at the end.
- **Not changing:** how `simplify` skill itself behaves. This subsystem just invokes it at new trigger points from the worker side.
- **No new posture keywords** beyond the three. `rewrite`, `exploratory`, `spike`, etc., are intentionally excluded — if the three don't cover a situation, leave the field null.

## Key Decisions

- **`execution_note` is a real Rust field, not a convention.** Rationale: first-class structured data makes supervisor UX clean (visible in `action=show`, can be listed/filtered later), avoids the fragility of keyword-scanning in free-text description fields, and makes enforcement straightforward.
- **Mixed enforcement strategy — `additive-only` hard, others advisory.** Rationale: `additive-only` is mechanically checkable from a diff without ambiguity; the other two require evidence inspection that is the task-verifier's job. Don't pretend git-ordering enforcement works when it doesn't.
- **Simplify trigger is a hard-coded counter at every 3rd close**, not a flexible rhythm or supervisor-triggered. Rationale: predictable and visible. A flexible trigger ("when you feel like it") gets skipped; a supervisor-triggered trigger centralizes discipline on the supervisor.
- **System-wide test check requires running the tests**, not just verifying they exist. Rationale: a test file's presence is a weak signal; an executed test is evidence.
- **Drop operational validation (§2.3) from subsystem F.** Rationale: cas-src ships via `cargo build` and distribution sync, not a deploy pipeline. The monitoring/rollback framing is a poor fit. Worth revisiting if Cassy grows a production deployment surface.
- **Counter is stateless via task list query**, not a persistent field. Rationale: avoids another schema change for a derived value; list query is cheap.

## Dependencies / Assumptions

- **Assumption: `mcp__cas__task action=list` supports filtering by `assignee + epic + status` in the current API.** Needs planning-time verification — if the filter combination is not supported, R13 requires a small API extension or falls back to full scan + client-side filter.
- **Assumption: the existing `simplify` skill operates on git-staged or git-recent work** and can be invoked with a scope argument. If not, R11 needs reframing or the simplify skill needs a small extension.
- **Assumption: `close_ops.rs` has a stable location for injecting the `additive-only` check without touching unrelated close-path logic.** Planning should verify. The recently-landed cas-82d6 fix may have restructured this area.
- **Dependency on task-verifier agent content update for R5 and R6.** task-verifier is at `cas-cli/src/builtins/agents/task-verifier.md` (claude + codex mirrors) — Phase 1 implementation edits it to reference `execution_note` in its verification flow.

## Outstanding Questions

### Resolve Before Planning

*(empty — all product-level decisions made in this brainstorm)*

### Deferred to Planning

- **[Affects R1, R15]** [Technical] Migration number and schema — planning chooses the next migration number (the current max is at m080 based on earlier grep), defines the exact column (`execution_note TEXT NULL` vs `execution_note TEXT CHECK (... in (...))` enum-at-DB-level), and decides whether the enum is enforced in SQL or only at the MCP tool layer.
- **[Affects R4]** [Technical] Exact git diff invocation in the `additive-only` close gate — does the Rust close_ops code shell out to `git`, or use an existing in-repo git wrapper? The worker's self-check in the skill can be prose-documented but the Rust backstop needs a concrete implementation.
- **[Affects R7, R8, R9]** [Technical] How does the worker identify "2 levels out" boundaries mechanically? Prose guidance is fine in the skill for Phase 1, but planning should note that this is LLM-judgment and document it as such in the skill so workers don't over-engineer it into a call-graph tool.
- **[Affects R10, R11, R13]** [Technical] How is "the current EPIC" known to the worker? Factory sessions have an `epic_id` context — planning should confirm workers have access to it via `whoami` or similar, and choose the exact query shape.
- **[Affects R5, R6]** [Technical] task-verifier agent content update to use `execution_note` context — specifically, the prompt sections that instruct the verifier to look for test-first / characterization evidence. Planning should draft the exact diff.
- **[Affects R15]** [Technical] TUI task panel display of `execution_note` — whether this is a Phase 1 scope item or deferred. Argument for deferring: minimal value if supervisors are setting the field directly via MCP. Argument for including: visibility during interactive review. Leans toward deferring.
- **[Affects R10]** [Needs research] Current state of the `simplify` skill — does it need any argument to scope the simplification to recent work, or does it discover scope from git state? Planning reads the current skill definition and decides.

## Next Steps

→ Hand off to planning (cas-supervisor). Plan a Phase 1 EPIC scoped to subsystem F: schema migration for `execution_note`, Rust close_ops `additive-only` backstop, cas-worker skill additions (posture section + system-wide test check #6 + simplify-as-you-go section), task-verifier content update to consume `execution_note` context.

This is the smallest Phase 1 EPIC in the program — deliberately. Ship it as the first real Phase 1 cutover after the brainstorms for A / B are planned, to get cas-worker visibly improved while larger EPICs plan.

The Rust work is minimal: one migration, one or two column additions to the Task struct, one close_ops gate, one parameter exposure at the MCP tool layer. The skill work is the substance — ~3 new sections in `cas-worker.md` totaling maybe 80 lines.
