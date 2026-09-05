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
snapshot_router="$repo_root/scripts/check-scoped-snapshot-tests.sh"
snapshot_router_test="$repo_root/scripts/test-check-scoped-snapshot-tests.sh"
watchdog="$repo_root/.github/workflows/merge-queue-watchdog.yml"
watchdog_script="$repo_root/scripts/cancel-stale-merge-group-runs.sh"
runner_unit="$repo_root/ops/systemd/cassy-actions-runner.service"
runner_unit_2="$repo_root/ops/systemd/cassy-actions-runner-2.service"
runner_wrapper_2="$repo_root/ops/systemd/run-cassy-actions-runner-2.sh"
runner_installer="$repo_root/scripts/install-cassy-actions-runner.sh"
runner_isolation="$repo_root/scripts/check-cassy-actions-runner-isolation.sh"
rust_setup="$repo_root/scripts/setup-cassy-actions-rust.sh"
release_runner_trust="$repo_root/scripts/check-release-runner-trust.sh"
stale_queue_watchdog="$repo_root/.github/workflows/stale-queued-run-watchdog.yml"
stale_queue_script="$repo_root/scripts/cancel-stale-non-merge-group-queued-runs.sh"
watchdog_policy="$repo_root/scripts/watchdog-policy.sh"
watchdog_behavior_test="$repo_root/scripts/test-watchdog-scripts.sh"

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

named_step_block() {
    local block="$1" name="$2"
    awk -v header="      - name: $name" '
        $0 == header { inside = 1; next }
        inside && /^      - / { exit }
        inside { print }
    ' <<<"$block"
}

