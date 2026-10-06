#!/usr/bin/env python3
"""Run a worker JS/browser/build command under the assembly host budget."""
import argparse
import importlib.util
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

import host_memory

spec = importlib.util.spec_from_file_location('assembly_proof', Path(__file__).with_name('assembly-proof.py'))
proof = importlib.util.module_from_spec(spec)
spec.loader.exec_module(proof)


def constrained(command):
    # Last authoritative argument wins, including npm/npx forwarded arguments.
    words = [Path(word).name for word in command]
    flag = '--workers' if 'playwright' in words or 'cli.js' in words and any('@playwright' in w for w in command) else (
        '--maxWorkers' if 'vitest' in words or 'vitest.mjs' in words else None)
    if not flag:
        return command
    result, skip, requested = [], False, None
    for word in command:
        if skip:
            skip, requested = False, word
        elif word == flag:
            skip = True
        elif word.startswith(flag + '='):
            requested = word[len(flag) + 1:]
        else:
            result.append(word)
    if flag == '--maxWorkers':
        return result + ['--maxWorkers=2']
    # Playwright keeps one browser worker unless the caller (journey-eval.sh)
    # asks for more; the explicit request is honoured up to four (cas-3ae7).
    workers = int(requested) if requested and requested.isdigit() else 1
    return result + [f'--workers={min(max(workers, 1), 4)}']


def run(command, env=None, directory=None):
    env = dict(os.environ if env is None else env)
    wait = proof.positive_knob(env, 'CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS') or 600
    poll = proof.positive_knob(env, 'CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLL_SECS') or 1
    with host_memory.admission('worker', env, proof.memory_budget, wait, poll, directory) as (admitted_env, fds):
        child = subprocess.Popen(constrained(command), env=admitted_env, pass_fds=fds, start_new_session=True)
        handlers = {}
        def interrupted(sig, frame):
            raise InterruptedError('worker suite interrupted by ' + signal.Signals(sig).name)
        try:
            for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
                handlers[sig] = signal.signal(sig, interrupted)
            while child.poll() is None:
                if proof.memory_budget(env)['budget_bytes'] < proof.GUARD_HEADROOM_BYTES:
                    raise ValueError('worker suite reached assembly memory headroom; stopping its process group')
                time.sleep(poll)
            return child.returncode if child.returncode >= 0 else 128 - child.returncode
        finally:
            for sig in (signal.SIGCONT, signal.SIGTERM):
                try:
                    os.killpg(child.pid, sig)
                except ProcessLookupError:
                    pass
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()
            # A normally exited shell may have left background descendants.
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            for sig, handler in handlers.items():
                signal.signal(sig, handler)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command:
        parser.error('a command is required after --')
    return run(command)


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, InterruptedError) as exc:
        print('worker memory admission: ' + str(exc), file=sys.stderr)
        sys.exit(1)
