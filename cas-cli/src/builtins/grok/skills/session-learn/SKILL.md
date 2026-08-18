---
name: session-learn
description: Use when asked to save or extract session learnings, or when ending a substantive session with durable concepts, corrections, decisions, gaps, or patterns.
managed_by: cas
---

# session-learn — 7-signal session classifier

Borrowed from `third-brain-v5-skills/skills/session-learn` (MIT, reference_third_brain_v5_skills_borrow_source) and adapted to the Cassy memory schema. The third-brain version writes to a wiki tree; this version writes to the Cassy memory + rule stores via `cas__memory remember` so findings benefit from Cassy's existing dedup, embedding, and recall pipeline.

## When to use

- **Auto-trigger** (default OFF): the `Stop` hook runs this classifier when `[memory] session_learn_auto = true` is set in `.cas/config.toml`. The flag defaults to `false` for v1 because each invocation pays one Haiku call (~1–3 s, ~$0.001) and the user should opt in.
- **Manual** ("extract this session", "save what we learned", "extract knowledge"): the user invokes the skill at any point. Honors the same `<5 tool calls` skip rule unless the user explicitly says "extract even though it's short".
- **Chained from other skills** (third-brain pattern: cognitive-compile, wiki-ingest, deep-research). Cassy does not yet have those skills, but the contract is the same — call this skill with the session's transcript path.

## What you produce

Exactly one batched output: a JSON array of memory drafts, each with the 7-signal classification, the proposed Cassy entry_type / tags / scope, and a confidence score. The caller (Rust handler or interactive user) routes them through `cas__memory action=remember` so the standard overlap-detection gate decides whether each draft lands.

**You do NOT write to the store directly.** Return drafts only; the caller writes. This separation lets the user preview drafts in interactive mode and lets the hook handler apply the overlap gate.

---

## The 7 signals

| # | Signal | What triggers it | Cassy `entry_type` | Typical `tags` | Scope |
|:--|--------|------------------|------------------|----------------|-------|
| 1 | **Concept** | A new domain term the agent learned (e.g., "verification jail", "supervisor-owned review"). | `learning` | `concept`, plus the term itself | `project` (term is project-specific) or `global` (cross-project) |
| 2 | **Entity** | A person, project, tool, repo, or library that came up by name and is worth remembering for future recall. | `context` | `entity`, plus type (`person`/`tool`/`repo`/`library`) | usually `project` |
| 3 | **Correction** | The user pushed back on something the agent did or said, in a way that should bind future behavior. | `preference` | `correction`, plus the topic | usually `global` (corrections often outlive a project) |
| 4 | **Pattern** | A recurring pitfall, gotcha, or "I always forget X" moment. Promote to a rule candidate if it fires twice in the same session. | `learning` | `pattern`, plus the area (`testing`, `git`, `tooling`, …) | `project` if codebase-specific, `global` if tool-general |
| 5 | **Idea** | A proposal the user or agent floated that wasn't acted on but is worth saving. | `context` | `idea`, plus the area | usually `project` |
| 6 | **Decision** | An architectural / process / scope decision with a rationale that should outlive the session. | `context` | `decision`, plus the area | `project` (rare global decisions are fine too) |
| 7 | **Gap** | Something the agent didn't know but should have. Becomes input to a follow-up question or doc-read. | `observation` | `gap`, plus the area, plus the speculative source (e.g., `needs-docs`, `needs-question`) | usually `project` |

**Signal vs. content-type:** signals describe the *epistemic role* of the memory. `entry_type` describes *how Cassy treats it for recall*. The mapping above is the default; the classifier may override on a case-by-case basis when a particular finding clearly fits a different entry_type (e.g., a correction so specific to a codebase pattern that `learning` fits better than `preference`). Record the override in the draft's `notes` field.

---

## Quality rules

- **Skip floor.** If the session has < 5 tool calls, return `[]`. Trivial sessions produce noisy memories. (The Rust hook handler enforces this same floor before invoking the classifier so we never spend a Haiku call on noise.)
- **Dedupe at the source.** Before drafting, scan the existing memory store via `cas__search` for each candidate finding. If a near-duplicate exists, do not draft a new one — instead include the existing memory's ID in your output's `dedup_hits` field so the caller can record the corroboration without creating a duplicate. The overlap-detection gate downstream is a second backstop, not the first one.
- **Confidence honesty.** Each draft must carry `confidence ∈ [0.0, 1.0]`. The Rust handler suppresses drafts with `confidence < 0.6` unless they're tagged `correction` (corrections fire at `≥ 0.5` because user pushbacks are high-signal even when terse).
- **One signal per draft.** A finding that arguably fits two signal types is two drafts, not one merged "mixed" draft — the downstream entry_type routing depends on the signal being unambiguous.
- **No general programming knowledge.** Only emit what was project-, user-, or session-specific. "Always close file handles" is not a memory; "this codebase uses `tracing::warn!` in cloud/syncer/pull.rs but `eprintln!` in cloud/syncer/push.rs" is.

