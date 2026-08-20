#!/usr/bin/env bash
# Deterministic self-test for scripts/check-release-runner-trust.sh.
#
# This is the execution-time half of the self-hosted trust boundary: the
# workflow `if:` gates runner assignment, this gates what runs after
# assignment. A guard nobody tests is a guard that quietly stops guarding, so
# every rejection direction is exercised here.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$script_dir/check-release-runner-trust.sh"

pass=0
fail=0

run_guard() {
    env -i PATH="$PATH" HOME="${HOME:-/tmp}" \
        GITHUB_EVENT_NAME="${EVENT:-push}" \
        GITHUB_REPOSITORY="${REPO:-Richards-LLC/cassy}" \
        SELF_HOSTED_ENABLED="${ENABLED-enabled}" \
        GITHUB_REF="${REF:-refs/tags/v3.4.0}" \
        CARGO_TARGET_DIR="${TARGET_DIR:-/var/lib/cassy-actions/cache/cargo-target}" \
        SCCACHE_DIR="${SCCACHE:-/var/lib/cassy-actions/cache/sccache}" \
        SCCACHE_SERVER_PORT="${PORT:-4227}" \
        bash "$guard" 2>&1
}

expect_accept() {
    local label="$1"
    if run_guard >/dev/null; then
        printf 'ok   %s\n' "$label"
        pass=$((pass + 1))
    else
        printf 'FAIL %s (trusted input was rejected)\n' "$label"
        fail=$((fail + 1))
    fi
}

expect_reject() {
    local label="$1" needle="$2" output status
    set +e
    output="$(run_guard)"
    status=$?
    set -e
    if [[ "$status" -ne 0 ]] && grep -qF -- "$needle" <<<"$output"; then
        printf 'ok   %s\n' "$label"
        pass=$((pass + 1))
    else
        printf 'FAIL %s (status %s, output: %s)\n' "$label" "$status" "$output"
        fail=$((fail + 1))
    fi
}

REF=refs/tags/v3.4.0 expect_accept 'an annotated release tag push is trusted'
REF=refs/heads/main expect_accept 'the release-prep commit on main is trusted'
TARGET_DIR=/var/lib/cassy-actions/cache/cargo-target-2 \
    SCCACHE=/var/lib/cassy-actions/cache/sccache-2 PORT=4228 \
    expect_accept 'the second isolated runner slot is trusted'

REF=refs/heads/factory/some-worker \
    expect_reject 'a factory branch cannot use the release lane' 'outside the trusted release set'
REF=refs/pull/1/merge \
    expect_reject 'a pull-request ref cannot use the release lane' 'outside the trusted release set'
REF=refs/heads/gh-readonly-queue/main/x \
    expect_reject 'a merge-queue ref cannot use the release lane' 'outside the trusted release set'
EVENT=pull_request_target \
    expect_reject 'a non-push event is refused' 'requires a push event'
REPO=attacker/cassy-fork \
    expect_reject 'a fork repository is refused' 'canonical repository'
ENABLED= \
    expect_reject 'routing that was never enabled is refused' 'not explicitly enabled'
TARGET_DIR=/home/pippenz/Petrastella/cas-src/target \
    expect_reject 'a factory worktree target directory is refused' 'not an approved isolated slot tuple'
SCCACHE=/home/pippenz/.cache/sccache \
    expect_reject 'the operator cache directory is refused' 'not an approved isolated slot tuple'
PORT=4226 \
    expect_reject 'a foreign sccache port is refused' 'not an approved isolated slot tuple'
TARGET_DIR=/var/lib/cassy-actions/cache/cargo-target \
    SCCACHE=/var/lib/cassy-actions/cache/sccache-2 PORT=4228 \
    expect_reject 'a mixed slot tuple is refused' 'not an approved isolated slot tuple'

printf '\n%s passed, %s failed\n' "$pass" "$fail"
test "$fail" -eq 0
