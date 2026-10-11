#!/usr/bin/env bash
# Real subprocess contracts: routing, receipts, refusal/retry and cache fallback.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ci="$repo_root/.github/workflows/ci.yml"
release="$repo_root/.github/workflows/release.yml"
setup="$repo_root/.github/actions/setup-rust-linux/action.yml"
fallback="$repo_root/scripts/sccache-unavailable.sh"
verified="$repo_root/scripts/run-verified-tests.sh"
snapshot_router="$repo_root/scripts/check-scoped-snapshot-tests.sh"
snapshot_router_test="$repo_root/scripts/test-check-scoped-snapshot-tests.sh"
scoped_surface="$repo_root/scripts/check-scoped-test-surface.sh"
scoped_surface_test="$repo_root/scripts/test-check-scoped-test-surface.sh"
watchdog_script="$repo_root/scripts/cancel-stale-merge-group-runs.sh"
runner_isolation="$repo_root/scripts/check-cassy-actions-runner-isolation.sh"
runner_pruner="$repo_root/scripts/prune-cassy-actions-cache.sh"
runner_pruner_test="$repo_root/scripts/test-prune-cassy-actions-cache.sh"
runner_job_lock="$repo_root/scripts/cassy-actions-cache-job-lock.sh"
runner_job_lock_test="$repo_root/scripts/test-cassy-actions-cache-job-lock.sh"
runner_mount_guard="$repo_root/scripts/check-cassy-actions-cache-mount.sh"
runner_mount_guard_test="$repo_root/scripts/test-check-cassy-actions-cache-mount.sh"
rust_setup="$repo_root/scripts/setup-cassy-actions-rust.sh"
stale_queue_script="$repo_root/scripts/cancel-stale-non-merge-group-queued-runs.sh"
watchdog_policy="$repo_root/scripts/watchdog-policy.sh"
watchdog_behavior_test="$repo_root/scripts/test-watchdog-scripts.sh"

pass=0
fail=0
skip=0
policy_parser="$repo_root/scripts/ci_tiers/policy.py"

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

release_job_block() { python3 "$policy_parser" job .github/workflows/release.yml "$1"; }
named_step_position() { python3 "$policy_parser" position "$2" <<<"$1"; }

if [[ -x "$rust_setup" ]]; then
    printf 'ok   shared self-hosted Rust setup script is executable\n'
    pass=$((pass + 1))
else
    printf 'FAIL shared self-hosted Rust setup script is executable\n'
    fail=$((fail + 1))
fi

# These fixtures exercise Linux runner mount, /proc and flock contracts.
# Keep executable admission on Darwin, but do not count a platform skip as pass.
run_linux_runner_fixture() {
    local script="$1" fixture="$2" label="$3" reason="$4"
    if [[ ! -x "$script" || ! -x "$fixture" ]]; then
        printf 'FAIL %s must be executable\n' "$label"
        fail=$((fail + 1))
    elif [[ "$(uname -s)" == Darwin ]]; then
        printf 'SKIP %s on Darwin: %s\n' "$label" "$reason"
        skip=$((skip + 1))
    elif "$fixture" >/dev/null; then
        printf 'ok   %s passes\n' "$label"
        pass=$((pass + 1))
    else
        printf 'FAIL %s must pass\n' "$label"
        fail=$((fail + 1))
    fi
}

run_linux_runner_fixture "$runner_pruner" "$runner_pruner_test" \
    'runner cache pruning behavior test' 'Linux cgroups, flock, findmnt and GNU deletion/accounting tools required'
run_linux_runner_fixture "$runner_mount_guard" "$runner_mount_guard_test" \
    'runner cache mount guard behavior test' 'Linux findmnt/mountpoint device and FSROOT semantics required'
run_linux_runner_fixture "$runner_job_lock" "$runner_job_lock_test" \
    'runner job-lifetime cache lock behavior test' 'Linux /proc identity and inherited flock descriptor required'

