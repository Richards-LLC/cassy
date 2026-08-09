# Code-history epic release-readiness gate

**State in one line:** **GO on 2026-08-08** for assembled commit `8ab2804673c811f6553d65b40a7c16e0ae4ed61e`, with one explicit release-scoped exception: the operator waived the unavailable Claude cross-vendor review lane after the supplemental review findings were repaired and every software, migration, privacy, live-runtime, and branch gate passed.

## Overview

| Workstream | Status | Owner | Target | Change since the rejected tip |
| --- | --- | --- | --- | --- |
| Assembled branch | PASS | Release gate | `8ab28046` | `origin/main`, merge base, local epic, and origin epic are pinned; every child has `Unmerged=0` |
| Review disposition | PASS WITH WAIVER | Release operator | This release only | Supplemental review's 2 P1 and 6 P2 findings were repaired; unavailable Claude lane received an explicit operator waiver |
| Prior-release migration | PASS | Migration runtime | schema v225 → v229 | False `m225=BOOTSTRAP` ledger state now reconciles safely and audibly; repeat run applies zero migrations |
| Full software matrix | PASS | Workspace | All crates and targets | One final workspace test run and one all-target check passed on the assembled tip |
| History and code paths | PASS | History/index runtime | Isolated repository | Status, text search, exact-symbol search, provenance, epochs, verdict, code indexing, and reconciliation contracts passed |
| Ambient recall and vectors | PASS | Recall runtime | Isolated provider run | One query vector per event fanned out across Knowledge, History, and Code; no private hit; backlog drained to zero |
| Hook budgets and privacy | PASS | Hook runtime | SessionStart/UserPromptSubmit | SessionStart was 11,089 bytes; UserPromptSubmit emitted zero bytes; neither auto-stored memory or leaked the concrete path |
| Artifact and diff hygiene | PASS | Release gate | `origin/main...8ab28046` | No private paths, credential markers, external report assets, changed-code shortcut markers, whitespace errors, or uncommitted changes |

Status labels are written explicitly so the table remains meaningful without color.

## Changes since the last rejected gate

- **Migration cursor could advance past a lower gap → ordered-prefix truth.** Detection stops at the first missing lower migration and later detection is reconsidered only after lower work succeeds.
- **A released `m225=BOOTSTRAP` row could conceal a missing `commit_links.link_method` column → safe recorded-migration reconciliation.** Recorded migrations with false predicates are replayed only when their statements are provably additive or `IF NOT EXISTS`; prior ledger evidence is retained in `cas_migration_reconciliations`.
- **Missing knowledge attribution could survive an advertised v229 ledger → final schema convergence.** The exact released v225 database now gains `origin` and `origin_project_id`, and production knowledge reads succeed.
- **SessionStart protected context exceeded 12 KiB → 1,199-byte margin.** Required worker lifecycle guidance remains present at 11,089 bytes in the production-shaped reproduction.
- **Eight supplemental review findings → bounded repairs.** Ambient latency/corpus bounds, executable epoch honesty, reconciliation/watcher/vector races, lag aging, exact-symbol priority, path sanitation, and the verification fixture are all ancestors of the final tip.

## Gate evidence

### Branch and review

| Evidence | Result | Source |
| --- | ---: | --- |
| Final local HEAD equals origin epic | PASS | Both `8ab2804673c811f6553d65b40a7c16e0ae4ed61e` after a fresh fetch |
| Current main is the merge base and an ancestor | PASS | `8f6b557db7c3b994edfe23efa643dda1760e2bc0` |
| Child factory branches stranded | **0** | Fresh `epic_status` for `cas-6212`; all 27 child lanes show `Unmerged=0` |
| Unresolved supplemental P0/P1 findings | **0** | All bounded repair commits are ancestors of `8ab28046` |
| Claude cross-vendor review | WAIVED | Explicit operator decision recorded on the release gate and epic; applies only to this release |

