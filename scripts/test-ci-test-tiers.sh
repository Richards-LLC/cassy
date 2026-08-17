#!/usr/bin/env bash
# Static contract test for the two-tier CI policy introduced by cas-eb39.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ci="$repo_root/.github/workflows/ci.yml"
release="$repo_root/.github/workflows/release.yml"
setup="$repo_root/.github/actions/setup-rust-linux/action.yml"
fallback="$repo_root/scripts/sccache-unavailable.sh"
ruleset="$repo_root/docs/branch-protection/main-ruleset.json"
makefile="$repo_root/cas-cli/Makefile"

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
require_text "$ci_text" 'make -C cas-cli test-ci-tiers' 'Fast Validation invokes release publication guard scripts'

scoped="$(job_block scoped-validation)"
require_text "$scoped" "refs/heads/factory/" 'scoped tier selects factory branches'
require_text "$scoped" "github.event_name == 'pull_request'" 'pull requests use scoped tier'
require_text "$scoped" 'github.base_ref != github.event.repository.default_branch' 'scoped tier skips protected-default PRs'
require_text "$scoped" 'cargo check -p cas --lib --tests' 'scoped tier checks target surface'
require_text "$scoped" 'scripts/run-scoped-tests.sh -p cas --lib' 'scoped tier runs one test binary'

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
    require_text "$block" 'github.event.pull_request.head.sha || github.sha' "$job deduplicates push/PR runs by head SHA"
    require_text "$block" 'cancel-in-progress: false' "$job queues duplicate required-check runs without cancellation"
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
require_text "$suite_build" 'target/debug/cas' 'suite archive preserves the CLI path used by integration tests'
require_text "$suite_shards" 'needs: fast-validation-suite-build' 'shards wait for the shared test archive'
require_text "$suite_shards" 'actions/download-artifact@v4' 'shards download the shared nextest archive'
require_text "$suite_shards" 'cargo nextest run --archive-file fast-validation-suite.tar.zst --no-fail-fast --partition count:${{ matrix.shard }}/3' 'shards execute every archived workspace nextest binary exactly once'
require_text "$suite" 'needs: fast-validation-suite-shards' 'required full-suite context fans in every shard'
require_text "$suite" 'test "$SHARDS" = success' 'required full-suite context rejects failed shards'
require_text "$(<"$makefile")" '$(CARGO) nextest run --workspace --no-fail-fast' 'local make test matches CI workspace nextest scope'
require_text "$docs" 'cargo test -p cas --doc' 'doctest coverage remains in Fast Validation'
require_text "$fan_in" 'fast-validation-preflight' 'required Fast Validation waits for preflight'
require_text "$fan_in" 'fast-validation-suite' 'required Fast Validation waits for the full suite'
require_text "$fan_in" 'fast-validation-docs' 'required Fast Validation waits for doctests'
require_text "$fan_in" 'test "$PREFLIGHT" = success' 'required Fast Validation rejects a failed preflight'
require_text "$fan_in" 'test "$SUITE" = success' 'required Fast Validation rejects a failed full suite'
require_text "$fan_in" 'test "$DOCS" = success' 'required Fast Validation rejects failed doctests'

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

printf '\ntest result: %s passed; %s failed\n' "$pass" "$fail"
if [[ "$fail" -ne 0 ]]; then
    exit 1
fi
