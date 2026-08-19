#!/usr/bin/env bash
# Static contract test for the two-tier CI policy introduced by cas-eb39.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ci="$repo_root/.github/workflows/ci.yml"
self_hosted="$repo_root/.github/workflows/self-hosted-fast-validation.yml"
release="$repo_root/.github/workflows/release.yml"
setup="$repo_root/.github/actions/setup-rust-linux/action.yml"
fallback="$repo_root/scripts/sccache-unavailable.sh"
ruleset="$repo_root/docs/branch-protection/main-ruleset.json"
makefile="$repo_root/cas-cli/Makefile"
verified="$repo_root/scripts/run-verified-tests.sh"
real_store_guard="$repo_root/scripts/check-real-store-untouched.sh"
migration_guard="$repo_root/scripts/check-release-migration-snapshots.sh"

pass=0
fail=0

require_text() {
    local haystack="$1" needle="$2" label="$3"
    if grep -qF -- "$needle" <<<"$haystack"; then
        printf 'ok   %s\n' "$label"
        pass=$((pass + 1))
    else
        printf 'FAIL %s (missing %s)\n' "$label" "$needle"
        fail=$((fail + 1))
    fi
}

require_absent() {
    local haystack="$1" needle="$2" label="$3"
    if grep -qF -- "$needle" <<<"$haystack"; then
        printf 'FAIL %s (unexpected %s)\n' "$label" "$needle"
        fail=$((fail + 1))
    else
        printf 'ok   %s\n' "$label"
        pass=$((pass + 1))
    fi
}

require_count() {
    local haystack="$1" needle="$2" expected="$3" label="$4"
    local actual
    actual="$(grep -oF -- "$needle" <<<"$haystack" | wc -l | tr -d '[:space:]')"
    if [[ "$actual" == "$expected" ]]; then
        printf 'ok   %s\n' "$label"
        pass=$((pass + 1))
    else
        printf 'FAIL %s (expected %s occurrences of %s; found %s)\n' "$label" "$expected" "$needle" "$actual"
        fail=$((fail + 1))
    fi
}

job_block() {
    local job="$1"
    awk -v header="  ${job}:" '
        $0 == header { inside = 1; next }
        inside && /^  [A-Za-z0-9_-]+:$/ { exit }
        inside { print }
    ' "$ci"
}

release_job_block() {
    local job="$1"
    awk -v header="  ${job}:" '
        $0 == header { inside = 1; next }
        inside && /^  [A-Za-z0-9_-]+:$/ { exit }
        inside { print }
    ' "$release"
}

ci_text="$(<"$ci")"
ruleset_text="$(<"$ruleset")"
self_hosted_text="$(<"$self_hosted")"
mapfile -t required_contexts < <(
    grep -oE '"context": "[^"]+"' "$ruleset" | sed -E 's/"context": "(.*)"/\1/'
)
if [[ "${#required_contexts[@]}" -eq 0 ]]; then
    printf 'FAIL main ruleset declares no required status checks\n'
    fail=$((fail + 1))
else
    for context in "${required_contexts[@]}"; do
        require_text "$ci_text" "name: $context" "ruleset-required context is a CI job: $context"
    done
fi
require_text "$ci_text" '- "factory/**"' 'factory pushes trigger CI'
require_text "$ci_text" '- "epic/**"' 'epic pushes trigger CI'
require_text "$ci_text" '- "v*"' 'release tags trigger CI'
require_text "$ci_text" 'merge_group:' 'CI accepts merge-queue merged-tree events'
require_text "$ci_text" 'make -C cas-cli test-ci-tiers' 'Fast Validation invokes release publication guard scripts'
require_count "$ruleset_text" '"context": "Fast Validation"' 1 'ruleset requires the complete Fast Validation rollup once'
require_count "$ruleset_text" '"context": "macOS Check"' 1 'ruleset requires macOS validation once'
require_absent "$ruleset_text" '"context": "Fast Validation — full suite"' 'ruleset does not mistake the lower suite fan-in for complete validation'
require_absent "$ruleset_text" '"context": "Fast Validation — doctests"' 'ruleset does not duplicate Fast Validation doctest coverage'
require_text "$ruleset_text" '"type": "merge_queue"' 'ruleset requires the GitHub merge queue'
require_text "$ruleset_text" '"grouping_strategy": "ALLGREEN"' 'merge queue validates every entry in its group'
require_text "$ruleset_text" '"max_entries_to_build": 1' 'merge queue avoids batching extra PRs into the latency path'
require_text "$ruleset_text" '"max_entries_to_merge": 1' 'merge queue merges one proven PR at a time'
require_text "$ruleset_text" '"min_entries_to_merge_wait_minutes": 0' 'merge queue adds no group-fill wait'

# Self-hosted pilot security/availability contract (cas-f5638). This repo is
# public, so fork/untrusted PR code must be unable to request the persistent
# runner. Required checks remain hosted: the local box is advisory and an
# outage cannot strand merge eligibility.
require_text "$self_hosted_text" 'push:' 'self-hosted pilot accepts canonical repository pushes'
require_text "$self_hosted_text" '- "factory/**"' 'self-hosted pilot accepts trusted factory branches'
require_text "$self_hosted_text" '- "epic/**"' 'self-hosted pilot accepts trusted epic branches'
for forbidden_event in pull_request: pull_request_target: workflow_run: issue_comment: repository_dispatch: workflow_dispatch:; do
    require_absent "$self_hosted_text" "$forbidden_event" "self-hosted pilot rejects event before runner assignment: $forbidden_event"