if [[ -x "$runner_isolation" ]]; then
    if CARGO_TARGET_DIR=/var/lib/cassy-actions/cache/cargo-target \
        SCCACHE_DIR=/var/lib/cassy-actions/cache/sccache \
        SCCACHE_SERVER_PORT=4227 "$runner_isolation" >/dev/null; then
        printf 'ok   runner isolation accepts slot 1 tuple\n'
        pass=$((pass + 1))
    else
        printf 'FAIL runner isolation must accept slot 1 tuple\n'
        fail=$((fail + 1))
    fi
    if CARGO_TARGET_DIR=/var/lib/cassy-actions/cache/cargo-target-2 \
        SCCACHE_DIR=/var/lib/cassy-actions/cache/sccache-2 \
        SCCACHE_SERVER_PORT=4228 "$runner_isolation" >/dev/null; then
        printf 'ok   runner isolation accepts slot 2 tuple\n'
        pass=$((pass + 1))
    else
        printf 'FAIL runner isolation must accept slot 2 tuple\n'
        fail=$((fail + 1))
    fi
    if CARGO_TARGET_DIR=/var/lib/cassy-actions/cache/cargo-target \
        SCCACHE_DIR=/var/lib/cassy-actions/cache/sccache-2 \
        SCCACHE_SERVER_PORT=4228 "$runner_isolation" >/dev/null 2>&1; then
        printf 'FAIL runner isolation must reject a mixed slot tuple\n'
        fail=$((fail + 1))
    else
        printf 'ok   runner isolation rejects a mixed slot tuple\n'
        pass=$((pass + 1))
    fi
else
    printf 'FAIL runner isolation script must be executable\n'
    fail=$((fail + 1))
fi

if [[ -x "$watchdog_script" ]]; then
    :
else
    printf 'FAIL merge-queue watchdog script is executable\n'
    fail=$((fail + 1))
fi

impact_fixture_log="$(mktemp)"
if python3 "$repo_root/scripts/test-ci-test-impact.py" >"$impact_fixture_log" 2>&1; then
    printf 'ok   impact selector Git/execution/recall fixtures pass\n'
    pass=$((pass + 1))
else
    printf 'FAIL impact selector fixtures\n'
    cat "$impact_fixture_log"
    fail=$((fail + 1))
fi
rm -f "$impact_fixture_log"

if [[ -x "$snapshot_router" ]]; then
    printf 'ok   snapshot router is executable\n'
    pass=$((pass + 1))
else
    printf 'FAIL snapshot router must be executable\n'
    fail=$((fail + 1))
fi
if [[ -x "$snapshot_router_test" ]]; then
    printf 'ok   snapshot router has an executable self-test\n'
    pass=$((pass + 1))
else
    printf 'FAIL snapshot router has no executable self-test\n'
    fail=$((fail + 1))
fi

if [[ -x "$scoped_surface" ]]; then
    printf 'ok   scoped test surface checker is executable\n'
    pass=$((pass + 1))
else
    printf 'FAIL scoped test surface checker must be executable\n'
    fail=$((fail + 1))
fi
if [[ -x "$scoped_surface_test" ]]; then
    printf 'ok   scoped test surface checker has an executable self-test\n'
    pass=$((pass + 1))
else
    printf 'FAIL scoped test surface checker has no executable self-test\n'
    fail=$((fail + 1))
fi

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

gate_row_parity="$repo_root/scripts/check-ci-gate-row-parity.sh"
gate_exemptions="$repo_root/docs/ci/gate-exemptions.md"
if [[ -x "$gate_row_parity" ]]; then
    if "$gate_row_parity" "$ci" "$repo_root/scripts/release-gate.sh" "$gate_exemptions"; then
        printf 'ok   protected CI jobs map to release-gate rows\n'
        pass=$((pass + 1))
    else
        printf 'FAIL protected CI jobs map to release-gate rows\n'
        fail=$((fail + 1))
    fi
    parity_tmp="$(mktemp)"
    sed '/\[fast-validation-suite-shards\]=archive-mode/d' "$gate_row_parity" >"$parity_tmp"
    chmod +x "$parity_tmp"
    if "$parity_tmp" "$ci" "$repo_root/scripts/release-gate.sh" "$gate_exemptions" >/dev/null 2>&1; then
        printf 'FAIL CI gate-row parity mutation removes a required mapping\n'
        fail=$((fail + 1))
    else
        printf 'ok   CI gate-row parity mutation catches a removed mapping\n'
        pass=$((pass + 1))
    fi
    rm -f "$parity_tmp"
else
    printf 'FAIL CI gate-row parity checker is executable\n'
    fail=$((fail + 1))
