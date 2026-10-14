# `main` branch protection — live required-set and merge-queue contract

**Status: the matching repository ruleset is live.** This file is the reviewed
configuration source for operator-visible changes. Capture a before/after API dump whenever
the live ruleset changes; do not treat editing this JSON as applying the change.

## 1. Required-set audit

The two required contexts are deliberately minimal, but neither removes coverage:

| Required context | Unique protection | Steady-state wall | Verdict |
| --- | --- | --- | --- |
| `Fast Validation` | On the canonical `merge_group` tree, its rollup rejects a failed preflight, the full-suite fan-in (and every exhaustive nextest shard), **and doctests**. | Queue receipt 32367317728: 3m38 from merge-group start to the rollup. | Required; a main PR reports a cheap hosted admission context, then the merged tree receives the exhaustive check. |
| `macOS Check` | On the canonical `merge_group` tree, Darwin/Xcode/SDK compilation that Linux does not exercise. | Queue receipt 32367317728: 4m43 before removing the duplicate no-MCP-proxy check. | Required; a main PR reports a cheap hosted admission context, then the merged tree receives the full Darwin compile. |

`Fast Validation — doctests` is intentionally not a separate required context: the required
`Fast Validation` fan-in already requires `fast-validation-docs` to succeed. Its coverage is
therefore retained in the fan-in, not dropped.

## 2. Merge queue

[`main-ruleset.json`](main-ruleset.json) requires the two contexts above, blocks branch
deletion and force-pushes, and requires GitHub's merge queue. The queue is single-entry
(`min/max entries to merge = 1`, zero fill wait, one concurrent build) so it supplies a
merged-tree revalidation without batching a second PR onto the headline latency path. It uses
`ALLGREEN`, so each entry in any future group must pass its required checks.

Apply or update command (capture the response as the after receipt):

    gh api --method PUT repos/Richards-LLC/cassy/rulesets/<id> \
      --input docs/branch-protection/main-ruleset.json

Verify afterwards, and roll back if needed:

    gh api repos/Richards-LLC/cassy/rulesets/<id>
    gh api --method PUT repos/Richards-LLC/cassy/rulesets/<id> --input <before-receipt.json>

`~DEFAULT_BRANCH` is used instead of a literal `refs/heads/main` so the rule follows the
default branch if it is ever renamed. Substitute `"refs/heads/main"` if you prefer it pinned.

The CI workflow must include the `merge_group` trigger and make both required contexts report
on the synthetic merged-tree SHA. Main PR contexts are deliberately cheap hosted admissions:
they let auto-merge enter the queue without compiling the PR head and the eventual merged tree
back-to-back. The full Fast Validation and Darwin checks run only on the canonical merged tree
before it lands. Without the `merge_group` trigger, entries wait until their status-check timeout
because no required context can report.

### Validation performed

Dry validation only, as required:

- The document parses as JSON (`jq empty`).
- Field names, nesting and the `rules[].type` values follow the repository-rulesets schema:
  `target: "branch"`, `conditions.ref_name.include/exclude`, and a `required_status_checks`
  rule whose `parameters.required_status_checks[]` entries are `{ "context": ... }` objects.
- Both required contexts are matched **programmatically** against job names, and the tier
  contract pins the `merge_group` trigger plus every required fan-in dependency.

Live API application remains operator-visible and must be receipted.

## 3. Which checks belong in the list — and which deliberately do not

`.github/workflows/ci.yml` defines three jobs. Their triggers decide eligibility, because
**a required check that never reports on a given ref blocks that ref forever.**

| Job (`name:`) | Triggers | Required? |
| --- | --- | --- |
| `Fast Validation` | Cheap hosted admission on `pull_request`; exhaustive rollup on `merge_group`, push to `main`, schedule, dispatch | **Yes — the rollup, not the lower full-suite fan-in** |
| `macOS Check` | Cheap hosted admission on `pull_request`; Darwin compile on `merge_group`, push to `main`, schedule, dispatch | **Yes** — see below |
| `Release-Profile & Build Guard (compile-only, no test suite)` | `if:` limits it to `schedule` or `refs/heads/main` | **No — must not be required** |

