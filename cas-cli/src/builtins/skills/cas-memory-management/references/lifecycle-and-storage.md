# Memory lifecycle and storage decisions

## Recent ordering

`memory recent` orders active entries by `recent_at desc, id desc`, where
`recent_at` is the later of creation and last update. The ID tie-break makes
entries with equal timestamps deterministic. The response repeats this exact
`ordered_by` value so callers do not infer ordering from display timestamps.

## Lifecycle without a parallel store

Keep one durable memory entry per continuing fact. Use the existing lifecycle
fields and actions rather than creating a second store or a scheduler:

- **Merge or supersede:** update the surviving entry with the current guidance,
  provenance, and any relevant `related_memories`; archive the replaced entry
  when it should leave normal retrieval. A superseded entry is historical
  context, not a new memory kind.
- **Expire:** set `valid_until` when a fact has a known end date. Keep it
  unarchived while it remains useful as time-bounded context; archive it when
  it should no longer appear in normal retrieval.
- **Revise:** use `update` for changed content, tags, title, importance, or
  validity. `updated_at` then moves the revised entry in `recent`.
- **Restore:** use `unarchive` if an archived entry becomes active again. Do
  not duplicate it merely to make it visible.

## Choose the authoritative record

| Need | Use | Why |
| --- | --- | --- |
| An enduring fact, preference, lesson, or local constraint | **Memory** | Persists across sessions and is retrieved as working context. |
| Work to assign, sequence, block, verify, or close | **Task** | Carries ownership, dependencies, and lifecycle state. |
| A curated project-wide reference page | **Knowledge** | Distilled, navigable documentation rather than personal working context. |
| A normative product, API, or architecture decision with scope and acceptance criteria | **Spec / ADR** | Provides an approved contract and decision record. |

If an item fits more than one row, keep the operational source of truth in the
right system and link or summarize it elsewhere. For example, a task can leave
a memory of its reusable lesson, and an approved spec can have a knowledge page
that explains it.
