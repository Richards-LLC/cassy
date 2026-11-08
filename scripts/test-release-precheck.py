#!/usr/bin/env python3
"""Exact-SHA hosted dispatch, honest errors, cleanup and uncertain dispatch recovery."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('release-precheck.py')


class HostedPrecheck(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.remote = self.root/'remote.git'
        self.repo = self.root/'fresh'
        subprocess.run(['git','init','--bare',str(self.remote)],check=True,capture_output=True)
        subprocess.run(['git','clone',str(self.remote),str(self.repo)],check=True,capture_output=True)
        self.git('remote','set-url','origin','https://github.com/org/repo.git')
        self.git('config',f'url.{self.remote}.insteadOf','https://github.com/org/repo.git')
        self.git('config','user.email','fixture@example.test')
        self.git('config','user.name','Fixture')
        (self.repo/'source').write_text('no build dependencies')
        self.git('add','source')
        self.git('commit','-m','fixture')
        self.sha = self.git('rev-parse','HEAD')
        binary = self.root/'gh'
        binary.write_text('''#!/usr/bin/env python3
import json,os,sys
from pathlib import Path
args=sys.argv[1:]
with open(os.environ['GH_CALLS'],'a') as out: out.write(json.dumps(args)+'\\n')
run={'databaseId':123,'headSha':os.environ['EXPECTED_SHA'],
     'url':'https://github.com/org/repo/actions/runs/123','status':'queued','conclusion':None,
     'createdAt':'2000-01-01T00:00:00Z'}
if args[:2]==['repo','view']: print('{"nameWithOwner":"org/repo"}')
elif args[:2]==['workflow','run']:
 assert args[2]=='ci.yml' and args[-2]=='--ref' and args[-1].startswith('precheck/'),args
 Path(os.environ['DISPATCHED']).touch()
 if os.environ.get('FALLBACK')!='1': print(run['url'])
elif args[:2]==['run','view']:
 run.update(status='completed',conclusion=os.environ.get('CONCLUSION','success'),
            createdAt='2026-11-01T01:00:00Z',updatedAt='2026-11-01T01:19:00Z',
            jobs=[{'name':'Fast Validation','conclusion':'success'}])
 print(json.dumps(run))
elif args[:2]==['run','list']:
 print(json.dumps([run] if Path(os.environ['DISPATCHED']).exists() and os.environ.get('HIDDEN')!='1' else []))
else: sys.exit('unexpected gh call: '+repr(args))
''')
        binary.chmod(0o755)
        self.env = dict(os.environ,PATH=str(self.root)+os.pathsep+os.environ['PATH'],
                        GH_CALLS=str(self.root/'calls'),EXPECTED_SHA=self.sha,
                        DISPATCHED=str(self.root/'dispatched'))

    def git(self,*args):
        return subprocess.check_output(['git','-C',str(self.repo),*args],text=True).strip()

    def run_precheck(self,*args):
        return subprocess.run(['python3','-I',str(SCRIPT),self.sha,*args,'--json'],
                              cwd=self.repo,env=self.env,text=True,capture_output=True)

    def test_push_dispatch_and_report_without_local_setup(self):
        result = self.run_precheck()
        self.assertEqual(result.returncode,0,result.stderr)
        receipt = json.loads(result.stdout)
        self.assertEqual(receipt['sha'],self.sha)
        self.assertEqual(receipt['ref'],'precheck/'+self.sha)
        self.assertEqual(subprocess.check_output(['git','--git-dir',str(self.remote),'rev-parse',receipt['ref']],text=True).strip(),self.sha)
        self.assertEqual(self.git('status','--porcelain'),'')
        status = self.run_precheck('--status')
        self.assertEqual(status.returncode,0,status.stderr)
        completed = json.loads(status.stdout)
        self.assertEqual(completed['elapsed_minutes'],19)
        self.assertEqual(completed['jobs'][0]['conclusion'],'success')
        self.assertEqual(self.git('ls-remote','origin','refs/heads/'+receipt['ref']),'')
        # Completed status is still readable after the temporary ref is gone.
        self.assertEqual(self.run_precheck('--status').returncode,0)

    def test_uncertain_dispatch_recovers_without_clock_or_duplicate_dispatch(self):
        self.env.update(FALLBACK='1',HIDDEN='1')
        first = self.run_precheck()
        self.assertNotEqual(first.returncode,0)
        self.env['HIDDEN']='0'
        recovered = self.run_precheck()
        self.assertEqual(recovered.returncode,0,recovered.stderr)
        self.assertEqual(json.loads(recovered.stdout)['run_id'],123)
        calls = [json.loads(line) for line in (self.root/'calls').read_text().splitlines()]
        self.assertEqual(sum(call[:2]==['workflow','run'] for call in calls),1)

    def test_failures_are_json_without_tracebacks_or_invented_run_receipts(self):
        missing = self.run_precheck('--status')
        self.assertNotEqual(missing.returncode,0)
        self.assertIn('error',json.loads(missing.stdout))
        malformed = self.root/'malformed.json'
        malformed.write_text('{}')
        invalid = self.run_precheck('--status','--receipt',str(malformed))
        self.assertNotEqual(invalid.returncode,0)
        self.assertIn('receipt',json.loads(invalid.stdout)['error'].lower())
        self.assertNotIn('Traceback',invalid.stderr)
        self.assertEqual(self.run_precheck().returncode,0)
        self.env['CONCLUSION']='failure'
        failed = self.run_precheck('--status')
        self.assertEqual(failed.returncode,1)
        self.assertEqual(json.loads(failed.stdout)['conclusion'],'failure')


if __name__ == '__main__':
    unittest.main()
