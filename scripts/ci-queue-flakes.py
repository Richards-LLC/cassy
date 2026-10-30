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

HERE = Path(__file__).resolve().parent
QUARANTINE = '.config/flaky-quarantine.json'


def git(repo, *args):
    return subprocess.check_output(['git', '-C', str(repo), *args], text=True).strip()


def write_json(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2, ensure_ascii=False)+'\n')


def quarantine(repo):
    data = json.loads((repo/QUARANTINE).read_text())
    if data.get('schema') != 1 or not isinstance(data.get('tests'), list):
        raise ValueError('invalid quarantine schema')
    seen = set()
    for entry in data['tests']:
        key = identity(entry)
        if key in seen or not re.fullmatch(r'https://github.com/[\w.-]+/[\w.-]+/issues/[1-9]\d*', entry.get('issue','')):
            raise ValueError('quarantine entries need unique test identities and linked fix issues')
        if len(set(entry.get('runs', []))) < 2:
            raise ValueError('quarantine requires two independent flake runs')
        seen.add(key)
    return data['tests']


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
    active = quarantine(repo) if queued else []
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
            if index:selection += sorted({re.escape(t['file']) for t in failed})+['--workers=1']
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command',required=True)
    command = commands.add_parser('run')
    command.add_argument('runner',choices=['rust','playwright'])
    command.add_argument('--repo',type=Path,default=HERE.parent)
    command.add_argument('--shard',required=True)
    command.add_argument('--out',type=Path,required=True)
    args = parser.parse_args()
    try:
        return run(args)
    except (OSError,ValueError,KeyError,ET.ParseError,subprocess.SubprocessError) as error:
        print(f'ERROR: {error}',file=sys.stderr)
        return 1


if __name__=='__main__':
    sys.exit(main())
