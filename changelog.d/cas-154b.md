### Fixed — Cloud sync

- **Keep peer task state current during background work** — Entity sync has its own scheduled worker, so long indexing or maintenance no longer delays its next pull. Knowledge and quality telemetry release the entity lease and serialize their own cursors, allowing task updates to arrive on the configured cadence without a manual sync.