fi

classifier="$repo_root/scripts/classify-ci-diff.sh"
fast_classifier="$repo_root/scripts/classify-fast-admission.sh"
if [[ -x "$fast_classifier" ]]; then
    printf 'ok   fast admission classifier is executable\n'
    pass=$((pass + 1))
else
    printf 'FAIL fast admission classifier is executable\n'
    fail=$((fail + 1))
fi
if [[ -x "$classifier" ]]; then
    # These committed fixtures pin both directions: the explicitly safe
    # classes are Rust-unaffected, while every code or mixed change is full.
    require_text "$("$classifier" 967e85c7^ 967e85c7)" 'empty' 'empty ancestry merge fast-passes'
    require_text "$("$classifier" c6c4122f^ c6c4122f)" 'docs-only' 'docs-only change fast-passes'
    require_text "$("$classifier" 49b434bf^ 49b434bf)" 'docs-only' 'PR 630 CODEMAP-only change fast-passes'
    require_text "$("$classifier" 66b059b4^ 66b059b4)" 'rust-touched' 'mixed version bump plus changelog runs Rust tier'
    require_text "$("$classifier" bb7417ef^ bb7417ef)" 'rust-touched' 'code-only change runs Rust tier'
    require_text "$("$classifier" 7c233bef^ 7c233bef)" 'hub-web-only' 'hub-web-only change skips Rust work'
    require_text "$("$classifier" c070753a^ c070753a)" 'slack-bridge-only' 'slack-bridge-only change skips Rust work'
    require_text "$("$classifier" 15edf2ef^ 15edf2ef)" 'version-bump' 'two-file package version bump fast-passes'
    require_text "$("$classifier" 15edf2ef^ eab3901c)" 'version-bump' 'workspace-wide seven-file version bump fast-passes'
    require_text "$("$classifier" 66b059b4^ 66b059b4)" 'rust-touched' 'version bump plus changelog runs Rust tier'
    require_text "$("$classifier" bb7417ef^ bb7417ef)" 'rust-touched' 'code diff runs Rust tier'
    builtin_markdown_base="237a6c7e^"
    builtin_markdown_head="237a6c7e"
    require_text "$("$classifier" "$builtin_markdown_base" "$builtin_markdown_head")" 'rust-touched' 'embedded builtin Markdown runs Rust tier'

    # Mutation contract: the fixture changes only Markdown files under
    # `cas-cli/src/builtins/`. Removing the source-tree guard must make it
    # classify docs-only, so this test goes red if that guard disappears.
    classifier_without_source_guard="$(mktemp)"
    sed '/cas-cli\/src\/\*) docs_only=false; break ;;/d' "$classifier" >"$classifier_without_source_guard"
    chmod +x "$classifier_without_source_guard"
    if [[ "$("$classifier_without_source_guard" "$builtin_markdown_base" "$builtin_markdown_head")" == 'rust-touched' ]]; then
        printf 'FAIL builtin Markdown mutation removes the compiled-source guard\n'
        fail=$((fail + 1))
    else
        printf 'ok   builtin Markdown mutation catches removed compiled-source guard\n'
        pass=$((pass + 1))
    fi
    rm -f "$classifier_without_source_guard"

    # Fail-closed contract (cas-b505, audit finding 7): a Git failure must never
    # read as an empty diff with exit 0. Two producers: an unresolvable ref, and
    # a git executable whose `diff` fails after base resolution succeeded — the
    # case the composite action would otherwise trust as a real `empty`.
    classifier_failure_case() {
        local label="$1" base="$2" head="$3" path_prefix="$4"
        local output status
        set +e
        if [[ -n "$path_prefix" ]]; then
            output="$(PATH="$path_prefix:$PATH" "$classifier" "$base" "$head" 2>/dev/null)"
        else
            output="$("$classifier" "$base" "$head" 2>/dev/null)"
        fi
        status=$?
        set -e
        if [[ "$status" != 0 && "$output" != empty ]]; then
            printf 'ok   %s\n' "$label"
            pass=$((pass + 1))
        else
            printf 'FAIL %s (exit %s, stdout %q)\n' "$label" "$status" "$output"
            fail=$((fail + 1))
        fi
    }
    classifier_failure_case 'unresolvable ref fails the classifier instead of reading empty' 'audit-nonexistent-base' 'HEAD' ''
    failing_git_dir="$(mktemp -d)"
    real_git="$(command -v git)"
    printf '#!/usr/bin/env bash\nif [[ "$1" == diff ]]; then echo "fatal: injected diff failure" >&2; exit 128; fi\nexec %q "$@"\n' "$real_git" >"$failing_git_dir/git"
    chmod +x "$failing_git_dir/git"
    classifier_failure_case 'injected failing git diff fails the classifier instead of reading empty' 'HEAD~1' 'HEAD' "$failing_git_dir"
    rm -rf "$failing_git_dir"
