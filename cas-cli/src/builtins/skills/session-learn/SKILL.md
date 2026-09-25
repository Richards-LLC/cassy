---
name: session-learn
description: Use when asked to extract or save session learnings; classifies the session into concept, entity, correction, pattern, idea, decision, and gap drafts, then hands each accepted draft to cas-memory-management.
metadata:
  managed_by: cas
---

# session-learn — 7-signal session classifier

This skill is adapted from `third-brain-v5-skills/skills/session-learn` (MIT). The third-brain version writes to a wiki tree. This version writes to Cassy memory through `memory action=remember`, so findings go through Cassy's dedup, embedding and recall.

## When to use

- **Manual:** the user asks to "extract this session", "save what we learned" or "extract knowledge". Skip sessions with fewer than 5 tool calls unless the user explicitly asks anyway.
- **Automatic:** the `Stop` hook runs the same classification when `[memory] session_learn_auto = true` is set in `.cas/config.toml`. It defaults to `false` because each run costs one model call. The hook does not read this file: it runs in-process with its own single-turn classifier prompt, and it is given the duplicate candidates because it cannot search.

## The 7 signals

| # | Signal | What triggers it | Cassy `entry_type` | Typical `tags` | Scope |
|:--|--------|------------------|------------------|----------------|-------|
| 1 | **Concept** | A new domain term learned in the session (e.g. "verification jail"). | `learning` | `concept`, plus the term | `project`, or `global` if cross-project |
| 2 | **Entity** | A person, project, tool, repo or library worth recalling by name. | `context` | `entity`, plus its type (`person`/`tool`/`repo`/`library`) | usually `project` |
| 3 | **Correction** | The user pushed back in a way that should bind future behaviour. | `preference` | `correction`, plus the topic | usually `global` |
| 4 | **Pattern** | A recurring pitfall or gotcha. If it fires twice in one session, it is a rule candidate. | `learning` | `pattern`, plus the area (`testing`, `git`, …) | `project` if codebase-specific, `global` if tool-general |
| 5 | **Idea** | A proposal floated but not acted on. | `context` | `idea`, plus the area | usually `project` |
| 6 | **Decision** | An architecture, process or scope decision, with its rationale. | `context` | `decision`, plus the area | usually `project` |
| 7 | **Gap** | Something the agent did not know but should have. | `observation` | `gap`, plus the area and a source hint (`needs-docs`, `needs-question`) | usually `project` |

A signal is the memory's *epistemic role*; `entry_type` is how Cassy recalls it. The mapping above is the default. Override it when a finding clearly fits another type, and say why in `notes`.

## Procedure (manual)

1. **Collect candidates.** Read the session and list findings that are project-, user- or session-specific. General programming advice is not a memory.
2. **Dedupe.** For each candidate, run `search action=search query="<the finding>"`. If a near-duplicate exists, do not draft it again; note the existing memory's ID in `dedup_hits`.
3. **Draft.** One signal per draft. Fill every field of the schema below and give an honest `confidence`.
4. **Preview.** Show the drafts to the user and drop the ones they reject.
5. **Store.** For each accepted draft with empty `dedup_hits` and `confidence ≥ 0.6` (≥ 0.5 for corrections), run `memory action=remember content="<content>" entry_type=<entry_type> scope=<scope> tags="<tags>"`. The overlap gate there is the backstop.

**Done when** every accepted draft is stored or deliberately skipped, and you have told the user which memory IDs were created.

## Draft schema

Every draft carries every field, including ones that repeat a known memory:

```json
{
  "signal": "correction",
  "entry_type": "preference",
  "scope": "global",
  "tags": ["correction", "scope-discipline"],
  "content": "When a worker flags a real gap, amend the acceptance criteria instead of working around it.",
  "confidence": 0.85,
  "dedup_hits": [],
  "notes": "optional: why a non-default entry_type or scope"
}
```

## Kill switch

```toml
[memory]
session_learn_auto = false   # default; set true to run on every Stop
```

Manual invocation works regardless of the flag.

## See also

- `cas-memory-management` — how memories are stored, recalled and pruned.
