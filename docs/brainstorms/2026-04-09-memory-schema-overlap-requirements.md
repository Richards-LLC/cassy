---
date: 2026-04-09
topic: memory-schema-overlap-detection
---

# Memory Schema + Overlap Detection

## Problem Frame

Cassy memories today use minimal frontmatter (`name`, `description`, `type`) and accumulate linearly with every bug fix, learning, or context discovery. The search index is keyword-only; there is no structured retrieval (no `module:cas-mcp`, no `severity:critical`). There is no duplicate prevention at creation time. Over weeks, multiple memories about the same problem silently accumulate, drift apart, and eventually contradict each other. The primary operational cost is not writing memories — it's that a future agent searching a topic gets four flavors of the same fact and cannot tell which is canonical.

The cas-6103 EPIC planned a structured memory schema + overlap detection + refresh workflow. An audit (cas-1a2e) confirmed none of it shipped: the `cas-memory-management` skill at `cas-cli/src/builtins/skills/cas-memory-management.md` is 26 lines listing valid actions, and `cas-core` memory store does not parse or validate any structured frontmatter. However, substantial design content exists as orphaned prompt-level specifications at `~/.claude/skills/cas-memory-management/references/` (schema.yaml 202L, body-templates.md 148L, overlap-detection.md 175L, refresh-workflow.md 509L — ~1000 lines total), plus matching task descriptions cas-559d / cas-e332 / cas-b331.

This EPIC salvages that design, decides what ships in Phase 1, and ships the primary value — preventing duplicate memories at creation time — with real Rust enforcement rather than agent self-discipline.

## Requirements