else
    printf 'FAIL CI diff classifier is executable\n'
    fail=$((fail + 1))
fi

tag_push_output="$(mktemp)"
if BASE_SHA="0000000000000000000000000000000000000000" \
    GITHUB_OUTPUT="$tag_push_output" \
    bash < <(python3 "$policy_parser" action-run .github/actions/classify-required-diff/action.yml); then
    require_text "$(<"$tag_push_output")" 'class=rust-touched' 'tag push with all-zero BASE_SHA falls back to Rust tier'
    require_text "$(<"$tag_push_output")" 'fast-pass=false' 'tag push all-zero BASE_SHA never fast-passes'
    require_text "$(<"$tag_push_output")" 'rust-unaffected=false' 'tag push all-zero BASE_SHA never skips Rust'
else
    printf 'FAIL tag push all-zero BASE_SHA runs the shared classifier action\n'
    fail=$((fail + 1))
fi
rm -f "$tag_push_output"

first_branch_output="$(mktemp)"
if BASE_SHA="0000000000000000000000000000000000000000" \
    ZERO_BASE_REF="HEAD" \
    GITHUB_OUTPUT="$first_branch_output" \
    bash < <(python3 "$policy_parser" action-run .github/actions/classify-required-diff/action.yml); then
    require_text "$(<"$first_branch_output")" 'class=empty' 'first branch push uses its protected-base fallback'
    require_text "$(<"$first_branch_output")" 'fast-pass=true' 'empty first branch push fast-passes without Rust'
    require_text "$(<"$first_branch_output")" 'rust-unaffected=true' 'empty first branch push skips Cargo'
else
    printf 'FAIL first branch push runs the shared classifier action\n'
    fail=$((fail + 1))
fi
rm -f "$first_branch_output"

unknown_base_output="$(mktemp)"
if BASE_SHA="1111111111111111111111111111111111111111" \
    GITHUB_OUTPUT="$unknown_base_output" \
    bash < <(python3 "$policy_parser" action-run .github/actions/classify-required-diff/action.yml); then
    require_text "$(<"$unknown_base_output")" 'class=rust-touched' 'unresolvable base falls back to Rust tier'
    require_text "$(<"$unknown_base_output")" 'rust-unaffected=false' 'unresolvable base never skips Rust'
else
    printf 'FAIL unresolvable base runs the shared classifier action\n'
    fail=$((fail + 1))
fi
rm -f "$unknown_base_output"

bridge_touched_output="$(mktemp)"
if BASE_SHA="c070753a^" \
    GITHUB_OUTPUT="$bridge_touched_output" \
    bash < <(python3 "$policy_parser" action-run .github/actions/classify-required-diff/action.yml); then
    require_text "$(<"$bridge_touched_output")" 'bridge-check-needed=true' 'a diff touching slack-bridge runs the bridge checks'
else
    printf 'FAIL slack-bridge diff runs the shared classifier action\n'
    fail=$((fail + 1))
fi
rm -f "$bridge_touched_output"

bridge_untouched_output="$(mktemp)"
if BASE_SHA="HEAD" \
    GITHUB_OUTPUT="$bridge_untouched_output" \
    bash < <(python3 "$policy_parser" action-run .github/actions/classify-required-diff/action.yml); then
    require_text "$(<"$bridge_untouched_output")" 'bridge-check-needed=false' 'a diff that leaves slack-bridge alone skips the bridge checks'
else
    printf 'FAIL unchanged slack-bridge runs the shared classifier action\n'
    fail=$((fail + 1))
fi
rm -f "$bridge_untouched_output"

required_pr_jobs=(fast-validation macos-check)

