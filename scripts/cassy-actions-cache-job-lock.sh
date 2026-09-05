#!/usr/bin/env bash
# Hold a shared cache lock from GitHub Runner's job-started hook until its
# job-completed hook. The pruner takes the same lock exclusively.
set -euo pipefail

fail() {
    printf 'error: %s\n' "$1" >&2
    exit 1
}

production_cache=/var/lib/cassy-actions/cache
production_state=/var/lib/cassy-actions/job-locks
production_guard=/var/lib/cassy-actions/check-cache-mount.sh
cache_root="$(realpath -e -- "${CASSY_ACTIONS_CACHE_ROOT:-$production_cache}")" ||
    fail 'cache root must exist'
state_root="$(realpath -e -- "${CASSY_ACTIONS_STATE_ROOT:-$production_state}")" ||
    fail 'job lock state root must exist'
mount_guard_bin="${CASSY_ACTIONS_MOUNT_GUARD_BIN:-$production_guard}"
lock_wait_seconds="${CASSY_ACTIONS_LOCK_WAIT_SECONDS:-300}"
slot="${CASSY_ACTIONS_RUNNER_SLOT:-}"
self="$(realpath -e -- "$0")"

case "${1:-}" in
    --job-started|--job-completed|--hold) mode="$1" ;;
    '')
        case "$(basename -- "$0")" in
            cache-job-started.sh) mode=--job-started ;;
            cache-job-completed.sh) mode=--job-completed ;;
            *) fail 'invoke as cache-job-started.sh, cache-job-completed.sh, or with an explicit mode' ;;
        esac
        ;;
    *) fail "unknown mode: $1" ;;
esac

