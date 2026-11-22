### Fixed — CI attribution

- **Replay delivery tests before the base build** — Lane failure attribution now reruns the recorded original archive or build arguments, filter and shard before building the base into the shared target directory. Base dependencies can no longer hide a deterministic delivery regression during the head replay. Missing replay metadata and empty replays remain unresolved; identical-invocation flakes remain unattributed and keep the lane red.