done
require_text "$self_hosted_text" "github.repository == 'Richards-LLC/cassy'" 'self-hosted job pins the canonical repository'
require_text "$self_hosted_text" "vars.CASSY_SELF_HOSTED_PILOT == 'enabled'" 'self-hosted job skips cleanly until an online listener is explicitly enabled'
require_text "$self_hosted_text" 'github.event.repository.fork == false' 'self-hosted job rejects fork repositories'
require_text "$self_hosted_text" "github.event_name == 'push'" 'self-hosted job repeats the push-only trust gate'
require_text "$self_hosted_text" "startsWith(github.ref, 'refs/heads/factory/')" 'self-hosted job pins trusted factory refs'
require_text "$self_hosted_text" 'group: cassy-public-trusted' 'self-hosted job uses the restricted runner group'
require_text "$self_hosted_text" 'labels: cas-ci-32core' 'self-hosted job uses its unique runner label'
require_text "$self_hosted_text" 'permissions:' 'self-hosted workflow declares explicit token permissions'
require_text "$self_hosted_text" 'contents: read' 'self-hosted workflow token is read-only'
require_text "$self_hosted_text" 'CARGO_BUILD_JOBS: "12"' 'self-hosted compile leaves CPU capacity for the worker fleet'
require_text "$self_hosted_text" 'CARGO_TARGET_DIR is not isolated from factory worktrees' 'self-hosted job fails if host isolation is lost'
require_text "$self_hosted_text" 'test "${SCCACHE_SERVER_PORT:?}" = 4227' 'self-hosted job pins its isolated sccache server'
require_text "$self_hosted_text" 'SCCACHE_DIR is not isolated from the operator cache' 'self-hosted job rejects the operator sccache directory'
require_text "$self_hosted_text" 'cargo nextest archive --workspace --archive-file fast-validation-suite.tar.zst' 'self-hosted pilot measures the same suite archive'
require_text "$self_hosted_text" '--partition "count:${shard}/3"' 'self-hosted pilot evaluates every suite shard'
require_absent "$(<"$ruleset")" 'Self-hosted pilot' 'self-hosted pilot is not a required status check'

suite_build="$(job_block fast-validation-suite-build)"
suite_shards="$(job_block fast-validation-suite-shards)"
require_text "$suite_build" 'runs-on: ubuntu-latest' 'required archive build retains automatic hosted availability'
require_text "$suite_shards" 'runs-on: ubuntu-latest' 'required suite shards retain automatic hosted availability'

scoped="$(job_block scoped-validation)"
require_text "$scoped" "refs/heads/factory/" 'scoped tier selects factory branches'
require_text "$scoped" "github.event_name == 'pull_request'" 'pull requests use scoped tier'
require_text "$scoped" 'github.base_ref != github.event.repository.default_branch' 'scoped tier skips protected-default PRs'
require_text "$scoped" 'cargo check -p cas --lib --tests' 'scoped tier checks target surface'
require_text "$scoped" 'scripts/run-scoped-tests.sh -p cas --lib' 'scoped tier runs one guarded test binary'

# Merge-latency contract for the scoped lane (cas-3e14). This lane is not a
# required check, but it reports last on a factory PR, so a merge that waits for
# every check waits for it. Measured on PR #479: 8m28s of Rust work on a
# docs-only diff, finishing 9s before the merge and 8m after the required lanes
# were green. Both mitigations below must stay, and both must stay fail-closed.
require_text "$scoped" 'id: classify-diff' 'scoped lane classifies before expensive work'
require_text "$scoped" './.github/actions/classify-required-diff' 'scoped lane uses the shared classifier'
require_text "$scoped" 'fetch-depth: 0' 'scoped lane computes a real merge base'
require_text "$scoped" 'github.event.pull_request.head.sha || github.sha' 'scoped lane classifies the PR head rather than its synthetic merge commit'
require_text "$scoped" 'github.event.pull_request.base.sha || github.event.before' 'scoped lane compares against its own event base'
require_text "$scoped" "steps.classify-diff.outputs.rust-unaffected != 'true'" 'scoped lane gates Rust work only after a safe classification'
require_text "$scoped" 'id: pr-dedupe' 'scoped lane checks for a PR event on the same head SHA'
require_text "$scoped" 'scripts/check-ci-pr-event-coverage.sh' 'scoped lane uses the shared fail-closed PR coverage guard'
require_text "$scoped" "steps.pr-dedupe.outputs.covered != 'true'" 'scoped lane gates expensive work on the dedupe verdict'
require_text "$scoped" 'scoped-validation-${{ github.event.pull_request.number || github.ref }}' 'scoped lane groups runs by pull request, falling back to the branch ref'
require_text "$scoped" 'cancel-in-progress: true' 'scoped lane cancels its own superseded runs'
require_text "$scoped" 'pull-requests: read' 'scoped lane reads PR metadata without write access'

# The dedupe may only ever silence the PUSH copy. If the pull-request event
# could also stand down, a head SHA could reach a merge with no validation at
# all — the exact hole the fail-closed design exists to prevent.
dedupe_step="$(awk '
    /^      - id: pr-dedupe$/ { inside = 1 }
    inside && /^      - id: classify-diff$/ { exit }
    inside { print }
' <<<"$scoped")"
require_text "$dedupe_step" "if: github.event_name == 'push'" 'only the push copy may dedupe; the PR event always validates'

# Every expensive step must carry BOTH gates. A step gated on only one of them
# would run Rust work in a case the other already ruled out — or worse, skip on
# an empty output from a step that never ran.
for expensive in \
    './.github/actions/setup-rust-linux' \
    'taiki-e/install-action@nextest' \
    'cargo check -p cas --lib --tests' \
    'scripts/run-scoped-tests.sh -p cas --lib'; do
    step_block="$(awk -v needle="$expensive" '
        /^      - / {
            if (block != "" && index(block, needle)) { printf "%s", block; found = 1; exit }
            block = ""
        }
        { block = block $0 "\n" }
        END { if (!found && index(block, needle)) printf "%s", block }
    ' <<<"$scoped")"
    require_text "$step_block" "steps.pr-dedupe.outputs.covered != 'true'" "scoped lane step is dedupe-gated: $expensive"
    require_text "$step_block" "steps.classify-diff.outputs.rust-unaffected != 'true'" "scoped lane step is classification-gated: $expensive"
done

# Removing the factory push trigger in the name of dedupe would leave the
# supervisor's `git merge --no-ff` integration path — which never opens a pull
# request — with no validation whatsoever.
require_text "$scoped" "github.event_name == 'push'" 'scoped lane still validates factory branch pushes'

# Exercise the real coverage guard. Only an open pull request whose head is
# exactly this commit may silence the lane; every other answer runs it.
coverage_guard="$repo_root/scripts/check-ci-pr-event-coverage.sh"
if [[ -x "$coverage_guard" ]]; then
    coverage_tmp="$(mktemp -d)"
    mkdir -p "$coverage_tmp/bin"
    cat >"$coverage_tmp/bin/gh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"${FAKE_GH_LOG:?}"
