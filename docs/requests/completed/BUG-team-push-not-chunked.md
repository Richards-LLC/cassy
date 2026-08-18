---
from: Petra Stella Cloud team
date: 2026-06-01
priority: P1
status: completed
resolved_by: cas-585c
---

# Team push bundles all entity types into one unbounded request → 413 on the cloud

## Resolution

Fixed in `cas-585c`. Current code still reproduced the reported failure before
the change: `CloudSyncer::push_team` in `cas-cli/src/cloud/syncer/team_push.rs`
was building one combined team-push map for all upsert entity types, serializing
it once, and POSTing it once to `/api/teams/{team_id}/sync/push`, while the
personal push path already chunked with `split_into_sub_batches` in
`cas-cli/src/cloud/syncer/push.rs`.

Team push now mirrors the personal push shape for upserts: it iterates entity
types, splits each entity type into size-limited sub-batches using the shared
splitter, sends one gzip POST per sub-batch, aggregates `SyncResult` counts, and
marks only the rows in the successful or failed sub-batch. Deletes remain on the
existing one-per-entity delete path.

Regression coverage: `team_push_chunks_upserts_by_payload_budget` in
`cas-cli/tests/team_sync_test.rs` fails before the fix by observing the original
single bundled request behavior, then passes after the fix by confirming three
large team entry upserts drain as three bounded requests with correct aggregate
counts and queue cleanup.

`cas cloud sync` consistently fails the team-push leg with `413 Payload too large` once the team queue grows past ~2 MB compressed. Personal push on the same run succeeds because it chunks; team push does not chunk at all.

## Affected version

`cas 1.95.0` (observed in the field on 2026-06-01).

## Symptom

User runs `cas cloud sync` after some offline time. Output excerpt:

```
✓ Push complete
    Entries: 0 inserted, 626 updated
    Tasks:   0 inserted, 1415 updated
    Rules:   0 inserted, 78 updated
    Skills:  0 inserted, 3 updated
    Events:  91 inserted, 9909 updated
    File changes: 22 inserted, 5312 updated
  ⚠ Team push encountered 1 error(s); items re-queued for next sync
    - Team push failed with status 413: {"error":"Payload too large"}
✓ Pull complete
...
✓ Team pull: 626 entries, 1421 tasks, 78 rules, 3 skills (2128 total)
```

Personal push (the first block) and team pull both succeed. Team push always 413s and the items get re-queued — so every subsequent `cas cloud sync` repeats the same failure forever, because the queue never drains and the next bundle is the same size or larger.

## Root cause — confirmed in source

The cloud's body-size guard lives at `petra-stella-cloud/lib/gzip.ts`:

```ts
export const MAX_BODY_SIZE = 10 * 1024 * 1024;       // 10 MB decompressed
export const MAX_COMPRESSED_SIZE = 2 * 1024 * 1024;  // 2 MB compressed
```

It throws `PayloadTooLargeError` (→ HTTP 413 in the route handler). This is below Vercel's 4.5 MB platform body limit on purpose; raising it server-side is a separate cloud-side ticket but does not solve the architectural problem.

The asymmetry between personal and team push:

| | Personal (`cas-cli/src/cloud/syncer/push.rs`) | Team (`cas-cli/src/cloud/syncer/team_push.rs`) |
|---|---|---|
| Per-entity-type requests | **Yes** — one `push_batch` call per entity type | **No** — single bundled `payload` map |
| Sub-batch by byte budget | **Yes** — `split_into_sub_batches(...)` uses `config.max_payload_bytes` | **No** |
| Per-request size | Bounded by `config.max_payload_bytes` | **Unbounded** — grows with queue depth |

Specifically, `team_push.rs:35-101` stuffs all 12 entity types (`entries`, `tasks`, `rules`, `skills`, `sessions`, `verifications`, `events`, `prompts`, `file_changes`, `commit_links`, `agents`, `worktrees`) plus `project_canonical_id` and `client_version` into one `serde_json::Map`, gzips it once at `team_push.rs:151-153`, and POSTs once at `team_push.rs:160-165`. There is no equivalent of `split_into_sub_batches` on this path.

Failure handling is correct (`team_push.rs:229-240` re-enqueues every drained item on failure), so **no data is lost** — but the bundle stays the same size or grows on the next attempt, so the failure is sticky.

