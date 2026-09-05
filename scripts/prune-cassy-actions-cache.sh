#!/usr/bin/env bash
# Bound both persistent runner slots, but only when the complete GitHub Runner
# job lifecycle is idle. Compiler-process absence alone is not an idle proof.
set -euo pipefail

fail() {
    printf 'error: %s\n' "$1" >&2
    exit 1
}

log() {
    printf 'cassy-cache: %s\n' "$*"
}

production_root=/var/lib/cassy-actions/cache
cache_root="$(realpath -m -- "${CASSY_ACTIONS_CACHE_ROOT:-$production_root}")"
cgroup_root="${CASSY_ACTIONS_CGROUP_ROOT:-/sys/fs/cgroup}"
proc_root="${CASSY_ACTIONS_PROC_ROOT:-/proc}"
systemctl_bin="${CASSY_ACTIONS_SYSTEMCTL_BIN:-/usr/bin/systemctl}"
target_budget_bytes="${CASSY_ACTIONS_TARGET_BUDGET_BYTES:-50000000000}"
# sccache parses G as 1024^3 bytes; 8G is exactly 8,589,934,592 bytes.
sccache_budget_bytes="${CASSY_ACTIONS_SCCACHE_BUDGET_BYTES:-8589934592}"
slot_budget_bytes="${CASSY_ACTIONS_SLOT_BUDGET_BYTES:-60000000000}"
max_age_days="${CASSY_ACTIONS_CACHE_MAX_AGE_DAYS:-7}"
runner_services=(cassy-actions-runner.service cassy-actions-runner-2.service)
target_dirs=("$cache_root/cargo-target" "$cache_root/cargo-target-2")
sccache_dirs=("$cache_root/sccache" "$cache_root/sccache-2")

usage() {
    printf 'Usage: %s {--check-idle|--now|--scheduled}\n' "$0"
}

validate_config() {
    local value
    if [[ "$cache_root" != "$production_root" || "$cgroup_root" != /sys/fs/cgroup ||
          "$proc_root" != /proc || "$systemctl_bin" != /usr/bin/systemctl ]]; then
        [[ "${CASSY_ACTIONS_ALLOW_TEST_ROOT:-}" == 1 ]] ||
            fail 'alternate cache/job-state roots are allowed only in a test fixture'
    fi
    [[ "$cache_root" != / && -d "$cache_root" && ! -L "$cache_root" ]] ||
        fail 'cache root must be an existing non-symlink directory'
    [[ -x "$systemctl_bin" ]] || fail "systemctl is not executable: $systemctl_bin"
    for value in "$target_budget_bytes" "$sccache_budget_bytes" "$slot_budget_bytes"; do
        [[ "$value" =~ ^[1-9][0-9]*$ ]] || fail 'cache budgets must be positive byte counts'
    done
    [[ "$max_age_days" =~ ^[0-9]+$ ]] || fail 'cache max age must be a non-negative day count'
    (( target_budget_bytes + sccache_budget_bytes <= slot_budget_bytes )) ||
        fail "configured target+sccache budget exceeds slot cap: $target_budget_bytes + $sccache_budget_bytes > $slot_budget_bytes"
}

bytes_for() {
    du -sx -B1 -- "$1" | awk '{print $1}'
}