case "${FAKE_GH_MODE:?}" in
  hit) printf '%s\n' '479' ;;
  miss) printf '\n' ;;
  garbage) printf '%s\n' 'not-a-number' ;;
  error) exit 1 ;;
  *) exit 2 ;;
esac
EOF
    chmod +x "$coverage_tmp/bin/gh"

    run_coverage() {
        local mode="$1" event="${2:-push}"
        local output="$coverage_tmp/$mode.$event.output"
        : >"$output"
        GITHUB_OUTPUT="$output" GITHUB_EVENT_NAME="$event" GITHUB_SHA=deadbeefcafe \
            GITHUB_REPOSITORY=example/repo FAKE_GH_MODE="$mode" \
            FAKE_GH_LOG="$coverage_tmp/gh.log" \
            PATH="$coverage_tmp/bin:$PATH" "$coverage_guard" >/dev/null
        cat "$output"
    }

    : >"$coverage_tmp/gh.log"
    hit_coverage="$(run_coverage hit)"
    require_text "$hit_coverage" 'covered=true' 'an open PR on this exact head SHA dedupes the push copy'
    require_text "$hit_coverage" 'pr-number=479' 'dedupe records which pull request covers the commit'
    require_text "$(<"$coverage_tmp/gh.log")" 'commits/deadbeefcafe/pulls' 'coverage lookup asks about the exact head commit'
    require_text "$(<"$coverage_tmp/gh.log")" 'select(.head.sha == "deadbeefcafe")' 'coverage lookup accepts only a PR whose head is this commit'
    require_text "$(<"$coverage_tmp/gh.log")" 'select(.state == "open")' 'coverage lookup ignores closed pull requests'

    for mode in miss garbage error; do
        require_absent "$(run_coverage "$mode")" 'covered=true' "$mode pull-request evidence fails closed to running the lane"
    done
    require_absent "$(run_coverage hit pull_request)" 'covered=true' 'a pull request event never dedupes itself'

    no_repo_output="$coverage_tmp/no-repo.output"
    : >"$no_repo_output"
    GITHUB_OUTPUT="$no_repo_output" GITHUB_EVENT_NAME=push GITHUB_SHA=deadbeefcafe \
        GITHUB_REPOSITORY="" FAKE_GH_MODE=hit FAKE_GH_LOG="$coverage_tmp/gh.log" \
        PATH="$coverage_tmp/bin:$PATH" "$coverage_guard" >/dev/null
    require_absent "$(<"$no_repo_output")" 'covered=true' 'a missing repository slug fails closed to running the lane'

    rm -rf "$coverage_tmp"
else
    printf 'FAIL PR event coverage guard is executable\n'
    fail=$((fail + 1))
fi

required_jobs=(
    fast-validation-preflight
    fast-validation-suite-build
    fast-validation-suite-shards
    fast-validation-suite
    fast-validation-docs
    fast-validation
    macos-check
)
for job in "${required_jobs[@]}"; do
    block="$(job_block "$job")"
    require_text "$block" "refs/heads/main" "$job runs on main"
    require_absent "$block" 'refs/heads/epic/' "$job is not double-run from epic pushes"
    require_absent "$block" 'refs/heads/factory/' "$job is absent from factory pushes"
done

# Required contexts must always be emitted. The expensive work in each lane is
# gated only after a shared, fail-closed diff classification step succeeds.
for job in fast-validation-preflight fast-validation-suite-build fast-validation-suite-shards fast-validation-suite fast-validation-docs macos-check; do
    block="$(job_block "$job")"
    require_text "$block" 'id: classify-diff' "$job classifies before expensive work"
    require_text "$block" './.github/actions/classify-required-diff' "$job uses the shared classifier"
    require_text "$block" 'fetch-depth: 0' "$job computes a real merge base"
    require_text "$block" 'github.event.pull_request.head.sha || github.sha' "$job classifies the PR head rather than its synthetic merge commit"
    require_text "$block" "steps.classify-diff.outputs.rust-unaffected" "$job gates Rust work only after a safe classification"
done

classifier="$repo_root/scripts/classify-ci-diff.sh"
classify_action="$repo_root/.github/actions/classify-required-diff/action.yml"
if [[ -x "$classifier" ]]; then
    # These committed fixtures pin both directions: the explicitly safe
    # classes are Rust-unaffected, while every code or mixed change is full.
    require_text "$("$classifier" 967e85c7^ 967e85c7)" 'empty' 'empty ancestry merge fast-passes'
    require_text "$("$classifier" c6c4122f^ c6c4122f)" 'docs-only' 'docs-only change fast-passes'
    require_text "$("$classifier" 7c233bef^ 7c233bef)" 'hub-web-only' 'hub-web-only change skips Rust work'
    require_text "$("$classifier" 15edf2ef^ 15edf2ef)" 'version-bump' 'two-file package version bump fast-passes'
    require_text "$("$classifier" 15edf2ef^ eab3901c)" 'version-bump' 'workspace-wide seven-file version bump fast-passes'
    require_text "$("$classifier" 66b059b4^ 66b059b4)" 'rust-touched' 'version bump plus changelog runs Rust tier'
    require_text "$("$classifier" bb7417ef^ bb7417ef)" 'rust-touched' 'code diff runs Rust tier'
else
    printf 'FAIL CI diff classifier is executable\n'
    fail=$((fail + 1))
fi