named_step_position() {
    local block="$1" name="$2"
    awk -v header="      - name: $name" '$0 == header { print NR; exit }' <<<"$block"
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

# Self-hosted pilot security contract (cas-f5638). This repo is public, so
# fork/untrusted PR code must be unable to request the persistent runner. The
# old push-only pilot remains advisory; the merge-queue route below is the only
# required-path use of the box.
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
require_text "$self_hosted_text" 'RUSTC_WRAPPER: ""' 'pilot archive timing is not coupled to sccache availability'
require_text "$self_hosted_text" './scripts/check-cassy-actions-runner-isolation.sh' 'self-hosted job verifies an approved target/cache/port tuple'
require_absent "$self_hosted_text" 'sccache --start-server' 'workflow steps cannot start a cache server that Runner.Worker will reap'
require_absent "$self_hosted_text" 'sccache --zero-stats' 'pilot workflow does not depend on a cache server for its first receipt'
require_text "$self_hosted_text" 'cargo nextest archive --workspace --archive-file fast-validation-suite.tar.zst' 'self-hosted pilot measures the same suite archive'
require_text "$self_hosted_text" 'TMPDIR: ${{ runner.temp }}' 'self-hosted pilot keeps large temporary files off tmpfs'
require_absent "$self_hosted_text" 'cargo nextest run' 'self-hosted pilot leaves suite execution on hosted runners'
require_absent "$self_hosted_text" '--partition' 'self-hosted pilot does not duplicate hosted shard execution'
require_text "$self_hosted_text" './scripts/setup-cassy-actions-rust.sh' 'self-hosted pilot uses shared-home-safe Rust setup'
require_absent "$self_hosted_text" 'dtolnay/rust-toolchain@stable' 'self-hosted pilot does not mutate shared rustup through an action'
require_absent "$(<"$ruleset")" 'Self-hosted pilot' 'self-hosted pilot is not a required status check'

if [[ -x "$rust_setup" ]]; then
    printf 'ok   shared self-hosted Rust setup script is executable\n'
    pass=$((pass + 1))
else
    printf 'FAIL shared self-hosted Rust setup script is executable\n'
    fail=$((fail + 1))
fi
rust_setup_text="$(<"$rust_setup")"
require_text "$rust_setup_text" 'flock -x 9' 'shared Rust setup serializes rustup mutation'
require_text "$rust_setup_text" 'toolchain list' 'shared Rust setup checks the pre-provisioned toolchain first'
require_text "$rust_setup_text" 'toolchain install stable --profile minimal' 'shared Rust setup repairs a missing toolchain under the lock'
require_text "$rust_setup_text" 'RUSTUP_TOOLCHAIN=stable' 'shared Rust setup avoids changing the shared rustup default'

for job in fast-validation-preflight fast-validation-suite-build fast-validation-docs; do
    block="$(job_block "$job")"
    require_text "$block" 'name: Set up shared self-hosted Rust toolchain' "$job uses the shared-home-safe Rust setup"
    require_text "$block" 'run: ./scripts/setup-cassy-actions-rust.sh' "$job invokes the shared Rust setup helper"
    require_absent "$block" 'dtolnay/rust-toolchain@stable' "$job does not mutate shared rustup through an action"
done

pilot_doc="$(<"$repo_root/docs/ci/self-hosted-runner-pilot.md")"
require_text "$pilot_doc" 'restricted_to_workflows=false' 'runner-group policy permits synthetic merge-queue refs'
require_text "$pilot_doc" 'refs/heads/gh-readonly-queue/...' 'pilot documents queue-ref mismatch'
require_text "$pilot_doc" 'selected-workflow wildcards are rejected' 'pilot records GitHub wildcard limitation'
require_text "$pilot_doc" 'CARGO_CACHE_RUSTC_INFO=0' 'pilot documents Cargo rustc-info cache containment'
require_text "$pilot_doc" 'approval_policy=all_external_contributors' 'pilot pins approval for every outside-contributor fork workflow'
require_text "$pilot_doc" 'Ephemeral/JIT runners remain future' 'pilot records the deferred runner-isolation alternative'

runner_unit_text="$(<"$runner_unit")"
runner_unit_2_text="$(<"$runner_unit_2")"
require_text "$runner_unit_text" 'Environment=CARGO_CACHE_RUSTC_INFO=0' 'runner does not persist failed sccache rustc probes across jobs'
require_text "$runner_unit_text" 'Environment=SCCACHE_IDLE_TIMEOUT=0' 'runner keeps its private sccache server alive between merge-queue jobs'
require_text "$runner_unit_text" 'TasksMax=2048' 'runner reserves enough cgroup task slots for parallel sccache compiler spawns'
require_text "$runner_unit_2_text" 'WorkingDirectory=/var/lib/cassy-actions/runner-2' 'slot 2 has an independent runner checkout'
require_text "$runner_unit_2_text" 'Environment=CARGO_TARGET_DIR=/var/lib/cassy-actions/cache/cargo-target-2' 'slot 2 has an independent Cargo target'
require_text "$runner_unit_2_text" 'Environment=SCCACHE_DIR=/var/lib/cassy-actions/cache/sccache-2' 'slot 2 has an independent sccache directory'
require_text "$runner_unit_2_text" 'Environment=SCCACHE_SERVER_PORT=4228' 'slot 2 has an independent sccache server port'
require_text "$runner_unit_2_text" '/var/lib/cassy-actions/run-service-2.sh' 'slot 2 unit starts its dedicated wrapper'
require_text "$(<"$runner_wrapper_2")" '/var/lib/cassy-actions/runner-2/bin/runsvc.sh' 'slot 2 wrapper starts its independent listener'
require_text "$(<"$runner_installer")" 'RUNNER_SLOT must be 1 or 2' 'runner installer rejects unbounded slot identifiers'
require_text "$(<"$runner_installer")" 'runner_name=soundwave-cas-ci-2' 'runner installer registers a distinct slot-2 name'
require_text "$(<"$runner_installer")" 'service_name=cassy-actions-runner-2.service' 'runner installer enables the slot-2 unit'
require_text "$(<"$release_runner_trust")" 'check-cassy-actions-runner-isolation.sh' 'release trust guard accepts only approved slot tuples'
require_text "$pilot_doc" '2,048 cgroup task slots' 'pilot documents the parallel sccache task-slot budget'

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
    watchdog_text="$(<"$watchdog")"
    watchdog_script_text="$(<"$watchdog_script")"
    require_text "$watchdog_text" "cron: '*/5 * * * *'" 'merge-queue watchdog runs at GitHub’s five-minute floor'
    require_text "$watchdog_text" 'actions: write' 'merge-queue watchdog may cancel an orphaned run'
    require_text "$watchdog_text" 'run: ./scripts/cancel-stale-merge-group-runs.sh' 'merge-queue watchdog invokes its cancellation script'
    require_text "$watchdog_text" 'GITHUB_REPOSITORY: ${{ github.repository }}' 'merge-queue watchdog passes its explicit repository'
    require_text "$watchdog_text" "CASSY_WATCHDOG_DRY_RUN: 'false'" 'merge-queue watchdog disables dry-run in scheduled execution'
    require_text "$watchdog_script_text" 'event=merge_group&status=in_progress' 'watchdog inspects in-progress merge-group runs'
    require_text "$watchdog_script_text" 'event=merge_group&status=queued' 'watchdog also reclaims queued pre-claim starvation'
    require_text "$watchdog_script_text" "printf '%s\\n%s\\n'" 'watchdog combines in-progress and queued API responses'
    require_text "$watchdog_script_text" 'Fast Validation — suite archive build' 'watchdog scopes merge-group cancellation to the archive job'
    require_text "$watchdog_script_text" '.name == $name' 'watchdog scopes merge-group cancellation to CI workflow jobs'
    require_text "$watchdog_script_text" 'current_job_status' 'watchdog rechecks archive job state before cancellation'
    require_text "$(<"$watchdog_policy")" 'actions/runs/$run_id/cancel' 'watchdog cancels stale runs by id'
    require_text "$watchdog_script_text" 'age_seconds > hang_seconds' 'watchdog does not cancel at or below threshold'
else
    printf 'FAIL merge-queue watchdog script is executable\n'
    fail=$((fail + 1))
fi

suite_build="$(job_block fast-validation-suite-build)"
suite_shards="$(job_block fast-validation-suite-shards)"
route="$(job_block fast-validation-runner-route)"
require_text "$route" "github.event_name == 'merge_group'" 'runner route only offers the box to merge-queue trees'
require_text "$route" 'vars.CASSY_MERGE_QUEUE_SELF_HOSTED' 'runner route requires explicit self-hosted opt-in'
require_text "$route" '"$SELF_HOSTED_ENABLED" == enabled' 'runner route defaults to hosted unless the box is declared ready'
require_text "$route" 'runner=["self-hosted","Linux","X64","cas-ci-32core"]' 'merge-queue route selects the isolated runner label set'
require_text "$route" 'runner=["ubuntu-latest"]' 'disabled or non-queue traffic routes to hosted runners'
require_text "$route" 'mode=self-hosted' 'runner route exposes selected self-hosted mode'
require_text "$route" 'mode=hosted' 'runner route exposes selected hosted fallback mode'
require_absent "$route" 'actions/checkout' 'runner route does not execute merge-queue source before label selection'

require_text "$suite_build" 'needs: [fast-validation-runner-route, fast-validation-main-push-dedupe]' 'required archive build waits for explicit runner routing and the main-push tree gate'
require_text "$suite_build" 'fromJSON(needs.fast-validation-runner-route.outputs.runner)' 'archive build receives the fail-safe selected runner labels'
require_text "$suite_build" "needs.fast-validation-runner-route.outputs.mode != 'self-hosted'" 'archive rejects an untrusted self-hosted route before assignment'
require_text "$suite_build" "github.event_name == 'merge_group'" 'self-hosted archive is restricted to merge-queue events'
require_text "$suite_build" "github.repository == 'Richards-LLC/cassy'" 'self-hosted archive pins the canonical repository'
require_text "$suite_build" "vars.CASSY_MERGE_QUEUE_SELF_HOSTED == 'enabled'" 'self-hosted archive repeats explicit readiness opt-in'
require_text "$suite_build" "needs.fast-validation-runner-route.outputs.mode == 'hosted'" 'hosted setup remains selected for PR and fallback validation'
require_text "$suite_build" "needs.fast-validation-runner-route.outputs.mode == 'self-hosted'" 'self-hosted setup is limited to selected merge-queue validation'
require_text "$suite_build" 'Verify merge-queue self-hosted trust boundary' 'self-hosted archive verifies queue-only trust at execution'
require_text "$suite_build" 'refs/heads/gh-readonly-queue/*' 'self-hosted archive rejects non-queue refs'
suite_trust_step="$(named_step_block "$suite_build" 'Verify merge-queue self-hosted trust boundary')"
require_text "$suite_trust_step" './scripts/check-cassy-actions-runner-isolation.sh' 'self-hosted archive fail-closed trust step pins an approved slot tuple'
for job in fast-validation-preflight fast-validation-docs; do
    block="$(job_block "$job")"
    require_text "$block" 'needs: [fast-validation-runner-route, fast-validation-main-push-dedupe]' "$job waits for explicit runner routing and the main-push tree gate"
    require_text "$block" 'fromJSON(needs.fast-validation-runner-route.outputs.runner)' "$job receives the fail-safe selected runner labels"
    require_text "$block" "needs.fast-validation-runner-route.outputs.mode != 'self-hosted'" "$job rejects an untrusted self-hosted route before assignment"
    require_text "$block" "github.event_name == 'merge_group'" "$job limits self-hosted execution to merge-queue events"
    require_text "$block" "github.repository == 'Richards-LLC/cassy'" "$job pins self-hosted execution to the canonical repository"
    require_text "$block" "vars.CASSY_MERGE_QUEUE_SELF_HOSTED == 'enabled'" "$job repeats explicit self-hosted readiness opt-in"
    require_text "$block" "needs.fast-validation-runner-route.outputs.mode == 'hosted'" "$job retains hosted setup as the fail-safe"
    require_text "$block" "needs.fast-validation-runner-route.outputs.mode == 'self-hosted'" "$job limits isolated runner setup to selected merge-queue validation"
    require_text "$block" 'Verify merge-queue self-hosted trust boundary' "$job verifies queue-only trust at execution"
    require_text "$block" 'refs/heads/gh-readonly-queue/*' "$job rejects non-queue refs on the isolated runner"
    trust_step="$(named_step_block "$block" 'Verify merge-queue self-hosted trust boundary')"
    require_text "$trust_step" './scripts/check-cassy-actions-runner-isolation.sh' "$job fail-closed trust step pins an approved slot tuple"
    require_text "$block" 'Verify private self-hosted sccache' "$job confirms the persistent compiler cache remains isolated"
done
probe_step="$(named_step_block "$suite_build" 'Verify private self-hosted sccache')"
disable_step="$(named_step_block "$suite_build" 'Disable sccache for the self-hosted suite archive')"
archive_step="$(named_step_block "$suite_build" 'Build full suite archive')"
require_text "$probe_step" 'continue-on-error: true' 'self-hosted cache probe itself cannot fail the merge queue'
require_absent "$probe_step" './scripts/check-cassy-actions-runner-isolation.sh' 'fail-open cache probe does not own the slot trust decision'
require_text "$probe_step" 'cas-065a' 'self-hosted cache probe cites the tracked server defect'
require_text "$disable_step" "steps.classify-diff.outputs.rust-unaffected != 'true'" 'wrapper disable step requires Rust work'
require_text "$disable_step" "needs.fast-validation-runner-route.outputs.mode == 'self-hosted'" 'wrapper disable step is self-hosted only'
require_text "$disable_step" "echo 'RUSTC_WRAPPER=' >> \"\$GITHUB_ENV\"" 'wrapper disable step clears the compiler wrapper before Cargo'
require_absent "$disable_step" 'SCCACHE_GHA_ENABLED=false' 'wrapper disable step does not inject a cache backend prerequisite'

disable_position="$(named_step_position "$suite_build" 'Disable sccache for the self-hosted suite archive')"
archive_position="$(named_step_position "$suite_build" 'Build full suite archive')"
suite_archive_fail_open_contract_holds() {
    local probe="$1" disable="$2" archive="$3" disable_position="$4" archive_position="$5"
    [[ "$probe" == *'continue-on-error: true'* ]] \
        && [[ "$disable" == *"steps.classify-diff.outputs.rust-unaffected != 'true'"* ]] \
        && [[ "$disable" == *"needs.fast-validation-runner-route.outputs.mode == 'self-hosted'"* ]] \
        && [[ "$disable" == *"echo 'RUSTC_WRAPPER=' >> \"\$GITHUB_ENV\""* ]] \
        && [[ -n "$archive" ]] \
        && [[ -n "$disable_position" && -n "$archive_position" ]] \
        && (( disable_position < archive_position ))
}

if suite_archive_fail_open_contract_holds "${probe_step/continue-on-error: true/}" "$disable_step" "$archive_step" "$disable_position" "$archive_position"; then
    printf 'FAIL suite-archive mutation removes probe fail-open\n'
    fail=$((fail + 1))
else
    printf 'ok   suite-archive mutation catches removed probe fail-open\n'
    pass=$((pass + 1))
fi
if suite_archive_fail_open_contract_holds "$probe_step" "${disable_step/self-hosted/hosted}" "$archive_step" "$disable_position" "$archive_position"; then
    printf 'FAIL suite-archive mutation removes wrapper-disable self-hosted gate\n'
    fail=$((fail + 1))
else
    printf 'ok   suite-archive mutation catches removed wrapper-disable self-hosted gate\n'
    pass=$((pass + 1))
fi
if suite_archive_fail_open_contract_holds "$probe_step" "${disable_step/RUSTC_WRAPPER=/RUSTC_WRAPPER=sccache}" "$archive_step" "$disable_position" "$archive_position"; then
    printf 'FAIL suite-archive mutation restores compiler wrapper\n'
    fail=$((fail + 1))
else
    printf 'ok   suite-archive mutation catches restored compiler wrapper\n'
    pass=$((pass + 1))
fi
if suite_archive_fail_open_contract_holds "$probe_step" "$disable_step" "$archive_step" "$archive_position" "$disable_position"; then
    printf 'FAIL suite-archive mutation reorders wrapper disable after archive build\n'
    fail=$((fail + 1))
else
    printf 'ok   suite-archive mutation catches wrapper disable after archive build\n'
    pass=$((pass + 1))
fi
require_text "$suite_shards" 'runs-on: ubuntu-latest' 'required suite shards retain hosted parallel execution and availability'

scoped="$(job_block scoped-validation)"
require_text "$scoped" "refs/heads/factory/" 'scoped tier selects factory branches'
require_text "$scoped" "github.event_name == 'pull_request'" 'pull requests use scoped tier'
require_text "$scoped" 'github.base_ref != github.event.repository.default_branch' 'scoped tier skips protected-default PRs'
require_text "$scoped" 'cargo check -p cas --lib --tests' 'scoped tier checks target surface'
require_text "$scoped" 'scripts/run-scoped-tests.sh -p cas --lib' 'scoped tier runs one guarded test binary'
require_text "$scoped" 'Test snapshot-pinned CLI output surfaces' 'scoped tier names the snapshot surface target'
require_text "$scoped" 'scripts/check-scoped-snapshot-tests.sh --base-sha' 'scoped tier routes mapped snapshot surfaces'
require_text "$scoped" 'github.event.merge_group.base_sha' 'snapshot router receives merge-group base'
require_text "$scoped" 'origin/${{ github.event.repository.default_branch }}' 'snapshot router has a trusted zero-base fallback'

# Merge-latency contract for the scoped lane (cas-3e14). This lane is not a
# required check, but it reports last on a factory PR, so a merge that waits for
# every check waits for it. Measured on PR #479: 8m28s of Rust work on a
# docs-only diff, finishing 9s before the merge and 8m after the required lanes
# were green. Both mitigations below must stay, and both must stay fail-closed.
require_text "$scoped" 'id: classify-diff' 'scoped lane classifies before expensive work'
require_text "$scoped" './.github/actions/classify-required-diff' 'scoped lane uses the shared classifier'
require_text "$scoped" 'fetch-depth: 0' 'scoped lane computes a real merge base'
require_text "$scoped" 'github.event.pull_request.head.sha || github.sha' 'scoped lane classifies the PR head rather than its synthetic merge commit'
require_text "$scoped" 'github.event.pull_request.base.sha || github.event.merge_group.base_sha || github.event.before' 'scoped lane selects the event-specific comparison base'
require_text "$scoped" 'zero-base-ref: origin/${{ github.event.repository.default_branch }}' 'first factory push compares against the protected branch when before is all-zero'
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
    'scripts/run-scoped-tests.sh -p cas --lib' \
    'scripts/check-scoped-snapshot-tests.sh --base-sha'; do
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

# The snapshot router is deliberately a separate, conditional target inside
# Scoped Validation: it catches doctor_snapshot staleness before the merge
# queue (PRs #649/#650; PR #657 run 33435790948) without creating a required
# lane or widening ordinary library coverage.
require_text "$(<"$snapshot_router")" 'component_output_test__doctor_snapshot.snap|cas-cli/src/cli/doctor.rs|component_output_test' \
    'doctor.rs maps to component_output_test'
require_text "$(<"$snapshot_router")" 'component_output_test__status_empty_snapshot.snap|cas-cli/src/cli/status.rs|component_output_test' \
    'status.rs maps to component_output_test'
require_text "$(<"$snapshot_router")" 'no Scoped Validation mapping' \
    'unmapped committed snapshots fail loudly'
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
require_text "$(<"$makefile")" 'test-check-scoped-snapshot-tests.sh' \
    'test-ci-tiers runs the snapshot router self-test'

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
    require_text "$block" 'github.event.merge_group.base_sha' "$job compares merge-queue trees against the event base SHA"
    require_text "$block" "steps.classify-diff.outputs.rust-unaffected" "$job gates Rust work only after a safe classification"
done

classifier="$repo_root/scripts/classify-ci-diff.sh"
classify_action="$repo_root/.github/actions/classify-required-diff/action.yml"
if [[ -x "$classifier" ]]; then
    # These committed fixtures pin both directions: the explicitly safe
    # classes are Rust-unaffected, while every code or mixed change is full.
    require_text "$("$classifier" 967e85c7^ 967e85c7)" 'empty' 'empty ancestry merge fast-passes'
    require_text "$("$classifier" c6c4122f^ c6c4122f)" 'docs-only' 'docs-only change fast-passes'
    require_text "$("$classifier" 49b434bf^ 49b434bf)" 'docs-only' 'PR 630 CODEMAP-only change fast-passes'
    require_text "$("$classifier" 7c233bef^ 7c233bef)" 'hub-web-only' 'hub-web-only change skips Rust work'
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

# A branch-creation push also carries an all-zero `before`, but unlike a tag it
# has a protected-default comparison ref. The shared action must use that ref;
# an identical tree is the minimal empty-diff event fixture and must fast-pass.
first_branch_output="$(mktemp)"
if BASE_SHA="0000000000000000000000000000000000000000" \
    ZERO_BASE_REF="HEAD" \
    GITHUB_OUTPUT="$first_branch_output" \
    bash < <(awk '
        /^      run: \|$/ { in_run = 1; next }
        in_run { sub(/^        /, ""); print }
    ' "$classify_action"); then
    require_text "$(<"$first_branch_output")" 'class=empty' 'first branch push uses its protected-base fallback'
    require_text "$(<"$first_branch_output")" 'fast-pass=true' 'empty first branch push fast-passes without Rust'
    require_text "$(<"$first_branch_output")" 'rust-unaffected=true' 'empty first branch push skips Cargo'
else
    printf 'FAIL first branch push runs the shared classifier action\n'
    fail=$((fail + 1))
fi
rm -f "$first_branch_output"

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

# Merge-queue latency contract (cas-6600): protected PRs emit both required
# contexts quickly on hosted runners, then the canonical merge_group tree gets
# the expensive Fast Validation and Darwin work. This retains coverage on the
# exact tree that lands while avoiding a duplicate PR-head build.
fast_admission="$(job_block fast-validation)"
macos="$(job_block macos-check)"
for job in fast-validation-runner-route fast-validation-preflight fast-validation-suite-build fast-validation-suite-shards fast-validation-suite fast-validation-docs; do
    require_absent "$(job_block "$job")" "github.event_name == 'pull_request'" \
        "$job defers exhaustive PR-head work to the merge queue"
done
require_text "$fast_admission" 'Admit protected PR to merge-queue validation' 'Fast Validation emits a PR admission receipt'
require_text "$fast_admission" "if: github.event_name == 'pull_request'" 'Fast Validation admission is PR-only'
require_text "$fast_admission" "github.event_name != 'pull_request'" 'Fast Validation only gates dependencies on the canonical tree'
require_text "$macos" "github.event_name == 'pull_request' && 'ubuntu-latest' || 'macos-26'" 'macOS PR admission stays hosted while queue validation uses macOS'
require_text "$macos" 'Admit protected PR to merge-queue Darwin validation' 'macOS Check emits a PR admission receipt'
require_text "$macos" "github.event_name != 'pull_request'" 'macOS compilation is deferred to the canonical tree'
require_absent "$macos" 'cargo check -p cas --no-default-features' 'macOS does not duplicate the Linux no-MCP-proxy build'

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
require_text "$suite_build" 'CARGO_TARGET_DIR:-target}/debug' 'suite packaging reads the configured Cargo target directory'
require_text "$suite_build" "--transform='s,^cas$,target/debug/cas,'" 'suite packaging preserves the hosted runner archive layout'
require_text "$suite_shards" 'needs: [fast-validation-suite-build, fast-validation-main-push-dedupe]' 'shards wait for the shared test archive and main-push tree gate'
require_text "$suite_shards" 'actions/download-artifact@v4' 'shards download the shared nextest archive'
require_text "$suite_shards" 'tar -xzf fast-validation-suite-runner.tar.gz' 'shards restore the executable CLI runner payload'
require_text "$suite_shards" 'test -x target/debug/cas' 'shards verify the restored CLI runner remains executable'
require_absent "$suite_build" 'producer-mode:' 'archive has no consumer compatibility producer mode'
require_absent "$suite_shards" 'Restore self-hosted producer paths' 'shards do not restore producer-machine paths'
require_absent "$suite_shards" '/var/lib/cassy-actions/' 'shards do not depend on self-hosted producer filesystem paths'
require_text "$suite_shards" '--workspace-remap "$GITHUB_WORKSPACE"' 'shards remap the self-hosted archive workspace to their hosted checkout'
require_text "$suite_shards" 'INSTA_WORKSPACE_ROOT: ${{ github.workspace }}' 'shards pin insta snapshot lookup to their hosted checkout'
require_text "$suite_shards" 'scripts/run-verified-tests.sh nextest run --archive-file fast-validation-suite.tar.zst --workspace-remap "$GITHUB_WORKSPACE" --no-fail-fast --partition count:${{ matrix.shard }}/3' 'shards execute every archived workspace nextest binary exactly once'
require_text "$suite" 'needs: [fast-validation-suite-shards, fast-validation-main-push-dedupe]' 'required full-suite context fans in every shard after the main-push tree gate'
require_text "$suite" 'test "$SHARDS" = success' 'required full-suite context rejects failed shards'
require_text "$(<"$makefile")" '../scripts/run-verified-tests.sh nextest run --workspace --no-fail-fast' 'local make test verifies CI workspace nextest scope'
require_text "$docs" 'scripts/run-verified-tests.sh test -p cas --doc' 'doctest coverage remains in Fast Validation with an execution receipt'
require_text "$preflight" 'cargo build -p cas --no-default-features' 'Linux preflight retains no-MCP-proxy compilation coverage'
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

# Protected PRs emit only the required admission contexts. Compiling heavy
# lanes stay on integration pushes and supervisor-controlled runs.
require_text "$scoped" 'github.base_ref != github.event.repository.default_branch' 'non-required scoped lane skips main PRs'
for job in clippy test-compile-guard; do
    block="$(job_block "$job")"
    require_text "$block" "refs/heads/main" "$job runs on main"
    require_text "$block" "github.event_name == 'schedule'" "$job runs on schedule"
    require_text "$block" "github.event_name == 'workflow_dispatch'" "$job supports supervisor dispatch"
    require_absent "$block" "github.event_name == 'pull_request'" "$job never compiles protected PR heads"
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

# Heavy validation must run only at protected integration points. A second
# merge to main/staging must cancel each individual heavy lane from the older
# tree, without ever sharing a concurrency group with schedule/dispatch work.
require_text "$ci_text" '- staging' 'CI accepts staging integration pushes'
heavy_job_contract_holds() {
    local block="$1" group="$2"
    [[ "$block" == *"group: $group"* ]] \
        && [[ "$block" == *'cancel-in-progress: true'* ]] \
        && [[ "$block" != *'refs/heads/epic/'* ]] \
        && [[ "$block" != *'refs/heads/factory/'* ]]
}

for job in clippy test-compile-guard panic-isolation-release panic-isolation-release-fast build-benchmark; do
    block="$(job_block "$job")"
    group="heavy-tier-$job-\${{ github.event_name }}-\${{ github.ref }}"
    require_text "$block" "group: $group" "$job uses an event- and ref-keyed heavy-tier group"
    require_text "$block" 'cancel-in-progress: true' "$job cancels a superseded heavy run"
    require_text "$block" 'github.event_name' "$job keeps scheduled/dispatch work separate from push work"

    # Mutation coverage: each broken form must fail the same contract rather
    # than merely relying on a reviewer to notice a missing YAML line.
    missing_group="${block//group: $group/}"
    if heavy_job_contract_holds "$missing_group" "$group"; then
        printf 'FAIL heavy-tier mutation removes concurrency group: %s\n' "$job"
        fail=$((fail + 1))
    else
        printf 'ok   heavy-tier mutation catches removed concurrency group: %s\n' "$job"
        pass=$((pass + 1))
    fi
    flipped_cancel="${block/cancel-in-progress: true/cancel-in-progress: false}"
    if heavy_job_contract_holds "$flipped_cancel" "$group"; then
        printf 'FAIL heavy-tier mutation flips cancellation: %s\n' "$job"
        fail=$((fail + 1))
    else
        printf 'ok   heavy-tier mutation catches disabled cancellation: %s\n' "$job"
        pass=$((pass + 1))
    fi
    epic_trigger="$block"$'\n'"      || startsWith(github.ref, 'refs/heads/epic/')"
    if heavy_job_contract_holds "$epic_trigger" "$group"; then
        printf 'FAIL heavy-tier mutation adds epic trigger: %s\n' "$job"
        fail=$((fail + 1))
    else
        printf 'ok   heavy-tier mutation catches epic trigger: %s\n' "$job"
        pass=$((pass + 1))
    fi
done

for job in clippy test-compile-guard; do
    block="$(job_block "$job")"
    require_text "$block" "refs/heads/staging" "$job runs heavy validation on staging pushes"
done

# Stale QUEUED runs are a separate starvation class from cas-065a's
# merge_group orphan watchdog. This sweep sees every queued event, excludes
# merge_group, and rechecks the run before cancelling an eventually-consistent
# list result.
if [[ -x "$stale_queue_script" ]]; then
    stale_watchdog_text="$(<"$stale_queue_watchdog")"
    stale_queue_script_text="$(<"$stale_queue_script")"
    require_text "$stale_watchdog_text" "cron: '*/5 * * * *'" 'stale queued-run watchdog uses GitHub’s five-minute floor'
    require_text "$stale_watchdog_text" 'actions: write' 'stale queued-run watchdog may cancel a stranded run'
    require_text "$stale_watchdog_text" 'run: ./scripts/cancel-stale-non-merge-group-queued-runs.sh' 'stale queued-run watchdog invokes its cancellation script'
    require_text "$stale_watchdog_text" 'GITHUB_REPOSITORY: ${{ github.repository }}' 'stale queued-run watchdog passes its explicit repository'
    require_text "$stale_watchdog_text" "CASSY_WATCHDOG_DRY_RUN: 'false'" 'stale queued-run watchdog disables dry-run in scheduled execution'
    require_text "$stale_queue_script_text" 'actions/runs?status=queued' 'stale queued-run watchdog reads queued runs of every event'
    require_text "$stale_queue_script_text" '[[ "$event" != merge_group ]]' 'stale queued-run watchdog excludes cas-065a merge-group scope'
    require_text "$stale_queue_script_text" "gh api \"repos/\$repository/actions/runs/\$run_id\" --jq '.status'" 'stale queued-run watchdog rechecks status before cancellation'
    require_text "$stale_queue_script_text" 'age_seconds > queue_seconds' 'stale queued-run watchdog preserves runs at or below its threshold'
    require_text "$(<"$watchdog_policy")" 'actions/runs/$run_id/cancel' 'stale queued-run watchdog cancels by id'

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
    require_text "$(<"$watchdog_policy")" 'CASSY_WATCHDOG_STALE_SECONDS:-1200' 'both watchdogs share one threshold authority'
    require_absent "$watchdog_text$stale_watchdog_text" '1200' 'watchdog workflows do not duplicate the threshold'
    require_absent "$(<"$watchdog_policy")$watchdog_text$stale_watchdog_text" 'CASSY_MERGE_GROUP_HANG_SECONDS' 'legacy merge-group threshold has no second authority'
    require_absent "$(<"$watchdog_policy")$watchdog_text$stale_watchdog_text" 'CASSY_NON_MERGE_GROUP_QUEUE_SECONDS' 'legacy queued threshold has no second authority'
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

# Mutation coverage for the safety-critical event predicate: changing the
# receipt trust from merge_group to push must make the merge-queue fixture run
# the full suite rather than silently reusing an unrelated validation.
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

main_push_dedupe="$(job_block fast-validation-main-push-dedupe)"
require_text "$main_push_dedupe" 'scripts/check-ci-merge-queue-validation.sh' 'main-push Fast Validation gate uses the merge-queue receipt guard'
require_text "$main_push_dedupe" 'run-fast-validation: ${{ steps.tree-dedupe.outputs.run-fast-validation }}' 'main-push gate exports its fail-closed run decision'
require_text "$main_push_dedupe" 'validating-run-id: ${{ steps.tree-dedupe.outputs.validating-run-id }}' 'main-push gate exports the validating merge-queue run id'
require_text "$main_push_dedupe" 'prior-run-url: ${{ steps.tree-dedupe.outputs.prior-run-url }}' 'main-push gate exports the validating merge-queue run URL'

for job in fast-validation-preflight fast-validation-suite-build fast-validation-suite-shards fast-validation-suite fast-validation-docs; do
    block="$(job_block "$job")"
    require_text "$block" 'fast-validation-main-push-dedupe' "$job waits for the main-push tree gate"
    require_text "$block" "needs.fast-validation-main-push-dedupe.outputs.run-fast-validation == 'true'" "$job skips only a successfully receipt-matched main tree"
done
require_text "$macos" 'needs: fast-validation-main-push-dedupe' 'macOS Check waits for the same main-push tree gate'
require_text "$macos" "needs.fast-validation-main-push-dedupe.outputs.run-fast-validation == 'true'" 'macOS Check skips only a successfully receipt-matched main tree'
require_text "$fan_in" 'Report successful merge-queue validation reused by this main push' 'Fast Validation rollup gives the main-push shortcut a named receipt'
require_text "$fan_in" 'needs.fast-validation-main-push-dedupe.outputs.validating-run-id' 'Fast Validation notice names the validating merge-queue run'
require_text "$fan_in" 'merge-queue-validated-tree-${{ steps.merge-queue-tree.outputs.hash }}' 'successful merge-group Fast Validation publishes an exact-tree receipt'
require_text "$fan_in" "git rev-parse 'HEAD^{tree}'" 'merge-group receipt keys exact Git contents'

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
release_verify="$(release_job_block verify)"
release_linux="$(release_job_block build)"
release_macos="$(release_job_block build-macos)"
release_publish="$(release_job_block release)"
release_signature_receipt="$(release_job_block verify-published-macos-signature)"
scoped_validation="$(job_block scoped-validation)"
fast_preflight="$(job_block fast-validation-preflight)"
require_absent "$release_linux" 'needs: verify' 'Linux release build starts in parallel with input verification'
require_absent "$release_macos" 'needs: verify' 'macOS release build starts in parallel with input verification'
require_text "$release_publish" 'needs: [prebuilt-lookup, verify, build, build-macos]' 'release publication waits for verification and both platform supply paths'
for platform_build in "$release_linux" "$release_macos"; do
    require_text "$platform_build" '--profile "$RELEASE_PROFILE"' 'platform release build uses the thin-LTO profile'
    require_text "$platform_build" 'strip package/cas' 'platform package strips symbols before publication'
    require_text "$platform_build" '$RELEASE_DIR/cas' 'platform package selects the configured profile output'
done
require_text "$release_linux" 'check-blake3-no-avx512-build.sh "${CARGO_TARGET_DIR:-target}/x86_64-unknown-linux-gnu/$RELEASE_DIR/build"' 'Linux release audits BLAKE3 inputs from the selected profile'
require_text "$release_linux" 'test-check-portable-x86_64-isa.sh package/cas' 'Linux release audits the exact stripped executable'
require_text "$release_linux" 'name: cas-x86_64-unknown-linux-gnu' 'Linux release asset remains required'
require_text "$release_macos" 'name: cas-aarch64-apple-darwin' 'macOS release asset remains required'
require_text "$release_macos" 'codesign --sign - --force package/cas' 'macOS release re-signs the binary after stripping'
require_text "$release_macos" 'codesign --verify --verbose=4 package/cas' 'macOS release verifies the final package signature'
require_text "$release_text" 'workflow_dispatch:' 'release workflow exposes a manual signature-receipt dispatch'
require_text "$release_text" 'macos_artifact_url:' 'manual signature receipt accepts a published artifact URL'
require_text "$release_signature_receipt" 'runs-on: macos-26' 'signature receipt runs on macOS'
require_text "$release_signature_receipt" 'codesign -dv "$package/cas"' 'signature receipt prints macOS signature details'
require_text "$release_signature_receipt" 'codesign --verify --verbose=4 "$package/cas"' 'signature receipt rejects an invalid macOS signature'
require_text "$scoped_validation" './scripts/test-cas-install.sh' 'Scoped Validation runs portable installer fixtures'
require_text "$fast_preflight" './scripts/test-cas-install.sh' 'Fast Validation preflight runs portable installer fixtures'
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

# ---------------------------------------------------------------------------
# Fast release publication contract (cas-3b7c0 / GH #449).
#
# Measured baseline before this lane existed: tag push -> published release ran
# 13m18s (v3.1.0), 14m55s (v3.0.0), 20m03s (v3.2.0) and 26m26s (v2.72.0). The
# critical path was ALWAYS the two cold platform builds — `Create Release`
# itself never took more than 23s. macOS ARM64 alone cost 12-21 minutes and
# cannot move to the fleet, so the artifacts are built when the release-PR
# merges and the tag only publishes them.
#
# Everything below pins the properties that make that safe: the prebuild is an
# accelerator with a preserved cold fallback, publication is fail-closed on
# exactly one complete supply path, the codesign gate travels with whichever
# job produces Darwin bytes, and the self-hosted routing keeps the cas-6981
# trust posture with a hosted fail-safe.
# ---------------------------------------------------------------------------
prebuild="$repo_root/.github/workflows/release-prebuild.yml"
prebuild_text="$(<"$prebuild")"

prebuild_job_block() {
    local job="$1"
    awk -v header="  ${job}:" '
        $0 == header { inside = 1; next }
        inside && /^  [A-Za-z0-9_-]+:$/ { exit }
        inside { print }
    ' "$prebuild"
}

prebuild_gate="$(prebuild_job_block pending-release)"
prebuild_route="$(prebuild_job_block prebuild-runner-route)"
prebuild_linux="$(prebuild_job_block build)"
prebuild_macos="$(prebuild_job_block build-macos)"
release_lookup="$(release_job_block prebuilt-lookup)"
release_route="$(release_job_block release-runner-route)"

# Trigger surface. A public fork must not be able to reach the persistent box
# through the new workflow, and the standing CI-load policy keeps the heavy
# tier off factory/*, epic/*, tags and pull requests.
require_text "$prebuild_text" 'push:' 'release prebuild runs on canonical repository pushes'
require_text "$prebuild_text" '      - main' 'release prebuild is limited to main'
for forbidden_event in pull_request: pull_request_target: workflow_run: issue_comment: repository_dispatch:; do
    require_absent "$prebuild_text" "$forbidden_event" "release prebuild rejects event before runner assignment: $forbidden_event"
done
require_absent "$prebuild_text" '- "factory/**"' 'release prebuild never fires on factory branches'
require_absent "$prebuild_text" '- "epic/**"' 'release prebuild never fires on epic branches'
require_absent "$prebuild_text" 'tags:' 'release prebuild never fires on tags'
require_text "$prebuild_text" 'cancel-in-progress: false' 'a prebuild is never cancelled out from under the tag that will adopt it'

# The gate is what keeps an ordinary main push from costing two release builds.
require_text "$prebuild_gate" './scripts/detect-pending-release.sh' 'release prebuild gates the heavy tier on a pending release tree'
require_text "$prebuild_linux" "needs.pending-release.outputs.pending == 'true'" 'Linux prebuild only runs for a pending release tree'
require_text "$prebuild_macos" "needs.pending-release.outputs.pending == 'true'" 'macOS prebuild only runs for a pending release tree'

# Prebuilt bytes must be indistinguishable from what the tag-time fallback
# would have produced, including every audit and the codesign gate (cas-67c1).
for platform_prebuild in "$prebuild_linux" "$prebuild_macos"; do
    require_text "$platform_prebuild" '--profile "$RELEASE_PROFILE"' 'prebuilt artifact uses the shipped release profile'
    require_text "$platform_prebuild" 'strip package/cas' 'prebuilt artifact strips symbols before packaging'
done
require_text "$prebuild_linux" 'check-blake3-no-avx512-build.sh' 'Linux prebuild audits BLAKE3 build inputs'
require_text "$prebuild_linux" 'test-check-portable-x86_64-isa.sh package/cas' 'Linux prebuild audits the exact stripped executable'
require_text "$prebuild_macos" 'codesign --sign - --force package/cas' 'macOS prebuild re-signs the binary after stripping'
require_text "$prebuild_macos" 'codesign --verify --verbose=4 package/cas' 'macOS prebuild verifies the signature it publishes'
require_text "$prebuild_macos" 'runs-on: macos-26' 'macOS prebuild stays on the supported hosted image'
require_text "$prebuild_linux" 'name: cas-x86_64-unknown-linux-gnu' 'prebuilt Linux artifact uses the adopted asset name'
require_text "$prebuild_macos" 'name: cas-aarch64-apple-darwin' 'prebuilt macOS artifact uses the adopted asset name'

# Self-hosted routing: label selected before assignment, opt-in variable, and
# an execution-time trust guard on the machine itself.
for route in "$prebuild_route" "$release_route"; do
    require_text "$route" 'vars.CASSY_RELEASE_SELF_HOSTED' 'release routing requires explicit self-hosted opt-in'
    require_text "$route" 'runner=["self-hosted","Linux","X64","cas-ci-32core"]' 'release routing selects the isolated runner label set'
    require_text "$route" 'runner=["ubuntu-latest"]' 'release routing fails safe to hosted runners'
    require_absent "$route" 'actions/checkout' 'release routing does not execute source before label selection'
done
require_text "$prebuild_linux" './scripts/check-release-runner-trust.sh' 'Linux prebuild reasserts the trust boundary on the box'
require_text "$release_verify" './scripts/check-release-runner-trust.sh' 'release verification reasserts the trust boundary on the box'
require_text "$release_linux" './scripts/check-release-runner-trust.sh' 'fallback Linux build reasserts the trust boundary on the box'
require_text "$(<"$repo_root/scripts/check-release-runner-trust.sh")" 'refs/heads/main | refs/tags/v*' 'trust guard admits only release-prep and release-tag refs'

# The publishing job holds the only write-scoped token in the release. It must
# never execute on the shared persistent box.
require_text "$release_publish" 'runs-on: ubuntu-latest' 'the write-scoped publish job stays on hosted runners'
require_absent "$release_publish" 'cas-ci-32core' 'the write-scoped publish job cannot be routed to the box'

# Adoption is fail-safe on lookup and fail-closed on publication.
require_text "$release_lookup" './scripts/find-release-prebuild.sh' 'release looks up prebuilt artifacts for the tagged commit'
require_text "$release_lookup" 'Fixes #603' 'release lookup pins the prebuild race fix'
require_text "$release_lookup" 'actions: read' 'prebuilt lookup takes only read scope'
require_text "$release_lookup" 'RELEASE_PREBUILD_WAIT_SECONDS: 900' 'prebuilt lookup bounds its race wait'
require_text "$release_lookup" 'RELEASE_PREBUILD_POLL_SECONDS: 15' 'prebuilt lookup polls its race wait'
require_text "$release_linux" "needs.prebuilt-lookup.outputs.found != 'true'" 'the cold Linux build remains the fallback when no prebuild exists'
require_text "$release_macos" "needs.prebuilt-lookup.outputs.found != 'true'" 'the cold macOS build remains the fallback when no prebuild exists'
require_text "$release_publish" "needs.prebuilt-lookup.outputs.found == 'true'" 'publication distinguishes the adopted path'
require_text "$release_publish" "needs.build.result == 'skipped'" 'adoption requires the platform builds to have actually been skipped'
require_text "$release_publish" "needs.build-macos.result == 'skipped'" 'adoption requires the macOS build to have actually been skipped'
require_text "$release_publish" "needs.build.result == 'success'" 'the fallback path requires a successful Linux build'
require_text "$release_publish" "needs.build-macos.result == 'success'" 'the fallback path requires a successful macOS build'
require_text "$release_publish" "needs.verify.result == 'success'" 'no supply path can publish without the release-input gate'
require_text "$release_publish" 'gh run download' 'publication adopts artifacts from the prebuild run'
require_text "$release_publish" './scripts/check-portable-x86_64-isa.sh publish-audit/cas' 'publication re-audits the exact Linux executable it is about to publish'
require_text "$release_publish" 'sha256sum release/*.tar.gz' 'publication records the digests of the exact published bytes'

# The latency claim itself must be produced by a gate, not by eyeballing a run.
latency_receipt="$repo_root/scripts/release-latency-receipt.sh"
if [[ -x "$latency_receipt" ]]; then
    require_text "$(<"$latency_receipt")" 'budget=600' 'the release latency receipt defaults to the ten-minute target'
    require_text "$(<"$latency_receipt")" 'sort | first' 'latency is measured from the first run of the tag, never a rerun'
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
    require_text "$(<"$repo_root/cas-cli/Makefile")" "test-$guard_script.sh" "test-ci-tiers runs the $guard_script self-test"
done

require_text "$(<"$repo_root/docs/ci/release-fast-publication.md")" 'tag push -> published' 'fast publication architecture is documented'
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
require_count "$all_actions" 'continue-on-error: true' '8' 'every sccache setup action and the self-hosted probes fail open'
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

# The cold-build deliberate exemption. It measures a cold compiler, so a stats
# summary there would report an alarming 0% for a lane that is working correctly.
# The self-hosted suite archive is the separate cas-065a merge-queue exception:
# its wrapper is cleared after the fail-open probe, never as a cache prerequisite.
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