if [[ -x "$verified" ]]; then
    printf 'ok   verified-test receipt wrapper is executable\n'
    pass=$((pass + 1))
else
    printf 'FAIL verified-test receipt wrapper is executable\n'
    fail=$((fail + 1))
fi

if [[ -x "$stale_queue_script" ]]; then

    stale_tmp="$(mktemp -d)"
    mkdir -p "$stale_tmp/bin"
    cat >"$stale_tmp/bin/gh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"${FAKE_GH_LOG:?}"
if [[ "$*" == *'actions/runs?status=queued&per_page=100'* ]]; then
    cat <<'JSON'
{"workflow_runs":[
  {"id":101,"created_at":"1970-01-01T00:00:00Z","event":"push","head_branch":"main"},
  {"id":102,"created_at":"1970-01-01T00:00:00Z","event":"merge_group","head_branch":"gh-readonly-queue/main/pr-1"},
  {"id":103,"created_at":"1970-01-01T00:30:00Z","event":"pull_request","head_branch":"feature"},
  {"id":104,"created_at":"1970-01-01T00:00:00Z","event":"workflow_dispatch","head_branch":"main"}
]}
JSON
elif [[ "$*" == *'actions/runs/101'*'--jq .status'* ]]; then
    printf '%s\n' queued
elif [[ "$*" == *'actions/runs/104'*'--jq .status'* ]]; then
    printf '%s\n' completed
elif [[ "$*" == *'--method POST'*'actions/runs/101/cancel'* ]]; then
    exit 0
else
    printf 'unexpected fake gh invocation: %s\n' "$*" >&2
    exit 2
fi

EOF
    chmod +x "$stale_tmp/bin/gh"
    stale_output="$stale_tmp/output"
    if GITHUB_REPOSITORY=example/repo CASSY_NOW_EPOCH=2000 FAKE_GH_LOG="$stale_tmp/gh.log" \
        PATH="$stale_tmp/bin:$PATH" "$stale_queue_script" >"$stale_output" 2>&1; then
        require_text "$(<"$stale_output")" 'cancelling stale queued run=101 event=push' 'stale queued push run is cancelled'
        require_text "$(<"$stale_output")" 'skipping no-longer-queued run=104 current_status=completed' 'stale list entry is rechecked before cancellation'
        require_absent "$(<"$stale_tmp/gh.log")" 'actions/runs/102' 'merge-group candidate is left to cas-065a'
        require_absent "$(<"$stale_tmp/gh.log")" 'actions/runs/103' 'fresh queued run is retained'
        require_text "$(<"$stale_tmp/gh.log")" 'actions/runs/101/cancel' 'stale queued push run receives a cancel request'
        require_absent "$(<"$stale_tmp/gh.log")" 'actions/runs/104/cancel' 'status-raced queued run is not cancelled'
    else
        printf 'FAIL stale queued-run watchdog executes against queued-run fixture\n'
        fail=$((fail + 1))
    fi
    rm -rf "$stale_tmp"
else
    printf 'FAIL stale queued-run watchdog script is executable\n'
    fail=$((fail + 1))
fi

if [[ -x "$watchdog_policy" && -x "$watchdog_behavior_test" ]]; then
    if "$watchdog_behavior_test"; then
        printf 'ok   watchdog behavior fixtures pass for both scripts\n'
        pass=$((pass + 1))
    else
        printf 'FAIL watchdog behavior fixtures pass for both scripts\n'
        fail=$((fail + 1))
    fi
else
    printf 'FAIL watchdog policy and behavior fixtures are executable\n'
    fail=$((fail + 1))
fi

