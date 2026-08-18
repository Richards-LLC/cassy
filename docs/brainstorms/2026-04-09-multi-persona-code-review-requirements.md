---
date: 2026-04-09
topic: multi-persona-code-review
---

# Multi-Persona Code Review Pipeline

## Problem Frame

Cassy today has a single-pass `code-reviewer` agent at `cas-cli/src/builtins/agents/code-reviewer.md` that is primarily pattern-grep based (ast-grep for `.unwrap()`, `as any`, `console.log`, etc.) plus rule compliance checks. It cannot reason across logic flow, cross-file architectural consequences, or test coverage gaps in a changed diff.

Factory workers currently close tasks without a structured code-review gate, which means defects reach main. The goal is a multi-persona pipeline that catches classes of issues the single pass misses — with balanced recall across correctness, testing, maintainability, and project standards — and runs automatically at worker task close so findings are addressed by the agent that wrote the code.

Substantial prior design for this subsystem was captured in the cas-6103 EPIC task descriptions (cas-023c findings schema, cas-b250 personas, cas-2468 orchestrator pipeline). No output files persisted — all claimed artifacts at `~/.claude/skills/cas-code-review/` are absent. This requirements document salvages the prior design, reconciles it with Phase 0 distribution conventions (ships in `cas-cli/src/builtins/`), and resolves the product decisions that were skipped or made implicitly.

## Requirements

**Pipeline**
- **R1.** Seven reviewer personas total: four always-on (`correctness`, `testing`, `maintainability`, `project-standards`) plus three conditional (`security`, `performance`, `adversarial`). The `previous-comments` persona from prior design is dropped — Cassy is not PR-focused.
- **R2.** Conditional persona activation rules:
  - `security` — diff touches authentication boundaries, user input handling, or permission surfaces
  - `performance` — diff touches DB queries, data transforms, caching, or async code
  - `adversarial` — diff is 50+ changed non-test lines OR touches Cassy high-stakes modules: task verification flow (`close_ops`, `verify_ops`), factory coordination (spawn/message/queue/lifecycle), SQLite store mutations, hook system (`pre_tool`, `post_tool`), or MCP tool dispatch
- **R3.** Each persona emits structured JSON findings conforming to a shared schema: `title` (≤100 chars), `severity` (`P0`–`P3`), `file`, `line`, `why_it_matters`, `autofix_class` (`safe_auto`|`gated_auto`|`manual`|`advisory`), `owner` (`review-fixer`|`downstream-resolver`|`human`), `confidence` (0.0–1.0), `evidence` (array of code-grounded strings), `pre_existing` (bool), optional `suggested_fix`.
- **R4.** The orchestrator merges persona outputs with this pipeline: schema validation → confidence gate (suppress below 0.60 except P0 at 0.50+) → fingerprint deduplication (normalized file + line bucket ±3 + normalized title) → cross-reviewer agreement boost (+0.10 to merged confidence when 2+ reviewers hit the same fingerprint, capped at 1.0) → pre-existing separation → conservative route resolution (keep the more restrictive owner on disagreement) → partition and severity-sorted presentation.

**Invocation and Integration**
- **R5.** Primary invocation is automatic, at factory worker `task.close`, in `autofix` mode. The review runs before `task.close` completes and its outcome gates the close.
- **R6.** The existing `code-reviewer` agent at `cas-cli/src/builtins/agents/code-reviewer.md` (and codex mirror) is replaced entirely. Useful capability — rule compliance check via `mcp__cas__rule`, ast-grep structural red-flag patterns, language-specific checks — is absorbed into the new personas (`project-standards` for rules, `correctness` for structural patterns).
- **R7.** The `cas-worker` skill (`cas-cli/src/builtins/skills/cas-worker.md` + codex mirror) is updated in the same EPIC as cas-code-review to document the new close flow, including the P0 block behavior from R9 and the supervisor-override path.
- **R8.** Additional invocation modes supported but not primary: `interactive` (human-driven, full UX, bounded 2-round fix-and-rereview loop), `report-only` (read-only, no edits, safe for parallel runs), `headless` (skill-to-skill, returns a structured text envelope).

