---
name: cas-memory-management
description: How to store and retrieve persistent memories using CAS. Use for facts, preferences, learnings, and context that should persist across sessions. Trigger when discovering patterns, fixing bugs, resolving config issues, or learning how unfamiliar code works.
managed_by: cas
---

# CAS Memory Management

Store memories proactively — don't wait to be asked. Before creating a new memory, check whether CAS already has one on the same topic (see Overlap Detection below); the cheapest duplicate is the one you never write.

## When to Remember

- After discovering project-specific patterns or conventions
- After fixing non-trivial bugs (capture root cause + solution)
- After learning how unfamiliar code works
- When finding important architectural decisions
- After resolving configuration or setup issues

## Actions

- **Store**: `mcp__cas__memory action=remember title="..." content="..." entry_type=learning` (types: learning, preference, context, observation)
- **Find**: `mcp__cas__search action=search query="..." doc_type=entry`
- **Promote**: `mcp__cas__memory action=helpful id=<id>` — increases priority for future retrieval

**Valid `mcp__cas__memory` actions** (exact list — do not invent others): `remember`, `get`, `list`, `update`, `delete`, `archive`, `unarchive`, `helpful`, `harmful`, `mark_reviewed`, `recent`, `set_tier`, `opinion_reinforce`, `opinion_weaken`, `opinion_contradict`. There is **no `recall` action** — use `get` (by id) or `mcp__cas__search action=search` (by query).

## Host-Scoped Memories

Use a global memory tagged `host:<hostname>` for machine-specific constraints that should follow every project session on the same host: tmpfs mount rules, local disk budgets, staging directories, hardware quirks, or installed-tool constraints. SessionStart injects matching host-tagged global memories into each project on that machine, within the startup context budget.

- Example: `mcp__cas__memory action=remember scope=global tags=host:<hostname>,tmpfs title="Host tmpfs constraint" content="..." entry_type=context`
- Do not tag project-specific facts as host-scoped; keep those in project scope.
- If a constraint applies to multiple machines, create one global memory per hostname tag or use a different cross-project mechanism.

## Two Modes: Legacy and Structured

CAS memories support two frontmatter modes. Both are valid; structured mode is preferred for new memories but never required.

### Legacy mode (backward compatible)

Only the three legacy fields are required:

```yaml
---
name: <short title>
description: <one-line summary used in MEMORY.md index>
type: <user | feedback | project | reference | bugfix | architecture | learning>
---
```

Legacy memories continue to work unchanged. Validators warn on missing structured fields but never hard-fail reads or writes. No migration is forced.

### Structured mode (preferred for new memories)

Structured mode layers required fields on top of the legacy three:

- `track` — `bug` or `knowledge` (determined by `problem_type`)
- `module` — crate or area affected (e.g. `cas-mcp`, `cas-core`, `ghostty_vt_sys`)
- `problem_type` — enum; the value determines the track
- `severity` — `critical` | `high` | `medium` | `low`
- `date` — `YYYY-MM-DD`

Bug-track memories additionally require `symptoms` (1–5 items) and `root_cause` (enum). Knowledge-track has no extra required fields.

Optional for both: `tags` (max 8), `related_modules`, `related_memories`, `resolution_type`, `commit`, `applies_when`.

The full schema — including enum values, track classification, validation rules, and indexed fields — lives in **[references/schema.yaml](references/schema.yaml)**. Read it once before writing structured memories; refer back for enum values.

## Body Templates

Two body templates are shipped, one per track:

- **Bug track** — Problem / Symptoms / What Didn't Work / Solution / Why This Works / Prevention / Related
- **Knowledge track** — Context / Guidance / Why This Matters / When to Apply / Examples / Related

Use the template matching your `problem_type`'s track. If a memory captures both a bug and a general lesson, prefer the **bug** template and place the general guidance in the Prevention section.

Full templates, per-section guidance, and the bug-vs-knowledge decision table: **[references/body-templates.md](references/body-templates.md)**.

## Overlap Detection

Before you create a new memory, run the overlap check. The goal is to catch the case where CAS already has a memory about this problem — silent duplication is the primary way the memory set decays over time.