## The fix

Make team push mirror personal push:

1. **Iterate entity types instead of bundling.** For each non-empty entity type vector in the grouped result, build a single-type `payload` (just `{ "<type>": [...], "project_canonical_id": ..., "client_*": ... }`), the way `push_batch` already does on the personal path.
2. **Sub-batch by byte budget.** Reuse the existing `split_into_sub_batches` (or factor a shared helper) so each per-type request is capped by `config.max_payload_bytes`. The existing JSON-size accounting and 256-byte overhead heuristic should port over unchanged.
3. **Per-sub-batch POST + retry.** Match the `push_sub_batch` retry/backoff shape from personal push: 3 attempts, `400-499` short-circuits, transport errors retry. On terminal failure, re-enqueue **only the items in the failing sub-batch**, not the entire team queue (current code re-enqueues the whole drained set on the first failure).
4. **Aggregate `SyncResult` counts across sub-batches** the same way personal push does, so the user-facing summary remains a single line.

Deletes (`send_team_deletes`) are already one-per-entity and don't have this problem.

### Pseudo-shape

```rust
// team_push.rs — replace lines ~35-227 with something like:
for entity_type in ALL_TEAM_ENTITY_TYPES {
    let items = grouped.take(entity_type); // entries / tasks / etc.
    if items.is_empty() { continue; }

    for sub_batch in split_into_sub_batches(items, config.max_payload_bytes) {
        let payload = build_team_sub_batch_payload(
            entity_type, sub_batch, &project_id, client_version,
        );
        match push_team_sub_batch(team_id, entity_type, &payload, token) {
            Ok(resp) => result.merge(resp),
            Err(e) => {
                re_enqueue_sub_batch(&sub_batch, team_id);
                last_error = Some(e);
                // keep going for other entity types? or break? — match personal push policy
            }
        }
    }
}
```

The "what to do on partial failure" policy (continue other entity types vs. abort and re-queue everything) should match whatever personal push currently does. Reading `push.rs:45-175` suggests personal push **continues to the next entity type on per-batch failure** — team push should do the same.

## Acceptance criteria

1. **No single team-push request exceeds `config.max_payload_bytes` post-serialization** (pre-gzip).
2. **A backed-up team queue with ~10 MB of total upserts drains cleanly** in N successful smaller requests, with the same final result counts as a single bundled request would have produced if the cap hadn't tripped.
3. **Per-sub-batch failure re-enqueues only the failing sub-batch's items**, not the entire drained team queue.
4. **`SyncResult` totals** (`pushed_entries`, `pushed_tasks`, etc.) sum correctly across sub-batches.
5. **Personal push regression check:** `push.rs` behavior is unchanged (this BUG only touches team push).
6. **No data loss** on transient 5xx or transport errors — same re-enqueue guarantee as today, just at sub-batch granularity.
7. **E2E test against a mock cloud** with `MAX_COMPRESSED_SIZE = 2 MB` (the current production cap) that intentionally enqueues ~5 MB of team upserts: must succeed in multiple requests without 413.

## Demo statement (Definition of Done)

Starting from a Cassy instance with several MB of pending team-scope upserts in the sync queue, `cas cloud sync` completes the team-push leg in multiple smaller HTTP requests, returns "Push complete" with the full sentinel counts, and **no `413 Payload too large` line appears** in the output.

## What the cloud side is doing in parallel

Petra Stella Cloud will raise `MAX_COMPRESSED_SIZE` from 2 MB to ~4 MB (and `MAX_BODY_SIZE` 10 MB → 20 MB) as a short-term safety net for existing CLI builds, staying safely under Vercel's 4.5 MB platform body limit. This is **not** a substitute for the CLI fix — a sufficiently backed-up queue will still blow past 4 MB. Bundling is the architectural defect; chunking is the architectural fix.

## References

- Cloud body-size guard: `petra-stella-cloud/lib/gzip.ts:19-46`
- Team push (current bundled implementation): `cas-cli/src/cloud/syncer/team_push.rs:35-227`
- Personal push (correct chunked reference implementation): `cas-cli/src/cloud/syncer/push.rs:29-175`, `:285-411`
- Vercel serverless function request body limit: 4.5 MB
