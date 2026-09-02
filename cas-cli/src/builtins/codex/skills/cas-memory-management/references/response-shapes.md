---
managed_by: cas
---

# `remember` Response Shapes

`mcp__cs__memory action=remember` returns human-readable text plus a tagged
`structured_content` payload. The operation itself can return successfully
while a blocked response sets `is_error: true`; inspect the tagged `status`
field instead of parsing the text.

## Created

Low-overlap and moderate-overlap writes return:

```json
{
  "status": "created",
  "slug": "cas-abcd",
  "related_memories": [],
  "refresh_recommended": false
}
```

`related_memories` contains matching slugs for moderate overlap. The
`refresh_recommended` flag indicates that a candidate reached the
cross-reference cap.

## Blocked

Interactive high-overlap writes return `is_error: true` with:

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

Use `existing_slug` and `recommended_action`; do not retry the same insert.

## Autofix Outcomes

An explicit `mode=autofix` request returns `merged` when the atomic merge
succeeds:

```json
{
  "status": "merged",
  "slug": "cas-xxxx",
  "receipt": {
    "merged_into": "cas-xxxx",
    "expected_updated_at": "2026-01-01T00:00:00Z",
    "updated_at": "2026-01-01T00:00:01Z"
  }
}
```

If `expected_updated_at` is stale, the non-mutating response is:

```json
{
  "status": "conflict",
  "slug": "cas-xxxx",
  "expected_updated_at": "2026-01-01T00:00:00Z",
  "actual_updated_at": "2026-01-01T00:00:02Z"
}
```
