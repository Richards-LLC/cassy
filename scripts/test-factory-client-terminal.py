#!/usr/bin/env python3
"""Prove factory attach restores a real PTY on detach and termination signals.

Run against an installed binary or supervisor-built candidate:
  python3 scripts/test-factory-client-terminal.py --cas /path/to/cas --out <dir>
No factory agents are started: a disposable Unix socket supplies the frame.
"""

import argparse
import errno
import json
import os
from pathlib import Path
import pty
import select
import shutil
import signal
import socket
import subprocess
import tempfile
import termios
import time


RESETS = (b"\x1b[?1000l", b"\x1b[?1006l", b"\x1b[?2004l", b"\x1b[?1049l")
CASES = {"detach": None, "sigterm": signal.SIGTERM, "sighup": signal.SIGHUP,
         "sigint": signal.SIGINT, "blocked-output": signal.SIGTERM}


def check_exit(binary, case, out):
    with tempfile.TemporaryDirectory(prefix="cas-client-", dir="/tmp") as scratch:
        home = Path(scratch)
        sessions = home / ".cas/sessions"
        sessions.mkdir(parents=True)
        address = str(home / "client.sock")
        metadata = {
            "name": "terminal-proof", "created_at": "2026-10-22T00:00:00Z",
            "daemon_pid": os.getpid(), "socket_path": address,
            "supervisor": {"name": "fixture", "pid": None, "worktree_path": None},
            "workers": [], "epic_id": None,
        }
        (sessions / "terminal-proof.json").write_text(json.dumps(metadata))
        env = {key: value for key, value in os.environ.items() if not key.startswith("CAS_")}
        env.update(HOME=scratch, TERM="xterm-256color")
        master, slave = pty.openpty()
        before = termios.tcgetattr(slave)
        child = None
        peer = None
        captured = bytearray()
        frozen = case == "blocked-output"
        # crossterm may make stdin nonblocking. A separate open keeps that
        # read-side flag from also making the frozen output writer nonblocking.
        output = os.open(os.ttyname(slave), os.O_WRONLY | os.O_NOCTTY) if frozen else slave
        queued = 0
        signalled_at = None
        timed_out = False

        try:
            if frozen:
                # Fill the output PTY before starting the client and never drain
                # it. Once the client enters raw mode, its setup write must block.
                os.set_blocking(output, False)
                while queued < 8 * 1024 * 1024:
                    try:
                        queued += os.write(output, b"x" * 65536)
                    except BlockingIOError:
                        break
                else:
                    raise AssertionError("fixture did not fill terminal output")
                os.set_blocking(output, True)
            with socket.socket(socket.AF_UNIX) as listener:
                listener.bind(address)
                listener.listen(1)
                child = subprocess.Popen(
                    [binary, "attach", "terminal-proof"], cwd=scratch, env=env,
                    stdin=slave, stdout=output, stderr=output, start_new_session=True,
                )
                triggered = False
                was_raw = False
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    sources = ([] if frozen else [master]) + ([] if peer else [listener])
                    readable, _, _ = select.select(sources, [], [], 0.05)
                    if listener in readable:
                        peer, _ = listener.accept()
                        peer.sendall(b"\x1b[?1049hfixture frame\r\n")
                    if master in readable:
                        try:
                            data = os.read(master, 65536)
                        except OSError as error:
                            if error.errno != errno.EIO:
                                raise
                            data = b""  # Linux PTY EOF; macOS returns empty bytes.
                        if not data:
                            break
                        captured.extend(data)
                    raw_now = not (termios.tcgetattr(slave)[3] & (termios.ECHO | termios.ICANON))
                    ready = raw_now if frozen else b"\x1b[?1006h" in captured and b"fixture frame" in captured
                    if not triggered and peer and ready:
                        was_raw = not (termios.tcgetattr(slave)[3] & (termios.ECHO | termios.ICANON))
                        triggered = True
                        if frozen:
                            deadline = time.monotonic() + 5
                        if CASES[case] is None:
                            peer.shutdown(socket.SHUT_RDWR)
                        else:
                            signalled_at = time.monotonic()
                            child.send_signal(CASES[case])
                    if child.poll() is not None and (frozen or not readable):
                        break
                else:
                    if not triggered:
                        raise AssertionError(f"{case}: client did not become ready within 15s")
                    timed_out = True
                    child.kill()  # Only test cleanup; a timeout must remain FAIL.
                status = child.wait(timeout=1)
                signal_elapsed = time.monotonic() - signalled_at if signalled_at else None
                missing = [value.decode("ascii") for value in RESETS if value not in captured]
                restored = termios.tcgetattr(slave) == before
                expected_exit = status == 0 if CASES[case] is None else status == -CASES[case]
                terminal_ok = bool(queued) if frozen else restored and not missing
                result = {
                    "case": case, "exit_status": status, "entered_raw_mode": was_raw,
                    "raw_restored": restored, "missing_resets": missing,
                    "blocked_output_bytes": queued, "signal_elapsed_seconds": signal_elapsed,
                    "timed_out": timed_out,
                    "pass": triggered and was_raw and terminal_ok and expected_exit and not timed_out,
                }
                return result
        finally:
            if child and child.poll() is None:
                child.kill()
                child.wait()
            if peer:
                peer.close()
            (out / f"{case}.ansi").write_bytes(captured)
            if output != slave:
                os.close(output)
            os.close(master)
            os.close(slave)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cas", default="cas", help="installed binary or supervisor candidate")
    parser.add_argument("--case", choices=CASES, action="append", dest="cases")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    binary = shutil.which(args.cas)
    if not binary:
        parser.error(f"binary not found: {args.cas}")
    args.out.mkdir(parents=True, exist_ok=True)
    results = [check_exit(str(Path(binary).resolve()), case, args.out) for case in args.cases or CASES]
    (args.out / "report.json").write_text(json.dumps({"binary": binary, "results": results}, indent=2) + "\n")
    for row in results:
        print(f"{row['case']}: {'PASS' if row['pass'] else 'FAIL'} exit={row['exit_status']} "
              f"raw_restored={row['raw_restored']} missing_resets={row['missing_resets']!r} "
              f"signal_seconds={row['signal_elapsed_seconds']} timed_out={row['timed_out']}")
    return 0 if all(row["pass"] for row in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