**Autofix Behavior**
- **R9.** In `autofix` mode, P0 findings hard-block `task.close`. The worker either (a) fixes the finding and retries close, or (b) requests a downgrade captured as a task note; the downgrade requires supervisor override.
- **R10.** In `autofix` mode, `safe_auto`-routed findings are applied by a single fixer sub-agent within a bounded `max_rounds=2` loop. After each fix round the review re-runs to catch cascade findings. Hard stop at 2 rounds; any residual findings exit the loop as non-safe_auto.
- **R11.** In `autofix` mode, residual non-safe_auto findings become Cassy tasks via the Phase 1 review-to-task flow subsystem, with priority mapping `P0→0`, `P1→1`, `P2→2`, `P3→3`. `advisory` findings never become tasks; they appear in the orchestrator output only.

**Distribution and Model Tiering**
- **R12.** cas-code-review ships as a distribution skill at `cas-cli/src/builtins/skills/cas-code-review/` (and codex mirror), embedded via `include_str!` and registered in `BUILTIN_SKILLS` / `CODEX_BUILTIN_SKILLS`, following the Phase 0 convention. Persona files, findings schema reference, and any helper assets live under `references/`.
- **R13.** Fixed model tiering: all seven persona sub-agents run on Sonnet; the orchestrator, merge logic, and fixer run on Opus. Not inherited from caller. Not configurable in Phase 1.

## Success Criteria

- Replaces `code-reviewer` with no regression in the rule-compliance and structural ast-grep checks it currently runs — every red flag the old agent catches today is still caught by at least one new persona.
- Catches at least one class of real issue the current `code-reviewer` does not catch, demonstrated on a real cas-src change within two weeks of shipping. This is the primary validation of the "catches things code-reviewer misses" goal and the condition for declaring Phase 1 subsystem A successful.
- Runs automatically at factory worker `task.close` without any worker-side invocation boilerplate.
- Produces actionable findings (safe_auto applied, residual routed to tasks) fast enough that workers do not learn to bypass it. A specific latency budget is deferred to planning (see Outstanding Questions).

## Scope Boundaries

- Not a PR / GitHub integration. `previous-comments` persona dropped, PR mode detection deferred, no `gh` CLI dependency in Phase 1.
- Not a replacement for Cassy rules (`mcp__cas__rule`). Rules remain a separate system that `project-standards` consumes.
- Not a replacement for `mcp__cas__verification`. cas-code-review is a pre-close quality gate; verification records are still written by the existing task-verifier path.
- No new human code-review workflow. The goal is automation at worker close, not new ceremony.
- `review-fixer` is not shipped as a separate skill or agent. It is a sub-agent implemented inside cas-code-review's orchestrator flow.
- No new Cassy task priorities, verification statuses, or database schemas. Reuses existing `mcp__cas__task` priority mapping and close-operation surface.
- No changes to the factory supervisor workflow beyond receiving findings and handling the P0-override path.
- Source files at `~/.claude/skills/cas-code-review/` claimed by cas-6103 closure notes are absent — no in-place "move into distribution" is possible. The Phase 1 implementation will construct the skill files from scratch using the design in the cas-023c / cas-b250 / cas-2468 task descriptions and the roadmap doc as the source of truth.

## Key Decisions

- **Replace code-reviewer entirely** rather than wrap or coexist. Rationale: cleanest migration, no two-tool confusion, forces parity with the old agent's checks so nothing is silently dropped.
- **Seven personas, not eight.** Rationale: `previous-comments` was designed for PR workflows Cassy does not use. Adding it now would be dead code.
- **Primary trigger is worker `task.close`.** Rationale: tightest feedback loop, matches the primary success signal ("catches things code-reviewer misses"), and keeps the fix cost on the agent that introduced the code.
- **Hard block on P0 at close**, with a supervisor-override downgrade path. Rationale: soft-blocking would let P0 findings slip into main, directly defeating the recall goal.
- **Fixed model tiering (Sonnet personas / Opus orchestrator).** Rationale: predictable cost, preserves orchestrator synthesis quality, and the cas-6103 design held up under review — not worth bike-shedding in Phase 1.
- **Adversarial activation is heuristic-based not keyword-based** — the LLM orchestrator judges "high-stakes module" rather than pattern-matching file paths. Rationale: Cassy layering is not stable across refactors; a path-based trigger would drift.
- **Ship all three Phase 1 subsystems (cas-code-review + review-to-task + cas-worker update) as one EPIC.** Rationale: atomic cutover, no interim "working but incomplete" state, no deferred broken promises in the worker skill docs.

