#!/usr/bin/env python3
"""Run native tests once and publish first-failure repair issues."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
import xml.etree.ElementTree as ET
from datetime import date, datetime, timedelta, timezone
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

    def issues(self):
        result = []
        for page in range(1,1001):
            # State/label-filtered lists lag behind acknowledged mutations.
            # Enumerate history, then read authoritative candidate records.
            # This also makes an ambiguous issue-create response recoverable
            # without creating a duplicate during an indexing delay.
            query = {'state':'all','per_page':100,'page':page}
            batch = self.api('/issues?'+urlencode(query))
            for issue in batch:
                if 'pull_request' in issue: continue
                if not MARKER.search(issue.get('body') or ''): continue
                current = self.api(f'/issues/{issue["number"]}')
                if current['state']=='open':
                    result.append(current)
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
        for label,color in [('flaky','fbca04'),('priority:p0','b60205')]:
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


def identity(test):
    if test['runner'] == 'rust':
        return json.dumps(['rust', test['binary'], test['name']], ensure_ascii=False)
    if test['runner'] == 'playwright':
        return json.dumps(['playwright', test['project'], test['file'], test['titles']], ensure_ascii=False)
    if test['runner'] == 'step':
        return json.dumps(['step', test['name']], ensure_ascii=False)
    raise ValueError('unknown test runner')


def label(test):
    """A readable test name for summaries; identity() stays the stable key."""
    if test['runner'] == 'rust':
        return f"{test['binary']} {test['name']}"
    if test['runner'] == 'playwright':
        return ' > '.join([test['project'], test['file'], *test['titles']])
    return test['name']


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


def run(args):
    repo, out = args.repo.resolve(), args.out.resolve()
    head, tree = git(repo,'rev-parse','HEAD'), git(repo,'rev-parse','HEAD^{tree}')
    if subprocess.run(['git','-C',str(repo),'diff','--quiet','HEAD']).returncode:
        raise ValueError('tests require a clean tracked tree')
    if (out/'receipt.json').exists():
        raise ValueError('refusing to overwrite earlier test evidence')
    attempt = out/'initial'
    attempt.mkdir(parents=True,exist_ok=True)
    env = {**os.environ,'VERIFIED_TEST_REPO_ROOT':str(repo),
           'VERIFIED_TEST_LOG':str(attempt/'native.log'),'JOURNEY_OUTPUT':str(attempt/'playwright')}
    if args.runner == 'rust':
        cwd = repo
        command = [str(HERE/'run-verified-tests.sh'),'nextest','run',
                   '--archive-file',str(repo/'fast-validation-suite.tar.zst'),
                   '--workspace-remap',str(repo),'--no-fail-fast','--profile','queue',
                   '--retries','0','--partition',f'count:{args.shard}']
        native = repo/'target/nextest/queue/junit.xml'
        native.unlink(missing_ok=True)
    else:
        cwd = repo/'hub-web'
        command = ['npm','run','journeys','--','--workers=2','--retries=0',f'--shard={args.shard}']
        native = attempt/'playwright/report.json'
    started = time.monotonic()
    log = attempt/'console.log'
    with log.open('w') as output:
        status = subprocess.run(command,cwd=cwd,env=env,stdout=output,stderr=subprocess.STDOUT).returncode
    sys.stdout.write(log.read_text(errors='replace'))
    original_log = os.environ.get('VERIFIED_TEST_LOG')
    if original_log and (attempt/'native.log').is_file():
        shutil.copyfile(attempt/'native.log',original_log)
    report = attempt/('report.xml' if args.runner=='rust' else 'report.json')
    tests, error = [], None
    try:
        shutil.copyfile(native,report)
        tests = read_results(args.runner,report)
    except (OSError,ValueError,KeyError,ET.ParseError) as problem:
        error = str(problem)
    failed = [test for test in tests if not test['passed']]
    success = status==0 and not failed and error is None
    if git(repo,'rev-parse','HEAD')!=head or git(repo,'rev-parse','HEAD^{tree}')!=tree or subprocess.run(['git','-C',str(repo),'diff','--quiet','HEAD']).returncode:
        raise ValueError('the tested tree changed during execution')
    receipt = {'schema':1,'head':head,'tree':tree,'runner':args.runner,'shard':args.shard,
               'run_id':os.environ.get('GITHUB_RUN_ID',''),'outcome':'PASS' if success else 'FAIL',
               'initial_status':status,'failed':failed,'report_error':error,
               'attempts':[{'status':status,'seconds':time.monotonic()-started,'tests':tests}]}
    write_json(out/'receipt.json',receipt)
    print(f'{receipt["outcome"]}: {", ".join(label(t) for t in failed) or error or args.shard}; receipt {out/"receipt.json"}')
    return 0 if success else (status if status>0 else 1)


def next_working_day(today):
    deadline = today + timedelta(days=1)
    while deadline.weekday() >= 5:
        deadline += timedelta(days=1)
    return deadline.isoformat()


def reconcile(args):
    if not (os.environ.get('GITHUB_EVENT_NAME') == 'merge_group' or (os.environ.get('GITHUB_EVENT_NAME') == 'push' and os.environ.get('GITHUB_REF', '').startswith('refs/heads/epic/release-train-'))):
        raise ValueError('flake publication requires a train or merge-queue run')
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
        if receipt.get('outcome') not in ('PASS','FAIL'):
            raise ValueError('invalid test outcome')
        for test in receipt['failed']:
            observations[identity(test)] = (test,receipt)
    if not observations:
        write_json(args.receipts/'publication.json',{'schema':1,'published':[]})
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
            state['test'] = {k:test[k] for k in ({'rust':('runner','binary','name'),'playwright':('runner','project','file','titles'),'step':('runner','name')}[test['runner']])}
            run_id = receipt['run_id']
            if not re.fullmatch(r'[1-9]\d*',run_id): raise ValueError('invalid GitHub run id')
            if run_id not in state['runs']: state['runs'].append(run_id)
            state.setdefault('repair_deadline',next_working_day(datetime.now(timezone.utc).date()))
            date.fromisoformat(state['repair_deadline'])
            labels = set(l['name'] for l in issue['labels']) if issue else set()
            labels.discard('priority:p1')
            labels.update(['flaky','priority:p0'])
            runs = '\n'.join(f'- https://github.com/{os.environ["GITHUB_REPOSITORY"]}/actions/runs/{r}' for r in state['runs'])
            body = ('First native test failure. Staff this P0 repair ahead of feature work.\n\n'
                    f'Owner: issue assignee, staffed through issue intake. Fix or delete by {state["repair_deadline"]} (UTC date). '
                    'If deleted, record the behaviour this test covered in the repair task.\n\n'
                    f'Test id: `{key}`\n\nObserved runs:\n{runs}\n\n'
                    f'Evaluated commit: {receipt["head"]}\n\nLatest failure:\n'
                    '```text\n'+test['output'][:16000].replace('```','` ` `').replace('<!--','&lt;!--')+'\n```\n\n'
                    '<!-- cas-queue-flake: '+json.dumps(state,ensure_ascii=True)+' -->\n')
            payload = {'title':('Repair failing test: '+label(test))[:240],
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
            if run_id not in saved['runs'] or saved.get('repair_deadline')!=state['repair_deadline'] or 'priority:p0' not in [l['name'] for l in issue['labels']]:
                raise ValueError('GitHub did not persist the repair observation/deadline')
            published.append({'test':key,'issue':issue['html_url'],'runs':saved['runs'],
                              'repair_deadline':saved['repair_deadline']})
    write_json(args.receipts/'publication.json',{'schema':1,'published':published})
    for item in published: print(f'Repair: {item["test"]} — {item["issue"]}; due={item["repair_deadline"]}')
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'],'a') as summary:
            summary.write('### Test repairs\n'+''.join(f'- {p["issue"]}; due={p["repair_deadline"]}\n' for p in published))
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command',required=True)
    command = commands.add_parser('run')
    command.add_argument('runner',choices=['rust','playwright'])
    command.add_argument('--repo',type=Path,default=HERE.parent)
    command.add_argument('--shard',required=True)
    command.add_argument('--out',type=Path,required=True)
    command = commands.add_parser('reconcile')
    command.add_argument('--issues-repo',required=True)
    command.add_argument('--receipts',type=Path,required=True)
    args = parser.parse_args()
    try:
        return {'run':run,'reconcile':reconcile}[args.command](args)
    except (OSError,ValueError,KeyError,ET.ParseError,subprocess.SubprocessError) as error:
        print(f'ERROR: {error}',file=sys.stderr)
        return 1


if __name__=='__main__':
    sys.exit(main())
