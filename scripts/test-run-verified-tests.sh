#!/usr/bin/env bash
# Self-test for scripts/run-verified-tests.sh (GH #499).
set -uo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="${script_dir}/run-verified-tests.sh"
tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT

pass=0
fail=0

make_stub() {
    local name="$1" status="$2"
    local stub="${tmpdir}/${name}"
    {
        echo '#!/usr/bin/env bash'
        echo "cat <<'STUB_EOF'"
        cat
        echo 'STUB_EOF'
        echo "exit ${status}"
    } >"${stub}"
    chmod +x "${stub}"
    echo "${stub}"
}

expect() {
    local wanted="$1" needle="$2" label="$3"
    shift 3
    local output status
    output="$("$@" 2>&1)"
    status=$?
    local ok=1
    [[ "${wanted}" == pass && "${status}" -ne 0 ]] && ok=0
    [[ "${wanted}" == fail && "${status}" -eq 0 ]] && ok=0
    [[ "${ok}" -eq 1 && "${needle}" != - ]] && ! grep -qF -- "${needle}" <<<"${output}" && ok=0
    if [[ "${ok}" -eq 1 ]]; then
        pass=$((pass + 1))
        echo "ok   ${label}"
    else
        fail=$((fail + 1))
        echo "FAIL ${label} (exit ${status})" >&2
        echo "${output}" >&2
    fi
}

stub="${tmpdir}/green-nextest"
cat >"${stub}" <<'EOF'
#!/usr/bin/env bash
echo '    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.01s'
echo '    Starting 2 tests across 1 binary (0 skipped)'
sleep 0.4
echo '------------'
echo '     Summary [   0.012s] 2 tests run: 2 passed, 0 skipped'
EOF
chmod +x "${stub}"
expect pass 'PASS: 2 test(s) passed' 'nextest green receipt passes' \
    env CARGO="${stub}" CAS_WORKER_NEXTEST_DISCOVERY_TIMEOUT_SECS=0.2 \
    "${guard}" nextest run -p cas --lib module

count_file="${tmpdir}/count"
expect pass 'PASS: 2 test(s) passed' 'embedded guard exports count in supplied worktree' \
    env CARGO="${stub}" VERIFIED_TEST_REPO_ROOT="${tmpdir}" VERIFIED_TEST_COUNT_FILE="${count_file}" \
    bash -c "$(cat "${guard}")" worker-verified-tests nextest run -p cas --lib -E 'test(module)'
expect pass '2' 'count export contains guarded nonzero count' cat "${count_file}"

stub="$(make_stub zero-cargo 0 <<'EOF'
running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.00s
EOF
)"
expect fail '0 tests passed' 'zero-match Cargo result fails loudly' \
    env CARGO="${stub}" "${guard}" test -p cas --lib stale_filter

stub="$(make_stub zero-nextest 0 <<'EOF'
    Starting 0 tests across 1 binary (4 skipped)
------------
     Summary [   0.001s] 0 tests run: 0 passed, 4 skipped
EOF
)"
expect fail '0 tests passed' 'zero-match nextest result fails loudly' \
    env CARGO="${stub}" "${guard}" nextest run -p cas --lib stale_filter

echo 99 >"${count_file}"
expect fail '0 tests passed' 'zero-match retry clears stale exported count' \
    env CARGO="${stub}" VERIFIED_TEST_COUNT_FILE="${count_file}" "${guard}" nextest run -p cas --lib -E 'test(stale)'
expect pass - 'failed guard leaves no exported count' test ! -e "${count_file}"

# cas-7cb0: nextest may stall while a libtest child writes its --list output.
# The capped wrapper must diagnose and stop discovery before tests start.
cat >"${tmpdir}/stalled-nextest" <<'EOF'
#!/usr/bin/env bash
echo '    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.01s'
trap 'echo TERM > "${CAS_TEST_TERM_MARKER}"' TERM
# Closing the output pipe must not bypass the discovery deadline.
exec 1>&- 2>&-
sleep 2 &
wait
EOF
chmod +x "${tmpdir}/stalled-nextest"
cat >"${tmpdir}/sample" <<'EOF'
#!/usr/bin/env bash
echo 'fake libtest stack: StdoutLock::write'
EOF
chmod +x "${tmpdir}/sample"
expect fail 'nextest discovery timeout' 'stalled nextest lister has a bounded, named failure' \
    env PATH="${tmpdir}:${PATH}" CARGO="${tmpdir}/stalled-nextest" \
    VERIFIED_TEST_LOG="${tmpdir}/stalled.log" CAS_TEST_TERM_MARKER="${tmpdir}/terminated" \
    CAS_WORKER_NEXTEST_DISCOVERY_TIMEOUT_SECS=0.2 \
    "${guard}" nextest run -p cas --lib -E 'test(stalled)'
expect pass 'TERM' 'stalled discovery kills its process group' \
    cat "${tmpdir}/terminated"
expect pass 'phase=--list PID=' 'stalled discovery names phase and child PID' \
    grep -F 'phase=--list PID=' "${tmpdir}/stalled.log"
expect pass 'fake libtest stack: StdoutLock::write' 'stalled discovery logs a child sample' \
    grep -F 'fake libtest stack: StdoutLock::write' "${tmpdir}/stalled.log"

stub="$(make_stub no-harness 0 <<'EOF'
Finished `test` profile after a swallowed build failure
EOF
)"
expect fail 'no test harness ever reported' 'swallowed pre-harness failure fails loudly' \
    env CARGO="${stub}" "${guard}" test -p cas --doc

