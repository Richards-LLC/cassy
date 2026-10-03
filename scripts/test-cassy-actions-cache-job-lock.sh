#!/usr/bin/env bash
# Linux-only fixture: GNU tools, mount tables and /proc are the tested runner contracts.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
job_lock="$script_dir/cassy-actions-cache-job-lock.sh"
pruner="$script_dir/prune-cassy-actions-cache.sh"
"$script_dir/test-cassy-actions-process-identity.sh"
source "$script_dir/cassy-actions-process-identity.sh"
if [[ "$(uname -s)" != Linux ]]; then
    printf 'SKIP job lock integration: Linux /proc and util-linux flock required; owner comparison tested above\n'
    exit 0
fi
fixture_root="$(mktemp -d)"
holder_started=0
worker_pid=''
inherited_pid=''
cleanup() {
    if (( holder_started )); then
        run_completed >/dev/null 2>&1 || true
    fi
    [[ -z "$worker_pid" ]] || kill -TERM "$worker_pid" 2>/dev/null || true
    [[ -z "$inherited_pid" ]] || kill -TERM "$inherited_pid" 2>/dev/null || true
    if [[ -s "$state_root/slot-1.pid" ]]; then
        read -r remaining_pid _ <"$state_root/slot-1.pid"
        kill -TERM "$remaining_pid" 2>/dev/null || true
    fi
    rm -rf "$fixture_root"
}
trap cleanup EXIT

cache_root="$fixture_root/cache"
state_root="$fixture_root/state"
cgroup_root="$fixture_root/cgroup"
proc_root="$fixture_root/proc"
mkdir -p "$cache_root" "$state_root" "$fixture_root/volume" \
    "$cgroup_root/slot1" "$cgroup_root/slot2" "$proc_root/101" "$proc_root/102"
printf '101\n' >"$cgroup_root/slot1/cgroup.procs"
printf '102\n' >"$cgroup_root/slot2/cgroup.procs"
printf 'Runner.Listener\n' >"$proc_root/101/comm"
printf 'Runner.Listener\n' >"$proc_root/102/comm"
for slot in '' '-2'; do
    mkdir -p "$cache_root/cargo-target$slot" "$cache_root/sccache$slot"
done
ln -s "$job_lock" "$fixture_root/cache-job-started.sh"
ln -s "$job_lock" "$fixture_root/cache-job-completed.sh"
started_hook="$fixture_root/cache-job-started.sh"
completed_hook="$fixture_root/cache-job-completed.sh"

cat >"$fixture_root/systemctl" <<'EOF'
#!/usr/bin/env bash
case "$*" in
    *cassy-actions-runner-2.service*) printf '/slot2\n' ;;
    *cassy-actions-runner.service*) printf '/slot1\n' ;;
    *) exit 2 ;;
esac
EOF

cat >"$fixture_root/mount-guard" <<'EOF'
#!/usr/bin/env bash
if [[ "${TEST_INHERIT_LOCK:-}" == 1 ]]; then
    sleep 30 &
    printf '%s\n' "$!" >"$TEST_INHERITED_PID"
fi
if [[ "${TEST_BLOCK_GUARD:-}" == 1 ]]; then
    : >"$TEST_GUARD_ENTERED"
    while [[ ! -e "$TEST_GUARD_RELEASE" ]]; do sleep 0.02; done
fi
EOF
chmod +x "$fixture_root/systemctl" "$fixture_root/mount-guard"

common_env=(
    CASSY_ACTIONS_ALLOW_TEST_ROOT=1
    CASSY_ACTIONS_CACHE_ROOT="$cache_root"
    CASSY_ACTIONS_STATE_ROOT="$state_root"
    CASSY_ACTIONS_MOUNT_GUARD_BIN="$fixture_root/mount-guard"
    RUNNER_TRACKING_ID=runner-would-reap-this-holder
)

# A real process named Runner.Worker owns each hook through its ancestry.
# Killing it leaves the detached holder alive, exactly as a runner crash does.
cat >"$fixture_root/worker.py" <<'PYWORKER'
import ctypes
import pathlib
import subprocess
import sys
import time
ctypes.CDLL(None).prctl(15, b"Runner.Worker", 0, 0, 0)
root = pathlib.Path(sys.argv[1])
(root / "worker-ready").touch()
while True:
    request = root / "request"
    if request.exists():
        hook = request.read_text().strip()
        request.unlink()
        with (root / "hook-output").open("w") as output:
            result = subprocess.run([hook], stdout=output, stderr=subprocess.STDOUT)
        (root / "response").write_text(str(result.returncode))
    time.sleep(0.01)
