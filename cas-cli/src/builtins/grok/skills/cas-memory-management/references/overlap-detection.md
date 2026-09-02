---
managed_by: cas
---

# Memory Overlap Detection

`cas__memory action=remember` runs an overlap check by default before
writing a new SQLite entry. The check is part of the live memory API. It is
best-effort: if the search index cannot be queried, the write proceeds and the
failure is logged.

## Inputs

The checker combines the proposed `content`, optional `title`, and request
`tags`. When the content starts with YAML frontmatter, the overlap reader uses
`name`, `description`, `module`, `track`, and `root_cause` from that block.
The rest of the Markdown body supplies terms and references. Search-index
filters additionally support `problem_type`, `severity`, and `date`; see
[schema.yaml](schema.yaml).

## Scoring

The checker selects a bounded candidate set and scores each candidate from 0 to
5 across these dimensions:

| Dimension | Match signal |
| --- | --- |
| Problem statement | Same problem in the title, description, or body |
| Root cause | Same structured cause or equivalent diagnosed mechanism |
| Solution approach | Same intervention or fix shape |
| Referenced files | Shared central paths or symbols |
| Tags | Shared specific tags |

It subtracts one point for a module mismatch and one for a track mismatch,
with a floor of zero. Score conservatively when a match is uncertain.

## Outcomes

- **0–1, low overlap**: create the proposed entry normally.
- **2–3, moderate overlap**: create the entry and add bidirectional
  `related:<slug>` tags, up to the three-link cap. If a candidate is already at
  the cap, retain the relationship on the new entry and surface the
  `refresh_recommended` signal; consolidate through normal `update` and
  `archive` operations as appropriate.
- **4–5, high overlap**: interactive mode returns a structured blocked result;
  do not create a duplicate. Use its `existing_slug` and
  `recommended_action` to update the existing entry or ask for a decision.

## Autofix and Concurrency

Set `mode=autofix` on `remember` when an authorized headless caller should
merge a high-overlap proposal into the existing entry. The existing entry ID
is preserved while its content, title, tags, `entry_type`, importance, and
validity are replaced by the proposal.

Pass `expected_updated_at` as an RFC3339 timestamp observed from the target
entry when coordinating concurrent writers. If the timestamp is stale, the
operation returns `Conflict` and changes nothing. If omitted, the current
target timestamp is used for the one atomic merge.

## Deliberate Bypass

Set `bypass_overlap=true` only for a bulk import or a test that intentionally
creates duplicate entries. This is a boolean request field, not a command-line
option. Normal memory creation should leave it absent or false.

## Practical Workflow

1. Search for the strongest reference symbol or error text with
   `cas__search action=search ... doc_type=entry`.
2. Let `remember` perform its automatic check; do not duplicate an entry after
   a high-overlap response.
3. For moderate overlap, inspect the returned related slugs and keep the
   cross-reference bounded.
4. For a blocked interactive result, use `update` on the existing entry when
   the new content is a correction or refinement.
5. Use `archive` only when the old entry should leave normal retrieval; use
   `unarchive` if it becomes current again.
