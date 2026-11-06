### Fixed — Factory

- **Keep delivery alerts on their owning machine** — PR-lane failures, merged-delivery timeouts and worker merge wakes use the worker's registered factory session. A daemon stays quiet about foreign workers and leaves their merge receipts retryable for the owning machine.
