#!/usr/bin/env python3
"""Self-test for scripts/ci-step-watchdog.py (cas-02ab).

Replays the Nov 17 lane wedge: a compile that never finishes. FAKE_WEDGE
makes the fixture command park a process named rustc for crate cas_store.
"""

import os
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

WATCHDOG = Path(__file__).resolve().parent / "ci-step-watchdog.py"

# Each run appends one line; FAKE_WEDGE=always|first wedges every or only the first run.
COMMAND = r"""
echo run >>"$RUNS"
runs=$(wc -l <"$RUNS")
if [[ "$FAKE_WEDGE" == always || ( "$FAKE_WEDGE" == first && $runs -eq 1 ) ]]; then
    echo "   Compiling cas-store v0.1.0"
    exec -a rustc bash -c 'echo $$ >"$WEDGED_PID"; sleep 600; :' --crate-name cas_store
fi
sleep "${FAKE_SECONDS:-0}"
exit "${FAKE_EXIT:-0}"
"""


def run(tmp, timeout_minutes=0.05, **env):
    runs = tmp / f"runs-{time.monotonic_ns()}"
    pid_file = tmp / "wedged.pid"
    pid_file.unlink(missing_ok=True)
    environment = dict(os.environ, RUNS=str(runs), WEDGED_PID=str(pid_file), **env)
    started = time.monotonic()
    try:
        result = subprocess.run(
            [sys.executable, str(WATCHDOG), "--label", "own tests", "--timeout-minutes", str(timeout_minutes),
             "--heartbeat-seconds", "1", "--log", str(tmp / "watchdog.log"), "--", "bash", "-c", COMMAND],
            env=environment, capture_output=True, text=True, timeout=60)
    except subprocess.TimeoutExpired as error:
        # A broken watchdog can exit while its fixture still holds the pipes.
        # Stop only the session our own fixture recorded, retaining its error.
        if pid_file.exists():
            try:
                os.killpg(int(pid_file.read_text()), signal.SIGKILL)
            except ProcessLookupError:
                pass
        if error.stderr:
            print(error.stderr.decode(errors="replace") if isinstance(error.stderr, bytes) else error.stderr,
                  file=sys.stderr)
        raise
    return result, len(runs.read_text().splitlines()), time.monotonic() - started


def alive(pid_file):
    try:
        os.kill(int(pid_file.read_text()), 0)
        return True
    except (OSError, ValueError):
        return False


def main():
    failures = []

    def check(ok, name, detail=""):
        print(f"{'ok  ' if ok else 'FAIL'} {name}")
        if not ok:
            failures.append(name)
            print(detail)

    with tempfile.TemporaryDirectory() as directory:
        tmp = Path(directory)

        # A wedge on both attempts fails at the deadline with the phase named, once per attempt.
        result, runs, seconds = run(tmp, FAKE_WEDGE="always")
        errors = [line for line in result.stdout.splitlines() if line.startswith("::error title=Lane step wedged::")]
        check(result.returncode == 124 and runs == 2 and len(errors) == 2
              and all("last phase: rustc cas_store" in line for line in errors)
              and seconds < 30 and not alive(tmp / "wedged.pid"),
              "a wedged compile fails at the deadline, naming rustc cas_store, after one retry",
              result.stdout + result.stderr)

        # A wedge that clears on the retry passes.
        result, runs, _ = run(tmp, FAKE_WEDGE="first")
        check(result.returncode == 0 and runs == 2 and result.stdout.count("::error") == 1,
              "the one retry recovers a wedged first attempt", result.stdout + result.stderr)

        # A command's own failure is not retried.
        result, runs, _ = run(tmp, FAKE_EXIT="3")
        check(result.returncode == 3 and runs == 1 and "::error" not in result.stdout,
              "a failing command keeps its status with no retry", result.stdout + result.stderr)

        # A normal run emits progress before its exit record and retains it.
        # Memory telemetry is best effort (memory() can report unavailable).
        result, runs, _ = run(tmp, timeout_minutes=1, FAKE_SECONDS="2.5")
        beats = result.stdout.splitlines()[:-1]
        check(result.returncode == 0 and runs == 1 and len(beats) >= 2
              and all(beat in (tmp / "watchdog.log").read_text() for beat in beats),
              "a normal run emits and retains heartbeat diagnostics", result.stdout + result.stderr)

        # Losing diagnostic tools must not stop deadline enforcement.
        tools = tmp / "bin"
        tools.mkdir()
        ps = tools / "ps"
        ps.write_text("#!/bin/sh\nexit 1\n")
        ps.chmod(0o755)
        result, runs, seconds = run(tmp, FAKE_WEDGE="always", PATH=str(tools) + os.pathsep + os.environ["PATH"])
        check(result.returncode == 124 and runs == 2 and seconds < 30 and not alive(tmp / "wedged.pid"),
              "a failed process snapshot still stops both wedged attempts", result.stdout + result.stderr)

    print(f"\n{5 - len(failures)} passed, {len(failures)} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
