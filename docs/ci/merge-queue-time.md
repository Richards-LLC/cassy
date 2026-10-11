# Merge-queue time on a release (cas-12ab)

Finding for the release-latency epic (cas-baa3). Release 3.47.0 recorded
`MERGED_TO_PUBLISHER_SECS=2506`. This note covers what that interval was, how
long the merge queue took, and whether a validated-tree receipt can shorten it.

## What the 2,506 seconds were

The run directory is `~/.cas/artifacts/release/v3.47.0-release-3.47.0`.

| Mark | UTC (2026-10-06) | Source |
| --- | --- | --- |
| Gate green | 07:24:18 | `gate.green.epoch` |
| Pipeline start | 07:24:29 | `pipeline.start.epoch` |
| Queue run 37429588301 created | 07:25:39 | `gh run view` |
| PR #1134 merged | 07:43:54 | `pipeline.merged.epoch` |
| `--publish` invoked | 07:43:54 | `interventions.log` |
| Publisher start | 08:25:40 | `publisher.start.epoch` |
| Tag complete | 08:44:44 | `release.tag-complete.epoch` |
| Published | 08:48:04 | `release-latency.receipt` |

`MERGED_TO_PUBLISHER_SECS` measures from the merge to the publisher start, so
it is not merge-queue time. `--publish` started the moment the PR merged, then
stopped on a publish blocker (`blockers.log`: `publish task=cas-2a16`). The
release binary failed the x86_64 ISA audit, and the fix was a second merge to
main: PR #1135, `fix(release): pin aes 0.9.2 so the x86_64 binary stays
AVX-512 free`. The 41.8 minutes are that hotfix's round trip.

The merge queue itself took 19.4 minutes from pipeline start to merge. The
queue run's critical path was its preflight job:

| Job | Started | Completed |
| --- | --- | --- |
| Fast Validation — preflight (no test suite) | 07:27:18 | 07:42:48 (15.5 min) |
| Fast Validation — suite shard 1/3 | 07:27:41 | 07:32:27 |
| Fast Validation — full suite (fan-in) | 07:32:30 | 07:32:45 |
| macOS Check | 07:26:18 | 07:34:03 |
| Fast Validation (fan-in) | 07:42:51 | 07:43:08 |

## The queue re-proved a tree that was already proven

The full local gate ran on `f084dd7b5`, tree `f1bab7758`. The merge-queue tree
and the landed merge commit `863c42f54` have the same tree, `f1bab7758`. The
queue's 17 minutes re-validated bytes that the release gate had already
validated in full. The PR run had also validated them: it records a
`pr-validated-tree-<tree>` artifact when its lanes pass.

`scripts/check-ci-tree-validation.sh` already reuses that artifact, but only
for main pushes after the merge (`run-heavy=false` on `push` to
`refs/heads/main`). It never applies to a `merge_group` run.

## Can a validated-tree receipt be reused in the queue?

Yes, with the same evidence rule the main-push dedupe uses. A release PR's
queue tree equals its head tree whenever the PR is up to date with main. That
holds by construction for `release/<ver>`, because the cut rebases it onto
main. So on `merge_group`, `check-ci-tree-validation.sh` could look up
`pr-validated-tree-<HEAD^{tree}>`. Given a completed, successful
`pull_request` run for that tree, the required contexts could report success
without re-running the preflight and suite shards. That was the upper bound, about 17 of
the queue's 19.4 minutes on 3.47.0. The full gate runs no Commander journeys, so
the implementation below keeps them, and the real saving is smaller.

Constraints for the implementation:

- The queue must still report every required context on the `merge_group`
  ref. A deduplicated job reports success and names the prior run. It never
  skips the context.
- Any missing or ambiguous evidence keeps the full run: an expired artifact,
  an API error, a non-PR run, or a different tree. This mirrors the main-push
  dedupe.
- A queue entry batched behind another PR has a different tree, so it gets no
  receipt and runs in full. This is the case the queue exists for.
- The ISA audit and other publish-time checks stay where they are. 3.47.0's
  real delay was a publish blocker, and no CI receipt removes that.

cas-4cb8 implements this with the release train's own full-gate receipt rather
than the PR run's: a `cas/full-gate` status on the proven SHA, naming its tree.
On a matching queue tree, the suite, doctests and gate-covered preflight
steps are skipped. The Commander journeys (no full-gate row), the compile
checks and `macOS Check` still run. The branch-protection review is section 3 of
`docs/branch-protection/README.md`.

## What cas-12ab changed for factory lanes

- Push-once guidance: the cas-worker skill and the worker contract say to
  commit locally and push once, when parking with the merge request. Before
  this, one worker pushed 6 times in 31 minutes, which started 12 workflow
  runs.
- `.github/workflows/ci.yml` has a workflow-level concurrency group. A newer
  push to the same `factory/*` branch cancels the run it supersedes; every
  other event gets its own group.
- `scripts/release-gate.sh --fast-rows` writes
  `$GIT_COMMON_DIR/cas/fast-rows/<sha>.pass` for a committed tip. A worker's
  park refuses a tip without it wherever the fast rows ship. Tonight a version
  literal (cas-7aa5) and, earlier, cas-ca55's literal each cost a CI round;
  locally the same rows take about a minute.
