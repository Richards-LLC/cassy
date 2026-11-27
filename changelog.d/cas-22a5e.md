### Fixed — Releases

- **Released trains recognize completed artifact-only QA** — An independently approved QA child with portable evidence bound to the frozen release can satisfy its parent's child-completion check without being listed as shipped code. The parent still needs its own acceptance, and missing or stale QA evidence still blocks closeout.
