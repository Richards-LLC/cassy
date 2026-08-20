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
