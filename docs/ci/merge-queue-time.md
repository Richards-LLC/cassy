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
validated in full. At the time, the PR lanes also validated them and recorded
a `pr-validated-tree-<tree>` artifact. That legacy producer has since been
removed: protected PRs now emit admission contexts, and the canonical queue
tree receives the full validation.

`scripts/check-ci-tree-validation.sh` now delegates to
`scripts/check-ci-merge-queue-validation.sh`. On a main push it accepts the
successful queue's exact-tree `merge-queue-validated-tree-<tree>` receipt
(`run-heavy=false`). Missing, failed or ambiguous queue evidence runs the
checks. It does not accept the removed PR marker or reuse PR admission as
full validation.

## Can a validated-tree receipt be reused in the queue?

Yes, when trusted evidence proves the exact queue tree and covers the checks
being reused. A release PR that is up to date with main can have the same tree
as its queue entry; the validator checks the tree rather than inferring this
from a branch name. The original investigation proposed PR-artifact reuse,
but that is not an active proof source now. PR admission alone cannot skip
queue validation. The implementation below uses a full-gate status naming
the proven tree instead.

The historical upper bound was about 17 of the queue's 19.4 minutes on
3.47.0. The full gate runs no Commander journeys, so the implementation below
keeps them unless separately proven, and the real saving is smaller.

Constraints for the implementation:

- The queue must still report every required context on the `merge_group`
  ref. A deduplicated job reports success and names the prior run. It never
  skips the context.
- Any missing or ambiguous evidence keeps the full run: an expired artifact,
  an API error, an unsuccessful validating workflow, or a different tree. This mirrors the main-push
  dedupe.
- A queue entry batched behind another PR has a different tree, so it gets no
  receipt and runs in full. This is the case the queue exists for.
- The ISA audit and other publish-time checks stay where they are. 3.47.0's
  real delay was a publish blocker, and no CI receipt removes that.

cas-4cb8 implements this with the release train's own full-gate receipt rather
than the PR run's: a `cas/full-gate` status on the proven SHA, naming its tree.
On a matching queue tree, the suite, doctests and gate-covered preflight
steps are skipped. Later changes also cover journeys and preflight compilation
as described below. The November 4 operator decision (cas-04c3) subsequently
removed `macOS Check` from merge-group events and the required set; Release
Prebuild retains pre-publish macOS proof. The branch-protection review is section 3 of
`docs/branch-protection/README.md`.

cas-9f70 closes the journeys gap. Suppose the supervisor's full journey-eval
receipt proves every catalog journey on the exact release tree, and
`journey-receipt.py verify-full` accepts it. The train then also posts
`cas/full-journeys`, and the queue skips the Commander step entirely. Without
that receipt, the queue runs the journeys as before.

## Exact-tree preflight executable reuse (cas-a6df)

The v4.4.0 queue run `42396696687` took 15m51s even though its tree
`ead8df74` matched train run `42396241006`: Check took 4m14s and the
no-default-features build took 8m30s. The queue reused the complete train
receipt for tests, but still paid for these preflight compiles. Full job receipts
also bind runtime selection and runner environment, so the hosted train and
an isolated self-hosted queue cannot always share a job receipt.

Preflight now publishes a stripped `preflight-cli` executable. An identical-tree
queue can restore it from the validated full-gate run without weakening the
job receipt's selection or runner bindings. A matching job receipt restores
its original producer's executable. Restore checks `--version` on the current
runner; unavailable or incompatible artifacts fall back to compilation.
Release-plumbing changes that produce a different tree also compile normally.
The queue still runs CLI smoke and rebuilds/checks web assets after restore.

The workflow-body regression in `scripts/ci_tiers/test-policy.py` exercises
restore, wrong-tree/no-proof refusal, absent artifacts and incompatible binaries,
including a lost artifact after a job-receipt hit. Live queue duration under
eight minutes remains deferred to the next release ledger; local fixture
routing is not a release latency measurement.

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

## Archive dependency cache maintenance

The hosted suite archive and advisory main cache producer share the explicit
`fast-validation-suite-build` cache identity. Their profile is defined once in
`ci.yml`, visible during cache lookup and compilation: dev/test debug info is
zero and stripping removes debug info. The cache action retains compiler,
Cargo manifest/lock, toolchain and configuration fingerprints. It caches
compatible dependencies; queue runs still compile the current workspace and
execute the existing suite. A cache hit is never a test receipt.

After a main push reuses successful queue validation, the advisory producer
restores that same dependency key. An exact hit skips only the priming compile;
a partial hit, miss or absent hit output performs the normal archive build to
seed main's cache. It does not run the suite or upload another archive. The
producer is outside the required validation dependency graph, can fail without
failing validation, and stops after twenty minutes. It is disabled when the
queue uses the isolated self-hosted route. A normal main validation can also
save this key; hosted queue, tag, scheduled and manual runs restore without
saving it to their own refs.

The measured pre-change sample had eight cache misses in twelve queue runs.
The latest cold archive job took 13.87 minutes, including 12.37 minutes in its
build step. These are baseline measurements from the queue speed plan
`art-1b84b905`, not a measured saving from cache priming. Completion evidence
must name a real main-scoped cache entry and a later queue log restoring its
key; until a version containing this change reaches main, that final
cross-ref restoration remains unmeasured.
