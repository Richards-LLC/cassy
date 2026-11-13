### Fixed — Release pipeline
- **Reduce dependency cache eviction** — Factory, train and queue builds reuse main's compiler cache without writing branch-specific objects into the shared cache quota. Main continues to seed compiler and release dependency caches, whose keys exclude workspace version bumps.
