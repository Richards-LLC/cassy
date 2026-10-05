#!/usr/bin/env python3
"""Require explicit, verifiable release-rescue lessons before receipts finish."""
import os
from pathlib import Path
import re
import shlex
import sqlite3
import subprocess
import sys

FILES = ('blockers.log', 'supervisor-interventions.md')
STAGES = set('preflight assemble prep ledger gate pr-body pipeline publish post-publication announce report receipts host-update'.split())
LOG = 'cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md'


def git(root, *args):
    return subprocess.check_output(['git', '-C', str(root), *args], text=True).strip()


def executable_lessons(root):
    try:
        source = (root / 'scripts/release-gate.sh').read_text()
        registered = set(re.search(r'gate_check_ids=\((.*?)\)', source, re.S)[1].split())
        learned = set(re.findall(r'\*\*([a-z0-9-]+)\*\*', (root / LOG).read_text()))
        return registered & learned
    except (OSError, TypeError):
        return set()


def task_open(root, identifier):
    database = os.environ.get('CAS_RELEASE_LEARNING_TASK_DB')
    if not database:
        common = Path(git(root, 'rev-parse', '--path-format=absolute', '--git-common-dir'))
        database = common.parent / '.cas/cas.db'
    try:
        with sqlite3.connect(Path(database).resolve().as_uri() + '?mode=ro', uri=True) as connection:
            row = connection.execute('SELECT status FROM tasks WHERE id = ?', (identifier,)).fetchone()
        return bool(row and row[0] in ('open', 'in_progress', 'blocked', 'awaiting_merge'))
    except (sqlite3.Error, OSError):
        return False


def evidence_rows(run_dir):
    for filename in FILES:
        path = run_dir / filename
        if not path.exists():
            continue
        for number, line in enumerate(path.read_text().splitlines(), 1):
            stripped = line.strip()
            if not stripped or stripped.startswith('#'):
                continue
            yield filename, number, line


def check(root, run_dir):
    lessons = executable_lessons(root)
    failures = 0
    for filename, number, line in evidence_rows(run_dir):
        learned = re.findall(r'(?:^|\s)learn=([a-z0-9:-]+)(?=\s|$)', line)
        tasks = re.findall(r'(?:^|\s)task=(cas-[a-z0-9]+)(?=\s|$)', line)
        stages = (line.split()[0].split(',') if filename == 'blockers.log' else [])
        valid_stage = not stages or all(stage in STAGES for stage in stages)
        if valid_stage and (any(row in lessons for row in learned) or any(task_open(root, task) for task in tasks)):
            continue
        failures += 1
        print(f'ERROR receipts release-learning: unlearned {filename}:{number}: {line}', file=sys.stderr)
        # The cause and row must be supplied deliberately. Shell parameter
        # checks prevent this printed command from learning placeholder prose.
        symptom = re.sub(r'\s+(?:learn|task)=\S+', '', line).strip()
        command = ('scripts/release-gate.sh --learn ' + shlex.quote(symptom)
                   + ' "${RELEASE_FAILURE_CAUSE:?set diagnosed root cause}"'
                   + ' "${RELEASE_FAILURE_CHECK:?set executable gate row id}"'
                   + ' --run-dir ' + shlex.quote(str(run_dir))
                   + ' --evidence ' + shlex.quote(f'{filename}:{number}'))
        print(f'  → {command}', file=sys.stderr)
    if failures:
        return 1
    print('receipts release-learning: every rescue maps to a learned gate row or an open task')
    return 0


def map_rows(root, run_dir, check_id, references, validate_only=False):
    if not validate_only and check_id not in executable_lessons(root):
        raise ValueError(f'{check_id} has no failure-log entry with an executable gate row')
    changes = {}
    for reference in references:
        filename, number = reference.rsplit(':', 1)
        if filename not in FILES or not number.isdigit() or int(number) < 1:
            raise ValueError(f'invalid evidence reference: {reference}')
        path = run_dir / filename
        rows = changes.setdefault(path, path.read_text().splitlines())
        index = int(number) - 1
        if index >= len(rows) or not rows[index].strip() or rows[index].lstrip().startswith('#'):
            raise ValueError(f'missing evidence row: {reference}')
        rows[index] = re.sub(r'\s+learn=\S+', '', rows[index]) + f' learn={check_id}'
    if validate_only:
        return
    # Validate all requested rows before writing any annotation.
    for path, rows in changes.items():
        temporary = path.with_name(path.name + f'.learning-{os.getpid()}')
        temporary.write_text('\n'.join(rows) + '\n')
        temporary.replace(path)


def warn_tooling(root):
    try:
        main = git(root, 'rev-parse', '--verify', 'origin/main')
    except subprocess.CalledProcessError:
        main = git(root, 'rev-parse', '--verify', 'main')
    refs = git(root, 'for-each-ref', f'--no-merged={main}', '--format=%(refname) %(objectname)',
               'refs/heads/epic/', 'refs/heads/factory/',
               'refs/remotes/origin/epic/', 'refs/remotes/origin/factory/').splitlines()
    seen = set()
    for reference in refs:
        ref, sha = reference.split()
        if sha in seen:
            continue
        seen.add(sha)
        try:
            introduced = git(root, 'diff', '--name-only', f'{main}...{sha}', '--',
                             'scripts/release*.sh', 'scripts/release*.py', 'scripts/release-train.d',
                             'cas-cli/src/builtins/skills/cas-cut-release').splitlines()
            changed = set(git(root, 'diff', '--name-only', main, sha, '--', *introduced).splitlines()) if introduced else set()
        except subprocess.CalledProcessError:
            print(f'preflight warning: unable to inspect unreleased release tooling on {ref}')
            continue
        paths = [path for path in introduced if path in changed]
        if paths:
            print(f'preflight warning: unreleased release tooling on {ref} @{sha[:12]} is off main: {", ".join(paths)}')


def main():
    mode, directory, *args = sys.argv[1:]
    root = Path(directory).resolve()
    try:
        if mode == '--check':
            return check(root, Path(args[0]))
        if mode in ('--map', '--validate-map'):
            map_rows(root, Path(args[0]), args[1], args[2:], mode == '--validate-map')
            return 0
        if mode == '--warn-tooling':
            warn_tooling(root)
            return 0
        raise ValueError(f'unknown mode: {mode}')
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f'ERROR receipts release-learning: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
