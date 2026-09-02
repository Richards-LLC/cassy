---
managed_by: cas
---

# Memory Lifecycle and Record Choice

Memory entries are durable rows in Cassy's SQLite entry store. Their Markdown
body and optional YAML frontmatter are carried by the `content` field of the
memory API. Use the API actions to change lifecycle state; do not maintain a
parallel document or index.

## Recent Ordering

`mcp__cas__memory action=recent` orders active entries by
`recent_at desc, id desc`, where `recent_at` is the later of creation and last
update. The response repeats that `ordered_by` value so callers do not infer
ordering from display timestamps. The `limit` request field bounds the result
and defaults to 10.

## Lifecycle Actions

- **Revise**: `update` a continuing fact when its content, tags, or importance
  change. The live update action supports only those three mutable fields;
  title and validity are set when remembering an entry and are not update
  parameters.
- **Archive**: `archive` an entry that should leave normal retrieval. Archiving
  also removes it from the search index.
- **Restore**: `unarchive` an archived entry when it becomes useful again.
- **Time-bound**: set `valid_from` or `valid_until` on `remember` when a fact
  has a known validity window.
- **Review**: `get` records access; `helpful`, `harmful`, and `mark_reviewed`
  record feedback or review state.
- **Tier**: `set_tier` moves an entry among `working`, `cold`, and `archive`;
  `list` can filter by that tier.
- **Delete**: use `delete` only when the entry should be removed entirely and
  no longer needs an audit trail.

Keep one authoritative entry for a continuing fact. Update it rather than
creating a second entry merely because its wording changed. Use the
`related:<slug>` tags returned by moderate overlap detection when two distinct
entries genuinely belong together.

## Team and Personal Scope

`remember` can use an explicit `team_id`. In a team-linked project, the
`personal=true` request field keeps a note personal unless `team_id` is also
set; an explicit team always wins. `list` can filter by `scope`, `team_id`,
`tags`, `tier`, `sort`, and `sort_order`.

## Choose the Authoritative Record

| Need | Use | Why |
| --- | --- | --- |
| An enduring fact, preference, lesson, or local constraint | **Memory** | Persists across sessions and is retrieved as working context |
| Work to assign, sequence, block, verify, or close | **Task** | Carries ownership, dependencies, and lifecycle state |
| A curated project-wide reference page | **Knowledge** | Distilled, navigable documentation rather than working context |
| A normative product, API, or architecture decision | **Spec / ADR** | Provides an approved contract and decision record |

If an item fits more than one row, keep the operational source of truth in the
right system and link or summarize it elsewhere. A task may leave a reusable
memory, and an approved spec may have a knowledge page explaining it.