**Schema and Templates**
- **R1.** A canonical structured frontmatter schema is shipped as skill reference content. Fields: legacy (`name`, `description`, `type` — all still required for backward compat), plus new required fields when structured mode is used (`track` — `bug`|`knowledge`; `module`; `problem_type` — enum; `severity` — `critical`|`high`|`medium`|`low`; `date` — YYYY-MM-DD). Bug track additionally requires `symptoms` (array, 1–5) and `root_cause` (enum). Optional fields: `tags` (max 8), `related_modules`, `related_memories`, `resolution_type`, `commit`, `applies_when`.
- **R2.** Two body templates shipped: a **bug track** template (Problem / Symptoms / What Didn't Work / Solution / Why This Works / Prevention) and a **knowledge track** template (Context / Guidance / Why This Matters / When to Apply / Examples). Track choice is determined by `problem_type`.
- **R3.** Legacy memories continue to work without modification. Validators **warn** on missing structured fields but never hard-fail reads or writes. No migration is forced; no memories are rewritten by this EPIC.
- **R4.** The `cas-memory-management` skill is updated from its current 26-line stub to a full multi-file skill at `cas-cli/src/builtins/skills/cas-memory-management/SKILL.md` with `references/` containing the salvaged `schema.yaml`, `body-templates.md`, and `overlap-detection.md`. (`refresh-workflow.md` is deferred — see Scope Boundaries.) Ships following the Phase 0 convention: one `BuiltinFile` entry per file, claude + codex mirrors, `managed_by: cas` frontmatter.

**Overlap Detection (Primary Value)**
- **R5.** Overlap detection runs automatically on every `mcp__cas__memory action=remember` call as a pre-insert gate, implemented in **Rust** (`cas-core` memory store). Not optional on default calls. An opt-out flag (`--no-overlap-check`) is supported for bulk imports and tests.
- **R6.** The check follows the 4-step workflow from the salvaged spec: (1) extract key terms from the new memory's title/description/body/frontmatter — prefer reference symbols, then symptom strings, then title tokens; (2) search existing memories via the BM25 index, take top 3–5 candidates, prefer same-module candidates; (3) score each candidate 0–1 on each of 5 dimensions (problem statement, root cause, solution approach, referenced files, tags) with module-mismatch and track-mismatch each subtracting 1 from the final score; (4) act on the highest score.
- **R7.** Decision thresholds:
  - **Score 4–5 (high overlap)** — block the insert. Return a structured result telling the caller the existing slug, the dimension breakdown, and the recommended action (`update` in place). In autofix/headless mode, the caller should update the existing memory. In interactive mode, surface the match for user decision.
  - **Score 2–3 (moderate overlap)** — insert proceeds, but the new memory is cross-referenced bidirectionally: the new memory's frontmatter `related_memories` includes the matched slug(s), and the existing memory's `related_memories` is appended with the new slug. Cross-reference count capped at 3 — beyond that is a signal to recommend a refresh run.
  - **Score 0–1 (low overlap)** — insert proceeds normally with no links.
- **R8.** Performance budget: the overlap check adds no more than 500ms of latency for a memory store at the 10k-entry scale. Current store is ~50 entries, so the measured budget during Phase 1 implementation is <200ms.
- **R9.** Term extraction and dimension scoring are implemented as pure Rust heuristics — token overlap, exact-match on frontmatter fields, file-path overlap. No embedding model in Phase 1. The orphan design explicitly recommended "start with token overlap and upgrade only if precision suffers," which becomes a Phase 2 decision if recall is insufficient.
- **R10.** Scope of the check is **per-project only**. `~/.claude/projects/<project>/memory/` entries never cross-reference entries from other projects; search is constrained to the current project's memory set.

**Search Index Extensions**
- **R11.** The `mcp__cas__search` index is extended to parse memory frontmatter and store structured fields (`track`, `module`, `problem_type`, `severity`, `root_cause`, `tags`, `date`) as filterable metadata. Filter query syntax is supported such that agents can issue queries like `module:cas-mcp severity:critical` or `track:bug problem_type:runtime_error`. Exact query grammar is deferred to planning.
- **R12.** Index extension is backwards-compatible. Legacy memories with no structured fields remain searchable by keyword; they simply return zero hits on filter queries. Index backfill of legacy memories happens on the next index rebuild, not as part of this EPIC's migration work.
- **R13.** The existing BM25 path that overlap detection depends on must return candidates scoped by `module` when the new memory has a `module` field set, with same-module candidates ranked higher. This is the one search change that cannot be deferred — overlap detection's candidate selection depends on it.

## Success Criteria

- **Primary:** 2 weeks after shipping, the memory set has measurably fewer near-duplicate memories being created. Validated by running an overlap report on the live memory store — there should be zero score-4+ pairs created post-ship by normal agent workflow.
- Overlap detection runs in under 500ms for the current store size, does not noticeably slow down factory worker sessions.
- Structured search queries (`module:cas-mcp severity:critical`) return correct filtered results on post-schema memories.
- Legacy memories continue to function without any required changes, and validator warnings are visible but non-blocking.
- The salvaged orphan content (~900 lines) is fully represented in the shipped skill — nothing is silently dropped.

## Scope Boundaries

- **Not shipping:** `cas memory refresh` command (509-line LLM-driven workflow). Deferred to a Phase 2 EPIC with its own brainstorm and plan.
- **Not shipping:** legacy memory backfill / `cas memory migrate` command. Legacy memories stay legacy.
- **Not shipping:** the `Consolidate` / `Replace` / `Delete` outcomes from refresh-workflow.md — those are refresh-phase outcomes, not overlap-detection outcomes.
- **Not shipping:** semantic / embedding-based overlap scoring. Pure token-overlap heuristics only in Phase 1.
- **Not shipping:** cross-project overlap detection or search. Each project's memory store is an island.
- **Not shipping:** a memory lifecycle dashboard, visualizer, or report UI. The skill content documents how agents should interpret scoring internally.
- **Not changing:** the physical location of memory files (`~/.claude/projects/<project>/memory/<slug>.md`). Memories remain user-level, not repo-level.
- **Not changing:** `MEMORY.md` index format. The index is still a flat markdown file; structured metadata lives in per-memory frontmatter.
- **No new memory actions beyond overlap enforcement on `remember`.** No `cas memory overlap-check` standalone action, no `cas memory validate`, no `cas memory rebuild-index`. These are planning-phase considerations if needed.

## Key Decisions

- **Primary value is overlap prevention, not structured retrieval or refresh.** Rationale: "fewer duplicate memories piling up" was the user's success signal. Refresh and structured search are secondary / tertiary even though they appear prominently in the salvaged spec.
- **Rust-enforced pre-insert block for overlap detection** rather than skill-level self-discipline. Rationale: agent discipline fails silently; enforcement at the `action=remember` layer is the only way to actually reduce duplicates over time.
- **Search index extension is in scope.** Rationale: overlap detection depends on same-module candidate selection, so some index work is mandatory; once we're touching the index, shipping full structured filter support is a small incremental cost compared to "ship overlap, revisit search later."
- **Refresh command deferred to Phase 2.** Rationale: primary success signal is prevention, not reactive cleanup. The refresh workflow is ~3x the orphan content of schema+overlap combined — deserves its own EPIC. Overlap detection proactively prevents most of what refresh would reactively clean up.
- **Legacy memories left as-is.** Rationale: backward compat is a stated schema rule; migration is substantial one-time work that doesn't affect the success signal; if legacy becomes a problem later, a Phase 2 `cas memory migrate` command addresses it.
- **Pure heuristic scoring, not embeddings.** Rationale: embeddings add a model dependency, startup cost, and dimension-model staleness. Token overlap is good enough for 50–10k entry stores, which is our current and near-future scale.
- **Bidirectional cross-reference mutation is accepted behavior.** Rationale: the salvaged spec explicitly specifies that moderate-overlap creation mutates existing memories' `related_memories` arrays. This is side-effectful but intentional — the whole point is that the memory set becomes connected.
- **Multi-file skill distribution follows Phase 0 pattern.** Rationale: `cas-brainstorm` / `cas-ideate` proved the approach works. No new BuiltinFile variant required.

## Dependencies / Assumptions

- **Dependency on Phase 0 multi-file skill distribution pattern.** `cas-memory-management` becomes a directory skill like `cas-brainstorm` / `cas-ideate`, following the same `BuiltinFile`-per-file registration convention.
- **Dependency on BM25 search implementation** being callable from the memory `remember` path with a module-scope filter. Needs planning-time verification that the current search can be called this way internally, or gained-during-planning access to extend it.
- **Assumption: the orphan schema.yaml and overlap-detection.md designs are internally consistent.** Salvage-and-polish frame means planning starts from them and only revisits when they conflict with this requirements doc.
- **Assumption: current memory store is small enough (~50 entries) that overlap detection latency is comfortably within budget during Phase 1 implementation.** If the store grows to 1000+ entries before Phase 2, performance should be revalidated.
- **Assumption: `mcp__cas__memory action=remember` has one canonical Rust entry point that can host the pre-insert hook.** Planning should verify.

## Outstanding Questions

### Resolve Before Planning

*(empty — all product-level decisions made in this brainstorm)*

### Deferred to Planning

- **[Affects R5, R6]** [Technical] Where in `cas-core` does the memory store implement `action=remember`? Planning must identify the canonical entry point and confirm it's a single well-defined function that can host the pre-insert hook. Search results pointed at `cas-cli/src/store/markdown.rs` but this needs confirmation — `cas-core` may also be involved.
- **[Affects R6]** [Technical] Term extraction strategy — which Rust tokenizer? Reuse the one in the search-indexing path if possible. Fallback: implement a minimal one (CamelCase / snake_case / path / extension splitting, stop-word list, lowercasing).
- **[Affects R7]** [Technical] Exact shape of the `action=remember` response when blocked by overlap. Suggested shape: return the existing slug, per-dimension score breakdown, and a recommendation enum (`update_existing` | `create_with_cross_ref` | `create_normally`). Planning defines the MCP response contract.
- **[Affects R11]** [Technical] Filter query grammar for structured search — `module:cas-mcp` style vs `--module cas-mcp` CLI-flag style vs JSON payload. Planning should pick one grammar and document it; avoid shipping two.
- **[Affects R11, R12]** [Needs research] Does the current search index parse frontmatter at all, or does it only index body text? If body-only, planning must decide whether to add frontmatter parsing as part of indexing or store parsed frontmatter as a separate per-entry sidecar.
- **[Affects R4]** [Technical] Does `cas-memory-management` already have a non-trivial distribution variant that we'd overwrite? Verify current cas-cli/src/builtins/skills/cas-memory-management.md content and codex mirror — if they're the 26-line stubs I read, the expansion is clean; if a worker recently updated them, planning needs to reconcile.
- **[Affects R8]** [Needs research] Concrete latency measurement plan — planning should include a microbenchmark on a 1k-entry store before declaring the performance requirement met.
- **[Affects R13]** [Technical] If the current BM25 search does not accept a `module` filter in its public API, the overlap detection Rust code either extends the search API or implements candidate selection directly against the memory store. Planning decides which is cheaper.
- **[Affects scope]** [Needs research] Cross-reference the current `mcp__cas__memory` action list — are any existing actions affected by the new schema (e.g., `update`, `set_tier`, `mark_reviewed`)? Planning should inventory and decide whether structured validation applies to them or only to `remember`.

## Next Steps

→ Hand off to planning (cas-supervisor). Plan one Phase 1 EPIC covering structured schema + body templates + overlap detection (Rust pre-insert block) + search index extension, with legacy memories left untouched and `cas memory refresh` explicitly deferred to a Phase 2 EPIC.

Implementation should start from the salvaged orphan content at `~/.claude/skills/cas-memory-management/references/` (schema.yaml, body-templates.md, overlap-detection.md), reconciled against this requirements document. Include the salvaged files in the shipped skill verbatim where possible. The Rust overlap detection code should implement the 4-step workflow from overlap-detection.md, not re-design it.
