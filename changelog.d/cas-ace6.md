### Fixed — Factory

- **Replacement worker source binding** — A worker spawned onto a departed worker’s task can start and park inherited work under its own branch without a manual rebind, including queued spawns. Parked mismatches can be returned with `request_changes` before supervisor branch approval. Untouched queued successors keep their fresh target start instead of adopting unrelated history inherited in a former worker’s start base.
