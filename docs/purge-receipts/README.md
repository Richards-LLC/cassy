# cas-78c8 / GH #156 — fixture-memory purge receipts

The integration suite wrote five literal fixture strings into the real CAS
stores for months: 994 of 1705 rows at purge time. The leak is fixed by the
`CAS_TEST_PROTECTED_DBS` tripwire in `cas_store::shared_db` (commit 90f94d0d,
merged as 08dcb1f9); these files are the receipt for the one-time cleanup of
what had already landed.

Applied 2026-08-07 by `cas purge-test-fixtures --apply`, exact content
equality against `memory_migration::routing::FIXTURE_CONTENTS` (never `LIKE`).

| file | what it is |
| --- | --- |
| `cas-78c8-dryrun-preapply.txt` | pre-apply dry run: per-string counts + the full db/id/content delete set (994 rows) |
| `cas-78c8-apply-output.txt` | the applied run's own report |
| `cas-78c8-applied-manifest-project.tsv` | 782 rows deleted from `cas-src/.cas/cas.db` |
| `cas-78c8-applied-manifest-global.tsv` | 212 rows deleted from `~/.cas/cas.db` |

Result, verified independently of the tool's own report:

| database | entries before | after | fixture rows after | integrity |
| --- | --- | --- | --- | --- |
| `cas-src/.cas/cas.db` | 1255 | 473 | 0 | ok |
| `~/.cas/cas.db` | 450 | 238 | 0 | ok |

No genuine rows were lost: for both databases, every non-fixture id present in
the pre-delete snapshot is still present in the live database (0 missing).

The pre-delete snapshots are `VACUUM INTO` copies left beside each database as
`cas.db.fixture-purge-backup` — complete, integrity-checked SQLite files.
Restore with `mv <backup> <cas.db>`. They are intentionally NOT committed
(the project snapshot alone is ~390 MB) and can be deleted once the cleanup is
considered settled.
