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
# the raw captured output when a caller needs a durable receipt.

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
    (cd "${REPO_ROOT}" && python3 - "${CARGO}" "$@" <<'PY'
import os
import re
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
child = subprocess.Popen(
    [cargo, *args], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
    stderr=subprocess.STDOUT, start_new_session=True,
)
selector = selectors.DefaultSelector()
selector.register(child.stdout, selectors.EVENT_READ)
finished = re.compile(r"(?m)^\s*Finished\b")
starting = re.compile(r"(?m)^\s*Starting\s+\d+\s+tests?\b")
window = ""
discovery_at = None
tests_started = False

def emit(message):
    os.write(1, (message + "\n").encode(errors="replace"))

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
                        os.write(1, chunk)
            else:
                os.write(1, result.stdout)
            emit("--- end nextest discovery sample ---")
        except (OSError, subprocess.TimeoutExpired) as error:
            emit(f"nextest discovery sample failed for PID {pid}: {error}")

while True:
    events = selector.select(0.1)
    for key, _ in events:
        chunk = os.read(key.fd, 65536)
        if not chunk:
            selector.unregister(key.fileobj)
            break
        os.write(1, chunk)
        window = (window + chunk.decode(errors="replace"))[-8192:]
        clean = re.sub(r"\x1b\[[0-9;]*[A-Za-z]", "", window)
        if discovery_at is None and finished.search(clean):
            discovery_at = time.monotonic()
        if starting.search(clean):
            tests_started = True
    if discovery_at is not None and not tests_started:
        elapsed = time.monotonic() - discovery_at
        if elapsed >= limit:
            pid = listing_child(child.pid)
            emit(f"FAIL: nextest discovery timeout: phase=--list PID={pid} runner_PID={child.pid} elapsed={elapsed:.1f}s limit={limit:g}s")
            capture_sample(pid)
            try:
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                child.wait(timeout=0.5)
            except subprocess.TimeoutExpired:
                pass
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait()
            sys.exit(124)
    if not selector.get_map():
        break
sys.exit(child.wait() if child.returncode is None else child.returncode)
PY
    ) | tee "${log}"
else
    (cd "${REPO_ROOT}" && "${CARGO}" "$@" 2>&1) | tee "${log}"
fi
cargo_status="${PIPESTATUS[0]}"

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