# GitHub provides an all-zero `before` SHA when a pushed tag has no predecessor.
# Run the composite action body itself so this contract cannot regress into a
# `git merge-base 000... HEAD` failure before the required test lanes start.
tag_push_output="$(mktemp)"
if BASE_SHA="0000000000000000000000000000000000000000" \
    GITHUB_OUTPUT="$tag_push_output" \
    bash < <(awk '
        /^      run: \|$/ { in_run = 1; next }
        in_run { sub(/^        /, ""); print }
    ' "$classify_action"); then
    require_text "$(<"$tag_push_output")" 'class=rust-touched' 'tag push with all-zero BASE_SHA falls back to Rust tier'
    require_text "$(<"$tag_push_output")" 'fast-pass=false' 'tag push all-zero BASE_SHA never fast-passes'
    require_text "$(<"$tag_push_output")" 'rust-unaffected=false' 'tag push all-zero BASE_SHA never skips Rust'
else
    printf 'FAIL tag push all-zero BASE_SHA runs the shared classifier action\n'
    fail=$((fail + 1))
fi
rm -f "$tag_push_output"

# A non-zero-but-unresolvable base is another uncertainty case. The composite
# action must not fail a required check before deciding to run the Rust tier.
unknown_base_output="$(mktemp)"
if BASE_SHA="1111111111111111111111111111111111111111" \
    GITHUB_OUTPUT="$unknown_base_output" \
    bash < <(awk '
        /^      run: \|$/ { in_run = 1; next }
        in_run { sub(/^        /, ""); print }
    ' "$classify_action"); then
    require_text "$(<"$unknown_base_output")" 'class=rust-touched' 'unresolvable base falls back to Rust tier'
    require_text "$(<"$unknown_base_output")" 'rust-unaffected=false' 'unresolvable base never skips Rust'
else
    printf 'FAIL unresolvable base runs the shared classifier action\n'
    fail=$((fail + 1))
fi
rm -f "$unknown_base_output"

for job in fast-validation-preflight fast-validation-suite-build fast-validation-suite-shards fast-validation-suite fast-validation-docs fast-validation macos-check; do
    block="$(job_block "$job")"
    require_text "$block" "refs/tags/" "$job runs on release tags"
done

required_pr_jobs=(fast-validation macos-check)
for job in "${required_pr_jobs[@]}"; do
    block="$(job_block "$job")"
    require_text "$block" "github.event_name == 'pull_request'" "$job runs for pull requests"
    require_text "$block" 'github.base_ref == github.event.repository.default_branch' "$job targets the default PR base"
    require_text "$block" 'github.event.pull_request.number || github.ref' "$job groups runs by pull request rather than by head SHA"
    require_text "$block" 'cancel-in-progress: true' "$job releases runners held by its own superseded runs"
done

preflight="$(job_block fast-validation-preflight)"
suite="$(job_block fast-validation-suite)"
suite_build="$(job_block fast-validation-suite-build)"
suite_shards="$(job_block fast-validation-suite-shards)"
docs="$(job_block fast-validation-docs)"
fan_in="$(job_block fast-validation)"
require_text "$suite_shards" 'shard: [1, 2, 3]' 'suite uses three nextest shards'
require_text "$suite_shards" 'fail-fast: false' 'suite keeps running other shards after a failure'
require_text "$suite_build" 'cargo nextest archive --workspace --archive-file fast-validation-suite.tar.zst' 'suite compiles the workspace test graph once into an archive'
require_text "$suite_build" 'actions/upload-artifact@v4' 'suite build publishes the shared nextest archive'

# Archive payload is on the PR critical path (cas-59d3). Every shard downloads
# the whole artifact, so its size is multiplied by the shard count. Measured at
# 4034 MB: the three shards spent 404s / 257s / 63s downloading it in order to
# run 108s / 105s / 101s of tests, i.e. the apparent "shard skew" was download
# variance, not partition imbalance. Stripping debug info measured 65% smaller
# binaries on this workspace. Keep the override, and keep it scoped to the
# archive build so local and other lanes keep their normal debug info.
require_text "$suite_build" 'CARGO_PROFILE_DEV_DEBUG: "0"' 'suite archive drops dev debug info'
require_text "$suite_build" 'CARGO_PROFILE_TEST_DEBUG: "0"' 'suite archive drops test debug info'
require_text "$suite_build" 'CARGO_PROFILE_DEV_STRIP: debuginfo' 'suite archive strips dev debug sections'
require_text "$suite_build" 'CARGO_PROFILE_TEST_STRIP: debuginfo' 'suite archive strips test debug sections'
require_absent "$ci_text" 'CARGO_PROFILE_DEV_STRIP: symbols' 'archive keeps symbol names for readable panics'
for job in fast-validation-preflight fast-validation-docs clippy test-compile-guard; do
    require_absent "$(job_block "$job")" 'CARGO_PROFILE_DEV_DEBUG' "$job keeps normal debug info: $job"
done
require_text "$suite_build" 'tar -czf fast-validation-suite-runner.tar.gz target/debug/cas' 'suite packages the executable CLI runner with its mode bits'
require_text "$suite_shards" 'needs: fast-validation-suite-build' 'shards wait for the shared test archive'
require_text "$suite_shards" 'actions/download-artifact@v4' 'shards download the shared nextest archive'
require_text "$suite_shards" 'tar -xzf fast-validation-suite-runner.tar.gz' 'shards restore the executable CLI runner payload'
require_text "$suite_shards" 'test -x target/debug/cas' 'shards verify the restored CLI runner remains executable'
require_text "$suite_shards" 'scripts/run-verified-tests.sh nextest run --archive-file fast-validation-suite.tar.zst --no-fail-fast --partition count:${{ matrix.shard }}/3' 'shards execute every archived workspace nextest binary exactly once'
require_text "$suite" 'needs: fast-validation-suite-shards' 'required full-suite context fans in every shard'
require_text "$suite" 'test "$SHARDS" = success' 'required full-suite context rejects failed shards'
require_text "$(<"$makefile")" '../scripts/run-verified-tests.sh nextest run --workspace --no-fail-fast' 'local make test verifies CI workspace nextest scope'
require_text "$docs" 'scripts/run-verified-tests.sh test -p cas --doc' 'doctest coverage remains in Fast Validation with an execution receipt'
require_text "$(<"$real_store_guard")" 'run-verified-tests.sh' 'real-store guard requires an executed-test receipt'
require_text "$(<"$migration_guard")" 'run-verified-tests.sh nextest run -p cas --test component_output_test' 'release migration snapshots require an executed-test receipt'
if [[ -x "$verified" ]]; then
    printf 'ok   verified-test receipt wrapper is executable\n'
    pass=$((pass + 1))
else
    printf 'FAIL verified-test receipt wrapper is executable\n'
    fail=$((fail + 1))
fi
require_text "$fan_in" 'fast-validation-preflight' 'required Fast Validation waits for preflight'
require_text "$fan_in" 'fast-validation-suite' 'required Fast Validation waits for the full suite'
require_text "$fan_in" 'fast-validation-docs' 'required Fast Validation waits for doctests'
require_text "$fan_in" 'test "$PREFLIGHT" = success' 'required Fast Validation rejects a failed preflight'
require_text "$fan_in" 'test "$SUITE" = success' 'required Fast Validation rejects a failed full suite'
require_text "$fan_in" 'test "$DOCS" = success' 'required Fast Validation rejects failed doctests'

# Required-context closure (cas-5496): the ruleset must select the top-level
# Fast Validation rollup, never its lower suite-only fan-in. This is the
# coverage proof that a failed OR cancelled preflight, shard, or doctest blocks
# a merge: always() ensures the fan-in gate still runs after a dependency fails,
# and each need result must be success.
require_text "$ruleset_text" '"context": "Fast Validation"' 'required context selects the complete validation rollup'
require_text "$fan_in" 'always()' 'required validation rollup still runs after a failed dependency'
require_text "$fan_in" 'fast-validation-preflight' 'required validation closure includes preflight'
require_text "$fan_in" 'fast-validation-suite' 'required validation closure includes suite fan-in'
require_text "$fan_in" 'fast-validation-docs' 'required validation closure includes doctests'
require_text "$fan_in" 'test "$PREFLIGHT" = success' 'required validation rejects failed or cancelled preflight'
require_text "$fan_in" 'test "$SUITE" = success' 'required validation rejects failed or cancelled suite fan-in'
require_text "$fan_in" 'test "$DOCS" = success' 'required validation rejects failed or cancelled doctests'
require_text "$suite" 'always()' 'suite fan-in still runs after a failed shard'
require_text "$suite" 'test "$SHARDS" = success' 'suite fan-in rejects failed or cancelled shards'

# Merge queue validates GitHub's synthetic merged tree, not the PR head. Both
# required contexts and every Fast Validation dependency must report there.
for job in fast-validation-preflight fast-validation-suite-build fast-validation-suite-shards fast-validation-suite fast-validation-docs fast-validation macos-check; do
    require_text "$(job_block "$job")" "github.event_name == 'merge_group'" "$job runs on merge-queue merged trees"
done

# Main PRs validate the reusable compile surfaces while the release-profile
# panic probes and cold benchmark remain schedule/manual workloads.
require_text "$scoped" 'github.base_ref != github.event.repository.default_branch' 'non-required scoped lane skips main PRs'
for job in clippy test-compile-guard; do
    block="$(job_block "$job")"
    require_text "$block" "refs/heads/main" "$job runs on main"
    require_text "$block" "github.event_name == 'schedule'" "$job runs on schedule"
    require_text "$block" "github.event_name == 'workflow_dispatch'" "$job supports supervisor dispatch"
    require_text "$block" "github.event_name == 'pull_request'" "$job validates protected PR trees"
    require_text "$block" 'github.base_ref == github.event.repository.default_branch' "$job targets the protected PR base"
    require_absent "$block" 'refs/heads/epic/' "$job cannot run on epic pushes"
    require_absent "$block" 'refs/heads/factory/' "$job cannot run on factory pushes"
    require_absent "$block" 'refs/tags/' "$job cannot run on tag pushes"
    require_text "$block" 'id: tree-dedupe' "$job checks for an identical PR-validated tree first"
    require_text "$block" 'scripts/check-ci-tree-validation.sh' "$job uses the shared fail-closed tree guard"
    require_text "$block" "steps.tree-dedupe.outputs.run-heavy != 'false'" "$job gates expensive work on the tree receipt"
    require_text "$block" 'steps.tree-dedupe.outputs.prior-run-url' "$job logs the validating run URL when deduped"
done

for job in panic-isolation-release panic-isolation-release-fast build-benchmark; do
    block="$(job_block "$job")"
    require_text "$block" "github.event_name == 'schedule'" "$job runs on schedule"
    require_text "$block" "github.event_name == 'workflow_dispatch'" "$job supports supervisor dispatch"
    require_absent "$block" 'refs/heads/main' "$job does not consume runners on ordinary main pushes"
    require_absent "$block" "github.event_name == 'pull_request'" "$job does not delay PR validation"
done

receipt_job="$(job_block record-pr-validation)"
require_text "$ci_text" 'actions: read' 'CI may query validation receipt artifacts'
require_text "$receipt_job" 'needs: [fast-validation, macos-check, clippy, test-compile-guard]' 'tree receipt waits for every per-tree validation lane'
require_text "$receipt_job" "needs.fast-validation.result == 'success'" 'tree receipt requires successful Fast Validation'
require_text "$receipt_job" "needs.macos-check.result == 'success'" 'tree receipt requires successful macOS Check'
require_text "$receipt_job" "needs.clippy.result == 'success'" 'tree receipt requires successful Clippy'
require_text "$receipt_job" "needs.test-compile-guard.result == 'success'" 'tree receipt requires successful test compilation'
require_text "$receipt_job" "git rev-parse 'HEAD^{tree}'" 'tree receipt keys exact Git contents'
require_text "$receipt_job" 'name: pr-validated-tree-${{ steps.tree.outputs.hash }}' 'tree receipt artifact is named by tree hash'

tree_guard="$repo_root/scripts/check-ci-tree-validation.sh"
guard_tmp="$(mktemp -d)"
mkdir -p "$guard_tmp/bin"
cat >"$guard_tmp/bin/gh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"${FAKE_GH_LOG:?}"
case "${FAKE_GH_MODE:?}:$*" in
  hit:*actions/artifacts*) printf '%s\n' '{"artifacts":[{"expired":false,"workflow_run":{"id":123}}]}' ;;
  hit:*actions/runs/123*) printf '%s\n' '{"event":"pull_request","status":"completed","conclusion":"success","html_url":"https://example.test/actions/runs/123"}' ;;
  wrong-event:*actions/artifacts*) printf '%s\n' '{"artifacts":[{"expired":false,"workflow_run":{"id":456}}]}' ;;
  wrong-event:*actions/runs/456*) printf '%s\n' '{"event":"push","status":"completed","conclusion":"success","html_url":"https://example.test/actions/runs/456"}' ;;
  miss:*actions/artifacts*) printf '%s\n' '{"artifacts":[]}' ;;
  error:*) exit 1 ;;
  *) exit 2 ;;
