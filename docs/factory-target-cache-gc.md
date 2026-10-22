# Factory Cargo target-cache capacity

Cassy keeps each factory worktree's Cargo `target/` directory isolated. It does
not set a shared `CARGO_TARGET_DIR`: concurrent branches can build different
features, build scripts, generated sources, and dependency graphs, so sharing
that mutable output tree is not treated as safe without a separate correctness
proof.

Factory startup checks filesystem capacity before spawning workers. At the
configured high watermark it blocks new worker builds and tells the operator to
start supervisor-only (`--workers 0`) for inspection. The same status is exposed
by `factory action=gc_report` as both a readable list and a stable
`TARGET_CACHE_STATUS_JSON=...` record.

The defaults are:

```toml
[factory]
target_cache_high_watermark_percent = 85
target_cache_low_watermark_percent = 75
target_cache_min_idle_secs = 7200
target_cache_retention_count = 1
```

Use `factory action=gc_cleanup force=true dry_run=true` to preview. The
preview names every exact cache path, logical bytes, classification, reason,
and selected reclaim total. Cargo cache deletion requires both `force=true`
and the explicit `dry_run=false`; omitting `dry_run` remains preview-only.

Cleanup is oldest-first and stops selecting caches once their projected bytes
would reach the low watermark. With `factory.scratch_root` set, it probes the
same scratch filesystem as build admission and includes the configured builder
cap's 10 GiB per lane reserve when deciding pressure. The report names that
filesystem and reserve. Each device has its own selection and retention budget. It excludes registered live worktrees, any
worktree observed in the OS process table (cwd, command line, or open target
file), recent writes, and the configured newest-cache retention count. It
rechecks liveness, recency, and containment immediately before each mutation.

Eligible paths are exact, non-symlink `target/` children of the repository root
or a known factory worktree, plus the configured project's exact
`<scratch_root>/<project-key>/targets/<worker>` paths. Scratch cleanup holds
lane, Cargo and durable lifetime locks, validates the owner generation/inodes,
and quarantines beside the target on the same filesystem. The warmest configured
N eligible scratch targets remain retained. Missing-checkout scratch targets and
unmanaged scratch directories are review-only. Known roots include the default and configured
worktree base directories, durable worktree-store paths, and registered worker
clone paths, so external scratch layouts are covered without scanning arbitrary
host directories. Cassy never traverses symlinks and never deletes source, `.git`,
or Cassy databases. Before recursive removal it atomically renames the cache to a
sibling `.cas-target-gc-*` quarantine; an interrupted removal is reported and
safely resumed by a later GC pass.

Unknown-owner caches remain review-only during ordinary GC. After manual review,
name one exact absolute inventory path with
`factory action=gc_cleanup reclaim_unknown_target=/absolute/checkout/target force=true dry_run=false`.
Supported unknown caches are a missing-gitdir checkout's `target/` and the exact
retired `.cas/merge-sweeps/assembly-target`; a parent or pattern never authorizes
reclaim. Omitting either destructive flag previews and prints the refusal reason.

This named operation invokes only the existing target-cache admission and
quarantine pipeline. It deletes no checkout source or artifact evidence, reaps
no process, and cannot combine artifact deletion or prompt expiry. Explicit
consent never changes an unknown cache's ownership. Native process and handle
checks must establish idle; opaque evidence refuses. A positive configured quiet
window, same-user files, unchanged identities, and held build/Cargo/lifetime
locks remain mandatory. Recent writes, symlinks, live users, managed scratch
owners, or overlapping evidence roots preserve the cache with a reason.