**Release-Profile & Build Guard is excluded, and this is the load-bearing exclusion.** Its
`if: github.event_name == 'schedule' || github.ref == 'refs/heads/main'` means it does not run
on pull requests. Requiring it would leave every PR waiting on a check that can never arrive —
permanently unmergeable. Its own renamed title already says it is compile-only with no test
suite, so it is not the suite-executing gate anyway.

**macOS Check is included**, per the standing "Linux-green ≠ merge-ready" discipline — macOS
breakage has shipped before precisely because Linux was green. Its full compile runs on
`macos-26` with a pinned Xcode 26.3 `DEVELOPER_DIR` against the canonical merge-queue tree, so
runner or SDK trouble remains a merge blocker. The PR admission job is hosted and intentionally
does not claim Darwin coverage; it exists only to admit the PR to the tree that does provide it.

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

The two context strings in the JSON are exact matches for job `name:` values in
`.github/workflows/ci.yml`. GitHub matches required checks **by name string**. Renaming a job
without updating this file silently makes the required check un-reportable, and every affected
ref becomes unmergeable until it is fixed. GH #138 already renamed these jobs once. See the
"CI check names are pinned" section in [`../../CONTRIBUTING.md`](../../CONTRIBUTING.md).

## 3. Merge-queue full-gate reuse (cas-4cb8)

Release 3.47.0's queue entry spent 15.5 of its 19.4 minutes in the preflight
job (`docs/ci/merge-queue-time.md`). Commander web took 587 s of that, the
release publication guards 260 s, and the compile checks 72 s. The suite
shards ran in parallel. All of it re-proved tree `f1bab7758`, which the
release train's full gate had already proven, except for one thing: the
full gate runs no Commander journeys. This section records the review behind
the reuse, which skips only what the full gate proved.

### Required contexts and how each is satisfied

| Required context | On a queue tree with a matching full-gate receipt | Otherwise |
| --- | --- | --- |
| `Fast Validation` | Reports success only if the preflight job succeeds. The rollup's "Report the full-gate receipt reused by this merge-queue run" step requires `PREFLIGHT == success`. The suite build, shards, fan-in and doctest jobs are skipped (`run-fast-validation == 'false'`). The preflight runs, minus the steps the full gate proved. | All lanes run, and the rollup requires each to succeed (unchanged). |
| `macOS Check` | Runs in full on the queue tree. The local gate runs on Linux and cannot prove Darwin: `macos-check` runs when `reuse-source == 'full-gate'`. | Runs in full (unchanged). |

The ruleset is unchanged: the same two contexts are required, and both still
report on the `merge_group` SHA. Every required context still starts at
enqueue, so the 15-minute `check_response_timeout_minutes` is unaffected.

### What the preflight still runs on reuse

| Preflight step | On reuse | Why |
| --- | --- | --- |
| Commander web: typecheck, unit tests, build, visual QA, dist drift | skipped | Full-gate rows `hub-web-tests`, `hub-web-visual-qa`, `hub-web-dist-drift` |
| Commander journeys (`npm run journeys -- --workers=4`) | **runs**, unless a matching `cas/full-journeys` receipt (cas-9f70, below) | The full gate has no journeys row |
| Release publication guards (`make -C cas-cli test-ci-tiers`) | skipped | Full-gate row `ci-script-tests` runs the same target (it includes `test-cas-install.sh`) |
| `cargo check -p cas`, portable ISA audit test, `cargo build -p cas --no-default-features` | **run** | Cheap (72 s on 3.47.0); the no-MCP-proxy build has no full-gate row |
| Installer fixtures, trust-boundary checks, toolchain setup | run | Unchanged, seconds |

The suite shards and doctests are skipped. The full gate's `nextest`,
`workspace-tests` and `doctests` rows proved them on this tree.

The expected saving on a release like 3.47.0 is the 260 s of publication
guards, plus the Commander non-journey work, off the preflight critical path.
The queue then waits on the longer of the journeys and `macOS Check`.
Measuring the real saving takes one live queue run, recorded under the next
release's `release-latency.receipt`. With a matching full-journeys receipt
(cas-9f70, below) the journeys are skipped too, and the queue waits on
`macOS Check` and the compile checks.