PYWORKER

start_worker() {
    rm -f "$fixture_root/worker-ready" "$fixture_root/request" "$fixture_root/response"
    env "${common_env[@]}" CASSY_ACTIONS_RUNNER_SLOT=1 \
        python3 "$fixture_root/worker.py" "$fixture_root" &
    worker_pid=$!
    for _ in $(seq 1 100); do
        [[ -e "$fixture_root/worker-ready" ]] && return 0
        sleep 0.02
    done
    printf 'FAIL fake Runner.Worker did not start\n' >&2
    exit 1
}

run_hook() {
    local hook="$1" result
    rm -f "$fixture_root/response"
    printf '%s\n' "$hook" >"$fixture_root/request.tmp"
    mv "$fixture_root/request.tmp" "$fixture_root/request"
    for _ in $(seq 1 500); do
        if [[ -e "$fixture_root/response" ]]; then
            result="$(cat "$fixture_root/response")"
            cat "$fixture_root/hook-output"
            return "$result"
        fi
        sleep 0.02
    done
    printf 'FAIL hook did not respond\n' >&2
    return 1
}

run_started() { run_hook "$started_hook"; }
run_completed() { run_hook "$completed_hook"; }
start_worker

run_pruner() {
    env "${common_env[@]}" \
        CASSY_ACTIONS_CGROUP_ROOT="$cgroup_root" \
        CASSY_ACTIONS_PROC_ROOT="$proc_root" \
        CASSY_ACTIONS_SYSTEMCTL_BIN="$fixture_root/systemctl" \
        CASSY_ACTIONS_TARGET_BUDGET_BYTES=32768 \
        CASSY_ACTIONS_SCCACHE_BUDGET_BYTES=8192 \
        CASSY_ACTIONS_SLOT_BUDGET_BYTES=60000 \
        "$pruner" "$@"
}

run_started
holder_started=1
test -s "$state_root/slot-1.pid"
read -r holder_pid _ <"$state_root/slot-1.pid"
if tr '\0' '\n' <"/proc/$holder_pid/environ" | grep -qx 'RUNNER_TRACKING_ID=runner-would-reap-this-holder'; then
    printf 'FAIL job lock holder retained GitHub Runner tracking identity\n' >&2
    exit 1
fi
if run_started >/dev/null 2>&1; then
    printf 'FAIL duplicate job start replaced a live per-slot holder\n' >&2
    exit 1
fi
printf 'ok   installed-shape started hook leaves a detached untracked holder alive\n'
if run_pruner --now >/dev/null 2>&1; then
    printf 'FAIL forced prune crossed a live job shared lock\n' >&2
    exit 1
fi
scheduled_output="$(run_pruner --scheduled)"
[[ "$scheduled_output" == *'scheduled prune skipped'* ]]
run_completed
holder_started=0
run_pruner --now >/dev/null
printf 'ok   job-lifetime shared lock excludes scheduled and forced pruning\n'

# The Worker dies, but the old detached holder (and shared flock) survives.
run_started >/dev/null
holder_started=1
read -r orphan_pid _ <"$state_root/slot-1.pid"
kill -KILL "$worker_pid"
wait "$worker_pid" 2>/dev/null || true
kill -0 "$orphan_pid"
start_worker
run_started
read -r recovered_pid _ <"$state_root/slot-1.pid"
[[ "$recovered_pid" != "$orphan_pid" ]]
run_completed
holder_started=0
run_pruner --now >/dev/null
printf 'PASS crashed Runner.Worker lock recovery with real /proc and flock\n'

# A recycled owner PID must not keep a dead job alive, even if that PID exists.
run_started >/dev/null
holder_started=1
read -r old_holder token holder_start owner_pid owner_start <"$state_root/slot-1.pid"
printf '%s %s %s %s %s\n' "$old_holder" "$token" "$holder_start" "$owner_pid" \
    "$((owner_start + 1))" >"$state_root/slot-1.pid"
run_started >/dev/null
read -r new_holder _ <"$state_root/slot-1.pid"
[[ "$new_holder" != "$old_holder" ]]
run_completed
holder_started=0
printf 'ok   owner PID reuse is detected by process start identity\n'

