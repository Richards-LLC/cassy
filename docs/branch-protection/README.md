# `main` branch protection — live required-set and merge-queue contract

**Status: the matching repository ruleset is live.** This file is the reviewed
configuration source for operator-visible changes. Capture a before/after API dump whenever
the live ruleset changes; do not treat editing this JSON as applying the change.

## 1. Required-set audit

The sole required context is `Fast Validation` (operator decision, November 4, 2026):

| Required context | Unique protection | Steady-state wall | Verdict |
| --- | --- | --- | --- |
| `Fast Validation` | On the canonical `merge_group` tree, its rollup rejects a failed preflight, the full-suite fan-in (and every exhaustive nextest shard), **and doctests**. | Queue receipt 32367317728: 3m38 from merge-group start to the rollup. | Required; a main PR reports a cheap hosted admission context, then the merged tree receives the exhaustive check. |

`Fast Validation — doctests` is intentionally not a separate required context: the required
`Fast Validation` fan-in already requires `fast-validation-docs` to succeed. Its coverage is
therefore retained in the fan-in, not dropped.

## 2. Merge queue

[`main-ruleset.json`](main-ruleset.json) requires `Fast Validation`, blocks branch
deletion and force-pushes, and requires GitHub's merge queue. The queue permits at
least five concurrent builds and exactly one entry per merge group, with a minimum
of one entry and zero fill wait. It uses `ALLGREEN`, so each entry must pass its
required checks. Parallel builds remain enabled; singleton landing keeps a
release's landed tree identical to its prebuilt archives, even when other PRs
are queued (cas-d637). Higher build limits remain valid.

`cas setup --project <dir>` and `cas doctor` read the default branch's active queue
rules and report drift below five parallel builds, away from singleton landing,
or away from `ALLGREEN`. Origin identity and queue health share one GitHub request per invocation, bounded to five seconds.
Projects with no active merge-queue rule do not receive a drift warning. The release preflight uses
the same GraphQL document and prints one warning on drift or unavailable access
(including rate limits); it never fails a release for this advisory check.
The existing PR admission read also verifies the target queue uses singleton landing and `ALLGREEN`; release
admission refuses missing or unsafe settings before enabling auto-merge.
Diagnostics only print a repair command. That command reads the full ruleset,
changes only the queue limits/grouping and preserves higher build limits and other rules.
Neither setup, doctor nor release preflight writes the ruleset automatically.
Repair commands cover repository rulesets; unsupported sources report unavailable.