---

## Output schema

A JSON array (possibly empty) of draft objects:

```json
[
  {
    "signal": "correction",
    "entry_type": "preference",
    "scope": "global",
    "tags": ["correction", "scope-discipline"],
    "content": "<the imperative-form memory body>",
    "confidence": 0.85,
    "dedup_hits": [],
    "notes": "<optional, free-form, when the entry_type override or scope choice is non-obvious>"
  }
]
```

`dedup_hits` is `[]` when this is a genuinely new finding. When the classifier found a near-duplicate in the existing store, list the matching memory IDs there and **omit the rest of the body** — the caller treats `dedup_hits` ≠ [] as "no new memory, record corroboration" and the verbose body is wasted bandwidth.

If no drafts, return `[]`. Do not return prose; do not wrap in markdown.

---

## Decision: in-process vs subprocess

**v1 recommendation: in-process.** The Rust SessionStop hook handler embeds this skill's prompt body via `include_str!` and runs Haiku via `traced_prompt` directly. Cost: one Haiku call, ~1–3 s, ~$0.001 per session. The skill markdown serves as both the runtime prompt template and human-readable documentation.

**Escape hatch (v2 if needed): subprocess.** Spawn `claude --resume <session-id> --skill session-learn` so the classifier runs in the *same* harness session whose transcript it is reading. More expensive (full Claude Code boot + tool budget) but produces richer drafts because the model still has working context the transcript may have truncated. Not implemented in v1; revisit if v1 false-negative rate is high.

The choice lives in the hook handler, not in this skill. The skill prompt is the same in either case.

---

## Kill switch

Set in `.cas/config.toml`:

```toml
[memory]
session_learn_auto = false   # v1 default — opt in to enable
```

Manual invocation works regardless of the flag. The flag only gates the auto-trigger from the `Stop` hook.

---

## Worked example

Session-end transcript shows: user corrected me twice on scope discipline, I discovered the cas-code-review skill description was stale (a non-obvious pattern), and we made a design decision to ship cas-ec8f's fix as a 3-commit stack instead of a rebase. Expected output:

```json
[
  {
    "signal": "correction",
    "entry_type": "preference",
    "scope": "global",
    "tags": ["correction", "scope-discipline", "worker-conduct"],
    "content": "When a worker flags a real gap (e.g., a stale test or missing AC coverage), amend the AC and fix in-scope rather than working around it.",
    "confidence": 0.9,
    "dedup_hits": []
  },
  {
    "signal": "pattern",
    "entry_type": "learning",
    "scope": "project",
    "tags": ["pattern", "skills", "managed-by-cas"],
    "content": "Frontmatter `description:` fields in Cassy skill markdown are the first thing the LLM sees in the skills list; if they disagree with the body, the description wins in practice. Pin descriptions in regression tests.",
    "confidence": 0.85,
    "dedup_hits": []
  },
  {
    "signal": "decision",
    "entry_type": "context",
    "scope": "project",
    "tags": ["decision", "git", "epic-merge"],
    "content": "Per supervisor preference: ship multi-commit AC amendments as a stack on the worker branch (don't rebase prior commits); supervisor cherry-pick split handles routing at merge time.",
    "confidence": 0.9,
    "dedup_hits": []
  }
]
```

---

## See also

- `cas-cli/src/builtins/skills/cas-memory-management/SKILL.md` — how memories are stored, recalled, and pruned in Cassy.
- `cas-cli/src/hooks/handlers/handlers_middle/session_stop/stop_flow.rs` — the existing single-bucket `extract_learnings_sync` path that session-learn complements. session-learn is the richer multi-signal successor; the old path remains for legacy `[hooks] generate_summaries = true` users until session_learn_auto becomes the default.
- `third-brain-v5-skills/skills/session-learn/SKILL.md` — upstream pattern. Diffs: third-brain writes to a wiki tree; this skill writes via `cas__memory remember` so findings inherit Cassy dedup and recall.
