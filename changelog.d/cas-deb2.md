### Fixed — Factory

- **Stalled train advancement recovery** — Train merges reclaim expired or orphaned advancement leases, preserve the ordered delivery queue, and accept additional deliveries while an earlier review waits. Recovery records the retired lease and prevents stale drivers from overwriting current queue state.
