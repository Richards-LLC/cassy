#!/usr/bin/env python3
"""Exercise queue recovery through its CLI with native-report runner fixtures."""
import json
import os
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
excluded = ('-E' in args and 'not (' in args[args.index('-E')+1] and name in args[args.index('-E')+1])
if '--test-list-invert' in args:
    excluded = name in pathlib.Path(args[args.index('--test-list-invert')+1]).read_text()
failed = not excluded and (attempt == 1 or name == 'always-fail')
if excluded: name = 'healthy-control'
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
sys.exit(1 if failed else 0)
'''


class QueueRecovery(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        (self.repo/"hub-web").mkdir()
        (self.repo/".config").mkdir()
        (self.repo/".config/flaky-quarantine.json").write_text('{"schema":1,"tests":[]}')
        self.bin = self.repo/"bin"
        self.bin.mkdir()
        for name in ("cargo", "npm"):
            p = self.bin/name
            p.write_text(FIXTURE)
            p.chmod(0o755)
        self.git("init", "-q", "-b", "fixture/queue-recovery")
        self.git("config", "user.name", "Queue fixture")
        self.git("config", "user.email", "fixture@example.test")
        self.git("add", ".config")
        self.git("commit", "-qm", "fixture tree")
        self.env = {**os.environ, "PATH":str(self.bin)+os.pathsep+os.environ['PATH'],
                    "CARGO":str(self.bin/'cargo'), "GITHUB_EVENT_NAME":"merge_group",
                    "GITHUB_RUN_ID":"101", "GITHUB_REPOSITORY":"fixture/repo"}
        self.issues = []
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
                    result = [i for i in owner.issues if i['state']=='open' and
                              all(label in [l['name'] for l in i['labels']] for label in query.get('labels',[]))]
                    if query.get('page',['1'])[0] != '1': result = []
                elif path == '/issues' and method == 'POST':
                    result = {**data,'number':len(owner.issues)+1,'state':'open',
                              'html_url':f'https://github.com/fixture/repo/issues/{len(owner.issues)+1}',
                              'labels':[{'name':l} for l in data['labels']]}
                    owner.issues.append(result)
                    if owner.lose_create_response: code = 502
                elif path.startswith('/issues/') and method == 'PATCH':
                    result = owner.issues[int(path.split('/')[-1])-1]
                    result.update({**data,'labels':[{'name':l} for l in data['labels']]})
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

    def test_failed_selection_runs_once_more_on_the_same_tree(self):
        for engine in ('rust','playwright'):
            for name, outcome, code in [('sometimes','FLAKY',0),('always-fail','FAIL',1)]:
                with self.subTest(engine=engine,name=name):
                    (self.repo/'attempts.jsonl').unlink(missing_ok=True)
                    before = self.git('rev-parse','HEAD^{tree}')
                    result = self.run_queue(engine,name)
                    self.assertEqual(result.returncode,code,result.stdout+result.stderr)
                    receipt = json.loads((self.repo/'receipts/receipt.json').read_text())
                    self.assertEqual(receipt['outcome'],outcome)
                    self.assertEqual(receipt['initial_status'],1)
                    self.assertEqual(receipt['tree'],before)
                    calls = self.calls()
                    self.assertEqual(len(calls),2)
                    self.assertNotIn('--partition',calls[1])
                    self.assertFalse(any(a.startswith('--shard') for a in calls[1]))
                    self.assertLess(receipt['rerun_seconds'],30)
                    self.assertEqual(self.git('rev-parse','HEAD^{tree}'),before)

    def test_non_queue_events_never_rerun(self):
        for engine in ('rust','playwright'):
            with self.subTest(engine=engine):
                (self.repo/'attempts.jsonl').unlink(missing_ok=True)
                result = self.run_queue(engine,event='workflow_dispatch')
                self.assertNotEqual(result.returncode,0)
                self.assertEqual(len(self.calls()),1)

    def reconcile(self):
        return subprocess.run([sys.executable,str(SCRIPT),'reconcile','--issues-repo','fixture/repo',
                               '--receipts',str(self.repo/'receipts')],env=self.env,capture_output=True,text=True)

    def test_publication_deduplicates_runs_and_second_flake_quarantines_until_fixed(self):
        self.assertEqual(self.run_queue('rust').returncode,0)
        self.lose_create_response = True
        first = self.reconcile()
        self.assertEqual(first.returncode,0,first.stdout+first.stderr)
        self.assertEqual(len(self.issues),1)
        self.assertIn('priority:p1',[l['name'] for l in self.issues[0]['labels']])
        self.assertNotIn('flaky-quarantine',[l['name'] for l in self.issues[0]['labels']])
        again = self.reconcile()
        self.assertEqual(again.returncode,0,again.stderr)
        self.assertEqual(len(self.issues),1)
        self.assertNotIn('flaky-quarantine',[l['name'] for l in self.issues[0]['labels']])
        self.env['GITHUB_RUN_ID'] = '102'
        (self.repo/'attempts.jsonl').unlink()
        self.assertEqual(self.run_queue('rust').returncode,0)
        second = self.reconcile()
        self.assertEqual(second.returncode,0,second.stdout+second.stderr)
        self.assertEqual(len(self.issues),1)
        self.assertIn('flaky-quarantine',[l['name'] for l in self.issues[0]['labels']])
        (self.repo/'attempts.jsonl').unlink()
        excluded = self.run_queue('rust')
        self.assertEqual(excluded.returncode,0,excluded.stdout+excluded.stderr)
        self.assertEqual(len(self.calls()),1)
        self.assertIn(self.issues[0]['html_url'],excluded.stdout)
        self.issues[0]['state'] = 'closed'
        (self.repo/'attempts.jsonl').unlink()
        self.assertEqual(self.run_queue('rust').returncode,0)
        self.assertEqual(len(self.calls()),2)
        fixed = self.reconcile()
        self.assertEqual(fixed.returncode,0,fixed.stdout+fixed.stderr)
        self.assertEqual(len(self.issues),2)
        self.assertNotIn('flaky-quarantine',[l['name'] for l in self.issues[1]['labels']])

    def test_publication_refuses_incomplete_evidence_or_a_competing_writer(self):
        self.assertEqual(self.run_queue('rust').returncode,0)
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

    def test_open_issue_quarantine_excludes_only_the_named_test(self):
        for engine in ('rust','playwright'):
            with self.subTest(engine=engine):
                self.issues.clear()
                test = {'runner':engine,'binary':'fixture::lib','name':'sometimes',
                        'project':'journeys','file':'fixture.journey.ts','titles':['sometimes']}
                state = {'schema':1,'test':test,'runs':['100','101']}
                self.issues.append({'number':1,'state':'open','html_url':'https://github.com/fixture/repo/issues/1',
                                    'labels':[{'name':'flaky-quarantine'}],
                                    'body':'<!-- cas-queue-flake: '+json.dumps(state)+' -->'})
                (self.repo/'attempts.jsonl').unlink(missing_ok=True)
                result = self.run_queue(engine)
                self.assertEqual(result.returncode,0,result.stdout+result.stderr)
                receipt = json.loads((self.repo/'receipts/receipt.json').read_text())
                self.assertEqual(receipt['outcome'],'PASS')
                self.assertEqual(receipt['quarantine'][0]['runner'],engine)
                self.assertEqual(len(self.calls()),1)
                self.assertIn(self.issues[0]['html_url'],result.stdout)


def live_acceptance(out):
    """Explicit manual proof: native runner fixtures, real routed GitHub issues.

    Normal test runs always use the local HTTP fixture. This mode is dispatched
    only on a throwaway branch; its issue is closed after proving quarantine.
    """
    out.mkdir(parents=True,exist_ok=True)
    run_id = os.environ['GITHUB_RUN_ID']
    route = os.environ['GITHUB_REPOSITORY']
    proof = {'run_id':run_id,'checks':[]}
    fixture = out/'fixture'
    fixture.mkdir()
    (fixture/'hub-web').mkdir()
    bin_dir = fixture/'bin'
    bin_dir.mkdir()
    for name in ('cargo','npm'):
        p = bin_dir/name
        p.write_text(FIXTURE)
        p.chmod(0o755)
    for args in [('init','-q','-b','fixture/queue-flake-acceptance'),('config','user.name','Queue acceptance'),
                 ('config','user.email','queue-acceptance@example.test'),('commit','--allow-empty','-qm','Acceptance fixture tree')]:
        subprocess.run(['git',*args],cwd=fixture,check=True)
    env = {**os.environ,'GITHUB_EVENT_NAME':'merge_group','CASSY_ISSUES_REPO':route,
           'PATH':str(bin_dir)+os.pathsep+os.environ['PATH'],'CARGO':str(bin_dir/'cargo')}
    # The throwaway native fixture has its own commit; publication verifies it.
    env['GITHUB_SHA'] = subprocess.check_output(['git','rev-parse','HEAD'],cwd=fixture,text=True).strip()
    name = 'queue_acceptance_'+run_id
    issue = None
    def command(args, code=0):
        result = subprocess.run([sys.executable,str(SCRIPT),*args],env=env,capture_output=True,text=True)
        print(result.stdout+result.stderr,flush=True)
        if result.returncode != code: raise RuntimeError(f'acceptance expected exit {code}, got {result.returncode}')
    def queue(label,test,code):
        (fixture/'attempts.jsonl').unlink(missing_ok=True)
        env['FIXTURE_TEST'] = test
        command(['run','rust','--repo',str(fixture),'--shard','1/3','--out',str(out/label)],code)
        receipt = json.loads((out/label/'receipt.json').read_text())
        calls = [json.loads(line) for line in (fixture/'attempts.jsonl').read_text().splitlines()]
        return receipt,calls
    def patch_issue(payload):
        subprocess.run(['gh','api','--method','PATCH',f'repos/{route}/issues/{issue.rsplit("/",1)[1]}','--input','-'],
                       input=json.dumps(payload),env=env,capture_output=True,text=True,check=True)
    try:
        receipt,calls = queue('recovered',name,0)
        if receipt['outcome']!='FLAKY' or len(calls)!=2 or receipt['rerun_seconds']>=30:
            raise RuntimeError('fail-then-pass did not recover within the Rust budget')
        command(['reconcile','--issues-repo',route,'--receipts',str(out/'recovered'),'--expect','rust:1/3'])
        publication = json.loads((out/'recovered/publication.json').read_text())
        issue = publication['published'][0]['issue']
        if publication['published'][0]['quarantined']: raise RuntimeError('first flake quarantined prematurely')
        proof['checks'].append({'case':'fail-then-pass','outcome':receipt['outcome'],'eject':False,
                                'issue':issue,'rerun_seconds':receipt['rerun_seconds']})
        receipt,calls = queue('always-fail','always-fail',1)
        if receipt['outcome']!='FAIL' or len(calls)!=2: raise RuntimeError('persistent failure did not eject')
        proof['checks'].append({'case':'always-fail','outcome':'FAIL','eject':True,'rerun_seconds':receipt['rerun_seconds']})
        # Second-distinct-run labeling is covered by the deterministic HTTP
        # test. Here prove the authoritative live label/read path explicitly.
        patch_issue({'labels':['flaky','priority:p1','flaky-quarantine']})
        receipt,calls = queue('quarantined',name,0)
        if receipt['outcome']!='PASS' or len(calls)!=1 or not any(t['issue']==issue for t in receipt['quarantine']):
            raise RuntimeError('live labeled issue did not exclude its test')
        proof['checks'].append({'case':'open-labeled-issue','outcome':'PASS','excluded':True,'issue':issue})
        patch_issue({'state':'closed','state_reason':'completed'})
        receipt,calls = queue('fixed',name,0)
        if len(calls)!=2 or any(t['issue']==issue for t in receipt['quarantine']):
            raise RuntimeError('closing the fix issue did not remove quarantine')
        proof['checks'].append({'case':'closed-issue','excluded':False})
        proof['result'] = 'PASS'
    finally:
        if issue: patch_issue({'state':'closed','state_reason':'completed'})
        (out/'acceptance.json').write_text(json.dumps(proof,indent=2)+'\n')
    print(json.dumps(proof,indent=2),flush=True)


if __name__ == '__main__':
    if len(sys.argv)>1 and sys.argv[1]=='--live-acceptance':
        live_acceptance(Path(sys.argv[2]).resolve())
    else:
        unittest.main()
