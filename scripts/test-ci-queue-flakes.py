#!/usr/bin/env python3
"""Exercise first-failure reporting through its CLI with native-report runner fixtures."""
import json
import os
import runpy
from datetime import date, datetime, timezone
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread
from urllib.parse import urlparse, parse_qs

SCRIPT = Path(__file__).with_name("ci-queue-flakes.py")
FIXTURE = r'''#!/usr/bin/env python3
import json, os, pathlib, sys
import xml.etree.ElementTree as ET
root = pathlib.Path.cwd()
if root.name == 'hub-web': root = root.parent
state = root / 'attempts.jsonl'
args = sys.argv[1:]
with state.open('a') as log: log.write(json.dumps(args)+'\n')
attempt = len(state.read_text().splitlines())
name = os.environ.get('FIXTURE_TEST', 'sometimes')
failed = not os.environ.get('FIXTURE_PASS') and (attempt == 1 or name == 'always-fail')
if os.environ.get('FIXTURE_MISSING_REPORT'): sys.exit(2)
if pathlib.Path(sys.argv[0]).name == 'cargo':
    report = root/'target/nextest/queue/junit.xml'
    report.parent.mkdir(parents=True, exist_ok=True)
    suite = ET.Element('testsuite', name='fixture::lib')
    test = ET.SubElement(suite, 'testcase', name=name, classname='fixture::lib')
    if failed: ET.SubElement(test, 'failure').text = 'fixture assertion failed'
    ET.ElementTree(suite).write(report)
    print(f'Summary [0.00s] 1 tests run: {0 if failed else 1} passed, {1 if failed else 0} failed')
else:
    report = pathlib.Path(os.environ['JOURNEY_OUTPUT'])/'report.json'
    report.parent.mkdir(parents=True, exist_ok=True)
    report.write_text(json.dumps({'errors':[], 'suites':[{'title':'fixture.journey.ts','file':'fixture.journey.ts','specs':[{
        'title':name,'file':'fixture.journey.ts','line':1,'tests':[{'projectName':'journeys','expectedStatus':'passed','status':'unexpected' if failed else 'expected','results':[{'status':'failed' if failed else 'passed','retry':0,'errors':[{'message':'fixture assertion failed'}] if failed else []}]}]}]}]}))
sys.exit(int(os.environ.get('FIXTURE_EXIT',1 if failed else 0)))
'''


