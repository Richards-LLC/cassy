#!/usr/bin/env python3
"""Plan affected journey runs and fold the native Playwright report into schema v1.

The selector is invoked only through its public CLI. Caller-supplied IDs or
spec filters are never accepted as selection evidence.
"""
from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import re
import sqlite3
import subprocess
import sys
import tomllib
from pathlib import Path


def git(repo: Path, *args: str) -> str:
    return subprocess.run(['git', *args], cwd=repo, check=True, capture_output=True,
                          text=True).stdout.strip()


def cas_root(repo: Path) -> Path:
    if os.environ.get('CAS_ROOT'):
        return Path(os.environ['CAS_ROOT']).expanduser().resolve()
    return Path(git(repo, 'rev-parse', '--path-format=absolute', '--git-common-dir')).parent / '.cas'


def task_context(repo: Path, artifacts: Path | None, task_id: str | None) -> tuple[dict, Path]:
    root = cas_root(repo)
    with sqlite3.connect((root / 'cas.db').as_uri() + '?mode=ro', uri=True) as db:
        db.row_factory = sqlite3.Row
        if not task_id and artifacts:
            task_id = next((p.name for p in [artifacts, *artifacts.parents]
                            if re.fullmatch(r'cas-[A-Za-z0-9-]+', p.name)), None)
        if task_id:
            rows = db.execute('SELECT id,deliverables,notes FROM tasks WHERE id=?', (task_id,)).fetchall()
        else:
            name = os.environ.get('CAS_AGENT_NAME', '')
            rows = db.execute("SELECT id,deliverables,notes FROM tasks WHERE assignee=? AND status='in_progress'", (name,)).fetchall()
        if len(rows) != 1:
            raise ValueError('cannot resolve one delivery task; pass its artifact directory or --task <id>, or --affected <base> with an artifact directory')
        return dict(rows[0]), root


def task_base(repo: Path, head: str, task: dict) -> str:
    # A successful close persists the exact attributed base for independent QA,
    # including after the branch has merged (when merge-base would collapse).
    for line in reversed(task['notes'].splitlines()):
        match = re.search(r'JOURNEY_SELECTION: head=([a-f0-9]{40}) base=([a-f0-9]{40})', line)
        if match and match[1] == head:
            return match[2]
    target = (json.loads(task['deliverables']).get('work_target') or {}).get('target_branch')
    if not target:
        raise ValueError('delivery task has no declared target; use --affected <base>')
    refs = [f'origin/{target}', target]
    target_sha = None
    for ref in refs:
        try:
            target_sha = git(repo, 'rev-parse', '--verify', f'{ref}^{{commit}}')
            break
        except subprocess.CalledProcessError:
            pass
    if not target_sha:
        raise ValueError(f'task target {target} is unreadable; fetch it or pass --affected <base>')
    base = git(repo, 'merge-base', target_sha, head)
    # Task-ID subjects separate stacked deliveries on a reused branch. With no
    # attribution hint the wider target diff is conservative, never hand-picked.
    commits = git(repo, 'log', '--reverse', '--format=%H %P%x09%s', f'{base}..{head}')
    for line in commits.splitlines():
        parents, _, subject = line.partition('\t')
        if re.search(rf'(?<![\w-]){re.escape(task["id"])}(?![\w-])', subject):
            return parents.split()[1]
    return base


def default_artifacts(root: Path, task_id: str) -> Path:
    config_path = root / 'config.toml'
    config = tomllib.loads(config_path.read_text()) if config_path.exists() else {}
    base = Path(config.get('factory', {}).get('artifacts_root', '~/.cas/artifacts')).expanduser()
    store = root.resolve()
    label = re.sub(r'[^A-Za-z0-9_-]', '-', store.parent.name[:48])
    return base / f'{label}-{hashlib.sha256(os.fsencode(store)).hexdigest()}' / task_id


def selection(repo: Path, base: str, head: str, full: bool) -> list[dict]:
    git(repo, 'merge-base', '--is-ancestor', base, head)
    paths = git(repo, 'diff', '--name-only', base, head).splitlines()
    # Match the close gate: the selector executable belongs to the reviewed
    # revision, just like its catalog and source graph. A different checkout's
    # older selector may ignore CAS_JOURNEYS_HEAD entirely.
    source = git(repo, 'show', f'{head}:scripts/journeys-for-diff.py')
    command = ['python3', '-', '--all' if full else '--paths']
    if not full:
        command += paths
    env = dict(os.environ, CAS_JOURNEYS_ROOT=str(repo), CAS_JOURNEYS_BASE=base, CAS_JOURNEYS_HEAD=head)
    out = subprocess.run(command, input=source, cwd=repo, env=env, check=True, capture_output=True, text=True)
    rows = json.loads(out.stdout)['journeys']
    ids = [r['id'] for r in rows]
    if len(ids) != len(set(ids)) or any(not re.fullmatch(r'[A-Z]+-J\d+', i) for i in ids):
        raise ValueError('selector returned duplicate or malformed IDs')
    return sorted(rows, key=lambda r: r['id'])


