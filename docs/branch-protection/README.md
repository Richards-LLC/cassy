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
| Commander journeys (`npm run journeys -- --workers=4`) | **runs** | The full gate has no journeys row |
| Release publication guards (`make -C cas-cli test-ci-tiers`) | skipped | Full-gate row `ci-script-tests` runs the same target (it includes `test-cas-install.sh`) |
| `cargo check -p cas`, portable ISA audit test, `cargo build -p cas --no-default-features` | **run** | Cheap (72 s on 3.47.0); the no-MCP-proxy build has no full-gate row |
| Installer fixtures, trust-boundary checks, toolchain setup | run | Unchanged, seconds |

The suite shards and doctests are skipped. The full gate's `nextest`,
`workspace-tests` and `doctests` rows proved them on this tree.

The expected saving on a release like 3.47.0 is the 260 s of publication
guards, plus the Commander non-journey work, off the preflight critical path.
The queue then waits on the longer of the journeys and `macOS Check`.
Measuring the real saving takes one live queue run, recorded under the next
release's `release-latency.receipt`. A follow-up can also skip the journeys
once the train's receipt carries a full-journeys proof for the same tree.

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

### Trust

A `cas/full-gate` status can be posted by anyone with write access to the
repository: the same people who can push to `main`'s queue in the first place.
The status is a claim about one tree, checked against the queue tree byte for
byte, so a stale or mistyped claim cannot cover different code.

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
