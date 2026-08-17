# CI speed spike — evidence and implementation order

## Recommendation

Do **not** independently shard the current Fast Validation job. First land a single-build/test-artifact handoff and measure it on a real PR; then add nextest partitions only if the artifact transfer plus parallel test execution reduces the required critical path below five minutes. In parallel, dedupe main's heavy revalidation by tree hash. Confidence: high on the cache/duplication diagnosis; medium on exact shard savings until an artifact experiment is measured.

## Measured baseline

| Lane / step | Measured wall time | What limits it |
|---|---:|---|
| PR Fast Validation full suite | 7.2–10.7 min | Required critical path; compile plus test binary linking/execution |
| PR macOS Check | 4.2–4.3 min | Parallel, not critical today |
| PR preflight / doctests | ~4 / ~2.7 min | Parallel under Fast Validation |
| Main Panic Isolation (release) | 22.3 min | Release-profile build |
| Main Build Benchmark | 15.5 min | Intentionally cold benchmark build |
| Main Panic Isolation (release-fast) | 12.3 min | Release-profile build |
| Main Compile Guard / Clippy | 5.4 / 2.6 min | Heavy-tier work |
| Release tag → assets | 26 min | Platform/release build pipeline |

Source: supervisor's 2026-08-17 real-run measurements, recorded in `cas-338f` and task `cas-aa27`.

## Cache evidence and build/test split

| Run | Lane | sccache hits / requests | Reading |
|---|---|---:|---|
| 31374209269 | pre-cache-v2 samples | 0/21, 1/50, 3/1248 | Effectively no reuse |
| 31394678090 | Fast Validation seed | 22/51 (43%) | Cold/seed penalty remains material |
| 31395501309 | Fast Validation warm | 47/51 (92%) | Warm compiler cache is effective |
| 31395501309 | Compile Guard / Clippy / macOS | 19/21 (90%), 4/4 (100%), 29/32 (91%) | Reuse works across warm CI lanes |

The cache result is not evidence that independent shards are cheap. `cas-e948` measured that `rust-cache` prunes test executables: even warm, the archive-producing shard pays more than five minutes compiling. Three independent shards therefore turn the present 7–11 minute baseline into three 10+ minute graph builds. The first shard design must build once and hand test executables/target artifacts to test-only partition jobs; benchmark remains cold by design. Local sccache 0.10 also produced 0/45 cross-worktree Rust hits when absolute checkout paths differed, so CI hit rates must not be generalized to factory worktrees.

## Research findings

| Topic | Finding | Source |
|---|---|---|
| Merge queue | GitHub creates a temporary `merge_group` with latest base and queued changes; required checks run again for that group. It improves correctness/throughput, but cannot itself satisfy the five-minute target. | [GitHub merge queue docs](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/merging-a-pull-request-with-a-merge-queue) |
| Nextest | Nextest supports `--partition hash:N/M`, `count`, and `slice`; partition only after a shared build or the compile cost repeats. | [Nextest configuration reference](https://nexte.st/docs/configuration/reference/) |
| Cache semantics | GitHub caches are branch/tag scoped and restored contents are untrusted; do not cache secrets. | [GitHub dependency caching](https://docs.github.com/en/actions/concepts/workflows-and-actions/dependency-caching) |
| rust-cache | `rust-cache` intentionally excludes workspace crates and incremental artifacts; that matches the observed missing test executables and makes it unsuitable as the sole shard handoff. | [rust-cache action](https://github.com/Swatinem/rust-cache) |
| sccache | GHA backend needs `SCCACHE_GHA_ENABLED=on` and action runtime credentials; cache-v2 warm evidence above shows the configured path is worth retaining. | [sccache GHA docs](https://github.com/mozilla/sccache/blob/main/docs/GHA.md) |
| Self-hosted | Private-repo-only, runner groups, minimal secrets, and ephemeral/JIT isolation are the security baseline; persistent 32-core runners improve queue/cache locality but expand blast radius. | [GitHub self-hosted runner security](https://docs.github.com/en/actions/how-tos/manage-runners/self-hosted-runners/add-runners) |

## Ranked eight-measure plan

| Rank | Measure | Expected critical-path saving | Effort / risk | Recommendation |
|---:|---|---:|---|---|
| 1 | One build plus artifact handoff, then test-only nextest partitions | 3–6 min PR (target Fast Validation <5) | Medium; artifact correctness/size | **Keep; first lane** |
| 2 | Tree-hash dedupe of main heavy tier after validated PR merge | 21–23 min main wall on duplicate trees | Medium; must fail closed on unknown hash | **Keep; first lane** |
| 3 | Preserve/tune sccache cache-v2 and publish per-job hit stats | 2–5 min warm PR, protects #1 | Low; cold misses remain | **Keep** |
| 4 | Required-set audit and merge-queue rollout | Queue safety, not raw saving; adds merge-group rerun | Medium; required event coverage | **Keep after <5-min PR proof** |
| 5 | Path filtering for docs/release-only changes | Up to full PR lane when no Rust/CI surface changes | Medium; false-negative risk | **Keep, fail closed** |
| 6 | 32-core private self-hosted runner pilot | 1–4 min queue/cache locality estimate | Medium-high security/operations | **Pilot after artifact proof** |
| 7 | Parallel/thin-LTO release platform builds | 8–14 min of 26-min release estimate | Medium; preserve portable-byte audits | **Keep** |
| 8 | Independent nextest shards without handoff | Negative: +10–20 min runner cost and likely slower wall | Low implementation, high waste | **Drop** |

## First two implementation lanes

1. `cas-e948`: replace independent shard compilation with a single build/test-artifact producer and test-only nextest partitions; emit compile, upload/download, and test durations separately.
2. Main tree-hash dedupe: skip only heavy work that proves it ran on the identical merged tree; retain the pinned tier policy and run on hash uncertainty.

## Threats to validity

Run IDs above are historical CI receipts from the cache-v2 rollout, not a controlled same-commit A/B. The next experiment must record cache restore, compilation, artifact upload/download, and test durations on the same PR. The projected minute savings are estimates derived from the supervisor baseline and are explicitly not promises.

## Provenance

Examined at `origin/main` `2fcc3fce`; CI measurements and run IDs recorded in `cas-338f`, `cas-aa27`, and historical `cas-eb39` notes on 2026-08-17. Exa queries: "GitHub merge queue required checks merge group workflow behavior", "cargo nextest partition sharding CI documentation", "self-hosted runners security private repositories cache tradeoffs", and "GitHub Actions skip duplicate workflows tree hash cache rust-cache sccache".
