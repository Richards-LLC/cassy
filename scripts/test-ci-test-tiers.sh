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

job_block() {
    local job="$1"
    awk -v header="  ${job}:" '
        $0 == header { inside = 1; next }
        inside && /^  [A-Za-z0-9_-]+:$/ { exit }
        inside { print }
    ' "$ci"
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

required_jobs=(
    fast-validation-preflight
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

for job in fast-validation-preflight fast-validation-suite fast-validation-docs fast-validation macos-check; do
    block="$(job_block "$job")"
    require_text "$block" "refs/tags/" "$job runs on release tags"
done

required_pr_jobs=(fast-validation macos-check)
for job in "${required_pr_jobs[@]}"; do
    block="$(job_block "$job")"
    require_text "$block" "github.event_name == 'pull_request'" "$job runs for pull requests"
    require_text "$block" 'github.base_ref == github.event.repository.default_branch' "$job targets the default PR base"
    require_text "$block" 'github.event.pull_request.head.sha || github.sha' "$job deduplicates push/PR runs by head SHA"
    require_text "$block" 'cancel-in-progress: false' "$job queues duplicate required-check runs without cancellation"
done

preflight="$(job_block fast-validation-preflight)"
suite="$(job_block fast-validation-suite)"
docs="$(job_block fast-validation-docs)"
fan_in="$(job_block fast-validation)"
require_text "$suite" 'cargo nextest run -p cas --no-fail-fast' 'suite retains full nextest coverage'
require_text "$suite" '--test-threads 4' 'suite uses measured I/O-friendly test concurrency'
require_absent "$suite" '--partition' 'suite avoids duplicate test-graph compilation across runners'
require_text "$docs" 'cargo test -p cas --doc' 'doctest coverage remains in Fast Validation'
require_text "$fan_in" 'fast-validation-preflight' 'required Fast Validation waits for preflight'
require_text "$fan_in" 'fast-validation-suite' 'required Fast Validation waits for the full suite'
require_text "$fan_in" 'fast-validation-docs' 'required Fast Validation waits for doctests'
require_text "$fan_in" 'test "$PREFLIGHT" = success' 'required Fast Validation rejects a failed preflight'
require_text "$fan_in" 'test "$SUITE" = success' 'required Fast Validation rejects a failed full suite'
require_text "$fan_in" 'test "$DOCS" = success' 'required Fast Validation rejects failed doctests'

# Main PRs schedule only the required Fast Validation lanes and macOS Check.
# The scoped subset is deliberately excluded, and advisory/release jobs remain
# push/schedule-only, so a non-required job cannot consume a PR runner first.
require_text "$scoped" 'github.base_ref != github.event.repository.default_branch' 'non-required scoped lane skips main PRs'
for job in clippy test-compile-guard panic-isolation-release panic-isolation-release-fast build-benchmark; do
    block="$(job_block "$job")"
    require_text "$block" "refs/heads/main" "$job runs on main"
    require_text "$block" "github.event_name == 'schedule'" "$job runs on schedule"
    require_text "$block" "github.event_name == 'workflow_dispatch'" "$job supports supervisor dispatch"
    require_absent "$block" "github.event_name == 'pull_request'" "$job cannot run on PRs"
    require_absent "$block" 'refs/heads/epic/' "$job cannot run on epic pushes"
    require_absent "$block" 'refs/heads/factory/' "$job cannot run on factory pushes"
    require_absent "$block" 'refs/tags/' "$job cannot run on tag pushes"
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
