# Retrieval parity harness

Guards the memory → knowledge migration (cas-b129) against silent knowledge
loss. Capture a baseline of what the **current** system retrieves, migrate,
then replay the same queries and diff.

## Use

```
cas retrieval-parity capture                 # before the migration
cas retrieval-parity replay                  # after; exits 1 on regression
cas retrieval-parity replay --json           # machine-readable report
```

Run from the repo root — the default `--queryset` and `--baseline` paths are
relative to it. `capture` writes `baseline-<machine>.json` here; `replay`
reads the baseline for the machine it runs on, so baselines from different
machines never get compared by accident.

Useful flags: `--rank-tolerance N` (how far a hit may slip before it counts,
default 3; per-case `rank_tolerance` in the query set wins over it),
`--allow-uncovered` on capture (records a knowingly partial baseline — see
Coverage below).

## What counts as a regression

| Situation | Verdict |
|---|---|
| A baseline hit's content is no longer retrievable | **regression** (`missing_hit`) |
| A baseline hit fell more than the rank tolerance | **regression** (`rank_drop`) |
| A channel that worked at capture is unavailable now | **regression** (`channel_lost`) |
| A baseline case is missing from the query set | **regression** (`case_missing`) |
| A hit moved *up*, or new hits appeared | fine — reported, not failed |

The asymmetry is the point: the migration may surface more or rank better; it
may not make previously reachable knowledge unreachable.

## Hits are matched on content, not on id

The migration re-keys entries — legacy `p-YYYY-MM-DD-NNN` ids do not survive
into the knowledge store. An id-keyed baseline would therefore report every
hit as lost and tell us nothing. Each hit is fingerprinted with a SHA-256 of
its content, normalized (lowercased, whitespace collapsed) so that re-wrapping
or reformatting during migration does not read as loss. Ids are still recorded
for tracing a regression back to the legacy row.

## Coverage

`capture` refuses to run unless the query set has a `by_type` case for every
entry type and a `by_tier` case for every tier present in the store. A gap
means the migration could drop everything of that type or tier while the
harness still reported parity. The fix is to widen `queryset.toml`;
`--allow-uncovered` exists for deliberate exceptions and is recorded as such.

## Read-only

Neither mode writes to the memory store or the knowledge store.

- Store reads use a connection opened `SQLITE_OPEN_READ_ONLY` without
  `CREATE`, so writes fail at the SQLite layer and a missing database is an
  error rather than a newly-created empty one. This deliberately bypasses
  `SqliteStore::open`, which takes a read-write connection and runs migrations.
- The search channel will not call `SearchIndex::open` unless a
  schema-compatible index already exists, because that constructor *deletes and
  rebuilds* the index on a field-count mismatch and creates it when absent. If
  the index is missing or stale the case reports `channel_lost`/unavailable
  instead. A parity run must never be the thing that rebuilds the index it is
  measuring.

Enforced by tests: `capture_and_replay_do_not_modify_the_store`,
`the_read_only_connection_rejects_writes`,
`a_missing_search_index_is_never_created_by_a_run`.

## Extending the query set

`queryset.toml` is data, not code, so new cases need no rebuild. Field
documentation is inline at the top of that file. Adding cases after a baseline
exists is safe: they are reported as uncompared notes until the next capture.
Renaming a case id, however, reads as one case deleted and another added.
