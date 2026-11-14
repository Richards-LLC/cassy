### Fixed — Storage

- **Faster schema migrations** — New stores and schema upgrades use the existing WAL+NORMAL SQLite policy, avoiding a full disk sync for every migration commit while preserving migration transactions and rollback.
