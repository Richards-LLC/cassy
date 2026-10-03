#!/usr/bin/env bash
# Linux-only provisioning/runner script: GNU coreutils and Linux kernel interfaces are required.
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
# shellcheck source=scripts/cassy-actions-process-identity.sh
source "$(dirname -- "$self")/cassy-actions-process-identity.sh"

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
    [[ $# == 5 ]] || fail 'internal holder requires slot, token and worker identity'
    slot="$2"
    token="$3"
    owner_pid="$4"
    owner_start="$5"
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
    local record_pid record_token holder_start owner_pid owner_start extra
    [[ -f "$pid_file" && ! -L "$pid_file" ]] || return 1
    read -r record_pid record_token holder_start owner_pid owner_start extra <"$pid_file" || return 1
    [[ "$record_pid" =~ ^[1-9][0-9]*$ && -n "$record_token" && -z "${extra:-}" ]] || return 1
    if [[ -z "$holder_start$owner_pid$owner_start" ]]; then
        # Old records have no owner identity. A live legacy holder remains
        # fail-closed until its completed hook retires it.
        printf '%s %s\n' "$record_pid" "$record_token"
    else
        [[ "$holder_start" =~ ^[0-9]+$ && "$owner_pid" =~ ^[1-9][0-9]*$ &&
           "$owner_start" =~ ^[0-9]+$ ]] || return 1
        printf '%s %s %s %s %s\n' "$record_pid" "$record_token" \
            "$holder_start" "$owner_pid" "$owner_start"
    fi
}

lock_slot() {
    # Serialize start/completion metadata separately from the shared cache
    # barrier. Children must not inherit this per-slot coordination lock.
    exec 8>>"$state_root/slot-$slot.guard"
    flock -x -w "$lock_wait_seconds" 8 || fail "timed out waiting for slot $slot lifecycle guard"
    trap 'flock -u 8' EXIT
}

holder_matches() {
    local holder_pid="$1" holder_token="$2" holder_start="${3:-}" arg
    [[ -z "$holder_start" ]] || process_matches "$holder_pid" "$holder_start" || return 1
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

stop_holder() {
    local holder_pid="$1" holder_start="$2" holder_token="$3"
    # Bind the signal to this process before rechecking its identity. A PID
    # reused between verification and signalling must never receive SIGTERM.
    python3 - "$holder_pid" "$holder_start" "$self" "$slot" "$holder_token" <<'PYSIGNAL'
import os
import pathlib
import signal
import sys
pid, start, script, slot, token = sys.argv[1:]
try:
    fd = os.pidfd_open(int(pid))
    try:
        root = pathlib.Path("/proc") / pid
        fields = (root / "stat").read_text().rsplit(") ", 1)[1].split()
        argv = (root / "cmdline").read_bytes().split(b"\0")
        expected = [value.encode() for value in (script, "--hold", slot, token)]
        if (fields[19] == start and fields[0] not in ("Z", "X")
                and root.stat().st_uid == os.getuid()
                and any(argv[i:i + 4] == expected for i in range(len(argv) - 3))):
            signal.pidfd_send_signal(fd, signal.SIGTERM)
    finally:
        os.close(fd)
except (ProcessLookupError, FileNotFoundError):
    pass
PYSIGNAL
}

retire_holder() {
    local holder_pid="$1" holder_token="$2" holder_start="$3" attempt
    stop_holder "$holder_pid" "$holder_start" "$holder_token" || fail "could not stop slot $slot cache lock holder"
    for ((attempt = 0; attempt < 100; attempt++)); do
        if ! process_matches "$holder_pid" "$holder_start"; then
            # A killed holder may not run its trap. The caller holds the slot
            # guard, and no new holder can publish a record yet.
            rm -f -- "$pid_file"
            return 0
        fi
        sleep 0.05
    done
    fail "slot $slot cache lock holder did not exit"
}

holder_main() {
    local current current_pid current_token current_start snapshot holder_start sleep_pid=''
    [[ -e /proc/self/fd/9 ]] || fail 'holder did not inherit the shared lock descriptor'
    [[ "$(readlink -f -- /proc/self/fd/9)" == "$lock_file" ]] ||
        fail 'holder inherited the wrong lock descriptor'
    flock -n -s 9 || fail 'holder did not inherit the shared cache lock'
    worker_matches "$owner_pid" "$owner_start" || fail 'owning Runner.Worker exited before holder startup'
    snapshot="$(process_snapshot "$$")" || fail 'cannot read holder process identity'
    read -r _ holder_start _ <<<"$snapshot"
    [[ ! -e "$pid_file" && ! -L "$pid_file" ]] || fail "job lock state already exists: $pid_file"
    umask 077
    printf '%s %s %s %s %s\n' "$$" "$token" "$holder_start" "$owner_pid" "$owner_start" >"$pid_file.tmp.$$"
    mv -T -- "$pid_file.tmp.$$" "$pid_file"
    cleanup_holder() {
        trap - EXIT INT TERM HUP
        [[ -n "$sleep_pid" ]] && kill -TERM "$sleep_pid" 2>/dev/null || true
        current="$(read_record 2>/dev/null || true)"
        read -r current_pid current_token current_start _ <<<"$current"
        if [[ "$current_pid $current_token $current_start" == "$$ $token $holder_start" ]]; then
            rm -f -- "$pid_file"
        fi
        # Closing fd9 is insufficient if a descendant holds a duplicated fd.
        flock -u 9
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

release_start_locks() {
    [[ "$cache_lock_transferred" == true ]] || flock -u 9
    flock -u 8
}

start_job() {
    local stale holder_pid token holder_start launched_start snapshot owner_pid owner_start owner record attempt
    lock_slot
    owner="$(worker_identity)" || fail 'job hook has no live owning Runner.Worker'
    read -r owner_pid owner_start <<<"$owner"
    exec 9>>"$lock_file"
    cache_lock_transferred=false
    trap release_start_locks EXIT
    flock -s -w "$lock_wait_seconds" 9 ||
        fail "timed out waiting for the cache prune barrier after $lock_wait_seconds seconds"
    "$mount_guard_bin" 8>&- || fail 'runner cache mount guard rejected job start'
    if [[ -e "$pid_file" || -L "$pid_file" ]]; then
        stale="$(read_record)" || fail "invalid or unsafe existing job lock state: $pid_file"
        local stale_owner_pid stale_owner_start
        read -r holder_pid token holder_start stale_owner_pid stale_owner_start <<<"$stale"
        if holder_matches "$holder_pid" "$token" "$holder_start"; then
            if [[ -z "$holder_start" ]] || worker_matches "$stale_owner_pid" "$stale_owner_start"; then
                fail "slot $slot already has a live job lock holder"
            fi
            retire_holder "$holder_pid" "$token" "$holder_start"
            printf 'runner slot %s reclaimed a lock from an exited Runner.Worker\n' "$slot"
        elif [[ -n "$holder_start" ]]; then
            process_matches "$holder_pid" "$holder_start" &&
                fail "slot $slot job lock state names an unrelated live process"
            rm -f -- "$pid_file"
        else
            kill -0 "$holder_pid" 2>/dev/null &&
                fail "slot $slot job lock state names an unrelated live process"
            rm -f -- "$pid_file"
        fi
    fi

    token="$(printf '%s-%s-%s\n' "$$" "$(date +%s%N)" "$RANDOM" | sha256sum | awk '{print $1}')"
    RUNNER_TRACKING_ID='' nohup "$self" --hold "$slot" "$token" "$owner_pid" "$owner_start" 8>&- 9>&9 \
        >>"$state_root/slot-$slot.log" 2>&1 &
    holder_pid=$!
    snapshot="$(process_snapshot "$holder_pid" 2>/dev/null || true)"
    read -r _ launched_start _ <<<"$snapshot"
    for ((attempt = 0; attempt < 100; attempt++)); do
        record="$(read_record 2>/dev/null || true)"
        read -r _ _ holder_start _ <<<"$record"
        if [[ "$record" == "$holder_pid $token $holder_start $owner_pid $owner_start" ]] &&
            holder_matches "$holder_pid" "$token" "$holder_start"; then
            # fd9 deliberately transfers its shared open description to the
            # holder. LOCK_UN here would also unlock the holder's barrier.
            cache_lock_transferred=true
            exec 9>&-
            printf 'runner slot %s acquired the shared cache lock (pid %s)\n' "$slot" "$holder_pid"
            return 0
        fi
        kill -0 "$holder_pid" 2>/dev/null || break
        sleep 0.05
    done
    if [[ -n "$launched_start" ]]; then
        stop_holder "$holder_pid" "$launched_start" "$token" || true
    fi
    fail "slot $slot cache lock holder did not become ready"
}

complete_job() {
    local record holder_pid holder_token holder_start owner_pid owner_start snapshot owner
    lock_slot
    record="$(read_record)" || fail "missing or invalid job lock state for slot $slot"
    read -r holder_pid holder_token holder_start owner_pid owner_start <<<"$record"
    holder_matches "$holder_pid" "$holder_token" "$holder_start" ||
        fail "slot $slot job lock state does not name its verified holder"
    if [[ -n "$holder_start" ]]; then
        owner="$(worker_identity)" || fail 'job completion has no live owning Runner.Worker'
        [[ "$owner" == "$owner_pid $owner_start" ]] || fail 'job completion belongs to another Runner.Worker'
    else
        snapshot="$(process_snapshot "$holder_pid")" || fail 'cannot read holder process identity'
        read -r _ holder_start _ <<<"$snapshot"
    fi
    retire_holder "$holder_pid" "$holder_token" "$holder_start"
    printf 'runner slot %s released the shared cache lock\n' "$slot"
}

case "$mode" in
    --hold) holder_main ;;
    --job-started) start_job ;;
    --job-completed) complete_job ;;
esac