### macOS Check latency: a warm cache from main

Once the Commander step skips diffs that leave hub-web alone, `macOS Check`
becomes the queue's slowest required check. PR #1191's Linux path ended at
about 8m20s; its macOS Check ended at 16m16s. The cause was a cold cache, not
the work. A queue ref reads only its own and main's Actions caches, and main
never ran the job, because a deduplicated main push skipped it. On 2026-10-14
every first-attempt queue entry sampled restored "No cache found", with 0 hits
and 698 misses, and took 11m51s–13m54s in `cargo check`. The sample was
PRs #1187, #1189, #1191, #1192 and #1193. The one warm run measured was the PR #1187
retry (cargo cache restored, 90% sccache hits). It took 5m27s in `cargo check`
and 404 s for the whole job. Starting 44 s into the run, the job ends at about
7m30s. That is below the Linux path, so the entry lands in about 8.5 minutes.

| Option | Darwin coverage per queue entry | Expected queue wall | Decision |
| --- | --- | --- | --- |
| (a) Run macOS only for "platform" diffs | Lost for most Rust diffs. A diff classifier cannot see an unchanged `cfg(target_os = "macos")` caller broken by a shared-API change. The only macOS-only queue failure since 2026-09-10 (PR #809) was inside such a block. | ~8m20s | Rejected: unsafe classification |
| (b) Warm the cache from main | Unchanged: every Rust diff is still compiled on Darwin. | ~8.5 min warm; today's ~16 min if the cache is evicted | **Chosen** |
| (c) Move it to release/nightly | Lost per entry; breakage is found after it lands. | ~8m20s | Rejected |

How (b) works (cas-861c0):

- `macos-check` also runs on every push to main, after the merge and off the
  queue's critical path, so main saves the Darwin cargo cache and sccache
  entries.
- `Swatinem/rust-cache` saves only on `refs/heads/main`. Each queue or tag ref
  used to save its own 333 MiB copy that no later entry could read. The
  repository sits at its 10 GB Actions cache cap, so those copies only evicted
  main's cache.
- `scripts/ci_tiers/test-policy.py` evaluates the real job condition for each
  event and the `save-if` expression, with a mutation control.

**Operator note: the required set is unchanged.** `Fast Validation` and
`macOS Check` stay required, and `main-ruleset.json` is untouched, so no live
ruleset update is needed. The cost is one macOS compile per Rust-touching push
to main, about 7–14 runner-minutes. It runs after the merge, so it adds no
merge latency. A red result there flags Darwin drift on main.
To verify after merge, the next queue entry's macOS Check log should show
"Cache restored successfully" and an sccache hit rate above 50%.

### What counts as a receipt

`scripts/check-ci-merge-queue-validation.sh` (the
`fast-validation-main-push-dedupe` job) sets `reuse-source=full-gate` on
`merge_group` only when every one of these holds. If anything is missing or
ambiguous, it runs the full validation:

1. The queue ref is `gh-readonly-queue/<base>/pr-<N>-<sha>`, and PR `<N>`
   resolves to a head SHA.
2. That head's tree is the queue tree (`HEAD^{tree}`), so the PR was up to date
   with `main` and the queue merge changed nothing. A batched or rebased entry
   has a different tree and runs in full.
3. The newest `cas/full-gate` status on that head is `success`, with the
   description exactly `PASS tree=<queue tree>`. A later failing status
   supersedes an earlier pass.

The release train posts that status in `--pipeline`
(`post_full_gate_tree_receipt`). It does so right after pushing the commit
that `gate.full.sha` proved: a full `release-gate.sh` run, not `--only`, on
that exact SHA. The pipeline already refuses a stale or partial gate, so the
status is only ever posted for a tree the full gate passed.

### Full-journeys receipt (cas-9f70)

The Commander step skips the journeys only when the guard also sets
`journeys-reuse=true`. That requires the full-gate receipt above. It also
requires the newest `cas/full-journeys` status on the same head to be
`success`, with the description exactly `PASS tree=<queue tree>`. A missing,
failed, superseded or other-tree status leaves `journeys-reuse=false`, and the
journeys run. A journeys status without a full-gate receipt skips nothing.

The train posts it in `--pipeline` (`post_full_journeys_tree_receipt`), right
after the full-gate status. It needs a receipt from
`scripts/journey-eval.sh <run-dir>/journeys --full`, the supervisor's
assembly run, or a receipt named by `CAS_RELEASE_TRAIN_JOURNEY_RECEIPT`. The
proven revision's `scripts/journey-receipt.py verify-full` judges it. It
refuses anything that is not all of the following:

- a schema-1 `journey-eval` receipt with scope `full`;
- `suite_exit` 0, with no runner errors;
- every result `PASS`, with at least one passed attempt and no failed or
  skipped attempt;
- a head commit whose tree is the release tree, byte for byte;
- a selection that is exactly the full catalog, re-derived by the release
  revision's own `scripts/journeys-for-diff.py --all`.

A refused or missing receipt posts nothing and logs why. The queue then runs
the journeys. A receipt that a full run reused from the background evaluator
(cas-bb5e) at a different tip passes only if that tip has the same full tree.

### Trust

A `cas/full-gate` status can be posted by anyone with write access to the
repository: the same people who can push to `main`'s queue in the first place.
The status is a claim about one tree, checked against the queue tree byte for
byte, so a stale or mistyped claim cannot cover different code. The same holds
for `cas/full-journeys`.

The journeys receipt is produced on the supervisor's host, not on the GitHub
runner. Both run the Chromium build pinned by `@playwright/test`, and that pin
is part of the tree. A host-only difference, such as fonts or CPU speed, could
pass locally and fail on the runner. The full gate already accepts the same
trade for the Rust suite. Any PR whose tree has no receipt still runs the
journeys on the runner.

### Proof

- Fixture proof: `scripts/ci_tiers/executable-contracts.sh` drives the guard
  with a fake `gh`. A matching receipt sets `reuse-source=full-gate`. A tree
  mismatch, a receipt naming another tree, a failed or superseded status,
  another context, no status, an API error, or a queue ref without a PR
  number all run the full validation.
- `scripts/ci_tiers/test-policy.py` pins the workflow on reuse:
  - only the suite and doctests are skipped;
  - the preflight still runs the journeys and compile checks;
  - only the publication guards and the gate-covered Commander steps are
    skipped;
  - the rollup requires the preflight;
  - `macOS Check` runs.
- Dry run against the live API: the guard was pointed at 3.47.0's PR #1134
  (head `f084dd7b5`, tree `f1bab7758`, the queue tree). It resolved the head
  and matched the tree, found no `cas/full-gate` status (none was posted
  then), and kept the full validation. Record:
  `~/.cas/artifacts/<project>/cas-4cb8/dry-run.{log,output}`.
- `scripts/test-release-train.sh` checks that `--pipeline` posts the status for
  the proven SHA and its tree.
- cas-9f70 fixture proof: `executable-contracts.sh` checks that a matching
  `cas/full-journeys` status sets `journeys-reuse=true`. The following leave
  it false, so the journeys run:
  - the full-gate receipt alone;
  - a journeys status for another tree;
  - a failed or superseded journeys status;
  - a journeys status without the full gate;
  - a PR head tree mismatch;
  - an API error.
- `test-policy.py` runs the real Commander step body with stub `npm`/`npx`:
  - both receipts: no journeys and no `npm ci`;
  - the full gate only: the journeys run;
  - a journeys receipt without the full gate: the whole step runs.
- `test-release-train.sh` drives `verify-full` with these receipts:
  - accepted: a valid one, and one whose head is a different commit with
    the same tree;
  - refused: another tree, affected scope, a missing catalog ID, a failed
    row, a non-zero suite exit, runner errors, an unknown head, garbage, and
    a missing file.

  It also checks that `--pipeline` posts `cas/full-journeys` for a verified
  receipt, and refuses and logs a receipt for the parent tree.
