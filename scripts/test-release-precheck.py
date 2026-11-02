#!/usr/bin/env python3
"""Dispatch from a dependency-free checkout, retaining an exact-SHA receipt."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('release-precheck.py')


class HostedPrecheck(unittest.TestCase):
    def test_push_dispatch_and_report_without_local_setup(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            remote = root / 'remote.git'
            repo = root / 'fresh'
            subprocess.run(['git', 'init', '--bare', str(remote)], check=True, capture_output=True)
            subprocess.run(['git', 'clone', str(remote), str(repo)], check=True, capture_output=True)
            def git(*args):
                return subprocess.check_output(['git', '-C', str(repo), *args], text=True).strip()
            git('remote','set-url','origin','https://github.com/org/repo.git')
            git('config',f'url.{remote}.insteadOf','https://github.com/org/repo.git')
            git('config', 'user.email', 'fixture@example.test')
            git('config', 'user.name', 'Fixture')
            (repo / 'source').write_text('a commit with no node_modules or build tools')
            git('add', 'source')
            git('commit', '-m', 'fixture')
            sha = git('rev-parse', 'HEAD')
            binary = root / 'gh'
            binary.write_text('''#!/usr/bin/env python3
import json,os,sys
from pathlib import Path
args=sys.argv[1:]
with open(os.environ['GH_CALLS'],'a') as out: out.write(json.dumps(args)+'\\n')
if args[:2]==['repo','view']: print('{"nameWithOwner":"org/repo"}')
elif args[:2]==['workflow','run']:
 assert args[2]=='ci.yml' and args[-2]=='--ref' and args[-1].startswith('precheck/'),args
 print('https://github.com/org/repo/actions/runs/123')
elif args[:2]==['run','view']:
 print(json.dumps({'headSha':os.environ['EXPECTED_SHA'],'status':'completed','conclusion':'success',
 'createdAt':'2026-11-01T01:00:00Z','startedAt':'2026-11-01T01:00:10Z','updatedAt':'2026-11-01T01:19:00Z',
 'jobs':[{'name':'Fast Validation — suite shard 1/3','conclusion':'success'}]}))
elif args[:2]==['run','list']:
 print(json.dumps([{'databaseId':123,'headSha':os.environ['EXPECTED_SHA'],'url':'https://github.com/org/repo/actions/runs/123','status':'queued','conclusion':None}]))
else: sys.exit('unexpected gh call: '+repr(args))
''')
            binary.chmod(0o755)
            env = dict(os.environ, PATH=str(root)+os.pathsep+os.environ['PATH'],
                       GH_CALLS=str(root/'calls'), EXPECTED_SHA=sha)
            result = subprocess.run(['python3', str(SCRIPT), sha[:12], '--json'], cwd=repo,
                                    env=env, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            receipt = json.loads(result.stdout)
            self.assertEqual(receipt['sha'], sha)
            self.assertEqual(receipt['ref'], 'precheck/'+sha)
            self.assertEqual(receipt['run_url'], 'https://github.com/org/repo/actions/runs/123')
            self.assertEqual(subprocess.check_output(['git','--git-dir',str(remote),'rev-parse',receipt['ref']],text=True).strip(),sha)
            self.assertFalse((repo/'node_modules').exists())
            self.assertEqual(git('rev-parse','HEAD'),sha)
            self.assertEqual(git('status','--porcelain'),'')
            status = subprocess.run(['python3',str(SCRIPT),sha,'--status','--json'],cwd=repo,
                                    env=env,text=True,capture_output=True)
            self.assertEqual(status.returncode,0,status.stderr)
            completed = json.loads(status.stdout)
            self.assertEqual(completed['elapsed_minutes'],19)
            self.assertEqual(completed['october_dispatch_minutes'],[48,53])
            self.assertEqual(completed['jobs'][0]['conclusion'],'success')
            # An invalid revision fails before pushing or dispatching anything.
            calls = (root/'calls').read_text()
            invalid = subprocess.run(['python3',str(SCRIPT),'missing-sha','--json'],cwd=repo,
                                     env=env,text=True,capture_output=True)
            self.assertNotEqual(invalid.returncode,0)
            self.assertEqual((root/'calls').read_text(),calls)


if __name__ == '__main__':
    unittest.main()