# A stale detailed record naming an unrelated reused PID can be discarded,
# but must never signal the new process at that PID.
sleep 30 &
reused_pid=$!
snapshot="$(process_snapshot "$reused_pid")"
read -r _ reused_start _ <<<"$snapshot"
printf '%s %s %s %s %s\n' "$reused_pid" stale-token "$((reused_start + 1))" \
    "$worker_pid" 1 >"$state_root/slot-1.pid"
run_started >/dev/null
holder_started=1
kill -0 "$reused_pid"
run_completed
holder_started=0
kill -TERM "$reused_pid"
wait "$reused_pid" 2>/dev/null || true
printf 'ok   holder PID reuse never signals an unrelated live process\n'

# A mount guard descendant deliberately retains the shared open description.
# Completing the job must LOCK_UN, rather than relying on fd close or exit.
kill -TERM "$worker_pid"
wait "$worker_pid" 2>/dev/null || true
TEST_INHERIT_LOCK=1 TEST_INHERITED_PID="$fixture_root/inherited-pid" start_worker
run_started >/dev/null
holder_started=1
inherited_pid="$(cat "$fixture_root/inherited-pid")"
run_completed
holder_started=0
kill -0 "$inherited_pid"
run_pruner --now >/dev/null
kill -TERM "$inherited_pid"
inherited_pid=''
kill -TERM "$worker_pid"
wait "$worker_pid" 2>/dev/null || true
start_worker
printf 'ok   explicit unlock releases the cache barrier across inherited descriptors\n'

run_started >/dev/null
holder_started=1
read -r failed_holder_pid _ <"$state_root/slot-1.pid"
kill -KILL "$failed_holder_pid"
for _ in $(seq 1 100); do
    kill -0 "$failed_holder_pid" 2>/dev/null || break
    sleep 0.02
done
mkdir -p "$proc_root/103"
printf '103\n' >>"$cgroup_root/slot1/cgroup.procs"
printf 'Runner.Worker\n' >"$proc_root/103/comm"
failed_holder_marker="$cache_root/cargo-target/holder-failed-marker"
mkdir -p "$failed_holder_marker"
if run_pruner --now >/dev/null 2>&1; then
    printf 'FAIL forced prune crossed Runner.Worker after holder failure\n' >&2
    exit 1
fi
worker_fallback_output="$(run_pruner --scheduled)"
[[ "$worker_fallback_output" == *'Runner.Worker owns an active job'* ]]
test -d "$failed_holder_marker"
rm -f "$state_root/slot-1.pid"
sed -i '/103/d' "$cgroup_root/slot1/cgroup.procs"
holder_started=0
printf 'ok   Runner.Worker keeps pruning fail-closed after abrupt holder loss\n'

guard_entered="$fixture_root/guard-entered"
guard_release="$fixture_root/guard-release"
TEST_BLOCK_GUARD=1 TEST_GUARD_ENTERED="$guard_entered" \
    TEST_GUARD_RELEASE="$guard_release" run_pruner --now >/dev/null &
pruner_pid=$!
for _ in $(seq 1 100); do
    [[ -e "$guard_entered" ]] && break
    sleep 0.02
done
test -e "$guard_entered"
run_started >/dev/null &
hook_pid=$!
sleep 0.1
if ! kill -0 "$hook_pid" 2>/dev/null; then
    printf 'FAIL job-started crossed an active exclusive prune lock\n' >&2
    exit 1
fi
test ! -e "$state_root/slot-1.pid"
touch "$guard_release"
wait "$pruner_pid"
wait "$hook_pid"
holder_started=1
test -s "$state_root/slot-1.pid"
run_completed
holder_started=0
printf 'ok   job start cannot cross an active prune barrier\n'

if run_completed >/dev/null 2>&1; then
    printf 'FAIL duplicate job completion accepted a missing holder\n' >&2
    exit 1
fi
printf 'ok   missing or duplicate lifecycle state fails closed\n'

sleep 30 &
unrelated_pid=$!
printf '%s %s\n' "$unrelated_pid" not-a-holder >"$state_root/slot-1.pid"
if run_completed >/dev/null 2>&1; then
    printf 'FAIL completion accepted an unrelated PID\n' >&2
    exit 1
fi
kill -0 "$unrelated_pid"
kill -TERM "$unrelated_pid"
wait "$unrelated_pid" 2>/dev/null || true
rm -f "$state_root/slot-1.pid"
printf 'ok   completion never signals an unverified PID\n'

printf 'PASS runner cache job lock lifecycle integration\n'
