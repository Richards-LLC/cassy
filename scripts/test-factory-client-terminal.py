#!/usr/bin/env python3
"""Prove factory attach restores a real PTY on detach and termination signals.

Run against an installed binary or supervisor-built candidate:
  python3 scripts/test-factory-client-terminal.py --cas /path/to/cas --out <dir>
No factory agents are started: a disposable Unix socket supplies the frame.
"""

import argparse
import errno
import fcntl
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
CASES = {"detach": None, "sigterm": signal.SIGTERM, "sighup": signal.SIGHUP, "sigint": signal.SIGINT}


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

        def controlling_terminal():
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

        try:
            with socket.socket(socket.AF_UNIX) as listener:
                listener.bind(address)
                listener.listen(1)
                child = subprocess.Popen(
                    [binary, "attach", "terminal-proof"], cwd=scratch, env=env,
                    stdin=slave, stdout=slave, stderr=slave, preexec_fn=controlling_terminal,
                )
                triggered = False
                was_raw = False
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    sources = [master] + ([] if peer else [listener])
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
                    if not triggered and peer and b"\x1b[?1006h" in captured and b"fixture frame" in captured:
                        was_raw = not (termios.tcgetattr(slave)[3] & (termios.ECHO | termios.ICANON))
                        triggered = True
                        if CASES[case] is None:
                            peer.shutdown(socket.SHUT_RDWR)
                        else:
                            child.send_signal(CASES[case])
                    if child.poll() is not None and not readable:
                        break
                else:
                    raise AssertionError(f"{case}: client did not become ready and exit within 15s")
                status = child.wait(timeout=1)
                missing = [value.decode("ascii") for value in RESETS if value not in captured]
                restored = termios.tcgetattr(slave) == before
                expected_exit = status == 0 if CASES[case] is None else status in (-CASES[case], 128 + CASES[case])
                result = {
                    "case": case, "exit_status": status, "entered_raw_mode": was_raw,
                    "raw_restored": restored, "missing_resets": missing,
                    "pass": triggered and was_raw and restored and not missing and expected_exit,
                }
                return result
        finally:
            if child and child.poll() is None:
                child.kill()
                child.wait()
            if peer:
                peer.close()
            (out / f"{case}.ansi").write_bytes(captured)
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
              f"raw_restored={row['raw_restored']} missing_resets={row['missing_resets']!r}")
    return 0 if all(row["pass"] for row in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