esac
EOF
chmod +x "$guard_tmp/bin/gh"

guard_tree="$(git -C "$repo_root" rev-parse 'HEAD^{tree}')"
run_guard() {
    local mode="$1"
    local output="$guard_tmp/$mode.output"
    : >"$output"
    GITHUB_OUTPUT="$output" GITHUB_EVENT_NAME=push GITHUB_REF=refs/heads/main \
        GITHUB_REPOSITORY=example/repo FAKE_GH_MODE="$mode" FAKE_GH_LOG="$guard_tmp/gh.log" \
        PATH="$guard_tmp/bin:$PATH" "$tree_guard" >/dev/null
    cat "$output"
}

hit_output="$(run_guard hit)"
require_text "$hit_output" 'run-heavy=false' 'matching successful PR receipt skips heavy work'
require_text "$hit_output" 'prior-run-url=https://example.test/actions/runs/123' 'matching receipt exposes the prior run URL'
require_text "$(<"$guard_tmp/gh.log")" "pr-validated-tree-$guard_tree" 'tree lookup queries the exact current Git tree'
for mode in miss wrong-event error; do
    output_path="$(run_guard "$mode")"
    require_text "$output_path" 'run-heavy=true' "$mode receipt evidence fails closed to heavy work"
    require_absent "$output_path" 'run-heavy=false' "$mode receipt evidence never dedupes"
