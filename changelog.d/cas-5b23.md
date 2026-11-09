### Fixed — Commander

- **Recover command status after failed cancellation** — A refused or timed-out Cancel resumes outcome polling, so a queued Finish can resolve and later supervisor commands become available.
