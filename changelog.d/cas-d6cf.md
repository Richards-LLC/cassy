### Fixed — Factory

- **Protected main stays clean** — Was: a protected-branch refusal without a pre-merge dispatch left an unpublished merge on local main. Now: both paths share guarded rollback and retain the source. Confirmed default-branch queues create or reuse an exact-source PR and enable auto-merge before attempting a local merge.