## Dependencies / Assumptions

- **Dependency on review-to-task flow (Phase 1 subsystem C).** R11 requires an automatic path from findings to `mcp__cas__task` creation. Same EPIC per sequencing decision.
- **Dependency on a `task.close` pre-close hook or verification extension point.** R5 assumes `close_ops.rs` has — or can cleanly accept — a place to inject a quality gate. Unverified against current code; flagged as a planning question.
- **Assumption: the prior cas-6103 design in task descriptions is directionally sound.** Salvage-and-polish frame means planning starts from that content and reconciles with this requirements doc rather than re-designing from first principles.
- **Assumption: Sonnet quality is sufficient for persona-level hunting.** If Phase 1 dogfooding shows Sonnet personas miss too much compared to Opus, R13 may need revisiting — documented as a known risk.

## Outstanding Questions

### Resolve Before Planning

*(empty — all product-level decisions made in this brainstorm)*

### Deferred to Planning

- **[Affects R5]** [Technical] Does `cas-cli/src/stores/tasks/close_ops.rs` have or easily admit a pre-close quality-gate hook? Planning needs to investigate whether cas-code-review integrates as (a) an explicit call from `cas-worker` skill instructions before `task.close`, (b) a new extension point in `close_ops.rs` that dispatches quality gates, or (c) a pre-tool hook on `task.close` that runs the review. Option (a) is lowest implementation cost; option (b) is most robust but requires Cassy source changes.
- **[Affects R6]** [Technical] `cas sync` has no deletion mechanism — removing `code-reviewer.md` from `BUILTIN_AGENTS` leaves stale files in downstream `.claude/agents/` directories. Planning must decide between (i) replace file content with a "deprecated, see cas-code-review" stub that still has `managed_by: cas` so sync overwrites it, (ii) add a `BUILTIN_DELETIONS` mechanism to `builtins.rs`, or (iii) leave stale and document in release notes.
- **[Affects R9]** [Technical] How does a worker request, and a supervisor grant, a P0 downgrade? Options: dedicated task note `note_type`, a supervisor-only MCP tool, a CLI flag on `task.close`. Should integrate with the existing supervisor verification override path if one already exists.
- **[Affects R10]** [Needs research] Fixer sub-agent tool permissions — it needs `Edit` / `Write` access to apply fixes, but in factory mode workers have constrained tool sets. Planning should verify the factory-mode tool restrictions and decide whether the fixer runs with the worker's permissions or elevated permissions.
- **[Affects R4]** [Technical] Base-SHA resolution for fork-safe diff computation. Prior design references `scripts/resolve-base.sh` but `cas-src` has no `scripts/` directory containing such a file. Planning should choose bash helper vs. inline Rust vs. defer entirely (cas-code-review could accept `base_sha` as an input, requiring the caller to resolve).
- **[Affects success criteria]** [Needs research] Concrete runtime budget. Before workers adopt the gate, planning should measure orchestrator + seven-persona parallel dispatch on a real cas-src change and publish p50 / p95 latency. A gate that adds more than ~90s to every task close is likely to drive worker bypass pressure; planning should set the target and the fallback behavior if it's blown.
- **[Affects R12]** [Technical] Multi-file skill distribution — Phase 0 shipped `cas-brainstorm` / `cas-ideate` by registering one `BuiltinFile` per reference file. cas-code-review has more helper files (findings schema, 7 persona files, output template, fixer prompt). Planning should confirm the same pattern scales or propose a directory-aware `BuiltinFile` variant.
- **[Affects R11]** [Technical] Residual findings → task creation — exact mapping from finding fields to `mcp__cas__task create` arguments (title, description, priority, labels, external_ref pointing back to the review run). Belongs in the review-to-task subsystem brainstorm, carry forward as a handoff item.

## Next Steps

→ Hand off to planning (cas-supervisor). Plan one Phase 1 EPIC that bundles cas-code-review + review-to-task flow + cas-worker update with atomic cutover.

The cas-code-review implementation should start from the designs captured in tasks cas-023c (findings schema), cas-b250 (persona definitions), cas-2468 (orchestrator pipeline), reconciled against this requirements document when they conflict. The roadmap doc `docs/compound-engineering-roadmap.md` §1.1 is the public-facing summary of the same material.
