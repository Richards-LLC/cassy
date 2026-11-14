## Core Concepts

- **Release preview's read-only boundary** - `cas release preview` writes no task, receipt, operation, sync-queue, artifact-cache or notification data and leaves every existing store file unchanged. It reads through ordinary SQLite read-only connections with normal locking, so SQLite may add its `cas.db-shm` and `cas.db-wal` coordination files when they are absent; preview does not promise that no file is created.
