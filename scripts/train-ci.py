#!/usr/bin/env python3
# gate[C-35 release-script]: Train full-set receipt precondition (train-ci.py check)
"""Whole-set CI receipts, release admission and checked queue reuse.

This never launches tests. CI owns execution; the daemon and release command
consume verified artifacts instead of running another local suite.
"""
import argparse
from datetime import datetime, timezone
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

ARTIFACT = 'train-full-gate'
WORKFLOW = 'full-validation.yml'
REQUIRED_JOBS = ('fast-validation-preflight', 'fast-validation-ci-tiers', 'fast-validation-web',
                 'fast-validation-suite', 'fast-validation-docs', 'queue-flake-publication')
PREFIX = 'epic/release-train-'
# cas-0fc56: Cassy dispatches the one train run at a release freeze; merges start none.
TRAIN_EVENT = 'workflow_dispatch'
MANIFESTS = {'cas-cli/Cargo.toml': 'cas', **{f'crates/{name}/Cargo.toml': name for name in
             ('cas-types', 'cas-search', 'cas-store', 'cas-core', 'cas-mcp')}}


# Shared by release preparation, train admission and queue plumbing checks.
def release_asset(path):
    return path in ('CHANGELOG.md', '.claude/CODEMAP.md', 'docs/PRODUCT_OVERVIEW.md',
                    'docs/qa/journeys.yaml', 'cas-cli/src/builtins/reference-history.json') or path.startswith(
        ('changelog.d/', 'docs/release-notes/', 'docs/release-reports/',
         'docs/qa/bug-families/', 'docs/product-overview.d/', 'hub-web/dist/'))


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
    The queue additionally validates the complete source-to-release plumbing diff.
    """
    body = git(repo, 'log', '-1', '--format=%B', head)
    source = re.search(r'^Train-CI-Head: ([0-9a-f]{40})$', body, re.M)
    version = re.search(r'^Release-Version: (\d+\.\d+\.\d+)$', body, re.M)
    if not source or not version or body.splitlines()[0] != f'release: v{version[1]}':
        return None
    parent = git(repo, 'rev-parse', f'{head}^1')
    if source[1] != parent:
        return None
    paths = git(repo, 'diff', '--no-renames', '--name-only', parent, head).splitlines()
    if 'cas-cli/Cargo.toml' not in paths:
        return None
    for path in paths:
        if path in MANIFESTS:
            before = tomllib.loads(git(repo, 'show', f'{parent}:{path}'))
            after = tomllib.loads(git(repo, 'show', f'{head}:{path}'))
            if path == 'cas-cli/Cargo.toml' and before['package']['version'] == version[1]:
                return None
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
        elif not release_asset(path):
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
               'artifact': 'train-rust-suite', 'outcome': 'PASS', 'failures': [], 'tests': 0}
    slots = set()
    for path in sorted(args.evidence.rglob('shard.json')):
        row = json.loads(path.read_text())
        slot = row.get('shard')
        attempt = row.get('run_attempt')
        if (any(row.get(key) != receipt[key] for key in identity() if key != 'run_attempt')
                or type(attempt) is not int or not 1 <= attempt <= receipt['run_attempt'] or slot in slots):
            raise ValueError('duplicate or foreign suite shard receipt')
        if attempt < receipt['run_attempt'] and (row['status'] != 0 or row['failures']
                or not row['tests'] or not (path.parent/'native.log').is_file()):
            raise ValueError('earlier-attempt suite shard did not succeed')
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


def elapsed(started, ended=None):
    start = datetime.fromisoformat(started.replace('Z', '+00:00'))
    stop = datetime.fromisoformat(ended.replace('Z', '+00:00')) if ended else datetime.now(timezone.utc)
    seconds = (stop - start).total_seconds()
    if seconds < 0:
        raise ValueError('workflow completion predates its start')
    return seconds


def job_identity(args):
    # Runtime selection, matrix and toolchain inputs matter even on one tree.
    inputs=json.loads(args.inputs)
    workflow=git(args.repo,'ls-tree','-r','HEAD','--','.github','scripts/train-ci.py','scripts/ci-queue-flakes.py')
    digest=hashlib.sha256(json.dumps({'workflow':workflow,'inputs':inputs,
        'os':os.environ.get('RUNNER_OS'),'arch':os.environ.get('RUNNER_ARCH')},sort_keys=True).encode()).hexdigest()
    tree=git(args.repo,'rev-parse','HEAD^{tree}')
    key=hashlib.sha256(json.dumps([tree,args.job,digest]).encode()).hexdigest()
    return {'schema':2,'scope':'job','tree':tree,'job':args.job,'job_name':args.job_name,
        'inputs_hash':digest,'key':key,'artifact':'full-gate-job-'+key}


def job_save(args):
    expected=job_identity(args)
    previous=args.out/'receipt.json'
    reused=json.loads(previous.read_text()) if previous.is_file() else {}
    if any(reused.get(key)!=value for key,value in expected.items()): reused={}
    result={**identity(),**expected,'outcome':'PASS' if args.status=='success' else 'FAIL',
        'producer_run':reused.get('producer_run',os.environ['GITHUB_RUN_ID'])}
    if reused.get('reused_from'):
        result['reused_from']=reused['reused_from']
    # Keep the native train reports with their job, so a cache hit can feed
    # the existing full-suite collector without inventing a second approval.
    reports=args.repo/'target/train-suite'
    if reports.is_dir():
        shutil.copytree(reports,args.out/'train-suite',dirs_exist_ok=True)
    write(previous,result)
    return result


def job_check(args):
    expected=job_identity(args)
    result={**expected,'reused':False,'producer_run':os.environ['GITHUB_RUN_ID']}
    (args.out/'receipt.json').unlink(missing_ok=True)
    slug=os.environ.get('GITHUB_REPOSITORY') or command([args.gh,'repo','view','--json','nameWithOwner','--jq','.nameWithOwner'],args.repo)
    def api(endpoint):
        return json.loads(command([args.gh,'api',f'repos/{slug}/{endpoint}'],args.repo))
    try:
        artifacts=api('actions/artifacts?name='+expected['artifact']+'&per_page=100')['artifacts']
        for artifact in artifacts:
            if artifact['name']!=expected['artifact'] or artifact.get('expired'):
                continue
            run_id=str(artifact['workflow_run']['id'])
            if run_id==os.environ['GITHUB_RUN_ID']:
                continue
            run=api('actions/runs/'+run_id)
            if run.get('status')!='completed' or run.get('path') not in ('.github/workflows/'+WORKFLOW,WORKFLOW):
                continue
            if run.get('event') not in ('merge_group',TRAIN_EVENT):
                continue
            with tempfile.TemporaryDirectory() as temporary:
                command([args.gh,'run','download',run_id,'--name',expected['artifact'],'--dir',temporary],args.repo)
                record=json.loads((Path(temporary)/'receipt.json').read_text())
                reports={str(path.relative_to(Path(temporary)/'train-suite')):path.read_bytes()
                    for path in (Path(temporary)/'train-suite').rglob('*') if path.is_file()}
            if any(record.get(key)!=value for key,value in expected.items()) or record.get('outcome')!='PASS':
                continue
            if record.get('run_id')!=run_id or record.get('head')!=run.get('head_sha'):
                continue
            jobs=api('actions/runs/'+run_id+'/jobs?filter=latest&per_page=100')['jobs']
            proven=[job for job in jobs if job['name'].split(' / ')[-1]==args.job_name and job.get('conclusion')=='success']
            if len(proven)!=1:
                continue
            producer=record.get('producer_run',run_id)
            if args.requires_artifact:
                outputs=api('actions/runs/'+str(producer)+'/artifacts')['artifacts']
                if not any(item['name']==args.requires_artifact and not item.get('expired') for item in outputs):
                    continue
            # Legacy recovered failures are not successful reusable proof.
            if record.get('recovery'):
                continue
            native=[json.loads(body) for name,body in reports.items() if name.endswith('shard.json')]
            if any(row.get('head')!=record['head'] or row.get('run_id')!=run_id or
                   row.get('status')!=0 or not row.get('tests') or row.get('failures') for row in native):
                continue
            for name,body in reports.items():
                target=args.repo/'target/train-suite'/name
                target.parent.mkdir(parents=True,exist_ok=True)
                if name.endswith('shard.json'):
                    row=json.loads(body)
                    write(target,{**row,**identity(),'reused_from':run_id})
                else:
                    target.write_bytes(body)
            result.update(reused=True,reused_from=run_id,producer_run=record.get('producer_run',run_id))
            write(args.out/'receipt.json',{**record,**identity(),'reused_from':run_id})
            break
    except (ValueError,OSError,KeyError,subprocess.SubprocessError) as error:
        # Cache outage or malformed evidence means execution, never approval.
        print('Job receipt unavailable; executing: '+str(error),file=sys.stderr)
    return result


def full_collect(args):
    jobs = json.loads(args.jobs_json)
    required = list(REQUIRED_JOBS)
    outputs = jobs.get('ci-diff', {}).get('outputs', {})
    if outputs.get('journeys-check-needed') == 'true':
        required.append('fast-validation-journeys')
    if outputs.get('bridge-check-needed') == 'true':
        required.append('fast-validation-slack-bridge')
    failures = [name for name in required if jobs.get(name, {}).get('result') != 'success']
    main = git(args.repo, 'rev-parse', args.main_baseline + '^{commit}')
    head = git(args.repo, 'rev-parse', 'HEAD')
    if subprocess.run(['git', 'merge-base', '--is-ancestor', main, head], cwd=args.repo).returncode:
        failures.append('main baseline is not included in the tested tree')
    job_receipts=[]
    for path in (args.evidence/'jobs').rglob('receipt.json'):
        job=json.loads(path.read_text())
        if job.get('scope')=='job':
            if job.get('tree')!=git(args.repo,'rev-parse','HEAD^{tree}') or job.get('run_id')!=os.environ['GITHUB_RUN_ID']:
                raise ValueError('per-job proof is for another evaluated run/tree')
            job_receipts.append(job)
    receipt = {**identity(), 'schema': 2, 'scope': 'full-set', 'job_receipts':job_receipts,
               'tree': git(args.repo, 'rev-parse', 'HEAD^{tree}'), 'main_baseline': main,
               'parent': git(args.repo, 'rev-parse', 'HEAD^1'),
               'artifact': ARTIFACT, 'run_url': f'https://github.com/{os.environ["GITHUB_REPOSITORY"]}/actions/runs/{os.environ["GITHUB_RUN_ID"]}',
               'outcome': 'FAIL' if failures else 'PASS', 'failures': failures,
               'journeys_required': outputs.get('journeys-check-needed') == 'true',
               'bridge_required': outputs.get('bridge-check-needed') == 'true',
               'required_jobs': required, 'jobs': {name: job.get('result') for name, job in jobs.items()},
               'elapsed_seconds': elapsed(args.started_at)}
    write(args.evidence/'receipt.json', receipt)
    print(json.dumps(receipt))
    if failures:
        raise ValueError('full-set jobs did not all pass: ' + ', '.join(failures))


def validate_full(receipt, tree):
    required = receipt.get('required_jobs', [])
    complete = set(REQUIRED_JOBS)
    if receipt.get('journeys_required'): complete.add('fast-validation-journeys')
    if receipt.get('bridge_required'): complete.add('fast-validation-slack-bridge')
    if (receipt.get('schema') != 2 or receipt.get('scope') != 'full-set'
            or receipt.get('tree') != tree or set(required) != complete
            or not isinstance(receipt.get('journeys_required'), bool)
            or not isinstance(receipt.get('bridge_required'), bool)
            or (receipt.get('outcome') == 'PASS' and any(receipt.get('jobs', {}).get(name) != 'success' for name in required))
            or not isinstance(receipt.get('elapsed_seconds'), (int, float))
            or receipt['elapsed_seconds'] < 0
            or not re.fullmatch(r'[0-9a-f]{40,64}', receipt.get('main_baseline', ''))):
        raise ValueError('missing, partial, legacy or wrong-tree full-set receipt')


def plumbing_only(repo, source, head):
    if subprocess.run(['git', 'merge-base', '--is-ancestor', source, head], cwd=repo,
                      stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode:
        return False
    paths = git(repo, 'diff', '--no-renames', '--name-only', source, head).splitlines()
    version_changed = False
    for path in paths:
        if path in MANIFESTS:
            before = tomllib.loads(git(repo, 'show', f'{source}:{path}'))
            after = tomllib.loads(git(repo, 'show', f'{head}:{path}'))
            version = after['package']['version']
            if not re.fullmatch(r'\d+\.\d+\.\d+', version): return False
            version_changed |= before['package']['version'] != version
            before['package']['version'] = version
            if before != after: return False
        elif path == 'Cargo.lock':
            before = tomllib.loads(git(repo, 'show', f'{source}:{path}'))
            after = tomllib.loads(git(repo, 'show', f'{head}:{path}'))
            for package in before['package']:
                if package['name'] in MANIFESTS.values():
                    manifest = next(key for key, name in MANIFESTS.items() if name == package['name'])
                    package['version'] = tomllib.loads(git(repo, 'show', f'{head}:{manifest}'))['package']['version']
            if before != after: return False
        elif path.startswith('hub-web/dist/'):
            # Only version-generated bundles can join release plumbing. The
            # reuse compile job rebuilds them and checks their committed bytes.
            continue
        elif not release_asset(path):
            return False
    return version_changed or not any(path.startswith('hub-web/dist/') for path in paths)


def reuse(args):
    """Consume the same verified artifact as release admission, never a bare status."""
    if args.adoption_out:
        (args.adoption_out/'receipt.json').unlink(missing_ok=True)
    ref = os.environ.get('GITHUB_REF', '')
    event = os.environ.get('GITHUB_EVENT_NAME', '')
    gh, slug = args.gh, os.environ['GITHUB_REPOSITORY']
    def api(endpoint):
        return json.loads(command([gh, 'api', f'/repos/{slug}/{endpoint}'], args.repo))
    def green_status(source, tree):
        latest = next((row for row in api(f'commits/{source}/statuses?per_page=100')
                       if row.get('context') == 'cas/full-gate'), {})
        return latest.get('state') == 'success' and latest.get('description') == 'PASS tree=' + tree
    head = git(args.repo, 'rev-parse', 'HEAD')
    if event == TRAIN_EVENT and ref.startswith('refs/heads/' + PREFIX):
        branch = ref.removeprefix('refs/heads/')
        pr_head = head
        queue_main = os.environ.get('MAIN_BASELINE') or git(args.repo, 'rev-parse',
            'origin/' + os.environ.get('GITHUB_DEFAULT_BRANCH', 'main'))
    else:
        match = re.fullmatch(r'refs/heads/gh-readonly-queue/[^/]+/pr-([0-9]+)-[0-9a-f]{40}', ref)
        if event != 'merge_group' or not match: return
        pr = api('pulls/'+match[1])
        pr_head = pr['head']['sha']
        queue_main = os.environ.get('QUEUE_BASE_SHA') or pr['base']['sha']
        branch = pr['head']['ref']
    queue_tree = git(args.repo, 'rev-parse', 'HEAD^{tree}')
    # Check the actual synthetic queue tree, including other queued entries.
    # Both the PR and the declared base must belong to this evaluated history.
    for required in (pr_head, queue_main):
        if subprocess.run(['git', 'merge-base', '--is-ancestor', required, head], cwd=args.repo,
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode:
            return
    if not branch.startswith(PREFIX): return
    runs = api(f'actions/workflows/{WORKFLOW}/runs?branch={branch}&event={TRAIN_EVENT}&per_page=100')['workflow_runs']
    candidates = sorted((run for run in runs if run['status'] == 'completed' and run['conclusion'] == 'success'),
                        key=lambda run: (run['id'], run.get('run_attempt', 1)), reverse=True)
    for candidate in candidates:
        source = candidate['head_sha']
        if not plumbing_only(args.repo, source, head): continue
        source_tree = git(args.repo, 'rev-parse', source + '^{tree}')
        if not green_status(source, source_tree): continue
        probe = argparse.Namespace(repo=args.repo, branch=branch, head=source, gh=gh, out=args.out, command='observe', exact=True)
        state = observe(probe)
        if state.get('outcome') != 'PASS' or state.get('main_baseline') != queue_main: continue
        # An adopted run resolves to the original native producer, whose CLI
        # and tree remain the authority even after several plumbing pushes.
        original = source
        source, source_tree = state['head'], state['tree']
        if source != original and not green_status(source, source_tree): continue
        if not plumbing_only(args.repo, source, head): continue
        if event == TRAIN_EVENT and args.adoption_out:
            write(args.adoption_out/'receipt.json', {**identity(), 'schema':2,
                'scope':'full-set-adoption', 'head':head, 'branch':branch, 'tree':queue_tree,
                'artifact':ARTIFACT, 'outcome':'PASS', 'main_baseline':queue_main,
                'source_head':source, 'source_run_id':state['run_id'],
                'source_run_attempt':state['run_attempt']})
        with args.github_output.open('a') as output:
            output.write('run-fast-validation=false\nreuse-source=full-gate\njourneys-reuse=true\n')
            output.write(f'validating-run-id={state["run_id"]}\nprior-run-url={state["run_url"]}\n')
            output.write(f'journeys-prior-url={state["run_url"]}\nreceipt-head={source}\nreceipt-tree={source_tree}\n')
        print(f'Full-set receipt {state["run_id"]} covers queue tree {queue_tree} after release plumbing from {source} (tree {source_tree}); main is unchanged.')
        return


def queue_timing(api, repo, branch, head):
    """Read the completed queue run bound to this release's validated tree."""
    trees = [git(repo, 'rev-parse', head+'^{tree}')]
    # A moved main can make the queue's final tree differ from the train head.
    # GitHub's recorded release PR merge supplies that authoritative final tree.
    pulls = api('pulls?state=all&per_page=100')
    for pr in pulls:
        if pr.get('head', {}).get('ref') == branch and pr.get('head', {}).get('sha') == head and pr.get('merged_at'):
            merge = api('git/commits/'+pr['merge_commit_sha'])
            trees.append(merge['tree']['sha'])
    for tree in dict.fromkeys(trees):
        artifacts = api('actions/artifacts?name=merge-queue-validated-tree-'+tree+'&per_page=100')
        for artifact in sorted(artifacts.get('artifacts', []), key=lambda item: item['id'], reverse=True):
            if artifact.get('expired'): continue
            run_id = artifact['workflow_run']['id']
            run = api(f'actions/runs/{run_id}')
            if run.get('event') != 'merge_group' or run.get('status') != 'completed' or run.get('conclusion') != 'success':
                continue
            committed = api('git/commits/'+run['head_sha'])
            if committed['tree']['sha'] != tree: continue
            jobs = api(f'actions/runs/{run_id}/jobs', '-f', 'filter=latest', '-f', 'per_page=100')['jobs']
            gates = [job for job in jobs if job['name'].split(' / ')[-1] == 'Full-set gate' and job['conclusion'] == 'success']
            if len(gates) == 1:
                return {'queue_seconds': elapsed(run['run_started_at'], gates[0]['completed_at']),
                        'queue_run_id': str(run_id), 'queue_run_attempt': run.get('run_attempt', 1), 'queue_head': run['head_sha'], 'queue_run_url': run['html_url']}
    return {}


