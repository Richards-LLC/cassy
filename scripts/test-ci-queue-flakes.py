#!/usr/bin/env python3
"""Exercise queue recovery through its CLI with native-report runner fixtures."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest

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

    def test_checked_in_quarantine_excludes_only_the_named_test(self):
        for engine in ('rust','playwright'):
            with self.subTest(engine=engine):
                test = {'runner':engine,'binary':'fixture::lib','name':'sometimes',
                        'project':'journeys','file':'fixture.journey.ts','titles':['sometimes'],
                        'issue':'https://github.com/fixture/repo/issues/1','runs':['100','101']}
                (self.repo/'.config/flaky-quarantine.json').write_text(json.dumps({'schema':1,'tests':[test]}))
                self.git('add','.config')
                self.git('commit','-qm','Quarantine flaky fixture; fix #1')
                (self.repo/'attempts.jsonl').unlink(missing_ok=True)
                result = self.run_queue(engine)
                self.assertEqual(result.returncode,0,result.stdout+result.stderr)
                receipt = json.loads((self.repo/'receipts/receipt.json').read_text())
                self.assertEqual(receipt['outcome'],'PASS')
                self.assertEqual(receipt['quarantine'],[test])
                self.assertEqual(len(self.calls()),1)
                self.assertIn(test['issue'],result.stdout)


if __name__ == '__main__':
    unittest.main()
