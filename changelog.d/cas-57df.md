### Fixed — Worker task queue

- **Refresh an untouched successor's source branch** — Queued work resumed after a block records its fresh target branch and start base before the first commit, including after a release-train move. Prior deliveries, prepared branches and dirty work remain protected.