# Return 0 for known idle, 1 for busy, and 2 for missing/unreadable state.
runner_job_state() {
    local unit cgroup cgroup_file pid comm busy=0
    for unit in "${runner_services[@]}"; do
        cgroup="$("$systemctl_bin" show --property=ControlGroup --value "$unit")" || return 2
        [[ "$cgroup" == /* ]] || return 2
        cgroup_file="$cgroup_root$cgroup/cgroup.procs"
        [[ -r "$cgroup_file" ]] || return 2
        while IFS= read -r pid; do
            [[ "$pid" =~ ^[0-9]+$ ]] || return 2
            [[ -r "$proc_root/$pid/comm" ]] || return 2
            comm="$(tr -d '\n' < "$proc_root/$pid/comm")" || return 2
            [[ -n "$comm" ]] || return 2
            [[ "$comm" == Runner.Worker ]] && busy=1
        done < "$cgroup_file"
    done
    (( busy == 0 ))
}

require_idle() {
    local state
    if runner_job_state; then
        return 0
    else
        state=$?
    fi
    (( state == 1 )) && fail 'refusing to prune while Runner.Worker owns an active job'
    fail 'refusing to prune because runner job state is missing or unreadable'
}

remove_stale() {
    local target="$1" path tree
    while IFS= read -r -d '' tree; do
        while IFS= read -r -d '' path; do
            require_idle
            rm -rf -- "$path"
        done < <(find "$tree" -xdev -mindepth 1 -maxdepth 1 \
            -mtime "+$max_age_days" -print0)
    done < <(find "$target" -xdev -type d -name incremental -print0)
    while IFS= read -r -d '' path; do
        require_idle
        rm -f -- "$path"
    done < <(find "$target" -xdev -type d -name deps -print0 | while IFS= read -r -d '' path; do
        find "$path" -xdev -mindepth 1 -maxdepth 1 \( -type f -o -type l \) \
            -mtime "+$max_age_days" -print0
    done)
}

prune_slot() {
    local index="$1" target="${target_dirs[$1]}" sccache="${sccache_dirs[$1]}"
    local target_bytes sccache_bytes allowed_target profile
    [[ -d "$target" && ! -L "$target" ]] || fail "target directory is unsafe or missing: $target"
    [[ -d "$sccache" && ! -L "$sccache" ]] || fail "sccache directory is unsafe or missing: $sccache"

    remove_stale "$target"
    sccache_bytes="$(bytes_for "$sccache")"
    (( sccache_bytes <= sccache_budget_bytes )) ||
        fail "slot $((index + 1)) sccache exceeds its configured 8G ceiling: $sccache_bytes > $sccache_budget_bytes"
    allowed_target=$((slot_budget_bytes - sccache_bytes))
    (( allowed_target > target_budget_bytes )) && allowed_target="$target_budget_bytes"
    target_bytes="$(bytes_for "$target")"

    if (( target_bytes > allowed_target )); then
        for profile in \
            "$target/debug" "$target/x86_64-unknown-linux-gnu/debug" \
            "$target/release" "$target/x86_64-unknown-linux-gnu/release" \
            "$target/release-fast" "$target/x86_64-unknown-linux-gnu/release-fast"; do
            [[ -e "$profile" && ! -L "$profile" ]] || continue
            require_idle
            rm -rf -- "$profile"
            target_bytes="$(bytes_for "$target")"
            (( target_bytes <= allowed_target )) && break
        done
    fi
    target_bytes="$(bytes_for "$target")"
    (( target_bytes <= allowed_target )) ||
        fail "slot $((index + 1)) target remains above its safe budget: $target_bytes > $allowed_target"
    (( target_bytes + sccache_bytes <= slot_budget_bytes )) ||
        fail "slot $((index + 1)) total exceeds $slot_budget_bytes bytes"
    log "slot=$((index + 1)) target=$target_bytes sccache=$sccache_bytes total=$((target_bytes + sccache_bytes)) cap=$slot_budget_bytes"
}

prune_all() {
    local index
    require_idle
    exec 9>"$cache_root/.cassy-actions-cache-prune.lock"
    flock -n 9 || fail 'another cache prune is already running'
    for index in 0 1; do prune_slot "$index"; done
}

main() {
    local state
    [[ $# == 1 ]] || { usage >&2; exit 2; }
    validate_config
    case "$1" in
        --check-idle)
            if runner_job_state; then
                log 'runner idle proof: no Runner.Worker exists in either service cgroup'
            else
                state=$?
                (( state == 1 )) && fail 'Runner.Worker owns an active job'
                fail 'runner job state is missing or unreadable'
            fi
            ;;
        --now) prune_all ;;
        --scheduled)
            if runner_job_state; then
                prune_all
            else
                state=$?
                if (( state == 1 )); then
                    log 'scheduled prune skipped: Runner.Worker owns an active job'
                    exit 0
                fi
                fail 'scheduled prune refused: runner job state is missing or unreadable'
            fi
            ;;
        *) usage >&2; exit 2 ;;
    esac
}

main "$@"