done
rm -rf "$guard_tmp"

all_actions="$(<"$setup")$(<"$ci")$(<"$release")"
require_text "$all_actions" 'mozilla-actions/sccache-action@v0.0.11' 'cache-v2-capable sccache action is pinned'
if grep -qF 'mozilla-actions/sccache-action@v0.0.5' <<<"$all_actions"; then
    printf 'FAIL retired sccache action v0.0.5 remains\n'
    fail=$((fail + 1))
else
    printf 'ok   retired sccache action v0.0.5 is absent\n'
    pass=$((pass + 1))
fi
require_text "$ci_text" 'SCCACHE_GHA_ENABLED: "true"' 'CI enables GitHub cache-v2 backend'
require_text "$(<"$release")" 'SCCACHE_GHA_ENABLED: "true"' 'release enables GitHub cache-v2 backend'
release_text="$(<"$release")"
release_linux="$(release_job_block build)"
release_macos="$(release_job_block build-macos)"
release_publish="$(release_job_block release)"
require_absent "$release_linux" 'needs: verify' 'Linux release build starts in parallel with input verification'
require_absent "$release_macos" 'needs: verify' 'macOS release build starts in parallel with input verification'
require_text "$release_publish" 'needs: [verify, build, build-macos]' 'release publication waits for verification and both platform builds'
for platform_build in "$release_linux" "$release_macos"; do
    require_text "$platform_build" '--profile "$RELEASE_PROFILE"' 'platform release build uses the thin-LTO profile'
    require_text "$platform_build" 'strip package/cas' 'platform package strips symbols before publication'
    require_text "$platform_build" '$RELEASE_DIR/cas' 'platform package selects the configured profile output'
done
require_text "$release_linux" 'check-blake3-no-avx512-build.sh "target/x86_64-unknown-linux-gnu/$RELEASE_DIR/build"' 'Linux release audits BLAKE3 inputs from the selected profile'
require_text "$release_linux" 'test-check-portable-x86_64-isa.sh package/cas' 'Linux release audits the exact stripped executable'
require_text "$release_linux" 'name: cas-x86_64-unknown-linux-gnu' 'Linux release asset remains required'
require_text "$release_macos" 'name: cas-aarch64-apple-darwin' 'macOS release asset remains required'
require_absent "$(<"$release")" 'gh release delete' 'release never replaces published assets after a receipt'
require_text "$(<"$release")" 'refusing to replace its assets' 'release rerun with an existing release fails loudly'
require_text "$(<"$release")" 'RELEASE_SLACK_RUBRIC.md#recovering-a-failed-or-partial-release' 'release rerun names its recovery procedure'
require_text "$(<"$repo_root/docs/RELEASE_SLACK_RUBRIC.md")" '### Recovering a failed or partial release' 'release rubric documents partial-release recovery'

# Exercise the actual Create Release shell body with a fake gh client. A
# release run that failed after creating its release object must not silently
# attach/replace assets on retry; a release object that does not exist must
# still proceed to its one normal create operation.
release_create_body="$(awk '
    $0 == "      - name: Create Release" { in_step = 1; next }
    in_step && $0 == "        run: |" { in_body = 1; next }
    in_body { sub(/^          /, ""); print }
' "$release" | sed -E 's/\$\{\{[^}]+\}\}/workflow-expression/g')"
retry_tmp="$(mktemp -d)"
trap 'rm -rf "$retry_tmp"' EXIT
mkdir -p "$retry_tmp/bin"
cat >"$retry_tmp/bin/gh" <<'EOF'
#!/usr/bin/env bash
case "$1 $2" in
  "release view") exit "${FAKE_RELEASE_EXISTS:?}" ;;
  "release create") printf '%s\n' "$*" >>"${FAKE_GH_LOG:?}" ;;
  *) echo "unexpected fake gh invocation: $*" >&2; exit 2 ;;
esac
EOF
chmod +x "$retry_tmp/bin/gh"

set +e
existing_output="$(GITHUB_REF=refs/tags/v9.9.9 FAKE_RELEASE_EXISTS=0 FAKE_GH_LOG="$retry_tmp/creates" PATH="$retry_tmp/bin:$PATH" bash -c "$release_create_body" 2>&1)"
existing_status=$?
set -e
test "$existing_status" -eq 1
grep -qF 'Release v9.9.9 already exists; refusing to replace its assets' <<<"$existing_output"
test ! -e "$retry_tmp/creates"
echo 'ok   partial-release retry refuses loudly and does not upload replacement bytes'

GITHUB_REF=refs/tags/v9.9.9 FAKE_RELEASE_EXISTS=1 FAKE_GH_LOG="$retry_tmp/creates" PATH="$retry_tmp/bin:$PATH" bash -c "$release_create_body"
grep -qF 'release create v9.9.9' "$retry_tmp/creates"
echo 'ok   first release run creates the release when no object exists'
rm -rf "$retry_tmp"
trap - EXIT

