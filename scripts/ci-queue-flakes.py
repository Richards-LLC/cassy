#!/usr/bin/env python3
"""Merge-queue-only failure recovery; preserve both attempts and the tested tree."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import time
import xml.etree.ElementTree as ET
from contextlib import contextmanager
from urllib.request import Request, urlopen
from urllib.error import HTTPError
from urllib.parse import urlencode
from uuid import uuid4

HERE = Path(__file__).resolve().parent
MARKER = re.compile(r'^<!-- cas-queue-flake: (.+?) -->$',re.MULTILINE)


class GitHub:
    """Single-attempt REST transport. Never retry an ambiguous mutation."""
    def __init__(self, repo):
        if not re.fullmatch(r'[\w.-]+/[\w.-]+', repo):
            raise ValueError('an explicit owner/repo issue route is required')
        self.repo = repo
        self.root = os.environ.get('GITHUB_API_URL','https://api.github.com').rstrip('/')
        self.token = os.environ.get('GH_TOKEN') or os.environ.get('GITHUB_TOKEN')
        if not self.token:
            raise ValueError('GH_TOKEN or GITHUB_TOKEN is required for durable flake tracking')

    def api(self, path='', method='GET', data=None):
        request = Request(f'{self.root}/repos/{self.repo}{path}', method=method,
                          data=json.dumps(data).encode() if data is not None else None,
                          headers={'Authorization':f'Bearer {self.token}', 'Accept':'application/vnd.github+json',
                                   'Content-Type':'application/json','X-GitHub-Api-Version':'2022-11-28'})
        with urlopen(request,timeout=30) as response:
            body = response.read()
            return json.loads(body) if body else None

    def issues(self, label=None):
        result = []
        for page in range(1,1001):
            query = {'state':'open','per_page':100,'page':page}
            if label: query['labels'] = label
            batch = self.api('/issues?'+urlencode(query))
            result.extend(i for i in batch if 'pull_request' not in i)
            if len(batch)<100: return result
        raise ValueError('issue pagination exceeded safety bound')

    @contextmanager
    def publication_lock(self):
        # An atomic ref creation serializes issue creation and observation
        # updates across workflow runs. It never changes the evaluated tree.
        # Crash leftovers deliberately fail closed, rather than risking duplicates.
        branch = self.api()['default_branch']
        base = self.api('/git/ref/heads/'+branch)['object']['sha']
        tree = self.api('/git/commits/'+base)['tree']['sha']
        owner = self.api('/git/commits','POST',{'tree':tree,'parents':[base],
                         'message':f'Queue flake publication lock {os.environ["GITHUB_RUN_ID"]} '+str(uuid4())})['sha']
        ref = 'heads/cas-queue-flake-publication-lock'
        try:
            self.api('/git/refs','POST',{'ref':'refs/'+ref,'sha':owner})
        except (HTTPError,OSError):
            # A lost HTTP response can still have committed the ref.
            if self.api('/git/ref/'+ref)['object']['sha'] != owner:
                raise ValueError('flake publication is locked; preserve artifacts and reconcile after the owning job finishes')
        try:
            yield
        finally:
            if self.api('/git/ref/'+ref)['object']['sha'] != owner:
                raise ValueError('flake publication lock ownership changed')
            self.api('/git/refs/'+ref,'DELETE')

    def ensure_labels(self):
        for label,color in [('flaky','fbca04'),('flaky-quarantine','d93f0b'),('priority:p1','b60205')]:
            try: self.api('/labels/'+label)
            except HTTPError as error:
                if error.code != 404: raise
                self.api('/labels','POST',{'name':label,'color':color})


def issue_state(issue):
    matches = MARKER.findall(issue.get('body') or '')
    if len(matches) != 1:
        raise ValueError(f'flake issue #{issue["number"]} needs one machine-readable test id line')
    data = json.loads(matches[0])
    if data.get('schema') != 1 or not isinstance(data.get('runs'),list) or not all(isinstance(r,str) and r.isdigit() for r in data['runs']):
        raise ValueError('invalid flake observation schema')
    identity(data['test'])
    return data


def git(repo, *args):
    return subprocess.check_output(['git', '-C', str(repo), *args], text=True).strip()


def write_json(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2, ensure_ascii=False)+'\n')


def quarantine(api):
    tests = []
    seen = set()
    for issue in api.issues('flaky-quarantine'):
        data = issue_state(issue)
        entry = {**data['test'],'issue':issue['html_url'],'runs':data['runs']}
        key = identity(entry)
        if key in seen or not entry['issue'].startswith(f'https://github.com/{api.repo}/issues/'):
            raise ValueError('quarantine entries need unique test identities and linked fix issues')
        seen.add(key)
        tests.append(entry)
    return tests


def identity(test):
    if test['runner'] == 'rust':
        return json.dumps(['rust', test['binary'], test['name']], ensure_ascii=False)
    if test['runner'] == 'playwright':
        return json.dumps(['playwright', test['project'], test['file'], test['titles']], ensure_ascii=False)
    raise ValueError('unknown quarantine runner')


def rust_filter(tests):
    def escape(s):
        return ''.join('\\'+c if c in '\\),' else c for c in s)
    return ' | '.join(f'(binary_id(={escape(t["binary"])}) & test(={escape(t["name"])}))' for t in tests)


def playwright_list(tests):
    rows = []
    for test in tests:
        parts = [f'[{test["project"]}]', test['file'], *test['titles']]
        if any('\n' in p or '›' in p for p in parts):
            raise ValueError('test identity cannot be represented by Playwright test-list')
        rows.append(' › '.join(parts))
    return '\n'.join(rows)+'\n'


def read_results(engine, report):
    tests = []
    if engine == 'rust':
        for suite in ET.parse(report).getroot().iter('testsuite'):
            for case in suite.findall('testcase'):
                if case.find('skipped') is not None:
                    continue
                failed = case.find('failure') is not None or case.find('error') is not None
                tests.append({'runner':'rust','binary':suite.attrib['name'], 'name':case.attrib['name'],
                              'passed':not failed, 'output':'\n'.join(case.itertext()).strip()[:16000]})
    else:
        data = json.loads(report.read_text())
        if data.get('errors'):
            raise ValueError('Playwright reported an infrastructure/collection error')
        def walk(suites, titles=(), depth=0):
            for suite in suites:
                here = (*titles, suite['title']) if depth else titles
                for spec in suite.get('specs', []):
                    for test in spec['tests']:
                        if test.get('expectedStatus', 'passed') != 'passed':
                            continue
                        results = test.get('results', [])
                        if len(results) != 1 or results[0].get('retry',0) != 0:
                            raise ValueError('expected exactly one native attempt per test')
                        result = results[0]
                        if result['status'] == 'skipped':
                            continue
                        tests.append({'runner':'playwright','project':test['projectName'], 'file':spec['file'],
                                      'titles':[*here, spec['title']], 'passed':result['status']=='passed',
                                      'output':json.dumps(result.get('errors',[]), ensure_ascii=False)[:16000]})
                walk(suite.get('suites', []), here, depth+1)
        walk(data.get('suites',[]))
    if not tests or len({identity(t) for t in tests}) != len(tests):
        raise ValueError('report has no executed tests or duplicate identities')
    return tests


def execute(command, cwd, env, log, timeout=None):
    started = time.monotonic()
    with log.open('w') as output:
        child = subprocess.Popen(command,cwd=cwd,env=env,stdout=output,stderr=subprocess.STDOUT,start_new_session=True)
        try:
            status = child.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            # The session belongs to this launch. Leave room inside the 30s
            # total budget for nextest's child cleanup, then kill this group.
            os.killpg(child.pid,signal.SIGTERM)
            try:
                child.wait(timeout=.4)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid,signal.SIGKILL)
                child.wait(timeout=.4)
            status = 124
    return status,time.monotonic()-started


def run(args):
    repo = args.repo.resolve()
    out = args.out.resolve()
    queued = os.environ.get('GITHUB_EVENT_NAME') == 'merge_group'
    head,tree = git(repo,'rev-parse','HEAD'),git(repo,'rev-parse','HEAD^{tree}')
    if subprocess.run(['git','-C',str(repo),'diff','--quiet','HEAD']).returncode:
        raise ValueError('queue tests require a clean tracked tree')
    active = quarantine(GitHub(args.issues_repo)) if queued else []
    excluded = [t for t in active if t['runner']==args.runner]
    summary = f'Queue quarantine: {len(excluded)} {args.runner} test(s)\n'+''.join(f'- {identity(t)} — {t["issue"]}\n' for t in excluded)
    print(summary,flush=True)
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'],'a') as stream:stream.write(summary)
    base_env = {**os.environ,'VERIFIED_TEST_REPO_ROOT':str(repo)}
    original_log = base_env.get('VERIFIED_TEST_LOG')
    if args.runner == 'rust':
        cwd = repo
        command = [str(HERE/'run-verified-tests.sh'),'nextest','run','--archive-file',str(repo/'fast-validation-suite.tar.zst'),
                   '--workspace-remap',str(repo),'--no-fail-fast']
        if queued:command += ['--profile','queue','--retries','0']
    else:
        cwd = repo/'hub-web'
        command = ['npm','run','journeys','--','--workers=2']
        if queued:command += ['--retries=0']
    # Strict non-queue callers retain the original command, including their
    # native zero-test guard. No quarantine or additional attempt applies.
    partition = ['--partition',f'count:{args.shard}'] if args.runner=='rust' else [f'--shard={args.shard}']
    if not queued:
        return subprocess.run(command+partition,cwd=cwd,env=base_env).returncode
    if (out/'receipt.json').exists():
        raise ValueError('refusing to overwrite earlier queue evidence')
    attempts = []
    for index in range(2):
        attempt = out/('initial' if index==0 else 'rerun')
        attempt.mkdir(parents=True,exist_ok=True)
        env = {**base_env,'VERIFIED_TEST_REPO_ROOT':str(repo),'VERIFIED_TEST_LOG':str(attempt/'native.log'),
               'JOURNEY_OUTPUT':str(attempt/'playwright')}
        selection = list(partition) if index==0 else []
        if args.runner=='rust':
            native = repo/'target/nextest/queue/junit.xml'
            native.unlink(missing_ok=True)
            if index:selection += ['-E',rust_filter(failed)]
            elif excluded:selection += ['-E',f'not ({rust_filter(excluded)})']
        else:
            native = attempt/'playwright/report.json'
            if index:selection += sorted({'(^|/)'+re.escape(t['file'])+'$' for t in failed})+['--workers=1']
            if excluded:
                skip = attempt/'quarantine.txt'
                skip.write_text(playwright_list(excluded))
                selection += ['--test-list-invert',str(skip)]
        status,elapsed = execute(command+selection,cwd,env,attempt/'console.log',29 if index and args.runner=='rust' else None)
        report = attempt/('report.xml' if args.runner=='rust' else 'report.json')
        if native.is_file():shutil.copyfile(native,report)
        tests = read_results(args.runner,report)
        attempts.append({'status':status,'seconds':elapsed,'tests':tests})
        if index==0:
            if original_log and (attempt/'native.log').is_file():shutil.copyfile(attempt/'native.log',original_log)
            failed = [t for t in tests if not t['passed']]
            if status==0:
                if failed:raise ValueError('successful runner reported failed tests')
                break
            if not failed:raise ValueError('runner failed without an executed failed test')
    final_tests = {identity(t):t for t in attempts[-1]['tests']}
    flakes = [t for t in failed if final_tests.get(identity(t),{}).get('passed')] if len(attempts)>1 else []
    complete = all(identity(t) in final_tests for t in failed) if len(attempts)>1 else True
    success = attempts[-1]['status']==0 and complete and all(t['passed'] for t in final_tests.values())
    if git(repo,'rev-parse','HEAD')!=head or git(repo,'rev-parse','HEAD^{tree}')!=tree or subprocess.run(['git','-C',str(repo),'diff','--quiet','HEAD']).returncode:
        raise ValueError('the tested tree changed during execution')
    receipt = {'schema':1,'head':head,'tree':tree,'runner':args.runner,'shard':args.shard,
               'run_id':os.environ.get('GITHUB_RUN_ID',''),'run_url':f'https://github.com/{os.environ.get("GITHUB_REPOSITORY","")}/actions/runs/{os.environ.get("GITHUB_RUN_ID","")}',
               'outcome':('FLAKY' if flakes else 'PASS') if success else 'FAIL',
               'initial_status':attempts[0]['status'],'rerun_seconds':attempts[-1]['seconds'] if len(attempts)>1 else 0,
               'flaky':flakes,'quarantine':excluded,'attempts':attempts}
    write_json(out/'receipt.json',receipt)
    print(f'{receipt["outcome"]}: {len(flakes)} flaky test(s); receipt {out/"receipt.json"}')
    return 0 if success else 1


def reconcile(args):
    if os.environ.get('GITHUB_EVENT_NAME') != 'merge_group':
        raise ValueError('flake publication is merge-queue-only')
    receipts = sorted(args.receipts.rglob('receipt.json'))
    if not receipts:
        raise ValueError('no queue receipts found; publication cannot silently pass')
    observations = {}
    slots = set()
    for path in receipts:
        receipt = json.loads(path.read_text())
        if receipt.get('schema') != 1 or receipt.get('run_id') != os.environ['GITHUB_RUN_ID']:
            raise ValueError('receipt does not belong to this workflow run')
        if os.environ.get('GITHUB_SHA') and receipt['head'] != os.environ['GITHUB_SHA']:
            raise ValueError('receipt is for a different evaluated commit')
        slot = receipt['runner']+':'+receipt['shard']
        if slot in slots: raise ValueError('duplicate shard receipt: '+slot)
        slots.add(slot)
        for test in receipt['flaky']:
            observations[identity(test)] = (test,receipt)
    if args.expect and slots != set(args.expect):
        raise ValueError(f'incomplete queue evidence: expected {args.expect}; found {sorted(slots)}')
    if not observations:
        print('Queue flake publication: no recovered failures')
        return 0
    api = GitHub(args.issues_repo)
    published = []
    with api.publication_lock():
        api.ensure_labels()
        issues = api.issues()
        for key,(test,receipt) in observations.items():
            matches = [(i,issue_state(i)) for i in issues if MARKER.search(i.get('body') or '') and identity(issue_state(i)['test']) == key]
            if len(matches)>1:
                raise ValueError('duplicate open flake issues need reconciliation: '+key)
            issue,state = matches[0] if matches else (None,{'schema':1,'runs':[]})
            # Store the actual identity fields, not the presentation JSON list.
            state['test'] = {k:test[k] for k in (('runner','binary','name') if test['runner']=='rust' else ('runner','project','file','titles'))}
            run_id = receipt['run_id']
            if not re.fullmatch(r'[1-9]\d*',run_id): raise ValueError('invalid GitHub run id')
            if run_id not in state['runs']: state['runs'].append(run_id)
            labels = set(l['name'] for l in issue['labels']) if issue else set()
            labels.update(['flaky','priority:p1'])
            if len(set(state['runs']))>=2: labels.add('flaky-quarantine')
            runs = '\n'.join(f'- https://github.com/{os.environ["GITHUB_REPOSITORY"]}/actions/runs/{r}' for r in state['runs'])
            body = ('Merge-queue fail-then-pass: **FLAKY**. Fix this test; close this issue with the fix PR.\n\n'
                    'An open `flaky-quarantine` issue excludes this identity from subsequent queue trees.\n\n'
                    f'Test id: `{key}`\n\nIndependent runs:\n{runs}\n\n'
                    f'Evaluated commit: {receipt["head"]}\n\nLatest initial failure:\n'
                    '```text\n'+test['output'][:16000].replace('```','` ` `').replace('<!--','&lt;!--')+'\n```\n\n'
                    '<!-- cas-queue-flake: '+json.dumps(state,ensure_ascii=True)+' -->\n')
            payload = {'title':('Flaky queue test: '+(test['name'] if test['runner']=='rust' else ' › '.join(test['titles'])))[:240],
                       'body':body,'labels':sorted(labels)}
            if issue:
                issue = api.api(f'/issues/{issue["number"]}','PATCH',payload)
            else:
                try: issue = api.api('/issues','POST',payload)
                except (OSError,HTTPError):
                    # Re-read after an ambiguous create; never blindly create twice.
                    found = [i for i in api.issues() if MARKER.search(i.get('body') or '') and identity(issue_state(i)['test'])==key]
                    if len(found)!=1: raise
                    issue = found[0]
                issues.append(issue)
            saved = issue_state(issue)
            if run_id not in saved['runs'] or (len(set(state['runs']))>=2 and 'flaky-quarantine' not in [l['name'] for l in issue['labels']]):
                raise ValueError('GitHub did not persist the flake observation/quarantine')
            published.append({'test':key,'issue':issue['html_url'],'runs':saved['runs'],
                              'quarantined':'flaky-quarantine' in [l['name'] for l in issue['labels']]})
    write_json(args.receipts/'publication.json',{'schema':1,'published':published})
    for item in published: print(f'FLAKY: {item["test"]} — {item["issue"]}; quarantine={item["quarantined"]}')
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'],'a') as summary:
            summary.write('### Queue flakes\n'+''.join(f'- {p["issue"]}; quarantine={p["quarantined"]}\n' for p in published))
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command',required=True)
    command = commands.add_parser('run')
    command.add_argument('runner',choices=['rust','playwright'])
    command.add_argument('--repo',type=Path,default=HERE.parent)
    command.add_argument('--shard',required=True)
    command.add_argument('--out',type=Path,required=True)
    command.add_argument('--issues-repo',default=os.environ.get('CASSY_ISSUES_REPO',os.environ.get('GITHUB_REPOSITORY','')))
    command = commands.add_parser('reconcile')
    command.add_argument('--issues-repo',required=True)
    command.add_argument('--receipts',type=Path,required=True)
    command.add_argument('--expect',action='append',default=[],help='required runner:shard receipt (repeatable)')
    args = parser.parse_args()
    try:
        return run(args) if args.command=='run' else reconcile(args)
    except (OSError,ValueError,KeyError,ET.ParseError,subprocess.SubprocessError) as error:
        print(f'ERROR: {error}',file=sys.stderr)
        return 1


if __name__=='__main__':
    sys.exit(main())