tree_guard="$repo_root/scripts/check-ci-tree-validation.sh"
merge_queue_guard="$repo_root/scripts/check-ci-merge-queue-validation.sh"
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
  merge-hit:*actions/artifacts*) printf '%s\n' '{"artifacts":[{"expired":false,"workflow_run":{"id":789}}]}' ;;
  merge-hit:*actions/runs/789*) printf '%s\n' '{"event":"merge_group","status":"completed","conclusion":"success","html_url":"https://example.test/actions/runs/789"}' ;;
  merge-in-progress:*actions/artifacts*) printf '%s\n' '{"artifacts":[{"expired":false,"workflow_run":{"id":790}}]}' ;;
  merge-in-progress:*actions/runs/790*) printf '%s\n' '{"event":"merge_group","status":"in_progress","conclusion":null,"html_url":"https://example.test/actions/runs/790"}' ;;
  merge-wrong-event:*actions/artifacts*) printf '%s\n' '{"artifacts":[{"expired":false,"workflow_run":{"id":791}}]}' ;;
  merge-wrong-event:*actions/runs/791*) printf '%s\n' '{"event":"push","status":"completed","conclusion":"success","html_url":"https://example.test/actions/runs/791"}' ;;
  miss:*actions/artifacts*) printf '%s\n' '{"artifacts":[]}' ;;
  gate-*:*pulls/1134*) printf '%s\n' '{"head":{"sha":"1111111111111111111111111111111111111111"}}' ;;
  gate-tree-mismatch:*git/commits/1111111111111111111111111111111111111111*) printf '%s\n' '{"tree":{"sha":"2222222222222222222222222222222222222222"}}' ;;
  gate-*:*git/commits/1111111111111111111111111111111111111111*) printf '{"tree":{"sha":"%s"}}\n' "${FAKE_TREE:?}" ;;
  gate-hit:*statuses*) printf '[{"context":"cas/full-gate","state":"success","description":"PASS tree=%s","target_url":"https://example.test/gate"}]\n' "$FAKE_TREE" ;;
  gate-tree-mismatch:*statuses*) printf '[{"context":"cas/full-gate","state":"success","description":"PASS tree=%s"}]\n' "$FAKE_TREE" ;;
  gate-status-other-tree:*statuses*) printf '%s\n' '[{"context":"cas/full-gate","state":"success","description":"PASS tree=3333333333333333333333333333333333333333"}]' ;;
  gate-failed:*statuses*) printf '[{"context":"cas/full-gate","state":"failure","description":"PASS tree=%s"}]\n' "$FAKE_TREE" ;;
  gate-superseded:*statuses*) printf '[{"context":"cas/full-gate","state":"failure","description":"FAIL tree=%s"},{"context":"cas/full-gate","state":"success","description":"PASS tree=%s"}]\n' "$FAKE_TREE" "$FAKE_TREE" ;;
  gate-other-context:*statuses*) printf '[{"context":"ci/other","state":"success","description":"PASS tree=%s"}]\n' "$FAKE_TREE" ;;
  gate-missing:*statuses*) printf '%s\n' '[]' ;;
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

run_merge_queue_guard() {
    local mode="$1"
    local output="$guard_tmp/$mode.output"
    : >"$output"
    GITHUB_OUTPUT="$output" GITHUB_EVENT_NAME=push GITHUB_REF=refs/heads/main \
        GITHUB_REPOSITORY=example/repo FAKE_GH_MODE="$mode" FAKE_GH_LOG="$guard_tmp/gh.log" \
        PATH="$guard_tmp/bin:$PATH" "$merge_queue_guard" >/dev/null
    cat "$output"
}

merge_hit_output="$(run_merge_queue_guard merge-hit)"
require_text "$merge_hit_output" 'run-fast-validation=false' 'matching successful merge-queue receipt skips main-push Fast Validation'
require_text "$merge_hit_output" 'validating-run-id=789' 'matching merge-queue receipt exposes the validating run id'
require_text "$merge_hit_output" 'prior-run-url=https://example.test/actions/runs/789' 'matching merge-queue receipt exposes the validating run URL'
require_text "$(<"$guard_tmp/gh.log")" "merge-queue-validated-tree-$guard_tree" 'merge-queue lookup queries the exact current Git tree'
for mode in miss merge-in-progress merge-wrong-event error; do
    output_path="$(run_merge_queue_guard "$mode")"
    require_text "$output_path" 'run-fast-validation=true' "$mode merge-queue evidence fails closed to Fast Validation"
    require_absent "$output_path" 'run-fast-validation=false' "$mode merge-queue evidence never dedupes"
done

require_text "$merge_hit_output" 'reuse-source=merge-queue' 'main-push reuse names the merge-queue receipt'

