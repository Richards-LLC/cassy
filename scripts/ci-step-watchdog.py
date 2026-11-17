#!/usr/bin/env python3
"""Run one CI step command under a deadline, a heartbeat and one retry (cas-02ab).

A wedged hosted-runner compile printed only warnings for 55 minutes and ended
"cancelled" (exit 143) with no log kept. This wrapper gives the step its own
deadline below the job limit, prints the live cargo/rustc/linker phase and
free memory every few minutes, and on the deadline names the last phase in a
GitHub ::error::, stops the command's process group and reruns it once. Only
the deadline retries: a command that exits nonzero on its own keeps its status.

Usage:
  ci-step-watchdog.py --label NAME --timeout-minutes N [--heartbeat-seconds S]
                      [--log PATH] -- COMMAND...

Exit: the command's status, or 124 when the retry also reaches the deadline.
"""

from __future__ import annotations

import argparse
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

# Process names that identify a build or test phase.
PHASES = ("cargo-nextest", "nextest", "rustc", "cc", "c++", "gcc", "clang", "ld", "ld.lld", "lld", "mold",
          "zig", "cargo")
GRACE_SECONDS = 20


def processes():
    """Every readable process as {pid: (ppid, argv, rss_kib, started_ticks)}."""
    table = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            stat = (entry / "stat").read_text()
            argv = (entry / "cmdline").read_bytes().split(b"\0")
            status = (entry / "status").read_text()
        except OSError:
            continue
        fields = stat[stat.rindex(")") + 2:].split()
        rss = next((int(line.split()[1]) for line in status.splitlines() if line.startswith("VmRSS:")), 0)
        table[int(entry.name)] = (int(fields[1]), [a.decode(errors="replace") for a in argv if a], rss, int(fields[19]))
    return table


def describe(argv):
    """`rustc cas_store` for a compile, else the program name."""
    names = [os.path.basename(arg) for arg in argv[:2]]
    program = next((name for name in names if name in PHASES), names[0] if names else "?")
    if "--crate-name" in argv[:-1]:
        return f"{program} {argv[argv.index('--crate-name') + 1]}"
    if program in ("cargo", "cargo-nextest") and len(argv) > 1:
        return f"{program} {' '.join(arg for arg in argv[1:3] if not arg.startswith('-'))}".strip()
    return program


def phase(root):
    """The command tree's newest build/test processes, then the largest
    process on the host: sccache runs rustc outside this tree."""
    table = processes()
    tree, frontier = set(), {root}
    while frontier:
        tree |= frontier
        frontier = {pid for pid, (ppid, *_rest) in table.items() if ppid in frontier and pid not in tree}
    active = sorted((table[pid] for pid in tree if pid in table and table[pid][1]
                     and os.path.basename(table[pid][1][0]) in PHASES + ("sccache",)), key=lambda p: -p[3])
    seen, names = set(), []
    for _ppid, argv, _rss, _start in active:
        name = describe(argv)
        if name not in seen:
            seen.add(name)
            names.append(name)
    largest = max((p for p in table.values() if p[1]), key=lambda p: p[2], default=None)
    host = f"; largest process {describe(largest[1])} {largest[2] // 1024} MiB" if largest else ""
    return (", ".join(names[:4]) or "no build process") + host


def memory():
    info = {}
    try:
        for line in Path("/proc/meminfo").read_text().splitlines():
            key, value = line.split(":", 1)
            info[key] = int(value.split()[0])
    except OSError:
        return "memory unavailable"
    gib = lambda kib: f"{kib / 1048576:.1f} GiB"
    swap = info.get("SwapTotal", 0) - info.get("SwapFree", 0)
    return f"MemAvailable {gib(info.get('MemAvailable', 0))} of {gib(info.get('MemTotal', 0))}, swap used {gib(swap)}"


def stop(process):
    """Stop the command's whole process group, then reap it."""
    for sig, wait in ((signal.SIGTERM, GRACE_SECONDS), (signal.SIGKILL, None)):
        try:
            os.killpg(process.pid, sig)
        except ProcessLookupError:
            break
        try:
            process.wait(timeout=wait)
            break
        except subprocess.TimeoutExpired:
            continue
    process.wait()


def attempt(args, number, total, log):
    def say(line):
        print(line, flush=True)
        log.write(line + "\n")
        log.flush()

    started = time.monotonic()
    process = subprocess.Popen(args.command, start_new_session=True)
    while True:
        remaining = args.timeout_minutes * 60 - (time.monotonic() - started)
        try:
            status = process.wait(timeout=max(0, min(args.heartbeat_seconds, remaining)))
            say(f"watchdog: {args.label} attempt {number}/{total} exited {status} after "
                f"{(time.monotonic() - started) / 60:.1f} min")
            return status, None
        except subprocess.TimeoutExpired:
            pass
        last = phase(process.pid)
        elapsed = (time.monotonic() - started) / 60
        if remaining <= 0:
            stop(process)
            return 124, f"{args.label} exceeded {args.timeout_minutes} min (attempt {number}/{total}); last phase: {last}; {memory()}"
        say(f"watchdog: {args.label} {elapsed:.1f} min (attempt {number}/{total}): {last}; {memory()}")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--label", required=True)
    parser.add_argument("--timeout-minutes", type=float, required=True)
    parser.add_argument("--heartbeat-seconds", type=float, default=180)
    parser.add_argument("--log", type=Path)
    parser.add_argument("command", nargs="+")
    args = parser.parse_args(argv)
    if args.command[0] == "--":
        args.command = args.command[1:]
    log_path = args.log or Path(os.devnull)
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with open(log_path, "a") as log:
        for number in (1, 2):
            status, wedged = attempt(args, number, 2, log)
            if wedged is None:
                return status
            log.write(f"wedged: {wedged}\n")
            log.flush()
            retry = "; rerunning once" if number == 1 else ""
            print(f"::error title=Lane step wedged::{wedged}{retry}", flush=True)
    return 124


if __name__ == "__main__":
    sys.exit(main())