def mark_red(repo, state, previous, parent):
    head = state['head']
    state['red_since'] = previous.get('red_since') or head
    state['red_head'] = head
    green = previous.get('last_green')
    state['introducing_merge'] = head if green == parent else None
    state['merge_range'] = []
    baseline = parent
    if green and subprocess.run(['git', 'merge-base', '--is-ancestor', green, head], cwd=repo).returncode == 0:
        baseline = green
    for record in git(repo, 'log', '--first-parent', '--format=%H%x00%s%x00%B%x1e', f'{baseline}..{head}').split('\x1e'):
        if record.strip():
            sha, title, body = record.strip().split('\x00', 2)
            state['merge_range'].append({'sha': sha, 'title': title,
                                         'tasks': sorted(set(re.findall(r'cas-[a-z0-9]+', body)))})
    state['last_green'] = green


def observe(args):
    branch = args.branch or git(args.repo, 'branch', '--show-current')
    if not branch.startswith(PREFIX):
        raise ValueError('train CI receipts apply only to release-train branches')
    requested_head = args.head or git(args.repo, 'rev-parse', branch)
    head = requested_head if getattr(args, 'exact', False) else prepared(args.repo, requested_head) or requested_head
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

    runs = api(f'actions/workflows/{WORKFLOW}/runs', '-f', f'branch={branch}', '-f', f'event={TRAIN_EVENT}', '-f', 'per_page=100')['workflow_runs']
    candidates = [run for run in runs if run['head_branch'] == branch and run['event'] == TRAIN_EVENT]
    # Keep only each head's latest attempt; a failed rerun cannot lend old green proof.
    # cas-bac4: a cancelled or skipped run has no verdict, so it ranks below
    # every other run at its head; a newer cancelled duplicate hid a green.
    latest = {}
    for candidate in sorted(candidates, key=lambda item: (item['conclusion'] not in ('cancelled', 'skipped'),
                                                          item['id'], item.get('run_attempt', 1))):
        latest[candidate['head_sha']] = candidate
    run = latest.get(head)
    if getattr(args, 'command', None) == 'check' and (not run or run['status'] != 'completed'
                                   or run['conclusion'] in ('cancelled', 'skipped')):
        # Walk nearest first, checking each commit so a reverted source edit
        # cannot hide in a net diff. Our validated version preparation may sit
        # between a documentation follow-up and its original green source.
        for ancestor in git(args.repo, 'rev-list', '--first-parent', head).splitlines():
            candidate = latest.get(ancestor)
            if candidate and candidate['status'] == 'completed' and candidate['conclusion'] == 'success':
                head, run = ancestor, candidate
                break
            paths = git(args.repo, 'diff-tree', '--root', '--no-commit-id', '--name-only',
                        '--no-renames', '-r', '-m', '--first-parent', ancestor).splitlines()
            if not all(release_asset(path) for path in paths) and not prepared(args.repo, ancestor):
                break
    state = {**previous, 'branch': branch, 'head': head, 'outcome': 'PENDING', 'pending_run_id': None}
    if not run or run['status'] != 'completed' or run['conclusion'] in ('cancelled', 'skipped'):
        if run and run['status'] != 'completed':
            # cas-bac4: whoever dispatched it, this head's run is under way.
            state['pending_run_id'] = str(run['id'])
        write(state_path, state)
        return state
    run_id, attempt = str(run['id']), run.get('run_attempt', 1)
    def red_without_receipt():
        # A completed failed GitHub run is enough to hold the train, but
        # cannot manufacture a tested full-set receipt or its timings.
        run_url = f'https://github.com/{slug}/actions/runs/{run_id}'
        red = {'branch': branch, 'head': head, 'run_id': run_id, 'run_attempt': attempt,
               'outcome': 'FAIL', 'failure_kind': 'run_failed_before_receipt',
               'failures': ['run failed before receipt'], 'receipt': None,
               'artifact': 'unavailable', 'run_url': run_url, 'complete_log': run_url}
        mark_red(args.repo, red, previous, git(args.repo, 'rev-parse', head + '^1'))
        write(state_path, red)
        return red

    destination = out/f'{run_id}-{attempt}'
    receipt_path = destination/'receipt.json'
    if not receipt_path.exists():
        # Download by run, never by latest branch artifact: a later push cannot
        # substitute its result for the commit the caller is about to cut.
        with tempfile.TemporaryDirectory(dir=out) as temporary:
            pending = Path(temporary)/'evidence'
            pending.mkdir()
            try:
                command([gh, 'run', 'download', run_id, '--name', ARTIFACT, '--dir', str(pending)], args.repo)
            except (ValueError, OSError, subprocess.SubprocessError):
                if run['conclusion'] == 'failure':
                    return red_without_receipt()
                raise
            if not (pending/'receipt.json').is_file() and run['conclusion'] == 'failure':
                return red_without_receipt()
            with (pending/'complete-workflow-logs.zip').open('wb') as log:
                command([gh, 'api', f'repos/{slug}/actions/runs/{run_id}/attempts/{attempt}/logs'], args.repo, log)
            pending.rename(destination)
    if not (destination/'complete-workflow-logs.zip').is_file():
        raise ValueError('complete workflow log archive is missing')
    with zipfile.ZipFile(destination/'complete-workflow-logs.zip') as logs:
        if not logs.namelist() or logs.testzip() is not None:
            raise ValueError('complete workflow log archive is empty or corrupt')
    receipt = json.loads(receipt_path.read_text())
    expected = {'schema': 2, 'head': head, 'branch': branch, 'run_id': run_id, 'run_attempt': attempt}
    if any(receipt.get(key) != value for key, value in expected.items()):
        raise ValueError('suite artifact does not match the evaluated GitHub run and head')
    adopted = receipt.get('scope') == 'full-set-adoption'
    if not adopted:
        validate_full(receipt, git(args.repo, 'rev-parse', head + '^{tree}'))
    if receipt.get('outcome') not in ('PASS', 'FAIL'):
        raise ValueError('suite receipt has no conclusive result')
    jobs = api(f'actions/runs/{run_id}/jobs', '-f', 'filter=latest', '-f', 'per_page=100')['jobs']
    fanin = [job for job in jobs if job['name'].split(' / ')[-1] == 'Full-set gate']
    if len(fanin) != 1 or (receipt['outcome'] == 'PASS' and (fanin[0]['conclusion'] != 'success' or run['conclusion'] != 'success')):
        raise ValueError('full-set artifact lacks its successful completed GitHub fan-in/run')
    if adopted:
        source = receipt.get('source_head', '')
        if (receipt.get('outcome') != 'PASS' or source == head
                or receipt.get('tree') != git(args.repo, 'rev-parse', head+'^{tree}')
                or not plumbing_only(args.repo, source, head)):
            raise ValueError('adoption does not identify a plumbing-only ancestor')
        original = observe(argparse.Namespace(repo=args.repo, branch=branch, head=source,
            gh=gh, out=out, command='observe', exact=True))
        if (original.get('outcome') != 'PASS' or original.get('scope') != 'full-set'
                or original['head'] != source or original['run_id'] != receipt.get('source_run_id')
                or original['run_attempt'] != receipt.get('source_run_attempt')
                or original['main_baseline'] != receipt.get('main_baseline')):
            raise ValueError('adoption does not bind its original successful full-set receipt')
        state = {**original, 'release_head':requested_head, 'adoption_receipt':str(receipt_path),
                 'adoption_head':head, 'adoption_run_id':run_id}
        write(state_path, state)
        return state
    for key in ('adoption_receipt', 'adoption_head', 'adoption_run_id'):
        state.pop(key, None)
    state = {**state, **receipt, 'receipt': str(receipt_path),
             'complete_log': str(destination/'complete-workflow-logs.zip')}
    if receipt['outcome'] == 'PASS':
        state['last_green'] = head
        state.pop('red_since', None)
        state.pop('red_head', None)
        state.pop('failure_kind', None)
    else:
        state.pop('failure_kind', None)
        mark_red(args.repo, state, previous, receipt['parent'])
    if run.get('run_started_at') and fanin[0].get('completed_at'):
        state['elapsed_seconds'] = elapsed(run['run_started_at'], fanin[0]['completed_at'])
    if requested_head != head:
        state['release_head'] = requested_head
        if previous.get('release_head') != requested_head or 'queue_seconds' not in previous:
            try:
                state.update(queue_timing(api, args.repo, branch, requested_head))
            except (ValueError, KeyError, OSError, subprocess.SubprocessError):
                # Timing can retry; it cannot invent a duration or invalidate the train proof.
                pass
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
    full = sub.add_parser('full-collect')
    full.add_argument('--repo', type=Path, default=Path.cwd())
    full.add_argument('--evidence', type=Path, required=True)
    full.add_argument('--jobs-json', required=True)
    full.add_argument('--main-baseline', required=True)
    full.add_argument('--started-at', required=True)
    reuse_parser = sub.add_parser('reuse')
    reuse_parser.add_argument('--repo', type=Path, default=Path.cwd())
    reuse_parser.add_argument('--gh', default=os.environ.get('GH_BIN', 'gh'))
    reuse_parser.add_argument('--out', type=Path)
    reuse_parser.add_argument('--adoption-out', type=Path)
    reuse_parser.add_argument('--github-output', type=Path, required=True)
    for name in ('observe', 'check'):
        read = sub.add_parser(name)
        read.add_argument('--repo', type=Path, default=Path.cwd())
        read.add_argument('--branch')
        read.add_argument('--head')
        read.add_argument('--out', type=Path)
        read.add_argument('--gh', default=os.environ.get('GH_BIN', 'gh'))
    for name in ('job-check','job-save'):
        job=sub.add_parser(name)
        job.add_argument('--repo',type=Path,default=Path.cwd())
        job.add_argument('--job',required=True)
        job.add_argument('--job-name',required=True)
        job.add_argument('--inputs',default='{}')
        job.add_argument('--out',type=Path,required=True)
        job.add_argument('--github-output',type=Path)
        job.add_argument('--gh',default=os.environ.get('GH_BIN','gh'))
        job.add_argument('--requires-artifact')
        if name=='job-save': job.add_argument('--status',required=True)
    args = parser.parse_args()
    if args.command in ('job-check','job-save'):
        result={'job-check':job_check,'job-save':job_save}[args.command](args)
        if getattr(args,'github_output',None):
            with args.github_output.open('a') as stream:
                for key in ('reused','artifact','producer_run'):
                    if key in result: stream.write(f'{key}={str(result[key]).lower()}\n')
        print(json.dumps(result))
    elif args.command == 'prepared':
        source = prepared(args.repo, args.head)
        if not source:
            return 1
        print(source)
    elif args.command == 'shard':
        shard(args)
    elif args.command == 'collect':
        collect(args)
    elif args.command == 'full-collect':
        full_collect(args)
    elif args.command == 'reuse':
        reuse(args)
    else:
        state = observe(args)
        print(json.dumps(state))
        if args.command == 'check' and state['outcome'] != 'PASS':
            if state.get('failure_kind') == 'run_failed_before_receipt':
                raise ValueError(f'train tip {state["head"]}: run failed before receipt ({state["run_url"]}); repair the train')
            raise ValueError(f'train tip {state["head"]} has no green full-set receipt ({state["outcome"]}); wait for CI or repair the train')
        if args.command == 'check' and state.get('main_baseline') != git(args.repo, 'rev-parse', 'origin/' + os.environ.get('GITHUB_DEFAULT_BRANCH', 'main')):
            raise ValueError('train full-set receipt predates the current main baseline')


if __name__ == '__main__':
    try:
        try:
            sys.exit(main() or 0)
        except (ValueError, OSError, KeyError, subprocess.SubprocessError, ET.ParseError, zipfile.BadZipFile) as error:
            print(f'train CI: {error}', file=sys.stderr)
            sys.exit(1)
    except BaseException as error:
        if not isinstance(error, SystemExit) or error.code:
            print("[C-35]", file=sys.stderr)
        raise
