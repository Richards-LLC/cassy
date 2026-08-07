# `main` branch protection — prepared ruleset and decision memo

**Status: PREPARED, NOT APPLIED. Applying this changes live repository settings and is
pippenz's call alone.** Nothing in this directory has been POSTed to GitHub. The only API
calls made while preparing it were read-only (`GET /rulesets`, `GET /branches/main/protection`).

Prepared for GH #142 (cas-f650).

## 1. The problem this addresses

Verified 2026-08-07 against `pippenz/cas`:

| Check | Result |
| --- | --- |
| `GET repos/pippenz/cas/rulesets` | `[]` — no rulesets exist |
| `GET repos/pippenz/cas/branches/main/protection` | `404 Branch not protected` |

So CI on `main` is **purely advisory today**. That is not theoretical: a red Fast Validation
rode the v2.46.0 release commit onto `main` and nothing stopped it or flagged it.

## 2. The prepared ruleset

[`main-ruleset.json`](main-ruleset.json) requires two status checks on the default branch,
and additionally blocks branch deletion and force-pushes.

Apply command (**do not run without a decision on §4**):

    gh api --method POST repos/pippenz/cas/rulesets \
      --input docs/branch-protection/main-ruleset.json

Verify afterwards, and roll back if needed:

    gh api repos/pippenz/cas/rulesets
    gh api --method DELETE repos/pippenz/cas/rulesets/<id>

`~DEFAULT_BRANCH` is used instead of a literal `refs/heads/main` so the rule follows the
default branch if it is ever renamed. Substitute `"refs/heads/main"` if you prefer it pinned.

### Validation performed

Dry validation only, as required:

- The document parses as JSON (`jq empty`).
- Field names, nesting and the `rules[].type` values follow the repository-rulesets schema:
  `target: "branch"`, `conditions.ref_name.include/exclude`, and a `required_status_checks`
  rule whose `parameters.required_status_checks[]` entries are `{ "context": ... }` objects.
- Both required contexts were matched **programmatically** against the job `name:` values in
  `.github/workflows/ci.yml`, not eyeballed.

Not validated: acceptance by the live API. That requires a POST, which is out of scope here.

## 3. Which checks belong in the list — and which deliberately do not

`.github/workflows/ci.yml` defines three jobs. Their triggers decide eligibility, because
**a required check that never reports on a given ref blocks that ref forever.**

| Job (`name:`) | Triggers | Required? |
| --- | --- | --- |
| `Fast Validation` | `pull_request`, push to `main`, schedule, dispatch | **Yes** |
| `macOS Check` | `pull_request`, push to `main`, schedule, dispatch | **Yes** — see below |
| `Release-Profile & Build Guard (compile-only, no test suite)` | `if:` limits it to `schedule` or `refs/heads/main` | **No — must not be required** |

**Release-Profile & Build Guard is excluded, and this is the load-bearing exclusion.** Its
`if: github.event_name == 'schedule' || github.ref == 'refs/heads/main'` means it does not run
on pull requests. Requiring it would leave every PR waiting on a check that can never arrive —
permanently unmergeable. Its own renamed title already says it is compile-only with no test
suite, so it is not the suite-executing gate anyway.

**macOS Check is included**, per the standing "Linux-green ≠ merge-ready" discipline — macOS
breakage has shipped before precisely because Linux was green. Two costs to accept knowingly:
it runs on `macos-26` with a pinned Xcode 26.3 `DEVELOPER_DIR`, so runner or SDK trouble
becomes a merge blocker, and macOS minutes are billed at a higher multiplier. If that proves
too costly, drop it from the JSON — but drop it deliberately, not by forgetting it.

## 4. Factory impact — the decision that actually matters

**This factory pushes merges DIRECTLY to `main`. It does not open PRs.** That is the crux.

A `required_status_checks` rule is evaluated against the commit at the tip of the push, not
only at PR-merge time. A brand-new commit created locally and pushed straight to `main` has no
check runs attached to it yet, so the required checks are "not passing" and **the push is
rejected**. CI cannot rescue this, because CI is triggered *by* the push that was just refused.

That is a genuine deadlock, not a slowdown. Consequences, ref by ref:

| Operation | Effect |
| --- | --- |
| Supervisor pushes a merge commit directly to `main` | **BLOCKED.** This is the current release path and it stops working. |
| Worker pushes `factory/<name>` | Unaffected — the ruleset targets only the default branch. |
| Supervisor pushes/merges an `epic/<slug>` branch | Unaffected — same reason. |
| Release tag push (`v*`, `refs/tags/…`) | Unaffected — this is a **branch** ruleset; tags are a separate target. `release.yml` triggers on tag push and keeps working. |
| Force-push / branch deletion on `main` | Blocked by the `non_fast_forward` and `deletion` rules. Intended. |

Also worth knowing before applying: **repository admins do not bypass rulesets automatically.**
Bypass requires an explicit `bypass_actors` entry. `bypass_actors` is deliberately left `[]`
here, so as written this rule applies to pippenz too.

### Recommendation

**Adopt the ruleset, and move `main` integration to PRs.** The supervisor pushes the epic
branch (unaffected), opens a PR into `main`, CI runs on the PR, and the merge lands only when
Fast Validation and macOS Check are green. This costs one `gh pr create` + one merge per
release cycle and it directly prevents the v2.46.0 failure — a red suite could no longer reach
`main` unnoticed.

Two alternatives, both weaker, recorded so the choice is informed:

1. **Apply the ruleset and add a bypass actor for the release identity.** Keeps direct pushes
   working, but a bypass used routinely is protection in name only — it would not have stopped
   v2.46.0 either.
2. **Do not apply; add a pre-push guard instead.** No settings change and no workflow change,
   but it is advisory again — the exact property that failed. Reasonable only as an interim
   step if the PR flow cannot be adopted now.

If option 1 is chosen, look the actor id up from the live API rather than guessing it; the
`bypass_actors` array is intentionally left empty here rather than filled with an unverified id.

There is a lower-risk trial available if the account tier supports it: `"enforcement":
"evaluate"` logs what *would* have been blocked without blocking anything. Evaluate mode is not
available on every plan, so confirm it applies to this repository before relying on it — if it
is unavailable, the equivalent is to apply with `"enforcement": "disabled"`, inspect, then flip
to `"active"`.

## 5. Check names are pinned

The two context strings in the JSON are exact matches for job `name:` values in
`.github/workflows/ci.yml`. GitHub matches required checks **by name string**. Renaming a job
without updating this file silently makes the required check un-reportable, and every affected
ref becomes unmergeable until it is fixed. GH #138 already renamed these jobs once. See the
"CI check names are pinned" section in [`../../CONTRIBUTING.md`](../../CONTRIBUTING.md).
