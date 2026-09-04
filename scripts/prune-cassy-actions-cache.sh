#!/usr/bin/env bash
# Bound one persistent self-hosted runner target cache at a job boundary.
set -euo pipefail

fail() {
    printf 'error: %s\n' "$1" >&2
    exit 1
}

production_root=/var/lib/cassy-actions/cache
cache_root="$(realpath -m -- "${CASSY_ACTIONS_CACHE_ROOT:-$production_root}")"
slot="${CASSY_ACTIONS_RUNNER_SLOT:?CASSY_ACTIONS_RUNNER_SLOT must be 1 or 2}"
budget_bytes="${CASSY_ACTIONS_TARGET_BUDGET_BYTES:-50000000000}"
max_age_days="${CASSY_ACTIONS_CACHE_MAX_AGE_DAYS:-7}"

if [[ "$cache_root" != "$production_root" && "${CASSY_ACTIONS_ALLOW_TEST_ROOT:-}" != 1 ]]; then
    fail "refusing non-production cache root without CASSY_ACTIONS_ALLOW_TEST_ROOT=1"
fi
[[ "$cache_root" != / && -d "$cache_root" ]] || fail "cache root must be an existing non-root directory"
[[ "$budget_bytes" =~ ^[1-9][0-9]*$ ]] || fail "target budget must be a positive byte count"
[[ "$max_age_days" =~ ^[0-9]+$ ]] || fail "cache max age must be a non-negative day count"

case "$slot" in
    1) target_dir="$cache_root/cargo-target" ;;
    2) target_dir="$cache_root/cargo-target-2" ;;
    *) fail "CASSY_ACTIONS_RUNNER_SLOT must be 1 or 2; got $slot" ;;
esac
[[ -d "$target_dir" ]] || fail "runner target directory does not exist: $target_dir"

exec 9>"$cache_root/.prune-slot-$slot.lock"
flock -n 9 || fail "cache prune already running for slot $slot"

incremental_dir="$target_dir/debug/incremental"
deps_dir="$target_dir/debug/deps"
before_bytes="$(du -s -B1 -- "$target_dir" | awk '{print $1}')"

# These are rebuildable Cargo outputs. Delete only entries older than the
# retention window, and only inside the validated per-slot target directory.
if [[ -d "$incremental_dir" ]]; then
    find "$incremental_dir" -mindepth 1 -maxdepth 1 -mtime "+$max_age_days" \
        -exec rm -rf -- {} +
fi
if [[ -d "$deps_dir" ]]; then
    find "$deps_dir" -mindepth 1 -maxdepth 1 \
        \( -type f -o -type l \) -mtime "+$max_age_days" -delete
fi

usage_bytes="$(du -s -B1 -- "$target_dir" | awk '{print $1}')"
if (( usage_bytes > budget_bytes )); then
    # Cargo target profiles are disposable as a coherent unit. Removing a
    # whole profile avoids leaving fingerprints that refer to selectively
    # deleted outputs; Cargo recreates it on the next build.
    safe_profiles=(
        "$target_dir/debug"
        "$target_dir/x86_64-unknown-linux-gnu/debug"
        "$target_dir/release"
        "$target_dir/x86_64-unknown-linux-gnu/release"
        "$target_dir/release-fast"
        "$target_dir/x86_64-unknown-linux-gnu/release-fast"
    )
    for profile in "${safe_profiles[@]}"; do
        [[ -e "$profile" ]] || continue
        rm -rf -- "$profile"
        usage_bytes="$(du -s -B1 -- "$target_dir" | awk '{print $1}')"
        (( usage_bytes > budget_bytes )) || break
    done
fi

if (( usage_bytes > budget_bytes )); then
    fail "slot $slot remains above target budget after safe profile pruning: $usage_bytes > $budget_bytes"
fi

printf 'runner cache prune: slot=%s target=%s before=%s after=%s budget=%s\n' \
    "$slot" "$target_dir" "$before_bytes" "$usage_bytes" "$budget_bytes"