High-level workflow:

1. **Extract key terms** from the new memory — prefer reference symbols (file paths, function names, commit SHAs), then symptom/error strings, then title tokens.
2. **Search existing memories** via `mcp__cas__search action=search doc_type=entry` using those terms. Take the top 3–5 candidates. If the new memory has a `module` field, prefer same-module candidates.
3. **Score each candidate 0–5** across five dimensions: problem statement, root cause, solution approach, referenced files, tags. Subtract 1 for `module` mismatch and 1 for `track` mismatch (floor at 0).
4. **Act on the highest score:**
   - **4–5 (high overlap)** — do not create. Update the existing memory in place (autofix/headless) or surface the match for user decision (interactive).
   - **2–3 (moderate overlap)** — create with bidirectional cross-references. Add the matched slug(s) to the new memory's `related_memories`; append the new slug to the existing memory's `related_memories`. Cap cross-references at 3 per memory.
   - **0–1 (low overlap)** — create normally.

Full workflow — scoring rules, update/cross-reference flows, edge cases (stale candidates, `supersedes`, legacy memories with no `module`), and the interactive vs headless modes — lives in **[references/overlap-detection.md](references/overlap-detection.md)**.

Phase 1 enforces the overlap check in Rust at the `action=remember` entry point (shipped in cas-4721). The check runs automatically on every call; pass `bypass_overlap=true` only for bulk imports and tests.

## `action=remember` response shape

`mcp__cas__memory action=remember` returns a structured response on `CallToolResult.structured_content` in addition to the legacy free-text block. Agents should pattern-match on the tagged `status` field rather than parsing the text. Phase 1 emits two variants (`Created` and `Blocked`); Phase 2 may add additional variants for `mode=autofix` outcomes — the serde-tagged shape makes that backwards-compatible.

### `Created` — the memory was inserted

```json
{
  "status": "created",
  "slug": "cas-abcd",
  "related_memories": [],
  "refresh_recommended": false
}
```

- `related_memories` is empty on a low-overlap insert or populated with the slugs of every cross-referenced match on a moderate-overlap (score 2–3) insert.
- `refresh_recommended` is `true` when at least one of those matches has already hit the 3-link cross-reference cap. When you see this, run a refresh on the module before creating more memories on the same topic.
- `CallToolResult.is_error` is `false`.

### `Blocked` — the insert was rejected (high overlap, score 4–5)

```json
{
  "status": "blocked",
  "reason": "high_overlap",
  "existing_slug": "cas-xxxx",
  "dimension_scores": {
    "problem_statement": 1,
    "root_cause": 1,
    "solution_approach": 1,
    "referenced_files": 1,
    "tags": 0,
    "penalty": 0,
    "net": 4
  },
  "recommended_action": "update_existing",
  "other_high_scoring": ["cas-yyyy"]
}
```

- Do NOT retry the insert. Update the existing memory at `existing_slug` in place (or surface the match to the user, depending on your interaction model).
- `recommended_action` is either `"update_existing"` (headless callers should apply automatically) or `"surface_for_user_decision"` (interactive callers should ask first).
- `other_high_scoring` lists additional slugs that also scored ≥4 — rare, but a signal that the module needs a refresh pass.
- `CallToolResult.is_error` is `true`. The tool call itself returns `Ok`; only the `is_error` flag signals failure so structured_content is always parseable.

### `mode` parameter

- `mode=interactive` (default, or omitted) — the behavior documented above. On a high-overlap match, return `Blocked` so the caller decides.
- `mode=autofix` — reserved for Phase 2. Passing it today returns an explicit `"mode=autofix is reserved for Phase 2"` error rather than silently falling back.

## References

- [`references/schema.yaml`](references/schema.yaml) — canonical frontmatter schema, enum values, validation rules
- [`references/body-templates.md`](references/body-templates.md) — bug + knowledge body templates and guidance
- [`references/overlap-detection.md`](references/overlap-detection.md) — 4-step duplicate prevention workflow
- [`references/lifecycle-and-storage.md`](references/lifecycle-and-storage.md) — recent ordering, lifecycle guidance, and when to use memory, tasks, knowledge, or specs