# cas-4cb8: a merge_group run reuses the release train's full-gate receipt for
# the exact tree: the PR head carries a `cas/full-gate` success status naming
# that tree, and the PR head's tree is the queue tree. Anything else runs the
# full Fast Validation.
run_queue_gate_guard() {
    local mode="$1"
    local output="$guard_tmp/queue-$mode.output"
    : >"$output"
    GITHUB_OUTPUT="$output" GITHUB_EVENT_NAME=merge_group \
        GITHUB_REF="refs/heads/gh-readonly-queue/main/pr-1134-4444444444444444444444444444444444444444" \
        GITHUB_REPOSITORY=example/repo FAKE_GH_MODE="$mode" FAKE_TREE="$guard_tree" \
        FAKE_GH_LOG="$guard_tmp/gh.log" PATH="$guard_tmp/bin:$PATH" "$merge_queue_guard" >/dev/null
    cat "$output"
}
gate_hit_output="$(run_queue_gate_guard gate-hit)"
require_text "$gate_hit_output" 'run-fast-validation=false' 'a matching full-gate tree receipt skips the queue Fast Validation lanes'
require_text "$gate_hit_output" 'reuse-source=full-gate' 'queue reuse names the full-gate receipt'
require_text "$gate_hit_output" 'prior-run-url=https://example.test/gate' 'queue reuse exposes the gate receipt URL'
require_text "$gate_hit_output" 'validating-run-id=full-gate:1111111111111111111111111111111111111111' 'queue reuse names the proven PR head'
for mode in gate-tree-mismatch gate-status-other-tree gate-failed gate-superseded gate-other-context gate-missing error; do
    output_path="$(run_queue_gate_guard "$mode")"
    require_text "$output_path" 'run-fast-validation=true' "$mode full-gate evidence fails closed to the full queue validation"
    require_absent "$output_path" 'run-fast-validation=false' "$mode full-gate evidence never skips the queue validation"
done
non_queue_output="$guard_tmp/queue-non-pr-ref.output"
: >"$non_queue_output"
GITHUB_OUTPUT="$non_queue_output" GITHUB_EVENT_NAME=merge_group GITHUB_REF=refs/heads/gh-readonly-queue/main/unknown \
    GITHUB_REPOSITORY=example/repo FAKE_GH_MODE=gate-hit FAKE_TREE="$guard_tree" \
    FAKE_GH_LOG="$guard_tmp/gh.log" PATH="$guard_tmp/bin:$PATH" "$merge_queue_guard" >/dev/null
require_absent "$(<"$non_queue_output")" 'run-fast-validation=false' 'a queue ref without a PR number never skips'

mutated_guard="$guard_tmp/check-ci-merge-queue-validation-mutated.sh"
sed 's/\.event == "merge_group"/.event == "push"/' "$merge_queue_guard" >"$mutated_guard"
chmod +x "$mutated_guard"
mutation_output="$guard_tmp/merge-event-mutation.output"
: >"$mutation_output"
GITHUB_OUTPUT="$mutation_output" GITHUB_EVENT_NAME=push GITHUB_REF=refs/heads/main \
    GITHUB_REPOSITORY=example/repo FAKE_GH_MODE=merge-hit FAKE_GH_LOG="$guard_tmp/gh.log" \
    PATH="$guard_tmp/bin:$PATH" "$mutated_guard" >/dev/null
require_text "$(<"$mutation_output")" 'run-fast-validation=true' 'mutating merge_group receipt trust prevents the shortcut'
require_absent "$(<"$mutation_output")" 'run-fast-validation=false' 'mutated event predicate cannot skip the main-push suite'
rm -rf "$guard_tmp"

all_actions="$(python3 "$policy_parser" scalars .github/actions/setup-rust-linux/action.yml .github/workflows/ci.yml .github/workflows/release.yml)"
if grep -qF 'mozilla-actions/sccache-action@v0.0.5' <<<"$all_actions"; then
    printf 'FAIL retired sccache action v0.0.5 remains\n'
    fail=$((fail + 1))
else
    printf 'ok   retired sccache action v0.0.5 is absent\n'
    pass=$((pass + 1))
fi
release_publish="$(release_job_block release)"
create_release_position="$(named_step_position "$release_publish" 'Create Release')"
install_path_dispatch_position="$(named_step_position "$release_publish" 'Dispatch install path proof')"
if [[ -n "$create_release_position" && -n "$install_path_dispatch_position" \
    && "$install_path_dispatch_position" -gt "$create_release_position" ]]; then
    printf 'ok   install-path proof dispatch runs after release publication\n'
    pass=$((pass + 1))
