---
name: cas-memory-management
description: Use when facts, preferences, learnings, decisions, or context should persist beyond the current session.
managed_by: cas
---

# Cassy Memory Management

Cassy stores durable memory entries in its SQLite-backed entry store. Use the
`cas__memory` tool for entry lifecycle operations, and use
`cas__search` when you need to find a memory by topic. Store useful
project facts proactively, especially after a non-trivial diagnosis or design
decision.

## Valid Actions

The list below is the dispatch order for `cas__memory`; keep it aligned
with the live service.

**Valid `cas__memory` actions** (exact list — do not invent others): `remember`, `get`, `list`, `update`, `delete`, `archive`, `unarchive`, `helpful`, `harmful`, `mark_reviewed`, `recent`, `set_tier`, `opinion_reinforce`, `opinion_weaken`, `opinion_contradict`.

## Common Operations

- **Remember**: `cas__memory action=remember title="..." content="..." entry_type=learning`
- **Find**: `cas__search action=search query="..." doc_type=entry`
- **Read**: `cas__memory action=get id=<entry-id>`
- **List**: `cas__memory action=list scope=project limit=20`
- **Revise**: `cas__memory action=update id=<entry-id> content="..."`
- **Feedback**: use `helpful`, `harmful`, or `mark_reviewed` with `id`.
- **Lifecycle**: use `archive` to remove an entry from normal retrieval and
  `unarchive` to restore it.
- **Recent**: `cas__memory action=recent limit=10`

## Request Fields

These are the fields on the unified `MemoryRequest`. `action` is required;
the other fields are optional and apply to the actions described here.

- `action`: operation name from the valid-actions list above.
- `id`: entry ID for `get`, `update`, `delete`, `archive`, `unarchive`,
  `helpful`, `harmful`, `mark_reviewed`, `set_tier`, and `opinion_*` actions.
- `content`: text for `remember` and `update`; evidence for `opinion_*` actions.
- `entry_type`: one of `learning`, `preference`, `context`, or `observation`
  for `remember` (default: `learning`).
- `tags`: comma-separated tags for `remember`; for `list`, every supplied tag
  must match case-insensitively.
- `title`: optional entry title for `remember`.
- `importance`: `0.0` through `1.0` for `remember` (default: `0.5`).
- `tier`: `working`, `cold`, or `archive` for `set_tier` and `list`.
- `limit`: maximum results for `list` and `recent` (`list` defaults to 20;
  `recent` defaults to 10).
- `scope`: `global`, `project`, or `all` for list filtering; remember defaults
  to project scope.
- `team_id`: team filter for `list`, or explicit team association for
  `remember`.
- `bypass_overlap`: set `true` only for bulk imports or tests that deliberately
  create overlapping entries; the default is `false`.
- `mode`: `interactive` (default) or `autofix` for `remember`. Autofix performs
  an atomic merge of a high-overlap entry.
- `expected_updated_at`: RFC3339 timestamp for a `remember` autofix merge. A
  stale value returns a non-mutating conflict.
- `sort`: `created`, `updated`, `importance`, or `title` for `list`.
- `sort_order`: `asc` or `desc` for `list` (default: `desc`).
- `valid_from`: optional RFC3339 start timestamp for `remember`.
- `valid_until`: optional RFC3339 expiry timestamp for `remember`.
- `personal`: set `true` on `remember` to skip team auto-promotion. An
  explicit `team_id` takes precedence.

## Content and Frontmatter

The `content` field is the authoritative memory body. It may begin with a YAML
frontmatter block followed by Markdown prose. Frontmatter is embedded in the
entry content; it is not a separate document or filesystem record.

The overlap scorer reads `name`, `description`, `module`, `track`, and
`root_cause` from that block. The search index also extracts the filter fields
documented in `cas-search`: `module`, `track`, `problem_type`, `severity`,
`root_cause`, and `date`. Keep frontmatter concise and put explanatory detail
in the body. See [schema.yaml](references/schema.yaml) and
[body-templates.md](references/body-templates.md).

## Overlap Detection

`remember` checks for overlapping entries by default. It scores problem
statement, root cause, solution approach, referenced files, and tags, then
applies module and track mismatch penalties.

- Low overlap creates the new entry.
- Moderate overlap creates the entry and adds bounded `related:<slug>` tags to
  cross-reference the matching entries.
- High overlap returns a structured blocked result in interactive mode. Follow
  its `existing_slug` and `recommended_action` instead of creating a duplicate.
- `mode=autofix` atomically replaces the overlapping entry's content while
  preserving its ID. Supply `expected_updated_at` when coordinating with
  another writer; a conflict does not change stored data.

Use `bypass_overlap=true` only when the caller is intentionally importing or
testing duplicate entries. See [overlap-detection.md](references/overlap-detection.md)
for scoring and cross-reference details. Structured response examples live in
[response-shapes.md](references/response-shapes.md).

## Choosing Memory vs Other Records

- Use a **memory** for an enduring fact, preference, lesson, or local constraint.
- Use a **task** for work that needs ownership, dependencies, verification, or
  closure.
- Use a **knowledge page** for a curated project reference.
- Use a **spec** for an approved product, API, or architecture contract.

Use `update` to revise a continuing fact, `archive` when it should leave normal
retrieval, `unarchive` when it becomes current again, and temporal validity
fields when the fact has a known time window. See
[lifecycle-and-storage.md](references/lifecycle-and-storage.md).