class QueueRecovery(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        (self.repo/"hub-web").mkdir()
        self.bin = self.repo/"bin"
        self.bin.mkdir()
        for name in ("cargo", "npm"):
            p = self.bin/name
            p.write_text(FIXTURE)
            p.chmod(0o755)
        self.git("init", "-q", "-b", "fixture/queue-recovery")
        self.git("config", "user.name", "Queue fixture")
        self.git("config", "user.email", "fixture@example.test")
        self.git("commit", "--allow-empty", "-qm", "fixture tree")
        self.env = {**os.environ, "PATH":str(self.bin)+os.pathsep+os.environ['PATH'],
                    "CARGO":str(self.bin/'cargo'), "GITHUB_EVENT_NAME":"merge_group",
                    "GITHUB_RUN_ID":"101", "GITHUB_REPOSITORY":"fixture/repo"}
        self.issues = []
        self.issue_read_failure = False
        self.refs = {}
        self.lose_create_response = False
        owner = self
        class API(BaseHTTPRequestHandler):
            def log_message(self, *args): pass
            def do_GET(self): self.respond('GET')
            def do_POST(self): self.respond('POST')
            def do_PATCH(self): self.respond('PATCH')
            def do_DELETE(self): self.respond('DELETE')
            def respond(self, method):
                path = urlparse(self.path).path.removeprefix('/repos/fixture/repo')
                query = parse_qs(urlparse(self.path).query)
                data = json.loads(self.rfile.read(int(self.headers.get('Content-Length',0))) or '{}')
                code, result = 200, {}
                if path == '' and method == 'GET': result = {'default_branch':'main'}
                elif path == '/git/ref/heads/main': result = {'object':{'sha':'base'}}
                elif path == '/git/commits/base': result = {'tree':{'sha':'tree'}}
                elif path == '/git/commits': result = {'sha':data['message']}
                elif path == '/git/refs':
                    if data['ref'] in owner.refs: code = 422
                    else: owner.refs[data['ref']] = data['sha']
                elif path.startswith('/git/ref/'):
                    ref = 'refs/'+path.removeprefix('/git/ref/')
                    if ref in owner.refs: result = {'object':{'sha':owner.refs[ref]}}
                    else: code = 404
                elif path.startswith('/git/refs/') and method == 'DELETE':
                    del owner.refs['refs/'+path.removeprefix('/git/refs/')]
                elif path.startswith('/labels'): result = {}
                elif path == '/issues' and method == 'GET':
                    # GitHub's state/label indexes can lag behind successful
                    # mutations; the unfiltered history and direct issue GET
                    # expose the current record (live acceptance regression).
                    result = owner.issues if query.get('state')==['all'] and not query.get('labels') else []
                    if query.get('page',['1'])[0] != '1': result = []
                    if owner.issue_read_failure: code, result = 503, {'message':'fixture issue read unavailable'}
                elif path == '/issues' and method == 'POST':
                    result = {**data,'number':len(owner.issues)+1,'state':'open',
                              'html_url':f'https://github.com/fixture/repo/issues/{len(owner.issues)+1}',
                              'labels':[{'name':l} for l in data['labels']]}
                    owner.issues.append(result)
                    if owner.lose_create_response: code = 502
                elif path.startswith('/issues/') and method == 'PATCH':
                    result = owner.issues[int(path.split('/')[-1])-1]
                    result.update({**data,'labels':[{'name':l} for l in data['labels']]})
                elif path.startswith('/issues/') and method == 'GET':
                    result = owner.issues[int(path.split('/')[-1])-1]
                else: code = 404
                self.send_response(code)
                self.send_header('Content-Type','application/json')
                self.end_headers()
                self.wfile.write(json.dumps(result).encode())
        server = ThreadingHTTPServer(('127.0.0.1',0), API)
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        self.env.update(GITHUB_API_URL=f'http://127.0.0.1:{server.server_port}',GH_TOKEN='fixture',CASSY_ISSUES_REPO='fixture/repo')
        self.env.pop('GITHUB_SHA',None)

    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.repo, text=True).strip()

    def calls(self):
        return [json.loads(line) for line in (self.repo/'attempts.jsonl').read_text().splitlines()]

    def run_queue(self, engine, test="sometimes", event="merge_group"):
        shutil.rmtree(self.repo/'receipts',ignore_errors=True)
        return subprocess.run([sys.executable,str(SCRIPT),'run',engine,'--repo',str(self.repo),
                               '--shard','1/3','--out',str(self.repo/'receipts')],
                              env={**self.env,"FIXTURE_TEST":test,"GITHUB_EVENT_NAME":event},
                              capture_output=True,text=True)

    def test_first_failure_runs_once_and_preserves_the_tree(self):
        for engine in ('rust','playwright'):
            for event in ('merge_group','push','workflow_dispatch'):
                for case in ({}, {'FIXTURE_PASS':'1','FIXTURE_EXIT':'2'}, {'FIXTURE_MISSING_REPORT':'1'}, {'FIXTURE_PASS':'1'}):
                    with self.subTest(engine=engine,event=event,case=case):
                        (self.repo/'attempts.jsonl').unlink(missing_ok=True)
                        before = self.git('rev-parse','HEAD^{tree}')
                        self.env.update(case)
                        result = self.run_queue(engine,event=event)
                        receipt = json.loads((self.repo/'receipts/receipt.json').read_text())
                        passed = case=={'FIXTURE_PASS':'1'}
                        self.assertEqual(result.returncode,0 if passed else int(case.get('FIXTURE_EXIT',2 if 'FIXTURE_MISSING_REPORT' in case else 1)),result.stdout+result.stderr)
                        self.assertEqual(receipt['outcome'],'PASS' if passed else 'FAIL')
                        self.assertEqual(receipt['tree'],before)
                        self.assertEqual(len(self.calls()),1)
                        self.assertEqual(self.git('rev-parse','HEAD^{tree}'),before)
                        self.assertEqual(self.git('status','--porcelain','--untracked-files=no'),'')
                        for key in case: self.env.pop(key)

    def reconcile(self):
        return subprocess.run([sys.executable,str(SCRIPT),'reconcile','--issues-repo','fixture/repo',
                               '--receipts',str(self.repo/'receipts')],env=self.env,capture_output=True,text=True)

    def workflow_reconcile(self):
        policy = runpy.run_path(str(SCRIPT.parent/'ci_tiers/policy.py'))
        job = policy['load'](SCRIPT.parent.parent/'.github/workflows/ci.yml')['jobs']['queue-flake-publication']
        download = next(step for step in job['steps'] if step.get('uses')=='actions/download-artifact@v4')
        self.assertEqual(download['with']['pattern'],'queue-flakes-*')
        destination = self.repo/download['with']['path']
        shutil.rmtree(destination,ignore_errors=True)
        # Successful reused siblings produce no native artifact. Publish the
        # observed failure even when only this shard actually executed.
        shutil.copytree(self.repo/'receipts',destination/'queue-flakes-fixture')
        (self.repo/'scripts').mkdir(exist_ok=True)
        (self.repo/'scripts/ci-queue-flakes.py').unlink(missing_ok=True)
        (self.repo/'scripts/ci-queue-flakes.py').symlink_to(SCRIPT)
        body = next(step['run'] for step in job['steps'] if 'reconcile --issues-repo' in step.get('run',''))
        return subprocess.run(['bash','-e','-o','pipefail','-c',body.replace('Richards-LLC/cassy','fixture/repo')],
                              cwd=self.repo,env=self.env,capture_output=True,text=True)

    def test_first_failure_publication_deduplicates_and_keeps_the_owner_deadline(self):
        scope = runpy.run_path(str(SCRIPT))
        for today,deadline in [('2026-11-12','2026-11-13'),('2026-11-13','2026-11-16'),('2026-11-14','2026-11-16')]:
            self.assertEqual(scope['next_working_day'](date.fromisoformat(today)),deadline)
        for engine in ('rust','playwright'):
            with self.subTest(engine=engine):
                (self.repo/'attempts.jsonl').unlink(missing_ok=True)
                self.assertEqual(self.run_queue(engine).returncode,1)
                self.lose_create_response = True
                first = self.workflow_reconcile()
                self.assertEqual(first.returncode,0,first.stdout+first.stderr)
                count = len(self.issues)
                issue = self.issues[-1]
                self.assertIn('priority:p0',[l['name'] for l in issue['labels']])
                state = scope['issue_state'](issue)
                self.assertEqual(state['repair_deadline'],scope['next_working_day'](datetime.now(timezone.utc).date()))
                # An overdue issue stays overdue when another run observes it.
                state['repair_deadline'] = '2026-01-01'
                issue['body'] = '<!-- cas-queue-flake: '+json.dumps(state)+' -->'
                again = self.reconcile()
                self.assertEqual(again.returncode,0,again.stdout+again.stderr)
                self.env['GITHUB_RUN_ID'] = str(int(self.env['GITHUB_RUN_ID'])+1)
                (self.repo/'attempts.jsonl').unlink()
                self.assertEqual(self.run_queue(engine).returncode,1)
                second = self.reconcile()
                self.assertEqual(second.returncode,0,second.stdout+second.stderr)
                self.assertEqual(len(self.issues),count)
                saved = scope['issue_state'](issue)
                self.assertEqual(saved['repair_deadline'],'2026-01-01')
                self.assertEqual(len(saved['runs']),2)
                self.assertEqual(saved['test']['runner'],engine)
                self.assertEqual(len(self.calls()),1)

    def test_publication_refuses_incomplete_evidence_or_a_competing_writer(self):
        self.assertEqual(self.run_queue('rust').returncode,1)
        path = self.repo/'receipts/receipt.json'
        receipt = json.loads(path.read_text())
        for problem in ('wrong run','foreign lock','missing receipt'):
            with self.subTest(problem=problem):
                if problem == 'wrong run': path.write_text(json.dumps({**receipt,'run_id':'999'}))
                elif problem == 'foreign lock':
                    path.write_text(json.dumps(receipt))
                    self.refs['refs/heads/cas-queue-flake-publication-lock'] = 'another-writer'
                else:
                    self.refs.clear()
                    path.unlink()
                result = self.reconcile()
                self.assertNotEqual(result.returncode,0,result.stdout)
                self.assertEqual(self.issues,[])
                if problem == 'foreign lock':
                    self.assertEqual(self.refs['refs/heads/cas-queue-flake-publication-lock'],'another-writer')


if __name__ == '__main__':
    unittest.main()
