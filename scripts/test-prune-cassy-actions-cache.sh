#!/usr/bin/env bash
# Behavioral regression tests for the self-hosted runner cache budget hook.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
pruner="$script_dir/prune-cassy-actions-cache.sh"
fixture_root="$(mktemp -d)"
trap 'rm -rf "$fixture_root"' EXIT

cache_root="$fixture_root/cache"
target="$cache_root/cargo-target"
mkdir -p "$target/debug/incremental/stale-session" "$target/debug/deps"
dd if=/dev/zero of="$target/debug/incremental/stale-session/object.o" bs=4096 count=1 status=none
dd if=/dev/zero of="$target/debug/deps/libstale.rlib" bs=4096 count=1 status=none
dd if=/dev/zero of="$target/debug/deps/libfresh.rlib" bs=4096 count=1 status=none
touch -d '8 days ago' "$target/debug/incremental/stale-session" "$target/debug/deps/libstale.rlib"

CASSY_ACTIONS_ALLOW_TEST_ROOT=1 \
CASSY_ACTIONS_CACHE_ROOT="$cache_root" \
CASSY_ACTIONS_RUNNER_SLOT=1 \
CASSY_ACTIONS_TARGET_BUDGET_BYTES=10000000 \
CASSY_ACTIONS_CACHE_MAX_AGE_DAYS=7 \
    "$pruner" >/dev/null

test ! -e "$target/debug/incremental/stale-session"
test ! -e "$target/debug/deps/libstale.rlib"
test -e "$target/debug/deps/libfresh.rlib"

printf 'ok   stale incremental sessions and deps artifacts are pruned; fresh deps remain\n'

budget_cache="$fixture_root/budget-cache"
budget_target="$budget_cache/cargo-target"
mkdir -p "$budget_target/debug/deps" "$budget_target/debug/.fingerprint" "$budget_target/debug/build"
dd if=/dev/zero of="$budget_target/debug/deps/current-test-binary" bs=4096 count=16 status=none
dd if=/dev/zero of="$budget_target/debug/.fingerprint/current.json" bs=4096 count=2 status=none
dd if=/dev/zero of="$budget_target/debug/build/current.o" bs=4096 count=2 status=none

CASSY_ACTIONS_ALLOW_TEST_ROOT=1 \
CASSY_ACTIONS_CACHE_ROOT="$budget_cache" \
CASSY_ACTIONS_RUNNER_SLOT=1 \
CASSY_ACTIONS_TARGET_BUDGET_BYTES=32768 \
CASSY_ACTIONS_CACHE_MAX_AGE_DAYS=7 \
    "$pruner" >/dev/null

test ! -e "$budget_target/debug"
test "$(du -s -B1 "$budget_target" | awk '{print $1}')" -le 32768
printf 'ok   an over-budget target drops a coherent rebuildable profile and returns below cap\n'

opaque_cache="$fixture_root/opaque-cache"
opaque_target="$opaque_cache/cargo-target"
mkdir -p "$opaque_target"
dd if=/dev/zero of="$opaque_target/operator-owned.bin" bs=4096 count=16 status=none
if CASSY_ACTIONS_ALLOW_TEST_ROOT=1 \
    CASSY_ACTIONS_CACHE_ROOT="$opaque_cache" \
    CASSY_ACTIONS_RUNNER_SLOT=1 \
    CASSY_ACTIONS_TARGET_BUDGET_BYTES=32768 \
    "$pruner" >/dev/null 2>&1; then
    printf 'FAIL opaque over-budget data was accepted\n' >&2
    exit 1
fi
test -e "$opaque_target/operator-owned.bin"
printf 'ok   unknown over-budget data is retained and fails closed\n'

if CASSY_ACTIONS_CACHE_ROOT="$cache_root" CASSY_ACTIONS_RUNNER_SLOT=1 \
    "$pruner" >/dev/null 2>&1; then
    printf 'FAIL alternate cache root was accepted without the test guard\n' >&2
    exit 1
fi
if CASSY_ACTIONS_ALLOW_TEST_ROOT=1 CASSY_ACTIONS_CACHE_ROOT="$cache_root" \
    CASSY_ACTIONS_RUNNER_SLOT=3 "$pruner" >/dev/null 2>&1; then
    printf 'FAIL invalid slot identifier was accepted\n' >&2
    exit 1
fi
printf 'ok   destructive scope requires the production root or explicit test guard and a known slot\n'
