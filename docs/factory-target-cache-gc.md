# Factory Cargo target-cache capacity

CAS keeps each factory worktree's Cargo `target/` directory isolated. It does
not set a shared `CARGO_TARGET_DIR`: concurrent branches can build different
features, build scripts, generated sources, and dependency graphs, so sharing
that mutable output tree is not treated as safe without a separate correctness
proof.

Factory startup checks filesystem capacity before spawning workers. At the
configured high watermark it blocks new worker builds and tells the operator to
start supervisor-only (`--workers 0`) for inspection. The same status is exposed
by `coordination action=gc_report` as both a readable list and a stable
`TARGET_CACHE_STATUS_JSON=...` record.

The defaults are:

```toml
[factory]
target_cache_high_watermark_percent = 85
target_cache_low_watermark_percent = 75
target_cache_min_idle_secs = 3600
target_cache_retention_count = 1
```

Use `coordination action=gc_cleanup force=true dry_run=true` to preview. The
preview names every exact cache path, logical bytes, classification, reason,
and selected reclaim total. Cargo cache deletion requires both `force=true`
and the explicit `dry_run=false`; omitting `dry_run` remains preview-only.

Cleanup is oldest-first and stops selecting caches once their projected bytes
would reach the low watermark. It excludes registered live worktrees, any
worktree observed in the OS process table (cwd, command line, or open target
file), recent writes, and the configured newest-cache retention count. It
rechecks liveness, recency, and containment immediately before each mutation.

Only an exact, non-symlink `target/` child of the repository root or a known
factory worktree is eligible. Known roots include the default and configured
worktree base directories, durable worktree-store paths, and registered worker
clone paths, so external scratch layouts are covered without scanning arbitrary
host directories. CAS never traverses symlinks and never deletes source, `.git`,
or CAS databases. Before recursive removal it atomically renames the cache to a
sibling `.cas-target-gc-*` quarantine; an interrupted removal is reported and
safely resumed by a later GC pass.