Schema: [active rules on a ref](https://docs.github.com/en/graphql/reference/git#ref),
[queue rule parameters](https://docs.github.com/en/graphql/reference/pulls#mergequeueparameters).

Apply or update command (capture the response as the after receipt):

    gh api --method PUT repos/Richards-LLC/cassy/rulesets/<id> \
      --input docs/branch-protection/main-ruleset.json

Verify afterwards, and roll back if needed:

    gh api repos/Richards-LLC/cassy/rulesets/<id>
    gh api --method PUT repos/Richards-LLC/cassy/rulesets/<id> --input <before-receipt.json>

`~DEFAULT_BRANCH` is used instead of a literal `refs/heads/main` so the rule follows the
default branch if it is ever renamed. Substitute `"refs/heads/main"` if you prefer it pinned.

The CI workflow must include the `merge_group` trigger and report `Fast Validation`
on the synthetic merged-tree SHA. Main PR contexts are deliberately cheap hosted admissions:
they let auto-merge enter the queue without compiling the PR head and the eventual merged tree
back-to-back. The full Fast Validation checks run on the canonical merged tree
before it lands. Darwin release compilation is proven separately before publication. Without the `merge_group` trigger, entries wait until their status-check timeout
because no required context can report.

### Validation performed

Dry validation only, as required:

- The document parses as JSON (`jq empty`).
- Field names, nesting and the `rules[].type` values follow the repository-rulesets schema:
  `target: "branch"`, `conditions.ref_name.include/exclude`, and a `required_status_checks`
  rule whose `parameters.required_status_checks[]` entries are `{ "context": ... }` objects.
- The required context is matched **programmatically** against job names, and the tier
  contract pins the `merge_group` trigger plus every required fan-in dependency.

Live API application remains operator-visible and must be receipted.

## 3. Which checks belong in the list — and which deliberately do not

These CI jobs illustrate the required-set boundary. Their triggers decide eligibility, because
**a required check that never reports on a given ref blocks that ref forever.**

| Job (`name:`) | Triggers | Required? |
| --- | --- | --- |
| `Fast Validation` | Cheap hosted admission on `pull_request`; exhaustive rollup on `merge_group`, push to `main`, schedule, dispatch | **Yes — the rollup, not the lower full-suite fan-in** |
| `macOS Check` | Informational notice on protected `pull_request`; advisory Darwin/cache lane on push to `main`, tags, schedule, dispatch; never `merge_group` | **No** |
| `Release-Profile & Build Guard (compile-only, no test suite)` | `if:` limits it to `schedule` or `refs/heads/main` | **No — must not be required** |

**Release-Profile & Build Guard is excluded, and this is the load-bearing exclusion.** Its
`if: github.event_name == 'schedule' || github.ref == 'refs/heads/main'` means it does not run
on pull requests. Requiring it would leave every PR waiting on a check that can never arrive —
permanently unmergeable. Its own renamed title already says it is compile-only with no test
suite, so it is not the suite-executing gate anyway.

**macOS Check is excluded** (cas-04c3). PR #1475's queue runs
41851954166, 41855101135 and 41860322468 never acquired their macos-26 runner:
empty runner name, zero steps, and GitHub's annotation
"The job was not acquired by Runner of type hosted even after multiple attempts".
That is hosted-runner non-acquisition, not a compile failure or our stale-archive
watchdog. The operator chose to remove the queue stage, rather than add retries.
The live ruleset already requires only `Fast Validation`; the checked-in template
also reflects the live 30-minute response timeout. No live ruleset write is part
of this code/docs change.

`Release Prebuild` still runs on release `merge_group` entries, including
`build-macos`. Its jobs are neither required contexts nor dependencies of
`Fast Validation`, so its macOS non-acquisition (also observed in runs
41855101589 and 41860322470) cannot eject a green queue entry. GitHub removes
entries for [failed required checks](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/managing-a-merge-queue#failing-ci);
Prebuild is outside that required dependency graph.
The release workflow adopts successful artifacts or calls the same prebuild
as a fallback, then requires both packaged-asset audits before publication.
See the [release-flow map](../ci/release-fast-publication.md).

## 4. Factory flow

Factory branches continue to push normally. Integration to `main` is a pull request, and its
queue entry replaces supervisor polling: after review, enable auto-merge with the queue's
configured `MERGE` method. GitHub validates the synthetic merged tree, then lands it or reports
the failing required context. Record the PR URL and queue entry; do not retry manual merging.

| Operation | Effect |
| --- | --- |
| Worker pushes `factory/<name>` | Unaffected — the ruleset targets only the default branch. |
| Supervisor pushes/merges an `epic/<slug>` branch | Unaffected — same reason. |
| `main` PR after review | Enable auto-merge; GitHub queues and validates the merged tree before landing. |
| Release tag push (`v*`, `refs/tags/…`) | Unaffected — this is a **branch** ruleset; tags are a separate target. `release.yml` triggers on tag push and keeps working. |
| Force-push / branch deletion on `main` | Blocked by the `non_fast_forward` and `deletion` rules. Intended. |

**Repository admins do not bypass rulesets automatically.** Bypass requires an explicit
`bypass_actors` entry. `bypass_actors` is deliberately left `[]`, so this rule applies to
repository admins as well.

### Queue ejections: wake, diagnose, re-enqueue once (cas-e369)

The factory daemon's CI watcher tracks two kinds of PR through the queue: parked worker
deliveries targeting `main`, and the PR of each open epic's `epic/` branch. It polls GitHub
every 30 s while a tracked PR is queued, otherwise every 60 s. When a tracked PR leaves the
queue unmerged, the next poll sends the supervisor a typed `merge_queue_ejected` wake. On
2026-10-14 PR #1198 sat ejected for four minutes because the epic PR was not tracked. The
wake names:

- the failed `merge_group` run, its failing job and step, and the error line. The error line
  is an infrastructure stop, a failed Rust test, a failed Playwright test (`✘`), or else the
  last `##[error]` line;
- the failure scope: `outside-diff`, `touches-diff` or `unclassified`;
- the recovery decision.

A failure is **outside the diff** in two cases:

- An infrastructure stop: a memory-admission stop, a runner that lost contact or shut down,
  or a full disk. A memory-admission stop is in the diff when the PR edits
  `scripts/host_memory.py` or `scripts/worker-memory.py`.
- A Commander step failure on a PR whose diff touches none of `hub-web/`,
  `docs/qa/journeys.md`, `docs/design/design-tokens.json` or `scripts/visual-qa.mjs`.

An unreadable diff is never outside. An outside-diff failure whose run finished within the
last 15 minutes is re-enqueued **once**, automatically, with `enqueuePullRequest` bound to
the ejected head. A second failure on that same head escalates instead of looping. So does
any `touches-diff` or `unclassified` failure, and the supervisor diagnoses it. A new push
(new head) starts a fresh one-retry budget.

Each decision is recorded once per failed run in the epic's notes, as a ✅ DECISION line
ending `(merge-queue-recovery pr=#N head=<sha> run=<id> decision=<auto-requeued|escalated|requeue-failed|supervisor-decides>)`.
That line is also the ledger, so repeated polls and daemon restarts never re-enqueue twice.
A parked delivery records the decision in its parent epic's notes, and in its own notes
when it has no epic.

Tests in `cas-cli/src/ui/factory/daemon/runtime/merge_queue_recovery_tests.rs` replay the
real ejections of PR #1198. The HUB-J4 flake is re-enqueued once, and the memory-admission
stop that followed escalates.

## 5. Check names are pinned

The context string in the JSON is an exact match for the job `name:` value in
`.github/workflows/full-validation.yml` (train/queue) or the reusable `.github/workflows/ci.yml` (other events). GitHub matches required checks **by name string**. Renaming a job
without updating this file silently makes the required check un-reportable, and every affected
ref becomes unmergeable until it is fixed. GH #138 already renamed these jobs once. See the
"CI check names are pinned" section in [`../../CONTRIBUTING.md`](../../CONTRIBUTING.md).

## 3. One full validation graph for train and queue

`.github/workflows/full-validation.yml` handles release-train pushes and
`merge_group` through the same reusable `.github/workflows/ci.yml`. Its outer
`Fast Validation` job preserves the required context. A newer train push cancels
its predecessor. Both events select Commander journeys against the last release,
run every registered script/CI-tier test, Commander vitest/typecheck/build,
preflight, doctests and the complete Rust archive/shards. Quarantine and one
rerun, including durable flake publication, apply equally to train and queue.

The full-set fan-in emits `train-full-gate/receipt.json` with schema 2, exact
source tree, main baseline, required/actual job outcomes and elapsed seconds.
`cas/full-gate` is posted only after that fan-in succeeds. Consumers also require
a successful completed GitHub workflow and its `Full-set gate` job; a status
alone, legacy Rust-only artifact or missing job cannot authorize a cut or reuse.
`scripts/release.sh` checks this receipt for a train’s starting tree before preparing
release metadata. An explicit non-train hotfix keeps its emergency path, discloses
that it has no train receipt, and receives the complete queue graph without reuse. An unchanged prepared retry retains the original starting tip.

### Queue reuse

`scripts/check-ci-merge-queue-validation.sh` delegates to the same receipt
consumer in `scripts/train-ci.py`. The PR head must have the queue tree. A
completed green source must be its ancestor, and its main baseline must equal
the queue event's base SHA. The entire remaining diff may contain only version
fields, matching lock versions, CHANGELOG/fragments, CODEMAP, builtin reference
history and release notes. A version-generated `hub-web/dist` delta is allowed
only with a version bump and is rebuilt and checked in the queue compile step.
Any product change, moved main, partial/legacy proof or uncertainty runs the full
graph. A release-plumbing continuation can reuse the source's earlier receipt.

On reuse, preflight first restores `preflight-cli` from the validated full-gate
run only when its receipt tree exactly matches the queue tree. A matching
per-job receipt can also restore its original producer's CLI. The producer
uploads a stripped copy of its no-default-features executable; restore checks
that it runs on the current runner before skipping Rust setup, check and build.
Missing artifacts, lookup failures, incompatible executables and release-plumbing
tree changes keep source compilation. A job receipt is reusable only while its
CLI artifact remains available. Every full-gate reuse still runs CLI smoke and
the web asset rebuild/drift check, including when the job receipt also matches.
CI tiers, web tests, journeys, Rust suite and doctests are already covered by
the full-set source proof and do not run again. The target is under eight
minutes; the live release ledger records the measured result rather than assuming it. Train and queue wall clocks are
measured from the GitHub run start through the required fan-in, and the daemon
records them in the friction ledger once per train/run attempt.

### macOS Check latency: a warm cache from main

This section previously described a required Darwin check on each queue entry.
That policy was superseded by the November 4 decision above: `macos-check` is
now advisory and excluded from `merge_group`, including full-gate reuse.

Its existing non-queue behavior is retained: protected PRs get a cheap hosted
notice; main pushes maintain Darwin dependency caches; tag pushes, schedules
and dispatches retain their compile checks. Only main saves the macOS cargo
cache. On a queue-proven main tree, an exact dependency-cache hit skips Cargo;
a miss compiles to seed it. This maintenance lane does not prove every landed
source change on Darwin. `Release Prebuild/build-macos` is the pre-publish
macOS proof and uses its own release profile/cache.

The original cold-cache measurements (PR #1191: Linux about 8m20s, Darwin
16m16s) explain that maintenance lane, but no longer describe queue latency.
`scripts/ci_tiers/test-policy.py` checks both main-cache behavior and exclusion
of ordinary and full-gate-reuse queue events.

### Receipt implementation and evidence

The producer and consumer live in `scripts/train-ci.py`. Workflow graph parity,
partial/legacy/wrong-tree refusal, plumbing reuse versus main movement, and the
actual release entry-point refusal are exercised in the registered policy,
train-CI, flake and release tests. `train_ci::tests` exercises measured durations
through the real friction store. Live train and queue proofs are recorded in the
release ledger after this source change lands.