if [[ "$mode" == --hold ]]; then
    [[ $# == 3 ]] || fail 'internal holder requires slot and token'
    slot="$2"
    token="$3"
else
    [[ $# -le 1 ]] || fail "$mode accepts no positional arguments"
fi

if [[ "$cache_root" != "$production_cache" || "$state_root" != "$production_state" ||
      "$mount_guard_bin" != "$production_guard" ]]; then
    [[ "${CASSY_ACTIONS_ALLOW_TEST_ROOT:-}" == 1 ]] ||
        fail 'alternate cache, state, or mount guard paths are allowed only in a test fixture'
fi
[[ "$slot" == 1 || "$slot" == 2 ]] || fail 'CASSY_ACTIONS_RUNNER_SLOT must be 1 or 2'
[[ "$lock_wait_seconds" =~ ^[1-9][0-9]*$ ]] || fail 'lock wait must be a positive second count'
[[ -d "$cache_root" && ! -L "$cache_root" ]] || fail 'cache root must be a non-symlink directory'
[[ -d "$state_root" && ! -L "$state_root" ]] || fail 'state root must be a non-symlink directory'
[[ -x "$mount_guard_bin" ]] || fail "mount guard is not executable: $mount_guard_bin"

lock_file="$cache_root/.cassy-actions-cache-job.lock"
pid_file="$state_root/slot-$slot.pid"

read_record() {
    local record_pid record_token extra
    [[ -f "$pid_file" && ! -L "$pid_file" ]] || return 1
    read -r record_pid record_token extra <"$pid_file" || return 1
    [[ "$record_pid" =~ ^[1-9][0-9]*$ && -n "$record_token" && -z "${extra:-}" ]] || return 1
    printf '%s %s\n' "$record_pid" "$record_token"
}

holder_matches() {
    local holder_pid="$1" holder_token="$2" arg
    local -a argv=()
    [[ -r "/proc/$holder_pid/cmdline" ]] || return 1
    [[ "$(stat -c %u -- "/proc/$holder_pid")" == "$(id -u)" ]] || return 1
    mapfile -d '' -t argv <"/proc/$holder_pid/cmdline"
    for ((arg = 0; arg + 3 < ${#argv[@]}; arg++)); do
        if [[ "${argv[$arg]}" == "$self" && "${argv[$((arg + 1))]}" == --hold &&
              "${argv[$((arg + 2))]}" == "$slot" && "${argv[$((arg + 3))]}" == "$holder_token" ]]; then
            return 0
        fi
    done
    return 1
}

holder_main() {
    local current sleep_pid=''
    [[ -e /proc/self/fd/9 ]] || fail 'holder did not inherit the shared lock descriptor'
    [[ "$(readlink -f -- /proc/self/fd/9)" == "$lock_file" ]] ||
        fail 'holder inherited the wrong lock descriptor'
    flock -n -s 9 || fail 'holder did not inherit the shared cache lock'
    [[ ! -e "$pid_file" && ! -L "$pid_file" ]] || fail "job lock state already exists: $pid_file"
    umask 077
    printf '%s %s\n' "$$" "$token" >"$pid_file.tmp.$$"
    mv -T -- "$pid_file.tmp.$$" "$pid_file"
    cleanup_holder() {
        trap - EXIT INT TERM HUP
        [[ -n "$sleep_pid" ]] && kill -TERM "$sleep_pid" 2>/dev/null || true
        current="$(read_record 2>/dev/null || true)"
        [[ "$current" == "$$ $token" ]] && rm -f -- "$pid_file"
        exit 0
    }
    trap cleanup_holder EXIT INT TERM HUP
    while :; do
        sleep 3600 9>&- &
        sleep_pid=$!
        wait "$sleep_pid" || true
        sleep_pid=''
    done
}

start_job() {
    local stale holder_pid token record attempt
    exec 9>>"$lock_file"
    flock -s -w "$lock_wait_seconds" 9 ||
        fail "timed out waiting for the cache prune barrier after $lock_wait_seconds seconds"
    "$mount_guard_bin" || fail 'runner cache mount guard rejected job start'
    if [[ -e "$pid_file" || -L "$pid_file" ]]; then
        stale="$(read_record)" || fail "invalid or unsafe existing job lock state: $pid_file"
        read -r holder_pid token <<<"$stale"
        if holder_matches "$holder_pid" "$token"; then
            fail "slot $slot already has a live job lock holder"
        fi
        kill -0 "$holder_pid" 2>/dev/null &&
            fail "slot $slot job lock state names an unrelated live process"
        rm -f -- "$pid_file"
    fi

    token="$(printf '%s-%s-%s\n' "$$" "$(date +%s%N)" "$RANDOM" | sha256sum | awk '{print $1}')"
    RUNNER_TRACKING_ID= nohup "$self" --hold "$slot" "$token" 9>&9 \
        >>"$state_root/slot-$slot.log" 2>&1 &
    holder_pid=$!
    for attempt in $(seq 1 100); do
        record="$(read_record 2>/dev/null || true)"
        if [[ "$record" == "$holder_pid $token" ]] && holder_matches "$holder_pid" "$token"; then
            printf 'runner slot %s acquired the shared cache lock (pid %s)\n' "$slot" "$holder_pid"
            return 0
        fi
        kill -0 "$holder_pid" 2>/dev/null || break
        sleep 0.05
    done
    kill -TERM "$holder_pid" 2>/dev/null || true
    fail "slot $slot cache lock holder did not become ready"
}

complete_job() {
    local record holder_pid holder_token attempt
    record="$(read_record)" || fail "missing or invalid job lock state for slot $slot"
    read -r holder_pid holder_token <<<"$record"
    holder_matches "$holder_pid" "$holder_token" ||
        fail "slot $slot job lock state does not name its verified holder"
    kill -TERM "$holder_pid" || fail "could not stop slot $slot cache lock holder"
    for attempt in $(seq 1 100); do
        if ! kill -0 "$holder_pid" 2>/dev/null; then
            [[ ! -e "$pid_file" && ! -L "$pid_file" ]] ||
                fail "slot $slot holder exited without retiring its state"
            printf 'runner slot %s released the shared cache lock\n' "$slot"
            return 0
        fi
        sleep 0.05
    done
    fail "slot $slot cache lock holder did not exit"
}

case "$mode" in
    --hold) holder_main ;;
    --job-started) start_job ;;
    --job-completed) complete_job ;;
esac