### Exact released-v225 migration

The fixture was created by the exact released binary `cas 2.53.0 (93e139d)`, not by synthetic current-schema SQL. Before upgrade it contained 173 migration rows with maximum ID 225, recorded m225 as `BOOTSTRAP`, lacked `commit_links.link_method`, and lacked both knowledge attribution columns. Legacy knowledge and commit-link rows were added before opening it with the current binary.

| Check | Before | After first current run | After second current run |
| --- | ---: | ---: | ---: |
| Advertised schema version | 225 | **229** | **229** |
| Migrations applied in run | n/a | **5** (reconciled 225, then 226–229) | **0** |
| `commit_links.link_method` | absent | present | present |
| Knowledge attribution columns | 0 of 2 | **2 of 2** | **2 of 2** |
| Reconciliation audit for m225 | absent | **previous value `BOOTSTRAP`** | unchanged |
| Legacy knowledge/provenance rows | present | preserved and readable | preserved and readable |

Production `knowledge list`, `history backfill`, `history status`, and text search with provenance all exited zero after upgrade. The preserved commit resolved to the original session and agent through a high-confidence `hook_observed` edge.

### Full software and live-runtime matrix

| Gate | Result | Fresh receipt on `8ab28046` |
| --- | ---: | --- |
| `cargo test --workspace --no-fail-fast` | PASS | Exit 0; `cas` lib 4,230 passed / 0 failed / 6 ignored; `cas-store` 616 / 0; component output 12 / 0; all remaining integration and doctest binaries completed without failure |
| `cargo check --workspace --all-targets` | PASS | Exit 0 in 26.74 s; existing warnings only |
| Workflow JavaScript | PASS | 121 / 121 |
| Current-main `cas-update` helper | PASS | 10 / 10 |
| Release migration guard | PASS | All four self-test paths |
| Isolated code/history index | PASS | Two commits indexed, lag 0; one Rust symbol indexed and mapped; exact-symbol search returned the mapped commit and high-confidence provenance |
| Epoch/verdict honesty | PASS | Empty epoch source returned an empty list; verdict returned `FIXED-UNVERIFIED` with an explicit no-running-fix-epoch rationale |
| Reconciliation/vector fallback contracts | PASS | Final workspace run includes stopped-daemon file retirement, empty-repository retirement, initial watcher reconciliation, logged-out scope-safe lexical fallback, and one-query three-namespace fan-out |
| Authenticated ambient live run | PASS | 1 / 1; four units embedded in three drain requests, pending 0; two role events made exactly two query requests; all three namespaces present; forbidden private hits 0 |

The authenticated receipt used `cas-embed-v1` at 1,024 dimensions. Drain latency was 2,350 ms; the worker and supervisor query events were 276 ms and 495 ms. The worker ranked Code first and the supervisor ranked History first. The endpoint does not expose billed tokens or price, so no monetary cost is invented.

### Hook and privacy envelope

| Check | Result |
| --- | ---: |
| SessionStart additional context | **11,089 bytes** |
| Hard acceptance boundary | 12,288 bytes |
| Margin | **1,199 bytes** |
| UserPromptSubmit additional context | **0 bytes** |
| Memory entries before → after | **0 → 0** |
| Knowledge pages before → after | **0 → 0** |
| Concrete private-path hits | **0** |

The SessionStart diagnostic remains truthful: every degradable section was already dropped and protected context exceeds the internal 9,216-byte target. That warning does not violate the 12,288-byte hard gate and does not truncate required lifecycle guidance.

### Whole-diff hygiene

Fresh added-line scans over `origin/main...8ab28046` found zero concrete absolute user-home paths, zero AWS/OpenAI/GitHub/private-key credential markers, zero external HTML runtime assets, and zero shortcut/incomplete markers in changed code. `git diff --check` passed.

## Fresh versus inherited evidence