# The cache is an optimization, never a CI availability dependency. The shared
# Linux setup must survive both a failed action download and a failed backend
# startup by clearing the globally configured compiler wrapper before Cargo.
require_text "$(<"$setup")" 'id: setup-sccache' 'sccache setup step has a probe handle'
require_text "$(<"$setup")" 'continue-on-error: true' 'sccache action download may fail open'
require_text "$(<"$setup")" 'if [[ "${{ steps.setup-sccache.outcome }}" != "success" ]] || ! sccache --start-server; then' 'sccache download and backend startup both select fallback'
require_text "$(<"$setup")" 'sccache backend unavailable — building uncached' 'sccache outage emits an uncached-build warning'
require_text "$(<"$setup")" 'echo "RUSTC_WRAPPER=" >> "$GITHUB_ENV"' 'sccache outage clears compiler wrapper'
require_text "$(<"$setup")" 'echo "SCCACHE_GHA_ENABLED=false" >> "$GITHUB_ENV"' 'sccache outage disables its backend'
require_count "$all_actions" 'mozilla-actions/sccache-action@v0.0.11' '5' 'every sccache setup is accounted for'
require_count "$all_actions" 'continue-on-error: true' '5' 'every sccache setup action and post-step fails open'
require_count "$all_actions" 'sccache --start-server' '5' 'every sccache setup probes backend availability'
require_count "$all_actions" 'sccache backend unavailable — building uncached' '5' 'every sccache outage logs its uncached fallback'
require_count "$all_actions" 'SCCACHE_PATH=$GITHUB_WORKSPACE/scripts/sccache-unavailable.sh' '5' 'every sccache fallback makes the action post hook harmless'
if [[ -x "$fallback" ]] \
    && "$fallback" --show-stats | grep -qF 'sccache unavailable; build ran uncached' \
    && "$fallback" --show-stats --stats-format=json | jq -e '.stats.compile_requests == 0 and .stats.cache_hits.counts == {}' >/dev/null; then
    printf 'ok   sccache post fallback is executable and emits valid zero stats\n'
    pass=$((pass + 1))
else
    printf 'FAIL sccache post fallback must be executable and emit valid zero stats\n'
    fail=$((fail + 1))
fi

# Compiler cache observability (cas-67a2). Warm lanes reach 90-92% sccache hits
# and pre-cache-v2 lanes measured 0-6%; that gap is worth minutes per run and is
# invisible unless each lane publishes its own hit rate. Pin both halves: every
# compiling lane is cache-v2 configured, and every compiling lane reports.
summary_script="$repo_root/scripts/ci-sccache-summary.sh"
require_text "$ci_text" 'SCCACHE_GHA_VERSION: "cas-v2"' 'CI pins one shared cache-v2 object namespace'
require_text "$release_text" 'SCCACHE_GHA_VERSION: "cas-v2"' 'release shares the CI cache-v2 object namespace'

# Lane label must match the job so a summary is attributable to its job.
declare -A compiling_lanes=(
    [scoped-validation]='Scoped Validation'
    [fast-validation-preflight]='Fast Validation — preflight'
    [fast-validation-suite-build]='Fast Validation — suite archive build'
    [fast-validation-docs]='Fast Validation — doctests'
    [clippy]='Clippy'
    [macos-check]='macOS Check'
    [test-compile-guard]='Test Compile Guard'
    [panic-isolation-release]='Panic Isolation — release profile'
    [panic-isolation-release-fast]='Panic Isolation — release-fast profile'
)
for job in "${!compiling_lanes[@]}"; do
    block="$(job_block "$job")"
    require_text "$block" "./scripts/ci-sccache-summary.sh \"${compiling_lanes[$job]}\"" \
        "$job publishes its own sccache hit stats"
    require_text "$block" 'if: always()' "$job reports cache stats even when the lane fails"
    require_absent "$block" 'SCCACHE_GHA_ENABLED: "false"' "$job keeps the cache-v2 backend enabled"
    require_text "$block" 'if [[ -x ./scripts/ci-sccache-summary.sh ]]; then' \
        "$job survives a checkout that predates the stats script"
done

# Version-skew guard (cas-3e14, absorbed defect). A `pull_request` run takes the
# workflow from the newer base but checks out the PR head, so a head that
# predates scripts/ci-sccache-summary.sh ran a missing file and exited 127.
# Measured: run 32144587241 on head e3868d2f failed SIX lanes — preflight,
# doctests, suite archive build, full suite, Fast Validation and macOS Check,
# four of them required — because an observability step could not find its
# script. `if: always()` made it worse by guaranteeing the step ran in every
# lane. Every invocation across both workflows must be existence-guarded, and
# the guarded and unguarded forms must stay in lockstep so a newly added lane
# cannot reintroduce the bare call.
summary_invocations="$(grep -c -F './scripts/ci-sccache-summary.sh "' <<<"$all_actions" || true)"
summary_guards="$(grep -c -F 'if [[ -x ./scripts/ci-sccache-summary.sh ]]; then' <<<"$all_actions" || true)"
if [[ "$summary_invocations" == "$summary_guards" && "$summary_invocations" -gt 0 ]]; then
    printf 'ok   every sccache stats invocation is existence-guarded (%s of them)\n' "$summary_guards"
    pass=$((pass + 1))
else
    printf 'FAIL every sccache stats invocation must be existence-guarded (%s invocations, %s guards)\n' \
        "$summary_invocations" "$summary_guards"
    fail=$((fail + 1))
fi
require_count "$all_actions" 'scripts/ci-sccache-summary.sh is absent at this checkout' "$summary_guards" \
    'every missing stats script says so instead of failing the lane'

# The guard body itself must behave: present and executable runs it, absent
# reports and exits 0. Exercised against the real shell, not just grepped.
skew_tmp="$(mktemp -d)"
skew_guard='if [[ -x ./scripts/ci-sccache-summary.sh ]]; then
  ./scripts/ci-sccache-summary.sh "Probe"
else
  echo "::notice title=sccache stats::scripts/ci-sccache-summary.sh is absent at this checkout; skipping cache reporting."
