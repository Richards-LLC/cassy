## Core Concepts

Schema migrations use a private SQLite connection with the same WAL+NORMAL policy as production stores. Store creation and upgrades retain per-migration transactions and rollback while avoiding FULL-mode disk syncs on every commit.