else
    printf 'FAIL install-path proof dispatch must run after release publication\n'
    fail=$((fail + 1))
fi

release_create_body="$(python3 "$policy_parser" release-run | sed -E 's/\$\{\{[^}]+\}\}/workflow-expression/g')"
retry_tmp="$(mktemp -d)"
trap 'rm -rf "$retry_tmp"' EXIT
mkdir -p "$retry_tmp/bin"
git -C "$repo_root" log --pretty=format:'- %s' 42219dce^..42219dce > "$retry_tmp/notes.md"
if grep -qFx -- '- Revert "docs(cas-a073): sweep Grok changelog through 1.0.40"' "$retry_tmp/notes.md"; then
    echo 'ok   quoted v3.28.0 commit subject remains literal release-note data'
else
    echo 'FAIL quoted v3.28.0 commit subject changed during note generation'
    fail=$((fail + 1))
fi
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
existing_output="$(GITHUB_REF=refs/tags/v9.9.9 RUNNER_TEMP="$retry_tmp" FAKE_RELEASE_EXISTS=0 FAKE_GH_LOG="$retry_tmp/creates" PATH="$retry_tmp/bin:$PATH" bash -c "$release_create_body" 2>&1)"
existing_status=$?
set -e
test "$existing_status" -eq 1
grep -qF 'Release v9.9.9 already exists; refusing to replace its assets' <<<"$existing_output"
test ! -e "$retry_tmp/creates"
echo 'ok   partial-release retry refuses loudly and does not upload replacement bytes'

GITHUB_REF=refs/tags/v9.9.9 RUNNER_TEMP="$retry_tmp" FAKE_RELEASE_EXISTS=1 FAKE_GH_LOG="$retry_tmp/creates" PATH="$retry_tmp/bin:$PATH" bash -c "$release_create_body"
grep -qF 'release create v9.9.9' "$retry_tmp/creates"
grep -qF -- "--notes-file $retry_tmp/notes.md" "$retry_tmp/creates"
echo 'ok   first release run creates the release when no object exists'


latency_receipt="$repo_root/scripts/release-latency-receipt.sh"
if [[ -x "$latency_receipt" ]]; then
    printf 'ok   release latency receipt is executable\n'
    pass=$((pass + 1))
else
    printf 'FAIL release latency receipt must exist and be executable\n'
    fail=$((fail + 1))
fi

for guard_script in detect-pending-release find-release-prebuild check-release-runner-trust release-latency-receipt; do
    if [[ -x "$repo_root/scripts/test-$guard_script.sh" ]]; then
        printf 'ok   %s has an executable self-test\n' "$guard_script"
        pass=$((pass + 1))
    else
        printf 'FAIL %s has no executable self-test\n' "$guard_script"
        fail=$((fail + 1))
    fi
done

rm -rf "$retry_tmp"
trap - EXIT

if [[ -x "$fallback" ]] \
    && "$fallback" --show-stats | grep -qF 'sccache unavailable; build ran uncached' \
    && "$fallback" --show-stats --stats-format=json | jq -e '.stats.compile_requests == 0 and .stats.cache_hits.counts == {}' >/dev/null; then
    printf 'ok   sccache post fallback is executable and emits valid zero stats\n'
    pass=$((pass + 1))
else
    printf 'FAIL sccache post fallback must be executable and emit valid zero stats\n'
    fail=$((fail + 1))
fi

summary_script="$repo_root/scripts/ci-sccache-summary.sh"

declare -A compiling_lanes=(
    [scoped-validation]='Scoped Validation'
    [scoped-validation-fast]='Scoped Validation (fast)'
    [fast-validation-preflight]='Fast Validation — preflight'
    [fast-validation-suite-build]='Fast Validation — suite archive build'
    [fast-validation-docs]='Fast Validation — doctests'
    [clippy]='Clippy'
    [macos-check]='macOS Check'
    [test-compile-guard]='Test Compile Guard'
    [panic-isolation-release]='Panic Isolation — release profile'
    [panic-isolation-release-fast]='Panic Isolation — release-fast profile'
)

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

printf '\ntest result: %s passed; %s failed; %s skipped\n' "$pass" "$fail" "$skip"
if [[ "$fail" -ne 0 ]]; then
    exit 1
fi
