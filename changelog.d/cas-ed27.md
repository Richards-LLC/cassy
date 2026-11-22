### Fixed — Factory test reliability

- **Remove flaky SQLite latency test** — Deleted only `a_task_sync_mutation_under_a_foreign_sqlite_write_lock_stays_in_budget` after its recorded failure in run 44970901886 and success in run 44957653373 at the same `64ad9d1a5c` commit. Shared helpers, the remaining latency tests and production code are unchanged.
