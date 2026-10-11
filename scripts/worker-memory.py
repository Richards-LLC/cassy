#!/usr/bin/env python3
"""Run a worker JS/browser/build command under the assembly host budget."""
import argparse
import importlib.util
import os
import json
from pathlib import Path
import shlex
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


def estimate(command, cwd=None, depth=0):
    """Only known bounded commands enter the light lane; shell hints cannot opt in.

    Resolve package scripts and literal shell statements before reserving their
    maximum weight. Unknown commands, expansions and shell fanout stay heavy.
    Direct Vitest is constrained before classification; shell Vitest must carry
    an explicit cap because its arguments cannot be rewritten safely.
    """
    heavy = host_memory.DEFAULT_ESTIMATE_BYTES
    cwd = Path.cwd() if cwd is None else Path(cwd)
    if not command or depth > 8:
        return heavy
    name, args = Path(command[0]).name, command[1:]
    if name in ('bash', 'sh', 'dash', 'zsh') and args[:1] in (['-c'], ['-lc']):
        if len(args) != 2 or any(char in args[1] for char in ('$','`','\n')):
            return heavy
        try:
            lexer = shlex.shlex(args[1], posix=True, punctuation_chars=True)
            lexer.whitespace_split = True
            lexer.commenters = ''
            words = list(lexer)
        except ValueError:
            return heavy
        statements, current, redirect = [], [], False
        for word in words:
            if redirect:
                if word in ('&&', ';', '&', '|', '||', '>', '>>', '<', '>&'):
                    return heavy
                redirect = False
                continue
            if word in ('>', '>>', '<', '>&'):
                if current and current[-1].isdigit(): current.pop()
                redirect = True
                continue
            if word in ('&&', ';'):
                if not current: return heavy
                statements.append(current)
                current = []
            elif word in ('&', '|', '||', '(', ')', '<<'):
                return heavy
            else:
                current.append(word)
        if redirect: return heavy
        if current: statements.append(current)
        weights = []
        for statement in statements:
            if statement[0] == 'cd' and len(statement) == 2:
                cwd = cwd / statement[1]
            else:
                weights.append(estimate(statement, cwd, depth + 1))
        return max(weights, default=heavy)
    if name == 'tsc' and not any(arg.startswith('--watch') or arg == '-w' for arg in args):
        return host_memory.GIB
    if name == 'vite' and args[:1] == ['build'] and not any(arg.startswith('--watch') for arg in args):
        return host_memory.GIB
    if name in ('node', 'nodejs') and args:
        script = Path(args[0]).name
        if script == 'vitest.mjs':
            return estimate(['vitest', *args[1:]], cwd, depth + 1)
        if script == 'run-verified-tests.mjs' and args[1:2] == ['vitest']:
            # This entry point enforces maxWorkers=2 itself.
            return 2 * host_memory.GIB
        if script == 'generate-tokens.mjs':
            return host_memory.GIB
    if name == 'vitest' and not any(arg == '-w' or arg.startswith(('--watch', '--browser')) for arg in args):
        cap = None
        for index, arg in enumerate(args):
            if arg.startswith('--maxWorkers='): cap = arg.split('=', 1)[1]
            elif arg == '--maxWorkers' and index + 1 < len(args): cap = args[index + 1]
        if cap and cap.isdigit() and 1 <= int(cap) <= 2 and (not args or args[0] == 'run'):
            return 2 * host_memory.GIB
    if name == 'npx' and args and not args[0].startswith('-'):
        return estimate(args, cwd, depth + 1)
    if name == 'npm' and args:
        script = args[1] if args[0] in ('run', 'run-script') and len(args) > 1 else args[0]
        # Extra caller arguments could change a bounded script's behavior.
        expected = 2 if args[0] in ('run', 'run-script') else 1
        if len(args) != expected: return heavy
        try:
            body = json.loads((cwd / 'package.json').read_text())['scripts'][script]
            return estimate(['sh', '-c', body], cwd, depth + 1)
        except (OSError, ValueError, KeyError, TypeError):
            return heavy
    if name in ('python3', 'python') and args and Path(args[0]).name == 'worker-memory.py' and args[1:2] == ['--']:
        return estimate(constrained(args[2:]), cwd, depth + 1)
    return heavy


def _literal_statements(script):
    """Split a literal `a && b ; c` shell script; None for anything else."""
    if any(char in script for char in ('$', '`', '\n')):
        return None
    try:
        lexer = shlex.shlex(script, posix=True, punctuation_chars=True)
        lexer.whitespace_split = True
        lexer.commenters = ''
        words = list(lexer)
    except ValueError:
        return None
    statements, current = [], []
    for word in words:
        if word in ('&&', ';'):
            if not current:
                return None
            statements.append(current)
            current = []
        elif any(char in word for char in '&|()<>'):
            return None  # background, pipes, subshells and redirections stay out
        else:
            current.append(word)
    return statements + [current] if current else (statements or None)


def _playwright_workers(args):
    for arg in args:
        if arg.startswith('--workers='):
            value = arg.split('=', 1)[1]
            return int(value) if value.isdigit() else None
    return None