stub="$(make_stub failing 42 <<'EOF'
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
EOF
)"
expect fail 'the test run exited 42' 'real Cargo failure propagates' \
    env CARGO="${stub}" "${guard}" test -p cas --doc

# cas-aee36: simulate termination of the capped foreground process group.
# Every PID/group below belongs to this fixture; never signal a host process.
cat >"${tmpdir}/kill-discovery.py" <<'PYKILL'
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

guard, temporary = sys.argv[1:]
root = Path(temporary)
fake = root / 'discovery-cargo.py'
fake.write_text('''#!/usr/bin/env python3
import json, os, subprocess, sys, time
from pathlib import Path
leave_child = os.environ.get('LEAVE_CHILD') == '1'
child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'],
                         stdout=subprocess.DEVNULL if leave_child else None)
marker = Path(os.environ['PID_FILE'])
ready = marker.with_suffix('.ready')
ready.write_text(json.dumps([os.getpid(), child.pid]))
ready.replace(marker)
print('    Finished `test` profile in 0.01s', flush=True)
if leave_child:
    print('    Starting 2 tests across 1 binary (0 skipped)', flush=True)
    print('     Summary [   0.012s] 2 tests run: 2 passed, 0 skipped', flush=True)
    sys.exit(0)
child.wait()
''')
fake.chmod(0o755)

def live(pid):
    result = subprocess.run(['ps', '-p', str(pid), '-o', 'stat='],
                            capture_output=True, text=True, timeout=2)
    return bool(result.stdout.strip()) and not result.stdout.strip().startswith('Z')

def exited_without_reaping(process, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        event = os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
        if event is not None:
            return event
        time.sleep(0.02)
    raise AssertionError(f'wrapper PID {process.pid} did not exit')

def cleanup(wrapper, pids):
    # The wrapper is still unreaped, so its group id cannot be reused.
    try:
        os.killpg(wrapper.pid, signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        pass
    # The fake Cargo leader waits 60s in signal cases; never target a reaped group.
    if pids and live(pids[0]):
        try:
            os.killpg(pids[0], signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            pass
    for pid in pids[1:]:
        if live(pid):
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
    wrapper.wait(timeout=3)

pid_file = root / 'pids-normal-exit'
env = dict(os.environ, CARGO=str(fake), PID_FILE=str(pid_file), LEAVE_CHILD='1',
           CAS_WORKER_NEXTEST_DISCOVERY_TIMEOUT_SECS='60')
pids = []
with (root / 'normal-exit.log').open('wb') as log:
    wrapper = subprocess.Popen([guard, 'nextest', 'run', '-p', 'cas', '--lib'],
                               env=env, stdout=log, stderr=subprocess.STDOUT,
                               start_new_session=True)
    try:
        deadline = time.monotonic() + 5
        while not pid_file.exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        assert pid_file.exists(), 'normal-exit fixture did not start'
        pids = json.loads(pid_file.read_text())
        assert live(pids[1]), 'normal-exit child must exist before cleanup'
        assert exited_without_reaping(wrapper, 5).si_status == 0, 'normal nextest exit must pass'
        deadline = time.monotonic() + 5
        while live(pids[1]) and time.monotonic() < deadline:
            time.sleep(0.02)
        assert not live(pids[1]), f'normal-exit Cargo descendant survived: {pids}'
    finally:
        cleanup(wrapper, pids)

for interrupted, target in [
    (signal.SIGTERM, 'group'),
    (signal.SIGINT, 'group'),
    (signal.SIGHUP, 'group'),
    (signal.SIGTERM, 'wrapper'),
]:
    pid_file = root / f'pids-{interrupted}-{target}'
    env = dict(os.environ, CARGO=str(fake), PID_FILE=str(pid_file),
               CAS_WORKER_NEXTEST_DISCOVERY_TIMEOUT_SECS='60')
    pids = []
    with (root / f'signal-{interrupted}-{target}.log').open('wb') as log:
        wrapper = subprocess.Popen([guard, 'nextest', 'run', '-p', 'cas', '--lib'],
                                   env=env, stdout=log, stderr=subprocess.STDOUT,
                                   start_new_session=True)
        try:
            deadline = time.monotonic() + 5
            while not pid_file.exists() and time.monotonic() < deadline:
                time.sleep(0.02)
            assert pid_file.exists(), 'fixture did not reach discovery'
            pids = json.loads(pid_file.read_text())
            assert all(live(pid) for pid in pids), 'fixture children must be alive before signal'
            if target == 'group':
                os.killpg(wrapper.pid, interrupted)
            else:
                os.kill(wrapper.pid, interrupted)
            assert exited_without_reaping(wrapper, 5).si_status != 0, 'interrupted run must not report success'
            deadline = time.monotonic() + 5
            while any(live(pid) for pid in pids) and time.monotonic() < deadline:
                time.sleep(0.02)
            assert not any(live(pid) for pid in pids), f'{interrupted}: discovery children survived: {pids}'
        finally:
            cleanup(wrapper, pids)
print('signal cleanup passed: normal exit, TERM, INT, HUP and wrapper TERM; no live discovery children')
PYKILL
expect pass 'signal cleanup passed' 'terminated discovery wrapper leaves no live children' \
    python3 "${tmpdir}/kill-discovery.py" "${guard}" "${tmpdir}"

if [[ "${fail}" -ne 0 ]]; then
    echo "FAIL: ${fail} verified-test guard self-test(s) failed" >&2
    exit 1
fi
echo "PASS: ${pass} verified-test guard self-test(s) passed."