def plan(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    head = git(repo, 'rev-parse', 'HEAD^{commit}')
    artifacts = args.artifacts.expanduser().resolve() if args.artifacts else None
    if args.full and os.environ.get('CAS_AGENT_ROLE', '').lower() not in ('', 'supervisor'):
        raise ValueError('full browser suite is supervisor-only; run scripts/journey-eval.sh <artifact-dir> for affected journeys')
    task = root = None
    if artifacts is None or (not args.full and not args.affected):
        task, root = task_context(repo, artifacts, args.task)
    if artifacts is None:
        artifacts = default_artifacts(root, task['id'])
    base = git(repo, 'rev-parse', '--verify', f'{args.affected}^{{commit}}') if args.affected else (head if args.full else task_base(repo, head, task))
    rows = selection(repo, base, head, args.full)
    if args.full and not rows:
        raise ValueError('full catalog selection is empty')
    # Only this producer constructs the native filter; matches supplemental
    # specs/parts by actual title ID, not solely the catalog main suite path.
    ids = [r['id'] for r in rows]
    grep = r'\b(?:' + '|'.join(re.escape(i) for i in ids) + r')\b' if ids else None
    value = {'schema': 1, 'repo': str(repo), 'artifacts': str(artifacts), 'base_sha': base,
             'head_sha': head, 'scope': 'full' if args.full else 'affected',
             'selection_ids': ids, 'journeys': rows, 'workers': args.workers, 'grep': grep}
    print(json.dumps(value))
    return 0


def native_tests(suites: list[dict]):
    for suite in suites:
        yield from native_tests(suite.get('suites', []))
        for spec in suite.get('specs', []):
            for test in spec.get('tests', []):
                yield spec['title'], test


def fold(plan: dict, report: dict | None, suite_exit: int, version: str) -> tuple[dict, int]:
    ids = plan['selection_ids']
    rows = {i: {'id': i, 'status': 'PASS', 'passed': 0, 'failed': 0, 'skipped': 0} for i in ids}
    errors = []
    if not version.strip():
        errors.append('native tool version absent')
    if ids and report is None:
        errors.append('native Playwright report absent; execution is unproven')
    if report:
        for title, test in native_tests(report.get('suites', [])):
            named = re.findall(r'\b[A-Z]+-J\d+\b', title)
            if len(set(named)) != 1 or named[0] not in rows:
                errors.append(f'unmapped or unselected native test: {title}')
                continue
            row = rows[named[0]]
            attempts = test.get('results', [])
            # Preserve earlier failures even when retries later pass. A control
            # rerun is separate evidence, never replacement of the failed run.
            row['passed'] += sum(r.get('status') == 'passed' for r in attempts)
            row['failed'] += sum(r.get('status') in ('failed', 'timedOut', 'interrupted') for r in attempts)
            row['skipped'] += sum(r.get('status') == 'skipped' for r in attempts)
            if not attempts or test.get('expectedStatus', 'passed') != 'passed' or test.get('status') not in ('expected',):
                row['status'] = 'FAIL'
        if report.get('errors'):
            errors.append('native runner reported errors')
    for row in rows.values():
        if not row['passed'] or row['failed'] or row['skipped']:
            row['status'] = 'FAIL'
    ci_url = os.environ.get('GITHUB_SERVER_URL', 'https://github.com') + '/' + os.environ.get('GITHUB_REPOSITORY', '') + '/actions/runs/' + os.environ.get('GITHUB_RUN_ID', '')
    ci = bool(os.environ.get('GITHUB_ACTIONS') and os.environ.get('GITHUB_REPOSITORY') and os.environ.get('GITHUB_RUN_ID'))
    receipt = {'schema': 1, 'producer': 'journey-eval', 'kind': 'ci' if ci else 'local',
               'scope': plan['scope'], 'base_sha': plan['base_sha'], 'head_sha': plan['head_sha'],
               'selection_ids': ids, 'results': list(rows.values()), 'tool_version': version,
               'suite_exit': suite_exit, 'created_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
               'native_report': 'playwright/report.json' if ids else None, 'errors': errors}
    if ci:
        receipt['ci_run_url'] = ci_url
    okay = suite_exit == 0 and not errors and all(r['status'] == 'PASS' for r in rows.values())
    return receipt, 0 if okay else (suite_exit or 1)


def write(args: argparse.Namespace) -> int:
    run = json.loads(args.plan.read_text())
    repo = Path(run['repo'])
    if git(repo, 'rev-parse', 'HEAD^{commit}') != run['head_sha']:
        raise ValueError('checkout advanced during the run; retain execution proof and rebind only unchanged documentation inputs')
    actual = [r['id'] for r in selection(repo, run['base_sha'], run['head_sha'], run['scope'] == 'full')]
    if actual != run['selection_ids']:
        raise ValueError('plan selection differs from journeys-for-diff; caller IDs cannot substitute for affected proof')
    report = json.loads(args.report.read_text()) if args.report.is_file() else None
    receipt, status = fold(run, report, args.suite_exit, args.tool_version)
    args.output.write_text(json.dumps(receipt, indent=2) + '\n')
    print(f"journey receipt: {args.output} ({run['scope']}; {len(run['selection_ids'])} IDs; native exit {args.suite_exit})")
    return status


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='action', required=True)
    p = sub.add_parser('plan')
    p.add_argument('artifacts', nargs='?', type=Path)
    p.add_argument('--repo', type=Path, default=Path(__file__).resolve().parent.parent)
    p.add_argument('--task')
    mode = p.add_mutually_exclusive_group()
    mode.add_argument('--affected')
    mode.add_argument('--full', action='store_true')
    p.add_argument('--workers', type=int, choices=range(1, 5), default=4)
    p.set_defaults(handler=plan)
    p = sub.add_parser('write')
    p.add_argument('--plan', required=True, type=Path)
    p.add_argument('--report', required=True, type=Path)
    p.add_argument('--suite-exit', required=True, type=int)
    p.add_argument('--tool-version', required=True)
    p.add_argument('--output', required=True, type=Path)
    p.set_defaults(handler=write)
    args = parser.parse_args()
    try:
        return args.handler(args)
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError, sqlite3.Error) as e:
        print(f'journey-receipt: {e}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
