#!/usr/bin/env python3
"""Receipts for the existing Rust suite, and read-only consumption of those receipts.

This never launches tests. CI owns execution; the daemon and release command
consume its exact-head artifact instead of running another local suite.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
import xml.etree.ElementTree as ET
import zipfile

ARTIFACT = 'train-rust-suite'
PREFIX = 'epic/release-train-'
MANIFESTS = {'cas-cli/Cargo.toml': 'cas', **{f'crates/{name}/Cargo.toml': name for name in
             ('cas-types', 'cas-search', 'cas-store', 'cas-core', 'cas-mcp')}}


def command(args, cwd=None, output=None):
    result = subprocess.run(args, cwd=cwd, stdout=output or subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=30, text=output is None)
    if result.returncode:
        raise ValueError(f'{args[0]} failed ({result.returncode}): {result.stderr}')
    return result.stdout.strip() if output is None else None


def git(repo, *args):
    return command(['git', *args], cwd=repo)


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode='w', dir=path.parent, delete=False) as stream:
        json.dump(value, stream, indent=2)
        stream.write('\n')
        temporary = stream.name
    os.replace(temporary, path)


def prepared(repo, head):
    """Recognize only this command's version-only commit, including its assets.

    The general diff classifier deliberately allows only manifest/lock bumps;
    real release preparation also writes notes, changelog and generated dist.
    Keep this exception local to train receipt reuse, never queue validation.
    """
    body = git(repo, 'log', '-1', '--format=%B', head)
    source = re.search(r'^Train-CI-Head: ([0-9a-f]{40})$', body, re.M)
    version = re.search(r'^Release-Version: (\d+\.\d+\.\d+)$', body, re.M)
    if not source or not version or body.splitlines()[0] != f'release: v{version[1]}':
        return None
    parent = git(repo, 'rev-parse', f'{head}^1')
    if subprocess.run(['git', 'merge-base', '--is-ancestor', source[1], parent], cwd=repo).returncode:
        return None
    paths = git(repo, 'diff', '--name-only', parent, head).splitlines()
    for path in paths:
        if path in MANIFESTS:
            before = tomllib.loads(git(repo, 'show', f'{parent}:{path}'))
            after = tomllib.loads(git(repo, 'show', f'{head}:{path}'))
            before['package']['version'] = version[1]
            if before != after:
                return None
        elif path == 'Cargo.lock':
            before = tomllib.loads(git(repo, 'show', f'{parent}:Cargo.lock'))
            after = tomllib.loads(git(repo, 'show', f'{head}:Cargo.lock'))
            for package in before['package']:
                if package['name'] in MANIFESTS.values():
                    package['version'] = version[1]
            if before != after:
                return None
        elif path not in ('CHANGELOG.md', '.claude/CODEMAP.md') and not path.startswith(
                ('changelog.d/', 'docs/release-notes/', 'hub-web/dist/')):
            return None
    return source[1]


def identity():
    return {'schema': 1, 'head': os.environ['GITHUB_SHA'],
            'branch': os.environ['GITHUB_REF_NAME'], 'run_id': os.environ['GITHUB_RUN_ID'],
            'run_attempt': int(os.environ.get('GITHUB_RUN_ATTEMPT', '1'))}


def shard(args):
    result = {**identity(), 'shard': args.shard, 'status': args.status, 'tests': 0, 'failures': []}
    args.out.mkdir(parents=True, exist_ok=True)
    if args.log.is_file():
        shutil.copyfile(args.log, args.out/'native.log')
    else:
        result['status'] = 1
        result['failures'].append('suite did not produce its native log')
    if args.junit.is_file():
        shutil.copyfile(args.junit, args.out/'junit.xml')
        for suite in ET.parse(args.junit).getroot().iter('testsuite'):
            for test in suite.findall('testcase'):
                if test.find('skipped') is not None:
                    continue
                result['tests'] += 1
                if test.find('failure') is not None or test.find('error') is not None:
                    result['failures'].append(f'{suite.attrib["name"]}::{test.attrib["name"]}')
    if not result['tests'] or result['failures']:
        result['status'] = 1
    write(args.out/'shard.json', result)


def collect(args):
    receipt = {**identity(), 'parent': git(args.repo, 'rev-parse', 'HEAD^1'),
               'before': os.environ.get('GITHUB_EVENT_BEFORE', ''),
               'run_url': f'https://github.com/{os.environ["GITHUB_REPOSITORY"]}/actions/runs/{os.environ["GITHUB_RUN_ID"]}',
               'artifact': ARTIFACT, 'outcome': 'PASS', 'failures': [], 'tests': 0}
    slots = set()
    for path in sorted(args.evidence.rglob('shard.json')):
        row = json.loads(path.read_text())
        slot = row.get('shard')
        if any(row.get(key) != receipt[key] for key in identity()) or slot in slots:
            raise ValueError('duplicate or foreign suite shard receipt')
        slots.add(slot)
        receipt['tests'] += row['tests']
        receipt['failures'].extend(row['failures'])
        if row['status'] != 0 or not (path.parent/'native.log').is_file():
            receipt['outcome'] = 'FAIL'
    if slots != {'1/3', '2/3', '3/3'} or args.build_status != 'success' or args.shards_status != 'success':
        receipt['outcome'] = 'FAIL'
    if not receipt['tests']:
        receipt['outcome'] = 'FAIL'
    if receipt['outcome'] == 'FAIL' and not receipt['failures']:
        receipt['failures'].append('suite build, shard execution or report collection incomplete')
    write(args.evidence/'receipt.json', receipt)
    write(args.evidence/'failures.json', receipt['failures'])
    print(json.dumps(receipt))


def observe(args):
    branch = args.branch or git(args.repo, 'branch', '--show-current')
    if not branch.startswith(PREFIX):
        raise ValueError('train CI receipts apply only to release-train branches')
    head = args.head or git(args.repo, 'rev-parse', branch)
    out = args.out or Path(git(args.repo, 'rev-parse', '--git-path', 'cas/train-ci'))/hashlib.sha256(branch.encode()).hexdigest()
    if not out.is_absolute():
        out = args.repo/out
    out.mkdir(parents=True, exist_ok=True)
    state_path = out/'state.json'
    previous = json.loads(state_path.read_text()) if state_path.exists() else {}
    gh = args.gh
    slug = command([gh, 'repo', 'view', '--json', 'nameWithOwner', '--jq', '.nameWithOwner'], args.repo)

    def api(endpoint, *fields):
        return json.loads(command([gh, 'api', '-X', 'GET', f'repos/{slug}/{endpoint}', *fields], args.repo))

    runs = api('actions/workflows/ci.yml/runs', '-f', f'branch={branch}', '-f', 'event=push', '-f', 'per_page=100')['workflow_runs']
    candidates = [run for run in runs if run['head_sha'] == head and run['head_branch'] == branch and run['event'] == 'push']
    run = max(candidates, key=lambda item: (item['id'], item.get('run_attempt', 1)), default=None)
    state = {**previous, 'branch': branch, 'head': head, 'outcome': 'PENDING'}
    if not run or run['status'] != 'completed' or run['conclusion'] in ('cancelled', 'skipped'):
        write(state_path, state)
        return state
    run_id, attempt = str(run['id']), run.get('run_attempt', 1)
    destination = out/f'{run_id}-{attempt}'
    receipt_path = destination/'receipt.json'
    if not receipt_path.exists():
        # Download by run, never by latest branch artifact: a later push cannot
        # substitute its result for the commit the caller is about to cut.
        with tempfile.TemporaryDirectory(dir=out) as temporary:
            pending = Path(temporary)/'evidence'
            pending.mkdir()
            command([gh, 'run', 'download', run_id, '--name', ARTIFACT, '--dir', str(pending)], args.repo)
            with (pending/'complete-workflow-logs.zip').open('wb') as log:
                command([gh, 'api', f'repos/{slug}/actions/runs/{run_id}/logs'], args.repo, log)
            pending.rename(destination)
    if not (destination/'complete-workflow-logs.zip').is_file():
        raise ValueError('complete workflow log archive is missing')
    with zipfile.ZipFile(destination/'complete-workflow-logs.zip') as logs:
        if not logs.namelist() or logs.testzip() is not None:
            raise ValueError('complete workflow log archive is empty or corrupt')
    receipt = json.loads(receipt_path.read_text())
    expected = {'schema': 1, 'head': head, 'branch': branch, 'run_id': run_id, 'run_attempt': attempt}
    if any(receipt.get(key) != value for key, value in expected.items()):
        raise ValueError('suite artifact does not match the evaluated GitHub run and head')
    if receipt.get('outcome') not in ('PASS', 'FAIL'):
        raise ValueError('suite receipt has no conclusive result')
    jobs = api(f'actions/runs/{run_id}/jobs', '-f', 'filter=latest', '-f', 'per_page=100')['jobs']
    fanin = [job for job in jobs if job['name'] == 'Fast Validation — full suite']
    if len(fanin) != 1 or (receipt['outcome'] == 'PASS' and fanin[0]['conclusion'] != 'success'):
        raise ValueError('suite artifact lacks its successful GitHub fan-in job')
    state = {**state, **receipt, 'receipt': str(receipt_path),
             'complete_log': str(destination/'complete-workflow-logs.zip')}
    if receipt['outcome'] == 'PASS':
        state['last_green'] = head
        state.pop('red_since', None)
        state.pop('red_head', None)
    else:
        state['red_since'] = previous.get('red_since') or head
        state['red_head'] = head
        green = previous.get('last_green')
        state['introducing_merge'] = head if green == receipt['parent'] else None
        state['merge_range'] = []
        if green and subprocess.run(['git', 'merge-base', '--is-ancestor', green, head], cwd=args.repo).returncode == 0:
            for line in git(args.repo, 'log', '--first-parent', '--format=%H %s', f'{green}..{head}').splitlines():
                sha, title = line.split(' ', 1)
                state['merge_range'].append({'sha': sha, 'title': title,
                                             'tasks': re.findall(r'cas-[a-z0-9]+', title)})
        state['last_green'] = green
    write(state_path, state)
    return state


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    preparation = sub.add_parser('prepared')
    preparation.add_argument('--repo', type=Path, default=Path.cwd())
    preparation.add_argument('--head', default='HEAD')
    native = sub.add_parser('shard')
    native.add_argument('--log', type=Path, required=True)
    native.add_argument('--junit', type=Path, required=True)
    native.add_argument('--status', type=int, required=True)
    native.add_argument('--shard', required=True)
    native.add_argument('--out', type=Path, required=True)
    fanin = sub.add_parser('collect')
    fanin.add_argument('--repo', type=Path, default=Path.cwd())
    fanin.add_argument('--evidence', type=Path, required=True)
    fanin.add_argument('--build-status', required=True)
    fanin.add_argument('--shards-status', required=True)
    for name in ('observe', 'check'):
        read = sub.add_parser(name)
        read.add_argument('--repo', type=Path, default=Path.cwd())
        read.add_argument('--branch')
        read.add_argument('--head')
        read.add_argument('--out', type=Path)
        read.add_argument('--gh', default=os.environ.get('GH_BIN', 'gh'))
    args = parser.parse_args()
    if args.command == 'prepared':
        source = prepared(args.repo, args.head)
        if not source:
            return 1
        print(source)
    elif args.command == 'shard':
        shard(args)
    elif args.command == 'collect':
        collect(args)
    else:
        state = observe(args)
        print(json.dumps(state))
        if args.command == 'check' and state['outcome'] != 'PASS':
            raise ValueError(f'train tip {state["head"]} has no green full-suite receipt ({state["outcome"]}); wait for CI or repair the train')


if __name__ == '__main__':
    try:
        sys.exit(main() or 0)
    except (ValueError, OSError, KeyError, subprocess.SubprocessError, ET.ParseError, zipfile.BadZipFile) as error:
        print(f'train CI: {error}', file=sys.stderr)
        sys.exit(1)
