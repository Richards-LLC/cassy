#!/usr/bin/env python3
"""Require explicit, verifiable release-rescue lessons before receipts finish."""
import os
import json
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
    return subprocess.check_output(['git', '-C', str(root), *args], text=True, stderr=subprocess.PIPE).strip()


def executable_lessons(root):
    try:
        source = (root / 'scripts/release-gate.sh').read_text()
        registered = set(re.search(r'gate_check_ids=\((.*?)\)', source, re.S)[1].split())
        learned = set(re.findall(r'\*\*([a-z0-9-]+)\*\*', (root / LOG).read_text()))
        return registered & learned
    except (OSError, TypeError):
        return set()


def task_database(root):
    database = os.environ.get('CAS_RELEASE_LEARNING_TASK_DB')
    if not database:
        common = Path(git(root, 'rev-parse', '--path-format=absolute', '--git-common-dir'))
        database = common.parent / '.cas/cas.db'
    return Path(database).resolve().as_uri() + '?mode=ro'


def task_open(root, identifier):
    try:
        with sqlite3.connect(task_database(root), uri=True) as connection:
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


def live_task_refs(root):
    """Return verified nonterminal task IDs and delivery branches, or unknown.

    Factory tasks often have no tasks.branch: the parked delivery carries its
    own branch. Branch names containing a task ID also cover in-progress work.
    """
    try:
        with sqlite3.connect(task_database(root), uri=True) as connection:
            rows = connection.execute(
                "SELECT id, branch, deliverables FROM tasks WHERE status IN "
                "('open', 'in_progress', 'blocked', 'awaiting_merge')").fetchall()
        identifiers = {row[0] for row in rows}
        branches = set()
        for _, branch, delivery in rows:
            if branch:
                branches.add(branch)
            parked = json.loads(delivery or '{}').get('parked_branch')
            if parked:
                branches.add(parked)
        return identifiers, branches
    except (sqlite3.Error, OSError, ValueError):
        return None


def branch_name(ref):
    for prefix in ('refs/heads/', 'refs/remotes/origin/'):
        if ref.startswith(prefix):
            return ref[len(prefix):]
    return ref


def has_live_task(ref, tasks):
    if tasks is None:
        return False
    identifiers, branches = tasks
    return (branch_name(ref) in branches
            or any(task in identifiers for task in re.findall(r'cas-[a-z0-9]+', branch_name(ref))))


def ancestor(root, older, newer):
    result = subprocess.run(['git', '-C', str(root), 'merge-base', '--is-ancestor', older, newer],
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode not in (0, 1):
        raise subprocess.CalledProcessError(result.returncode, result.args)
    return result.returncode == 0


def warn_tooling(root):
    try:
        main = git(root, 'rev-parse', '--verify', 'origin/main')
    except subprocess.CalledProcessError:
        main = git(root, 'rev-parse', '--verify', 'main')
    refs = [line.split() for line in git(
        root, 'for-each-ref', '--format=%(refname) %(objectname) %(committerdate:unix)',
        'refs/heads/epic/', 'refs/heads/factory/',
        'refs/remotes/origin/epic/', 'refs/remotes/origin/factory/').splitlines()]
    tasks = live_task_refs(root)
    # Only published, main-reachable version tags establish the age cutoff.
    # Annotated tags use tag creation time; lightweight tags use commit time.
    releases = git(root, 'for-each-ref', f'--merged={main}', '--sort=-creatordate',
                   '--format=%(creatordate:unix)', 'refs/tags/v[0-9]*').splitlines()
    cutoff = int(releases[0]) if releases else 0
    epics = {sha for ref, sha, _ in refs
             if branch_name(ref).startswith('epic/') and has_live_task(ref, tasks)}
    # Prefer the live epic's finding over an equal worker tip, and local refs
    # over equal remote aliases. Prune stale refs before marking tips seen.
    refs.sort(key=lambda row: (not (row[1] in epics and branch_name(row[0]).startswith('epic/')),
                               not row[0].startswith('refs/heads/'), row[0]))
    skipped = dict(merged=0, covered=0, stale=0, duplicate=0)
    seen = set()
    for ref, sha, date in refs:
        if ancestor(root, sha, main):
            skipped['merged'] += 1
            continue
        if tasks is not None and int(date) < cutoff and not has_live_task(ref, tasks):
            skipped['stale'] += 1
            continue
        if any(sha != epic and ancestor(root, sha, epic) for epic in epics):
            skipped['covered'] += 1
            continue
        if sha in seen:
            skipped['duplicate'] += 1
            continue
        seen.add(sha)
        try:
            # Criss-cross histories can have several equally valid bases. Use
            # their path union instead of Git's arbitrary triple-dot choice.
            introduced = set()
            for base in git(root, 'merge-base', '--all', main, sha).splitlines():
                introduced.update(git(root, 'diff', '--name-only', base, sha, '--',
                                      'scripts/release*.sh', 'scripts/release*.py', 'scripts/release-train.d',
                                      'cas-cli/src/builtins/skills/cas-cut-release').splitlines())
            changed = set(git(root, 'diff', '--name-only', main, sha, '--',
                              *sorted(introduced)).splitlines()) if introduced else set()
        except subprocess.CalledProcessError:
            print(f'preflight warning: unable to inspect unreleased release tooling on {ref}')
            continue
        paths = sorted(introduced & changed)
        if paths:
            print(f'preflight warning: unreleased release tooling on {ref} @{sha[:12]} is off main: {", ".join(paths)}')
    state = '; task state unavailable, old refs retained' if tasks is None else ''
    print(f"preflight tooling: skipped {sum(skipped.values())} refs "
          f"({skipped['merged']} merged into main, {skipped['covered']} covered by live epic, "
          f"{skipped['stale']} stale, {skipped['duplicate']} duplicate){state}")


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