fi'
mkdir -p "$skew_tmp/empty" "$skew_tmp/present/scripts"
printf '#!/usr/bin/env bash\necho "stats for $1"\n' >"$skew_tmp/present/scripts/ci-sccache-summary.sh"
chmod +x "$skew_tmp/present/scripts/ci-sccache-summary.sh"
if (cd "$skew_tmp/empty" && bash -c "$skew_guard") >"$skew_tmp/absent.log" 2>&1; then
    require_text "$(<"$skew_tmp/absent.log")" 'is absent at this checkout' 'a checkout without the stats script reports and passes'
else
    printf 'FAIL a checkout without the stats script must not fail the lane\n'
    fail=$((fail + 1))
fi
if (cd "$skew_tmp/present" && bash -c "$skew_guard") >"$skew_tmp/present.log" 2>&1; then
    require_text "$(<"$skew_tmp/present.log")" 'stats for Probe' 'a checkout with the stats script still reports its lane'
else
    printf 'FAIL a checkout with the stats script must still run it\n'
    fail=$((fail + 1))
fi
rm -rf "$skew_tmp"

for job in build build-macos verify; do
    block="$(release_job_block "$job")"
    require_text "$block" './scripts/ci-sccache-summary.sh' "release $job publishes its own sccache hit stats"
done

# The single deliberate exemption. It measures a cold compiler, so a stats
# summary there would report an alarming 0% for a lane that is working correctly.
benchmark="$(job_block build-benchmark)"
require_text "$benchmark" 'SCCACHE_GHA_ENABLED: "false"' 'Build Benchmark stays deliberately uncached'
require_text "$benchmark" 'RUSTC_WRAPPER: ""' 'Build Benchmark clears the compiler wrapper'
require_text "$benchmark" 'DELIBERATELY UNCACHED' 'Build Benchmark documents why it is exempt'
require_absent "$benchmark" 'ci-sccache-summary.sh' 'Build Benchmark reports no cache stats'

# Test-only lanes execute prebuilt binaries; a stats step there would report a
# cache that never had a compile to serve.
require_absent "$suite_shards" 'ci-sccache-summary.sh' 'suite shards compile nothing and report no cache stats'

# Observability must not be able to fail a build. Exercise the real script
# against a warm cache, a missing binary, and the probe's disabled-backend
# state; every path must exit 0 and still say something useful.
if [[ -x "$summary_script" ]]; then
    printf 'ok   sccache summary script is executable\n'
    pass=$((pass + 1))
    stats_tmp="$(mktemp -d)"
    mkdir -p "$stats_tmp/bin"
    cat >"$stats_tmp/bin/sccache" <<'EOF'
#!/usr/bin/env bash
if [[ " $* " == *" --stats-format=json "* ]]; then
    printf '%s\n' '{"stats":{"compile_requests":51,"requests_executed":51,"cache_errors":{"counts":{},"adv_counts":{}},"cache_hits":{"counts":{"Rust":47},"adv_counts":{}},"cache_misses":{"counts":{"Rust":4},"adv_counts":{}},"cache_writes":4}}'
else
    printf 'Compile requests                     51\nCompile requests executed            51\nCache hits                           47\nCache misses                          4\nCache writes                          4\nCache errors                          0\n'
fi
EOF
    chmod +x "$stats_tmp/bin/sccache"

    # Keep the exit-status assertion in this shell: running the probe inside a
    # command substitution would swallow both its status and the counters.
    summary_output=""
    run_summary() {
        local label="$1"
        shift
        local summary="$stats_tmp/summary.md" log="$stats_tmp/log"
        : >"$summary"
        if env "$@" GITHUB_STEP_SUMMARY="$summary" "$summary_script" 'Contract Lane' >"$log" 2>&1; then
            printf 'ok   sccache summary exits 0 (%s)\n' "$label"
            pass=$((pass + 1))
        else
            printf 'FAIL sccache summary must never fail a build (%s)\n' "$label"
            fail=$((fail + 1))
        fi
        summary_output="$(cat "$summary" "$log")"
    }

    run_summary warm "PATH=$stats_tmp/bin:$PATH" SCCACHE_GHA_ENABLED=true SCCACHE_GHA_VERSION=cas-v2
    require_text "$summary_output" '47 hits / 4 misses' 'warm lane summary reports hits and misses'
    require_text "$summary_output" 'hit rate 92%' 'warm lane summary reports a hit rate'
    require_text "$summary_output" 'cache v2' 'warm lane summary names the configured backend'
    require_text "$summary_output" 'written to the job summary' 'the lane states that its stats reached the job summary, not just the log'
    require_text "$(<"$stats_tmp/summary.md")" '| Hit rate | 92% |' 'the job summary file itself carries the rendered table'

    run_summary cold "PATH=$stats_tmp/bin:$PATH" SCCACHE_GHA_ENABLED=true CAS_SCCACHE_MIN_HIT_RATE=95
    require_text "$summary_output" '::warning title=sccache cold lane::' 'a lane below the hit-rate floor is annotated, not failed'

    # A PATH with no sccache but still a usable shell. If this host ships
    # sccache in a system directory the case is unobservable, so say so rather
    # than assert something the environment cannot demonstrate.
    mkdir -p "$stats_tmp/empty"
    if PATH="$stats_tmp/empty:/usr/bin:/bin" command -v sccache >/dev/null 2>&1; then
        printf 'ok   (not observable here) system sccache shadows the missing-binary case\n'
        pass=$((pass + 1))
    else
        run_summary missing "PATH=$stats_tmp/empty:/usr/bin:/bin" SCCACHE_GHA_ENABLED=true
        require_text "$summary_output" 'Cache statistics unavailable' 'a missing sccache binary degrades to a visible note'
    fi

    run_summary disabled "PATH=$stats_tmp/bin:$PATH" SCCACHE_GHA_ENABLED=false
    require_text "$summary_output" 'build ran uncached' 'the probe-disabled backend is reported as uncached'

    rm -rf "$stats_tmp"
else
    printf 'FAIL sccache summary script must exist and be executable\n'
    fail=$((fail + 1))
fi

printf '\ntest result: %s passed; %s failed\n' "$pass" "$fail"
if [[ "$fail" -ne 0 ]]; then
    exit 1
fi
