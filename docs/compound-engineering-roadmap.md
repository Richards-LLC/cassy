# Compound Engineering Roadmap

> **Status (2026-04-13)**
> - **Phase 0 shipped** (EPIC cas-ada0): 2 skills + 2 agents — `cas-brainstorm`, `cas-ideate`, `git-history-analyzer`, `issue-intelligence-analyst`.
> - **Phase 1 shipped 2026-04-09** (EPICs cas-3444 / cas-2c1c / cas-b4d1 / cas-0750): multi-persona code review pipeline (1.1), memory schema + overlap detection (1.2.1, 1.2.2), implementation unit template (1.3.2), worker execution methodology (incl. execution-posture signals 3.1).
> - **Phase 1.5 shipped 2026-04-12**: skill hardening EPIC (47 findings, 24 tasks) + compound wiring (brainstorm/ideate triggers, sassy supervisor personality).
> - **Phase 2+**: see Implementation Priority table for per-item status.
> - **Original source:** [EveryInc/compound-engineering-plugin](https://github.com/EveryInc/compound-engineering-plugin)

**Source:** [EveryInc/compound-engineering-plugin](https://github.com/EveryInc/compound-engineering-plugin)
**Reviewed:** 2026-04-06
**Purpose:** Feature ideas for Cassy inspired by Every's compound engineering workflow plugin.

---

## Overview

Compound Engineering is a Claude Code plugin that implements a full development lifecycle:
`brainstorm -> plan -> work -> review -> compound (capture learnings) -> repeat`.

The philosophy: each unit of engineering work should make subsequent units easier. 80% planning and review, 20% execution.

This document catalogs ideas worth adopting in Cassy, organized by priority tier.

---

## Tier 1: High Impact

### 1.1 Multi-Persona Code Review Pipeline

**What CE does:** The `/ce:review` skill spawns 6+ specialized reviewer agents in parallel, each with a focused hunting mandate, structured JSON output, and confidence calibration. Results are merged with deduplication, confidence gating, and severity-based routing.

**What Cassy has today:** A `code-reviewer` agent that runs as a single pass.

**What to build:**

#### 1.1.1 Persona-Based Parallel Review

Spawn multiple reviewer sub-agents in parallel, each with a specific focus:

| Persona | Focus | Always-On? |
|---------|-------|------------|
| correctness | Off-by-one, null propagation, race conditions, broken error handling | Yes |
| testing | Untested branches, missing edge cases, brittle tests, behavioral changes without tests | Yes |
| maintainability | Coupling, dead code, premature abstraction, poor naming | Yes |
| project-standards | CLAUDE.md/AGENTS.md compliance, naming conventions, pattern violations | Yes |
| security | Injection, auth bypasses, secrets in code, SSRF, path traversal | Conditional |
| performance | N+1 queries, unbounded memory, missing pagination, blocking I/O in async | Conditional |
| adversarial | Assumption violations, composition failures, cascade construction, abuse cases | Conditional (50+ changed lines or auth/payments/data mutations) |

Conditional reviewers are selected by the orchestrator based on diff content analysis.

#### 1.1.2 Structured Findings Schema

Every reviewer outputs JSON:

```json
{
  "reviewer": "persona-name",
  "findings": [
    {
      "title": "Short issue title (<=100 chars)",
      "severity": "P0|P1|P2|P3",
      "file": "relative/path.rs",
      "line": 42,
      "why_it_matters": "Impact and failure mode, not just what's wrong",
      "autofix_class": "safe_auto|gated_auto|manual|advisory",
      "owner": "review-fixer|downstream-resolver|human",
      "requires_verification": true,
      "suggested_fix": "Concrete minimal fix or null",
      "confidence": 0.85,
      "evidence": ["Code snippet or line reference", "Pattern description"],
      "pre_existing": false
    }
  ],
  "residual_risks": ["..."],
  "testing_gaps": ["..."]
}
```

#### 1.1.3 Confidence Gating

- Suppress findings below 0.60 confidence
- **Exception:** P0 findings at 0.50+ are never suppressed (critical-but-uncertain issues must surface)
- Calibration guidance per persona:
  - High (0.80+): Reproducible from code alone, full execution path traced
  - Moderate (0.60-0.79): Pattern present but can't fully confirm all conditions
  - Low (<0.60): Requires runtime/external conditions with no evidence

#### 1.1.4 Cross-Reviewer Agreement Boost

When 2+ independent reviewers flag the same issue:
- Fingerprint: `normalize(file) + line_bucket(line, +/-3) + normalize(title)`
- Merge: highest severity, highest confidence, union evidence
- Boost merged confidence by 0.10 (capped at 1.0)

#### 1.1.5 Severity vs. Routing Separation

Severity answers **urgency**. Routing answers **who acts next** and **whether the tool may edit files**.

| `autofix_class` | Default Owner | Meaning | Mutates Checkout? |
|-----------------|---------------|---------|-------------------|
| `safe_auto` | review-fixer | Local, deterministic fix | Yes |
| `gated_auto` | downstream-resolver | Concrete fix exists but changes behavior/contracts | No |
| `manual` | downstream-resolver | Actionable but needs design decisions | No |
| `advisory` | human | Report-only: risk notes, deployment items | No |

A P3 finding might be `gated_auto` (needs approval despite low severity). A P0 might be `advisory` (critical risk but no code fix). These are orthogonal.

#### 1.1.6 Review Modes

Single skill, four modes:

| Mode | Questions? | Fixes? | Artifacts? | Use Case |
|------|-----------|--------|-----------|----------|
| interactive | Yes | safe_auto + user-approved gated | Yes | Default human workflow |
| autofix | No | safe_auto only, one pass | Yes + todos | Factory workers |
| report-only | No | None | None | Parallel safety, read-only |
| headless | No | safe_auto only, one pass | Yes | Skill-to-skill invocation |

#### 1.1.7 Intent Summary

Before spawning reviewers, extract a 2-3 line intent summary (from PR description, commit messages, or user input). Pass to every reviewer. This shapes **how hard each reviewer looks**, contextualizing findings against stated goals.

---

### 1.2 Learnings/Knowledge Compounding System

**What CE does:** After every completed task, `/ce:compound` captures the solution in `docs/solutions/` with structured YAML frontmatter, categorized by problem type, with overlap detection, and a refresh workflow to keep docs current.

**What Cassy has today:** Memory system with markdown files and MEMORY.md index. Good persistence but informal structure.

**What to build:**

#### 1.2.1 Structured Frontmatter Schema for Memories

Extend Cassy memory files with richer frontmatter:

```yaml
---
name: SQLite WAL on NTFS3 causes MCP timeout
description: NTFS3 lacks POSIX locks, SQLite WAL hangs on .shm
type: bugfix  # or: knowledge, pattern, workflow
module: cas-mcp
problem_type: runtime_error  # enum: build_error, test_failure, runtime_error, performance_issue, etc.
severity: high
root_cause: config_error  # enum: missing_association, wrong_api, thread_violation, etc.
symptoms:
  - MCP tool calls timeout after 60s
  - Multiple cas serve processes on same DB
tags: [sqlite, ntfs, wal, mcp, timeout]
date: 2026-03-30
---
```

Benefits: enables search by module, problem_type, severity, tags. Current memories are prose-heavy with implicit categorization.

#### 1.2.2 Overlap Detection Before Creating

When `mcp__cas__memory action:remember` is called:
1. Search existing memories for overlap across 5 dimensions: problem statement, root cause, solution approach, referenced files, tags
2. High overlap (4-5 dimensions) -> update existing memory, not create new
3. Moderate overlap (2-3 dimensions) -> create new, flag relationship
4. Low overlap (0-1) -> create new

This prevents the drift that comes from having two memories about the same problem.

#### 1.2.3 Two Tracks: Bug vs. Knowledge

**Bug track sections:** Problem, Symptoms, What Didn't Work, Solution, Why This Works, Prevention
**Knowledge track sections:** Context, Guidance, Why This Matters, When to Apply, Examples

Current Cassy memories mix these. Separating them makes retrieval more predictable.

#### 1.2.4 Memory Refresh Command

`cas memory refresh [scope]` — audit and update stale memories against current codebase:

| Outcome | Signal | Action |
|---------|--------|--------|
| Keep | Still accurate | No edit |
| Update | Core solution valid, references drifted | In-place edits |
| Consolidate | 2+ memories overlap heavily | Merge into canonical |
| Replace | Core guidance now misleading | Write successor, delete old |
| Delete | No longer useful or applicable | Delete (git preserves) |

Cross-reference memory claims against codebase across: file paths still exist, recommended solution matches current code, code examples reflect current implementation, related memories consistent.

#### 1.2.5 Grep-First Search Pattern

For `mcp__cas__search`:
1. Extract keywords from query
2. Content-search pre-filter (scan file contents without reading into context)
3. Read frontmatter only of candidates (first 30 lines)
4. Score and rank by frontmatter matches
5. Full read of strong/moderate matches only

Scales from 10 to 1000+ memories without reading everything.

---

### 1.3 Structured Planning Pipeline

**What CE does:** Three-skill chain: ideate (divergent generation) -> brainstorm (interactive Q&A) -> plan (structured implementation units). Each produces a durable artifact that feeds the next.

**What Cassy has today:** Adversarial supervisor for EPICs with intake gate and structured specs. Good but informal compared to CE.

**What to build:**

#### 1.3.1 Requirements Documents with Stable IDs

Before EPIC creation, produce a requirements doc:

```markdown
## Requirements
- **R1**: Users must be able to X
- **R2**: System must handle Y within Z ms
- **R3**: API must maintain backward compatibility with V
```

EPIC tasks trace back to requirement IDs. Review findings reference which requirement they affect. This closes the loop between planning and verification.

#### 1.3.2 Implementation Unit Template

Each EPIC task should follow this structure:

```markdown
- [ ] **Unit N: [Name]**

**Goal:** What this unit accomplishes
**Requirements:** R1, R2
**Dependencies:** None / Unit X
**Files:**
- Create: `path/to/new_file.rs`
- Modify: `path/to/existing_file.rs`
- Test: `path/to/test_file.rs`
**Approach:** Key design or sequencing decision
**Execution note:** test-first | characterization-first | additive-only
**Patterns to follow:** Reference existing code to mirror
**Test scenarios:**
- Happy path: input X -> expected Y
- Edge case: empty input -> returns error
- Error path: network failure -> retries 3x then fails
**Verification:** Observable outcomes when complete
```

The key insight: **decisions, not code**. Capture approach, boundaries, dependencies, risks, test scenarios. No pre-written implementation code.

#### 1.3.3 Confidence-Check Deepening

After writing a spec, before handing to workers:
1. Score every section against a confidence checklist
2. Identify top 2-5 gaps (1-2 for lightweight tasks)
3. Dispatch targeted research (max 8 agents total)
4. Integrate findings into spec
5. Then hand off

Prevents over-engineering while catching real issues.

#### 1.3.4 Visual Communication in Plans

When complexity warrants:

| Content Pattern | Visual Aid |
|----------------|-----------|
| 4+ units with non-linear dependencies | Mermaid dependency graph |
| 3+ interacting system surfaces | Mermaid interaction diagram |
| 3+ behavioral modes/states | Comparison table |
| Multi-step workflow | Mermaid flow diagram |
| 3+ competing approaches | Comparison table |

Inline at point of relevance, explicitly framed as "directional guidance, not implementation specification."

---

### 1.4 PR Feedback Resolution Automation

**What CE does:** `/ce:resolve-pr-feedback` fetches unresolved PR threads, triages, clusters by theme when 3+ comments relate, dispatches resolver agents, commits/pushes/replies/resolves threads.

**What Cassy has today:** Nothing automated for PR feedback.

**What to build:**

#### 1.4.1 `cas pr resolve` Skill

1. Fetch unresolved PR threads via `gh api` / GraphQL
2. Triage: separate new feedback from already-handled
3. **Cluster analysis** (when 3+ items): group by theme + spatial proximity, investigate root cause (systemic vs. band-aid)
4. Dispatch resolver agents per thread/cluster
5. Commit, push, reply to threads with specific quotes
6. Resolve threads via GraphQL
7. Verify all resolved
8. Report: grouped by verdict (fixed, replied, not-addressing, needs-human)

Philosophy: "Agent time is cheap, tech debt is expensive" -- fix everything valid, including nitpicks.

---

### 1.5 Review-to-Task Flow

**What CE does:** Code review findings automatically become file-based todos with severity/priority mapping. Todos flow through create -> triage -> resolve lifecycle.

**What Cassy has today:** Tasks and code-reviewer are separate systems with no automatic connection.

**What to build:**

When code-reviewer produces findings:
- P0/P1 `safe_auto` or `gated_auto` -> auto-create Cassy tasks with priority `high`
- P2 `manual` -> auto-create Cassy tasks with priority `medium`
- P3 `advisory` -> include in report only, no task creation

Add `cas task triage` command for interactive review of pending tasks: approve, skip, or modify each one.

---

## Tier 2: Medium Impact

### 2.1 Specialized Research Agents

**What CE has:** Six specialized research agents, each with clear methodology.

**What Cassy should add:**

#### 2.1.1 Git History Analyzer

Dedicated agent for code archaeology:
- `git log --follow --oneline -20 <file>` for file evolution
- `git blame -w -C -C -C <file>` ignoring whitespace, following movement
- `git log --grep=<keyword>` for recurring themes
- `git shortlog -sn -- <path>` for contributor mapping
- `git log -S"pattern"` for when patterns were introduced/removed

Useful during planning and debugging to understand *why* code exists.

#### 2.1.2 Issue Intelligence Analyst

Extract strategic signal from GitHub issues:
1. Scan labels for priority patterns
2. Fetch high-priority issues first (truncated bodies for token efficiency)
3. Cluster by theme/root-cause (not symptom)
4. Score themes: issue_count, source_mix, trend_direction, confidence
5. Output: 3-8 themes with representative issues

Useful for ideation and prioritization.

#### 2.1.3 Learnings Researcher (Enhanced Search)

Specialized search agent for Cassy memories:
1. Extract keywords from feature/task
2. Category-based narrowing (performance -> performance memories)
3. Content-search pre-filter with parallel queries
4. Read frontmatter only of candidates
5. Score and rank
6. Full read of relevant matches only
7. Return distilled summaries with file path, relevance, key insight

---

### 2.2 Document Review (Plans/Specs)

**What CE does:** Reviews plans and requirements with different personas than code review.

**What Cassy should add:**

Separate document-review skill for EPIC specs with these personas:

| Persona | Focus |
|---------|-------|
| Coherence | Internal consistency, contradictions, terminology drift |
| Feasibility | Architecture reality, whether the plan is implementable |
| Scope Guardian | Scope creep detection, unjustified complexity |
| Adversarial | Challenge assumptions, stress-test decisions, surface alternative blindness |

Cassy's adversarial supervisor already does informal versions of these. Formalizing them as parallel sub-agents with structured output would improve spec quality before workers start.

---

### 2.3 Operational Validation by Default

**What CE does:** Every PR must include `## Post-Deploy Monitoring & Validation` with log queries, metrics to watch, healthy signals, failure triggers, validation window.

**What Cassy should adopt:**

Add to code-reviewer or PR template:
- Log queries to run after deployment
- Metrics to watch
- What "healthy" looks like
- What triggers rollback
- Validation window and owner

Even for no-production-impact changes: "No additional operational monitoring required" + one-line reason. Forces ops thinking into every change.

---

### 2.4 Adversarial Ideation

**What CE does:** `/ce:ideate` generates 30+ raw candidates via 3-4 parallel sub-agents with different thinking frames, then adversarially filters to 5-7 survivors with explicit rejection reasons.

**Key pattern:** Generate many -> critique ALL -> present survivors. Prevents fixation on early ideas.

**Thinking frames:**
1. User/operator pain
2. Inversion/removal/automation
3. Assumption-breaking
4. Leverage and compounding

**What Cassy should adopt:**

A `cas ideate` skill or EPIC planning phase that:
1. Scans codebase for pain points and leverage points
2. Dispatches 3-4 parallel ideation agents with different frames
3. Merges ~30 raw candidates
4. Adversarially filters with explicit rejection criteria (too vague, not grounded, too expensive, already covered)
5. Presents 5-7 survivors ranked by: groundedness, expected value, novelty, pragmatism, leverage

---

### 2.5 Onboarding Skill

**What CE does:** Auto-generates `ONBOARDING.md` with six sections answering questions new contributors ask in hour one.

**What Cassy should add:**

`cas onboard` or `/onboard` skill that generates:
1. **What Is This?** -- Purpose, problem solved, audience
2. **How It's Used** -- User/developer experience
3. **How It's Organized** -- Architecture diagram, directory tree, external dependencies
4. **Key Concepts** -- Domain terms + architectural abstractions
5. **Primary Flows** -- 1-3 entry paths with diagrams referencing specific files
6. **Developer Guide** -- Setup, running, testing, common change patterns

Cassy already has CODEMAP.md. This would complement it as a human-oriented intro vs. CODEMAP's breadcrumb navigation.

---

### 2.6 Bug Reproduction Skill

**What CE does:** Systematic hypothesis-driven debugging.

**What Cassy should add to its debugger agent:**

1. **Understand** -- Extract symptoms, expected behavior, reproduction steps
2. **Hypothesize** -- Form 2-3 theories about root cause BEFORE running anything
3. **Reproduce** -- Tests for logic bugs, browser for UI bugs, manual for environment-specific
4. **Investigate** -- Logs, traces, database state, code path tracing (validate/eliminate hypotheses)
5. **Document** -- Root cause (file:line), verified reproduction steps, evidence, suggested fix

Key principle: hypothesis-first prevents aimless exploration.

---

## Tier 3: Quick Wins

### 3.1 Execution Posture Signals on Tasks

Add to Cassy task metadata:

```
execution_note: test-first | characterization-first | additive-only
```

Workers read this and adapt their approach. `test-first` means write failing test before implementation. `characterization-first` means write tests capturing current behavior before modifying. `additive-only` means new files only, no existing code changes.

---

### 3.2 System-Wide Test Check

After implementing a unit, before marking complete:
1. Trace callbacks, middleware, observers **two levels out** from changed code
2. Verify integration tests use real objects (not mocks) for touched interaction points
3. Unit tests with mocks prove logic in isolation; integration tests with real objects prove layers work together
4. Both needed when touching callbacks, middleware, or error handling

Add as a step in the factory worker execution flow.

---

### 3.3 Simplify-as-you-go

After every 2-3 implementation units in an EPIC:
1. Review completed work for consolidation and reuse
2. Catch pattern accumulation before it becomes debt
3. Cassy already has a `simplify` skill -- trigger it automatically mid-EPIC

---

### 3.4 Fork-Safe Base Branch Resolution

CE's `resolve-base.sh` handles forks gracefully:

Priority order:
1. PR metadata (base repo + branch)
2. `origin/HEAD` symbolic ref
3. `gh repo view defaultBranchRef`
4. Common branch names (main, master, develop, trunk)

Resolves from the correct remote for that branch, not just `origin`. Useful for Cassy's code-reviewer when reviewing PRs from forks.

---

### 3.5 Model Tiering for Cost/Latency

CE uses mid-tier models (Sonnet) for persona sub-agents and the most capable model (Opus) for orchestration.

Cassy could adopt:
- Factory workers on Sonnet for speed/cost on scoped tasks
- Supervisor on Opus for synthesis, intent discovery, finding merge
- Code-reviewer personas on Sonnet; orchestrator on Opus

---

### 3.6 Pre-Existing Issue Separation

In code review, findings in unchanged code unrelated to the diff are marked `pre_existing: true` and separated into their own report section. They don't count toward the verdict.

This distinction matters: "clean PR with pre-existing debt" vs. "PR introduces new bugs" are different situations.

---

### 3.7 Protected Artifact Exclusion

CE protects `docs/brainstorms/`, `docs/plans/`, `docs/solutions/` -- if a reviewer recommends deleting files in these paths, the synthesis step discards that finding.

Cassy equivalent: protect `.claude/`, `docs/`, memory files from reviewer deletion recommendations.

---

### 3.8 Standards Path Filtering

Before spawning the project-standards reviewer, find `CLAUDE.md` and `AGENTS.md` files, then filter to those whose directory is an ancestor of at least one changed file. Pass only relevant standards.

Avoids passing the entire repo's standards and lets the reviewer focus on rules that actually apply.

---

### 3.9 Bounded Fix/Re-Review Cycle

In interactive review mode, cap the fix-then-re-review loop at 2 rounds. Prevents infinite review cycles where each fix introduces new findings.

---

### 3.10 Plan-Driven Requirements Verification

If a plan document exists for the reviewed code:
- Explicit plan match (caller-provided) -> P1 `manual` findings for unaddressed requirements
- Inferred plan match (auto-discovered) -> P3 `advisory` findings (hints, not contracts)

Ties review findings to planned work without blocking on ambiguous matches.

---

## Implementation Priority

Suggested order based on effort vs. impact:

| Priority | Item | Effort | Impact | Status |
|----------|------|--------|--------|--------|
| 1 | 1.1 Multi-persona review pipeline | Large | Very High | **Shipped (Phase 1, cas-3444)** — multi-persona reviewer pipeline on main |
| 2 | 1.5 Review-to-task flow | Small | High | Pending (Phase 2) |
| 3 | 3.1 Execution posture signals | Tiny | Medium | **Shipped (Phase 1, cas-0750)** — `execution_note` in use (test-first / characterization-first / additive-only) |
| 4 | 1.2.1 Structured frontmatter schema | Small | High | **Shipped (Phase 1, cas-2c1c)** — memory schema with typed frontmatter |
| 5 | 1.2.2 Overlap detection | Medium | High | **Shipped (Phase 1, cas-2c1c)** — overlap detection in `cas-core/src/memory/` |
| 6 | 1.4 PR feedback resolution | Medium | High | Pending (Phase 2) |
| 7 | 1.3.2 Implementation unit template | Small | Medium | **Shipped (Phase 1, cas-b4d1)** — template live in supervisor skill |
| 8 | 3.2 System-wide test check | Small | Medium | Pending (Phase 2) |
| 9 | 2.2 Document review for specs | Medium | Medium | Pending (Phase 2) |
| 10 | 2.1.1 Git history analyzer | Small | Medium | **Shipped (Phase 0, cas-ada0)** — `git-history-analyzer` agent |
| 11 | 1.2.4 Memory refresh | Medium | Medium | Pending (Phase 2) |
| 12 | 2.3 Operational validation | Tiny | Medium | Pending (Phase 2) |
| 13 | 3.5 Model tiering | Small | Medium | Pending (Phase 2) |
| 14 | 2.4 Adversarial ideation | Medium | Medium | **Shipped (Phase 0, cas-ada0)** — `cas-ideate` skill |
| 15 | 2.5 Onboarding skill | Small | Low | Pending (Phase 2) |
| 16 | 2.6 Bug reproduction skill | Small | Low | Pending (Phase 2) |
| 17 | 1.3.3 Confidence-check deepening | Medium | Medium | Pending (Phase 2) |
| 18 | 3.3 Simplify-as-you-go | Tiny | Low | Pending (Phase 2) — `simplify` skill exists, auto-trigger mid-EPIC not wired |
| 19 | Remaining Tier 3 items | Tiny each | Low each | Pending (Phase 2) |

**Phase 2 candidates (roughly in priority order):** 1.5 review-to-task flow, 1.4 PR feedback resolution, 3.2 system-wide test check, 2.2 document review for specs, 1.2.4 memory refresh, 2.3 operational validation, 3.5 model tiering.

**Also shipped in Phase 0 (cas-ada0) but not listed as standalone rows above:**
- `cas-brainstorm` skill — interactive Q&A requirements gathering (related to 1.3 Structured Planning Pipeline)
- `issue-intelligence-analyst` agent — token-efficient GitHub issue clustering (2.1.2 Issue Intelligence Analyst)