Every release-decisive command in this receipt was rerun after the final repair landed on `8ab28046`: branch pinning, migration, production knowledge/history/code paths, workspace tests, all-target check, authenticated ambient live run, hook budgets/no-store, fast gates, privacy scans, and diff hygiene. Earlier child-task receipts were used only to reconcile why the bounded repairs exist and to confirm their commits are ancestors; they do not substitute for a final software pass.

## Ignored and skipped proof

- The normal workspace run reports six ignored `cas` library tests. The credential-gated authenticated ambient test was then run explicitly and passed. Other ignored tests are platform, destructive, or manual harness cases and are not substitutes for any acceptance criterion.
- External CI was not rerun per operator direction. One complete local workspace run and one all-target check were executed on the final assembled tip; repeating identical 45-minute matrices was intentionally avoided.
- The Claude cross-vendor persona lane was unavailable because authentication was absent. The operator explicitly waived that lane for this release only. The waiver is an accepted review exception, not a claim that the lane ran.

## Risks

| Risk | Likelihood | Impact | Owner | Mitigation |
| --- | --- | --- | --- | --- |
| Waived cross-vendor review could have found an issue outside the supplemental review and full matrix | Unknown | High | Release operator | Release-scoped waiver is explicit; supplemental findings were repaired; final software, migration, privacy, and live-runtime gates are green |
| SessionStart has less than 1.2 KiB of hard-budget margin | Medium | Medium | Hook maintainers | Production-shaped regression enforces margin; internal-target diagnostic remains visible; future guidance growth must preserve the hard cap |
| Live provider latency varies | Medium | Low | Recall maintainers | One request per event is enforced; lexical fallback remains available; provider timeout and corpus bounds are tested |

## Release recommendation

**GO.** Merge `8ab2804673c811f6553d65b40a7c16e0ae4ed61e` to main. No unresolved software, migration, privacy, live-runtime, branch, or diff blocker remains. Preserve the operator's review waiver in the release record and do not generalize it to later releases.

## Provenance

- Analysis commit: `8ab2804673c811f6553d65b40a7c16e0ae4ed61e`.
- Comparison base: `origin/main` at `8f6b557db7c3b994edfe23efa643dda1760e2bc0`.
- Evidence window: 2026-08-08 20:44–21:07 America/New_York.
- Markdown source: `docs/reports/2026-08-08-code-history-release-readiness.md`.
- Raw command outputs: temporary release-gate staging directory outside the repository; not committed.
- Principal commands (paths normalized to avoid recording private checkout locations):

```text
git fetch origin epic/code-history-search-v1-full-m1-m9-git-pr-issue-ind-cas-6212 main
git rev-parse HEAD origin/epic/code-history-search-v1-full-m1-m9-git-pr-issue-ind-cas-6212 origin/main
git merge-base HEAD origin/main
cas update --schema-only --json
cas knowledge list --json
cas index code --json
cas history backfill --force --json
cas history status --json
cas history search "release gate provenance sentinel" --include-provenance --json
cas history search --symbol release_gate_symbol --include-provenance --json
cas history epochs --backfill --json
cas history verdict "release gate missing symptom" --fix-commit <fixture-commit> --threshold 1 --json
env ZIG=<repo-zig> CARGO_TARGET_DIR=<shared-target> cargo test --workspace --no-fail-fast
env ZIG=<repo-zig> CARGO_TARGET_DIR=<shared-target> cargo check --workspace --all-targets
env ZIG=<repo-zig> CARGO_TARGET_DIR=<shared-target> CAS_AMBIENT_LIVE_CONFIG_DIR=<authenticated-config> cargo test -p cas --lib -- ambient_recall::tests::authenticated_isolated_live_provider_receipt --ignored --nocapture --test-threads=1
node --test .claude/workflows/*.test.js
bash contrib/shell-helpers/tests/cas-update-test.sh
bash scripts/test-check-release-migration-snapshots.sh
git diff --check origin/main...HEAD
```
