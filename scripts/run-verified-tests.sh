#!/usr/bin/env bash
#
# Mechanical receipt for every Cargo test command whose success is consumed by
# a worker, CI lane, Make target, or release gate (GH #499).  Cargo and
# nextest both exit zero when a filter selects no tests.  Exit status alone is
# therefore not evidence that the intended suite ran.
#
# Usage:
#   scripts/run-verified-tests.sh nextest run -p cas --lib module
#   scripts/run-verified-tests.sh test -p cas --doc
#
# CARGO may replace the cargo binary in self-tests.  VERIFIED_TEST_LOG keeps
# the raw captured output when a caller needs a durable receipt. The bounded
# nextest discovery path requires python3, including downstream embedded use.

set -uo pipefail

REPO_ROOT="${VERIFIED_TEST_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
CARGO="${CARGO:-cargo}"

# A failed retry must not leave an earlier exported count.
if [[ -n "${VERIFIED_TEST_COUNT_FILE:-}" ]]; then
    rm -f -- "${VERIFIED_TEST_COUNT_FILE}" || exit 1
fi

if [[ $# -eq 0 ]]; then
    echo "error: pass a Cargo test command, e.g. nextest run -p cas --lib module" >&2
    exit 1
fi

if [[ -n "${VERIFIED_TEST_LOG:-}" ]]; then
    log="${VERIFIED_TEST_LOG}"
    : >"${log}"
    clean_log="$(mktemp)"
    trap 'rm -f "${clean_log}"' EXIT
else
    log="$(mktemp)"
    clean_log="$(mktemp)"
    trap 'rm -f "${log}" "${clean_log}"' EXIT
fi

echo "Running: ${CARGO} $*"
echo
if [[ "${1:-}" == nextest && "${2:-}" == run && -n "${CAS_WORKER_NEXTEST_DISCOVERY_TIMEOUT_SECS:-}" ]]; then
    # The capped worker runner sets this bound. Cargo may compile for much
    # longer; start the clock only after its Finished line, and disarm it when
    # nextest announces that the named tests are Starting.
    runner_pid=
    pending_wrapper_signal=
    forward_wrapper() {
        local signum="$1" status="$2"
        if [[ -z "${runner_pid}" ]]; then
            pending_wrapper_signal="${signum}:${status}"
            return
        fi
        trap - TERM INT HUP
        kill -s "${signum}" "${runner_pid}" 2>/dev/null || true
        wait "${runner_pid}" 2>/dev/null || true
        exit "${status}"
    }
    trap 'forward_wrapper TERM 143' TERM
    trap 'forward_wrapper INT 130' INT
    trap 'forward_wrapper HUP 129' HUP
    (cd "${REPO_ROOT}" && exec python3 - "${CARGO}" "$@" <<'PY'
import os
import re
import select
import selectors
import shutil
import signal
import subprocess
import sys
import tempfile
import time

cargo, *args = sys.argv[1:]
limit = float(os.environ["CAS_WORKER_NEXTEST_DISCOVERY_TIMEOUT_SECS"])
if limit <= 0:
    sys.exit("nextest discovery timeout must be positive")
child = None
pending_signal = None
selector = selectors.DefaultSelector()
finished = re.compile(r"(?m)^\s*Finished\b")
starting = re.compile(r"(?m)^\s*Starting\s+\d+\s+tests?\b")
window = ""
discovery_at = None
tests_started = False

def emit(message):
    write((message + "\n").encode(errors="replace"))

def write(data):
    os.write(1, data)
    os.write(3, data)

def listing_child(root):
    try:
        rows = subprocess.run(
            ["ps", "-A", "-o", "pid=,ppid="], capture_output=True,
            text=True, timeout=2, check=True,
        ).stdout.splitlines()
        parents = {}
        for row in rows:
            pid, ppid = map(int, row.split()[:2])
            parents[pid] = ppid
        depths = {root: 0}
        for _ in range(len(parents)):
            newer = {pid: depths[ppid] + 1 for pid, ppid in parents.items()
                     if ppid in depths and pid not in depths}
            if not newer:
                break
            depths.update(newer)
        # A libtest --list process is below Cargo/nextest in this tree.
        return max(depths, key=depths.get)
    except (OSError, ValueError, subprocess.SubprocessError):
        return root

def capture_sample(pid):
    sample = shutil.which("sample")
    if not sample:
        emit(f"nextest discovery sample unavailable for PID {pid}: sample command missing")
        return
    with tempfile.TemporaryDirectory(prefix="cas-nextest-sample-") as directory:
        report = os.path.join(directory, "sample.txt")
        try:
            result = subprocess.run(
                [sample, str(pid), "1", "-file", report],
                stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                timeout=6, check=False,
            )
            emit(f"--- nextest discovery sample: PID {pid}, sample exit {result.returncode} ---")
            if os.path.exists(report):
                with open(report, "rb") as source:
                    while chunk := source.read(65536):
                        write(chunk)
            else:
                write(result.stdout)
            emit("--- end nextest discovery sample ---")
        except (OSError, subprocess.TimeoutExpired) as error:
            emit(f"nextest discovery sample failed for PID {pid}: {error}")

watched_signals = (signal.SIGTERM, signal.SIGINT, signal.SIGHUP)
exit_queue = None
exit_seen = False
last_ps_probe = 0.0

def leader_exited():
    global exit_queue, exit_seen, last_ps_probe
    if child.returncode is not None:
        return True
    if hasattr(os, "waitid"):
        return os.waitid(os.P_PID, child.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT) is not None
    # macOS system Python 3.9 lacks waitid. kqueue observes exit without
    # reaping, leaving the child PID/PGID reserved until group cleanup ends.
    if exit_seen:
        return True
    if exit_queue is not None:
        try:
            if exit_queue.control(None, 1, 0):
                exit_seen = True
                return True
        except OSError:
            exit_queue.close()
            exit_queue = None
        else:
            return False
    # Other Python/OS combinations can still observe the unreaped zombie.
    now = time.monotonic()
    if now - last_ps_probe < 0.2:
        return False
    last_ps_probe = now
    try:
        state = subprocess.run(
            ["ps", "-p", str(child.pid), "-o", "stat="],
            capture_output=True, text=True, timeout=1,
        ).stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        return False
    exit_seen = state.startswith("Z")
    return exit_seen

def signal_group(signum):
    # An unreaped session leader reserves its PID/PGID until group cleanup ends.
    if child.returncode is not None:
        return
    try:
        os.killpg(child.pid, signum)
    except (ProcessLookupError, PermissionError):
        pass

def stop_child(signum):
    # The process group belongs only to this Popen child and its descendants.
    if child.returncode is not None:
        return
    if not leader_exited():
        signal_group(signum)
        deadline = time.monotonic() + 0.5
        while not leader_exited() and time.monotonic() < deadline:
            time.sleep(0.02)
    # Descendants may ignore the signal. The leader is still unreaped here.
    signal_group(signal.SIGKILL)
    try:
        child.wait(timeout=2)
    except subprocess.TimeoutExpired:
        try:
            emit(f"FAIL: could not reap nextest runner PID {child.pid} after SIGKILL")
        except BrokenPipeError:
            pass

def forward_signal(signum, _frame):
    global pending_signal
    if child is None:
        pending_signal = signum
        return
    for watched in watched_signals:
        signal.signal(watched, signal.SIG_IGN)
    stop_child(signum)
    sys.exit(128 + signum)

def discovery_timeout(elapsed):
    pid = listing_child(child.pid)
    emit(f"FAIL: nextest discovery timeout: phase=--list PID={pid} runner_PID={child.pid} elapsed={elapsed:.1f}s limit={limit:g}s")
    capture_sample(pid)
    stop_child(signal.SIGTERM)
    sys.exit(124)

# Install before launch so a signal received while Popen starts is not lost.
for watched in watched_signals:
    signal.signal(watched, forward_signal)
# Preserve the admitted target lease through the watchdog, while Popen still
# closes every unrelated inherited descriptor before launching Cargo.
lease_fd = os.environ.get("CAS_TARGET_LEASE_FD")
child = subprocess.Popen(
    [cargo, *args], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
    stderr=subprocess.STDOUT, start_new_session=True,
    pass_fds=(int(lease_fd),) if lease_fd else (),
)
if not hasattr(os, "waitid") and hasattr(select, "kqueue"):
    try:
        exit_queue = select.kqueue()
        exit_queue.control([
            select.kevent(child.pid, filter=select.KQ_FILTER_PROC,
                          flags=select.KQ_EV_ADD, fflags=select.KQ_NOTE_EXIT)
        ], 0, 0)
    except (OSError, AttributeError):
        if exit_queue is not None:
            exit_queue.close()
        exit_queue = None
if pending_signal is not None:
    forward_signal(pending_signal, None)
selector.register(child.stdout, selectors.EVENT_READ)
group_cleaned = False
try:
    while True:
        events = selector.select(0.1)
        for key, _ in events:
            chunk = os.read(key.fd, 65536)
            if not chunk:
                selector.unregister(key.fileobj)
                break
            write(chunk)
            window = (window + chunk.decode(errors="replace"))[-8192:]
            clean = re.sub(r"\x1b\[[0-9;]*[A-Za-z]", "", window)
            if discovery_at is None and finished.search(clean):
                discovery_at = time.monotonic()
            if starting.search(clean):
                tests_started = True
        if not group_cleaned and leader_exited():
            signal_group(signal.SIGKILL)
            group_cleaned = True
        if discovery_at is not None and not tests_started:
            elapsed = time.monotonic() - discovery_at
            if elapsed >= limit:
                discovery_timeout(elapsed)
        if not selector.get_map():
            break
    # EOF is not process completion: a child may close stdout and keep the
    # build-directory lock. Honor the discovery deadline and bound exit cleanup.
    wait_limit = 5.0
    if discovery_at is not None and not tests_started:
        wait_limit = min(wait_limit, max(0, limit - (time.monotonic() - discovery_at)))
    deadline = time.monotonic() + wait_limit
    while not leader_exited() and time.monotonic() < deadline:
        time.sleep(0.02)
    if not leader_exited():
        if discovery_at is not None and not tests_started:
            elapsed = time.monotonic() - discovery_at
            if elapsed >= limit:
                discovery_timeout(elapsed)
        emit(f"FAIL: nextest process exit timeout after stdout closed: runner_PID={child.pid} limit={wait_limit:g}s")
        stop_child(signal.SIGTERM)
        status = 124
    else:
        if not group_cleaned:
            signal_group(signal.SIGKILL)
        status = child.wait(timeout=2)
finally:
    selector.close()
    if child.returncode is None:
        stop_child(signal.SIGTERM)
    if exit_queue is not None:
        exit_queue.close()
sys.exit(status)
PY
    ) 3>>"${log}" &
    runner_pid=$!
    if [[ -n "${pending_wrapper_signal}" ]]; then
        forward_wrapper "${pending_wrapper_signal%%:*}" "${pending_wrapper_signal##*:}"
    fi
    wait "${runner_pid}"
    cargo_status=$?
    trap - TERM INT HUP
else
    (cd "${REPO_ROOT}" && "${CARGO}" "$@" 2>&1) | tee "${log}"
    cargo_status="${PIPESTATUS[0]}"
fi

# Cargo and nextest color their summary lines in CI.  Preserve the requested
# raw log while parsing an ANSI-free copy.
sed -E $'s/\x1b\\[[0-9;]*[A-Za-z]//g' "${log}" >"${clean_log}"

cargo_test_summaries="$(grep -c '^test result:' "${clean_log}" 2>/dev/null || true)"
cargo_test_passed="$(
    sed -n 's/^test result: [a-zA-Z]*\. \([0-9]\{1,\}\) passed.*/\1/p' "${clean_log}" 2>/dev/null |
        awk '{ total += $1 } END { print total + 0 }'
)"
nextest_summaries="$(grep -cE '^ *Summary +\[.*\] +[0-9]+ tests? run:' "${clean_log}" 2>/dev/null || true)"
nextest_passed="$(
    sed -n 's/^ *Summary \{1,\}\[.*\] *[0-9]\{1,\} tests\{0,1\} run: \([0-9]\{1,\}\) passed.*/\1/p' "${clean_log}" 2>/dev/null |
        awk '{ total += $1 } END { print total + 0 }'
)"

summaries=$((cargo_test_summaries + nextest_summaries))
passed=$((cargo_test_passed + nextest_passed))

fail() {
    echo "FAIL: $1" >&2
    shift
    for line in "$@"; do
        echo "      ${line}" >&2
    done
    exit 1
}

echo
echo "--- verified-test guard (GH #499) ---"
if [[ "${cargo_status}" -ne 0 ]]; then
    echo "FAIL: the test run exited ${cargo_status}." >&2
    exit "${cargo_status}"
fi
if [[ "${summaries}" -eq 0 ]]; then
    fail "cargo exited 0 but no test harness ever reported." \
        "No Cargo or nextest summary was emitted, so this command proved no test completed."
fi
if [[ "${passed}" -eq 0 ]]; then
    fail "0 tests passed across ${summaries} harness summary line(s)." \
        "A zero-test result is a failed verification, even when Cargo reports success."
fi

echo "PASS: ${passed} test(s) passed across ${summaries} harness summary line(s)."

# The capped worker runner consumes only this guarded, nonzero pass count.
if [[ -n "${VERIFIED_TEST_COUNT_FILE:-}" ]]; then
    printf '%s\n' "${passed}" >"${VERIFIED_TEST_COUNT_FILE}" || exit 1
fi
