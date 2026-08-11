#!/usr/bin/env bash
# Static contract test for the two-tier CI policy introduced by cas-eb39.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ci="$repo_root/.github/workflows/ci.yml"
release="$repo_root/.github/workflows/release.yml"
setup="$repo_root/.github/actions/setup-rust-linux/action.yml"

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

job_block() {
    local job="$1"
    awk -v header="  ${job}:" '
        $0 == header { inside = 1; next }
        inside && /^  [A-Za-z0-9_-]+:$/ { exit }
        inside { print }
    ' "$ci"
}

job_ids() {
    awk '
        /^jobs:$/ { inside_jobs = 1; next }
        inside_jobs && /^  [A-Za-z0-9_-]+:$/ {
            sub(/^  /, "")
            sub(/:$/, "")
            print
        }
    ' "$ci"
}

only_main_push_ref() {
    local job="$1" block="$2" ref
    require_text "$block" "github.ref == 'refs/heads/main'" "$job runs push work only on main"
    while IFS= read -r ref; do
        [[ -z "$ref" ]] && continue
        if [[ "$ref" != 'refs/heads/main' ]]; then
            printf 'FAIL %s permits non-main push ref %s\n' "$job" "$ref"
            fail=$((fail + 1))
        fi
    done < <(grep -oE 'refs/(heads|tags)/[^'"'"' )]+' <<<"$block" | sort -u)
}

ci_text="$(<"$ci")"
require_text "$ci_text" '- "factory/**"' 'factory pushes trigger CI'
require_text "$ci_text" '- "epic/**"' 'epic pushes trigger CI'
require_text "$ci_text" '- "v*"' 'release tags trigger CI'

scoped="$(job_block scoped-validation)"
require_text "$scoped" "refs/heads/factory/" 'scoped tier selects factory branches'
require_text "$scoped" "github.event_name == 'pull_request'" 'pull requests use scoped tier'
require_text "$scoped" 'github.base_ref != github.event.repository.default_branch' 'scoped tier skips protected-default PRs'
require_text "$scoped" 'cargo check -p cas --lib --tests' 'scoped tier checks target surface'
require_text "$scoped" 'scripts/run-scoped-tests.sh -p cas --lib' 'scoped tier runs one test binary'

while IFS= read -r job; do
    [[ "$job" == 'scoped-validation' ]] && continue
    block="$(job_block "$job")"
    require_text "$block" "refs/heads/main" "$job runs on main"
    if grep -qF 'refs/heads/factory/' <<<"$block"; then
        printf 'FAIL %s leaks into factory pushes\n' "$job"
        fail=$((fail + 1))
    else
        printf 'ok   %s is absent from factory pushes\n' "$job"
        pass=$((pass + 1))
    fi
done < <(job_ids)

required_pr_lanes=(fast-validation-preflight fast-validation-suite fast-validation-docs fast-validation macos-check)
for job in "${required_pr_lanes[@]}"; do
    block="$(job_block "$job")"
    require_text "$block" "github.event_name == 'pull_request'" "$job runs for pull requests"
    require_text "$block" 'github.base_ref == github.event.repository.default_branch' "$job targets the default PR base"
done

for job in fast-validation macos-check; do
    block="$(job_block "$job")"
    require_text "$block" 'github.event.pull_request.head.sha || github.sha' "$job deduplicates push/PR runs by head SHA"
    require_text "$block" 'cancel-in-progress: false' "$job queues duplicate required-check runs without cancellation"
done

preflight="$(job_block fast-validation-preflight)"
suite="$(job_block fast-validation-suite)"
docs="$(job_block fast-validation-docs)"
fan_in="$(job_block fast-validation)"
require_text "$suite" 'shard: [1, 2]' 'suite uses two parallel shards'
require_text "$suite" '--partition count:${{ matrix.shard }}/2' 'suite shards are exhaustive count partitions'
require_text "$suite" 'cargo nextest run -p cas --no-fail-fast' 'suite retains full nextest coverage'
require_text "$docs" 'cargo test -p cas --doc' 'doctest coverage remains in Fast Validation'
require_text "$fan_in" 'fast-validation-preflight' 'required Fast Validation waits for preflight'
require_text "$fan_in" 'fast-validation-suite' 'required Fast Validation waits for both suite shards'
require_text "$fan_in" 'fast-validation-docs' 'required Fast Validation waits for doctests'
require_text "$fan_in" 'test "$PREFLIGHT" = success' 'required Fast Validation rejects a failed preflight'
require_text "$fan_in" 'test "$SUITE" = success' 'required Fast Validation rejects a failed suite shard'
require_text "$fan_in" 'test "$DOCS" = success' 'required Fast Validation rejects failed doctests'

# PR is the canonical non-main required gate. No non-main push can launch one
# of its lanes, so the same SHA never duplicates a costly required check.
for job in fast-validation-preflight fast-validation-suite fast-validation-docs fast-validation macos-check; do
    block="$(job_block "$job")"
    only_main_push_ref "$job" "$block"
    require_text "$block" "github.event_name == 'schedule'" "$job runs on schedule"
    require_text "$block" "github.event_name == 'workflow_dispatch'" "$job runs by manual dispatch"
done

# Main PRs schedule only the required Fast Validation lanes and macOS Check.
# The scoped subset is deliberately excluded, and advisory/release jobs remain
# push/schedule-only, so a non-required job cannot consume a PR runner first.
require_text "$scoped" 'github.base_ref != github.event.repository.default_branch' 'non-required scoped lane skips main PRs'
for job in clippy test-compile-guard panic-isolation-release panic-isolation-release-fast build-benchmark; do
    block="$(job_block "$job")"
    if grep -qF "github.event_name == 'pull_request'" <<<"$block"; then
        printf 'FAIL %s can run on a main PR\n' "$job"
        fail=$((fail + 1))
    else
        printf 'ok   %s cannot run on a main PR\n' "$job"
        pass=$((pass + 1))
    fi
done

# Benchmark and release-profile panic work is a main/scheduled/manual verdict,
# never a worker or epic/tag-push cost.
for job in panic-isolation-release panic-isolation-release-fast build-benchmark; do
    block="$(job_block "$job")"
    only_main_push_ref "$job" "$block"
    require_text "$block" "github.event_name == 'schedule'" "$job runs on schedule"
    require_text "$block" "github.event_name == 'workflow_dispatch'" "$job runs by manual dispatch"
    if grep -qF "github.event_name == 'pull_request'" <<<"$block"; then
        printf 'FAIL %s runs on a pull request\n' "$job"
        fail=$((fail + 1))
    else
        printf 'ok   %s is main/schedule/manual only\n' "$job"
        pass=$((pass + 1))
    fi
done

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

printf '\ntest result: %s passed; %s failed\n' "$pass" "$fail"
if [[ "$fail" -ne 0 ]]; then
    exit 1
fi
