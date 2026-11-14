### Fixed — Releases

- **Release preview states its real read-only boundary** — Was: `cas release
  preview` promised no store writes, yet on a copied store without WAL files
  SQLite added a 32 KiB `cas.db-shm` and an empty `cas.db-wal`. Now: the
  command help, source contract and release-inventory docs say preview leaves
  every existing store file unchanged and writes no store data, while SQLite
  may create those coordination files. Locking and approval checks are
  unchanged.
