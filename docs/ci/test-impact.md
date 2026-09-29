# Change-scoped Rust CI

Factory pushes and every PR run import/dependency-selected nextest tests in the
existing Scoped Validation lanes. The fast-admission gate decides which factory
lane executes; it does not change the test selector. The queue still builds one
complete workspace archive and executes its unfiltered three shards. Doctests,
Darwin checks, snapshot-input routing and protected main-push receipt reuse retain
their existing gates. Scheduled/manual and unvalidated direct-main safety runs
also keep exhaustive validation.

`python3 scripts/ci-test-impact.py plan --base-sha <base> --out plan.json`
inspects the committed merge-base diff without invoking Cargo. It selects changed
crates and their reverse workspace dependencies, then follows coarse top-level
Rust module references/imports. Within cas, it selects unit-test module prefixes
and original integration suite stems. `scripts/cas-test-targets.py` supplies the
stem-to-harness map; `--check` rejects unregistered source suites before compilation.
Consolidated suites use `binary(integration_...) and test(original_stem::)`.
Opaque integration imports and subprocess entry points stay selected.

Ambiguous module roots retain all affected library tests. Crate consumers run
unfiltered. Missing/deleted roots, unsupported manifests, unknown changed paths,
build/test policy changes and malformed/unavailable history widen to crate or
workspace selection. A zero-matched run retries that package unfiltered; a second
zero-test result fails. Execution uses the existing zero-test receipt wrapper.
The selected invocation builds its tests directly; there is no separate duplicate
`cargo check` stage.

Before each scoped run, `history` downloads bounded recent `ci-test-impact-*`
artifacts through read-only GitHub APIs. Recorded failures only add suites or
widen scope. Artifact input never removes a test or runs a shell command. API or
schema uncertainty selects wider coverage. Downloads are size/time bounded and
cross-host redirects strip the API authorization header.

Every selected run uploads `target/ci-impact/receipt.json` and raw logs, even on
failure. Receipts name the committed head/base, changed paths, exact plan,
executed test count, wall time, failures and status. Each full-suite shard uploads
the same shape with `role=full`, comparing its observed failures against the
selection predicted for that queue diff. `post_merge_missed` lists failures outside
that prediction; recall is matched failures / observed failures. With no failures,
recall is null, not 100%. This is nextest selection recall, not a causal claim
about which earlier worker introduced a defect, and does not include doctests.

Download comparable receipts and run:

```bash
python3 scripts/ci-test-impact.py summarize --log before-or-after/receipt.json --out summary.json
```

Repeat `--log` for each receipt. The summary reports median test invocation time
and failure-weighted recall per role. Compare workflow/job wall times over equal
factory/PR sample windows to evaluate the CI-time acceptance criterion; fixture
runtime or fewer selected modules alone is not evidence of a faster CI median.
Run `python3 scripts/test-ci-test-impact.py` and `scripts/test-ci-test-tiers.sh`
for Git/transport/execution fixtures and policy pins without compiling Rust.