def taste_eligible(command, cwd=None, depth=0):
    """cas-833e: may this command take the taste lane beside a running proof?

    Only positively bounded commands qualify: a light command whose estimate
    is below the heavy default, a Playwright run capped at four workers
    (cas-bb5e measured about 2.6 GiB peak at four), or the Commander visual QA
    (one Chromium, sequential pages). Unknown commands, shell expansions,
    background jobs and pipelines never do.
    """
    cwd = Path.cwd() if cwd is None else Path(cwd)
    if not command or depth > 8:
        return False
    name, args = Path(command[0]).name, command[1:]
    if name in ('bash', 'sh', 'dash', 'zsh') and args[:1] in (['-c'], ['-lc']):
        statements = _literal_statements(args[1]) if len(args) == 2 else None
        if not statements:
            return False
        for statement in statements:
            if statement[0] == 'cd' and len(statement) == 2:
                cwd = cwd / statement[1]
            elif not taste_eligible(statement, cwd, depth + 1):
                return False
        return True
    if name in ('python3', 'python') and args and Path(args[0]).name == 'worker-memory.py' and args[1:2] == ['--']:
        return taste_eligible(constrained(args[2:]), cwd, depth + 1)
    if name == 'npm' and args[:1] == ['exec'] and '--' in args:
        # `npm exec --yes --package=playwright -- node scripts/visual-qa.mjs`
        return taste_eligible(args[args.index('--') + 1:], cwd, depth + 1)
    if name in ('node', 'nodejs') and args:
        script = Path(args[0])
        if script.name == 'visual-qa.mjs' and len(args) == 1:
            return True
        if script.name == 'cli.js' and '@playwright' in args[0] and args[1:2] == ['test']:
            workers = _playwright_workers(args)
            return workers is not None and 1 <= workers <= 4
    if (name == 'npx' and args[:2] == ['playwright', 'test']) or (name == 'playwright' and args[:1] == ['test']):
        workers = _playwright_workers(args)
        return workers is not None and 1 <= workers <= 4
    return estimate(command, cwd, depth) < host_memory.DEFAULT_ESTIMATE_BYTES


def run(command, env=None, directory=None):
    env = dict(os.environ if env is None else env)
    wait = proof.positive_knob(env, 'CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS') or 600
    poll = proof.positive_knob(env, 'CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLL_SECS') or 1
    command = constrained(command)
    # A compiler-cache server first started inside the suite would inherit the
    # suite's tree and outlive it (cas-7b7b9); start it outside admission.
    host_memory.start_compiler_cache(env)
    lane = 'taste' if taste_eligible(command) else None
    with host_memory.admission('worker', env, proof.memory_budget, wait, poll, directory,
                               estimate_bytes=estimate(command), lane=lane) as (admitted_env, fds), \
         host_memory.LeaseHolder(fds) as holder:
        try:
            in_taste_lane = json.loads(admitted_env.get(host_memory.LEASE_ENV, '{}')).get('lane') == 'taste'
        except ValueError:
            in_taste_lane = False
        # The command never receives the lease descriptors: a daemon or orphan
        # it leaves behind cannot hold the budget. The holder covers a killed
        # wrapper for as long as the command's process group runs.
        child = subprocess.Popen(command, env=admitted_env, start_new_session=True)
        holder.track(child.pid)
        handlers = {}
        def interrupted(sig, frame):
            raise InterruptedError('worker suite interrupted by ' + signal.Signals(sig).name)
        try:
            for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
                handlers[sig] = signal.signal(sig, interrupted)
            while child.poll() is None:
                memory = proof.memory_budget(env)
                if in_taste_lane:
                    # The lane runs inside the proof's reserve, where the
                    # budget is ~0 by design; protect the floor instead.
                    if memory['available_bytes'] < host_memory.TASTE_FLOOR_BYTES:
                        raise ValueError('taste lane reached the memory floor; stopping its process group')
                elif memory['budget_bytes'] < proof.GUARD_HEADROOM_BYTES:
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


BACKGROUND_REFUSAL = (
    'a background job (&) in an admitted suite command is stopped when the command '
    'returns: its receipt launcher dies while a detached native runner can live on '
    'unreported (cas-04ebf). Run the suite in the foreground of a persistent session '
    'with its output redirected to a log (Codex: exec and yield the session; Claude '
    'Code: run_in_background), then read the log.')


def background_job(shell_command):
    """Whether a literal shell command puts any job in the background.

    Only a bare `&` token is a background operator: `&&`, `>&`, `&>`, `&>>`
    and `|&` lex as their own tokens, and quoted text is never an operator.
    A command the lexer cannot parse counts when any bare `&` appears in it,
    so a quoting error never hides one.
    """
    try:
        lexer = shlex.shlex(shell_command, posix=True, punctuation_chars=True)
        lexer.whitespace_split = True
        lexer.commenters = ''
        return '&' in list(lexer)
    except ValueError:
        return '&' in shell_command.replace('&&', '').replace('>&', '').replace('&>', '').replace('|&', '')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--shell-command', help='literal hook command; classify before running bash -c')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = ['bash', '-c', args.shell_command] if args.shell_command is not None else (
        args.command[1:] if args.command[:1] == ['--'] else args.command)
    if args.shell_command is not None and args.command:
        parser.error('--shell-command cannot be combined with another command')
    if not command:
        parser.error('a command is required after --')
    # cas-04ebf: run() owns the command's whole process group and ends it when
    # the shell returns, so a backgrounded suite would be cut down mid-run.
    if args.shell_command is not None and background_job(args.shell_command):
        print('worker memory admission: ' + BACKGROUND_REFUSAL, file=sys.stderr)
        return 2
    return run(command)


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, InterruptedError) as exc:
        print('worker memory admission: ' + str(exc), file=sys.stderr)
        sys.exit(1)
