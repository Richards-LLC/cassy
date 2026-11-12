#!/usr/bin/env python3
"""Queue retries and flake evidence through real CLI/action handlers."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
import yaml

SCRIPT = Path(__file__).with_name('train-ci.py').resolve()
GH = '''#!/usr/bin/env python3
import json, os, pathlib, shutil, sys
root=pathlib.Path(os.environ['FORGE']); args=sys.argv[1:]
data=json.loads((root/'forge.json').read_text())
if args[:2]==['repo','view']: print('fixture/repo')
elif args[:2]==['run','download']:
 name=args[args.index('--name')+1]
 source=data.get('job_downloads',{}).get(name,data.get('artifact',''))
 if source is None: sys.exit(1)
 shutil.copytree(root/source,pathlib.Path(args[args.index('--dir')+1]),dirs_exist_ok=True)
elif args[0]=='api':
 endpoint=next(a.lstrip('/') for a in args if a.lstrip('/').startswith('repos/'))
 if '/actions/artifacts?' in endpoint: print(json.dumps({'artifacts':data.get('job_artifacts',[])}))
 elif endpoint.endswith('/artifacts'): print(json.dumps({'artifacts':data.get('job_outputs',[])}))
 elif '/actions/runs/' in endpoint and endpoint.rsplit('/',1)[-1].isdigit(): print(json.dumps(data['job_run']))
 elif '/pulls/' in endpoint: print(json.dumps(data['pr']))
 elif '/statuses?' in endpoint: print(json.dumps(data['statuses']))
 elif endpoint.endswith('/logs'): sys.stdout.buffer.write((root/'logs.zip').read_bytes())
 elif endpoint.split('?')[0].endswith('/jobs'): print(json.dumps({'jobs':data.get('job_jobs',[{'name':'Shared train and queue graph / Full-set gate','conclusion':data.get('fanin','success')}])}))
 else: print(json.dumps({'workflow_runs':data['runs']}))
else: sys.exit(2)
'''


class TrainReceipts(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.repo = self.root/'repo'
        self.repo.mkdir()
        self.git('init', '-qb', 'epic/release-train-fixture')
        self.git('config', 'user.email', 'test@example.invalid')
        self.git('config', 'user.name', 'Train fixture')
        self.commit('base')
        self.commit('green')
        self.git('update-ref', 'refs/remotes/origin/main', self.git('rev-parse', 'HEAD^'))
        self.env = {**os.environ, 'GITHUB_SHA': self.git('rev-parse', 'HEAD'),
                    'GITHUB_REF_NAME': 'epic/release-train-fixture', 'GITHUB_RUN_ID': '101',
                    'GITHUB_REPOSITORY': 'fixture/repo', 'GITHUB_RUN_ATTEMPT': '1', 'FORGE': str(self.root)}
        self.gh = self.root/'gh'
        self.gh.write_text(GH)
        self.gh.chmod(0o755)
    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.repo), *args], stderr=subprocess.PIPE, text=True).strip()

    def commit(self, text):
        (self.repo/'value').write_text(text)
        self.git('add', 'value')
        self.git('commit', '-qm', text)

    def run_cli(self, *args, expected=0):
        result = subprocess.run([sys.executable, str(SCRIPT), *map(str, args)], env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, expected, result.stdout+result.stderr)
        return json.loads(result.stdout) if result.stdout else None


    def test_same_tree_requeue_reuses_only_successful_job_receipts(self):
        # Execute the actual composite's shell, not a parallel CLI wiring.
        action=yaml.safe_load((SCRIPT.parent.parent/'.github/actions/full-gate-job/action.yml').read_text())
        body=action['runs']['steps'][0]['run']
        (self.repo/'scripts').mkdir()
        shutil.copyfile(SCRIPT,self.repo/'scripts/train-ci.py')
        self.git('add','scripts')
        self.git('commit','-qm','receipt command')
        self.env['GITHUB_SHA']=self.git('rev-parse','HEAD')
        inputs='{"selection":"all","matrix":1}'
        downloads={}; artifacts=[]; jobs=[]
        producer=self.env['GITHUB_SHA']
        def composite(phase,slot,status='success',selection=inputs,requires=''):
            output=self.root/'github-output'
            output.write_text('')
            result=subprocess.run(['bash','-e','-o','pipefail','-c',body],cwd=self.repo,
                env={**self.env,'PHASE':phase,'JOB':slot,'JOB_NAME':slot,'BINDING':selection,
                    'STATUS':status,'REQUIRES_ARTIFACT':requires,'GITHUB_OUTPUT':str(output),
                    'GITHUB_WORKSPACE':str(self.repo),'GH_BIN':str(self.gh)},capture_output=True,text=True)
            self.assertEqual(result.returncode,0,result.stdout+result.stderr)
            return dict(line.split('=',1) for line in output.read_text().splitlines())
        for slot,status in [('scripts','success'),('rust','failure')]:
            recovery=self.repo/'target/queue-flakes/step/receipt.json'
            recovery.parent.mkdir(parents=True,exist_ok=True)
            recovery.write_text(json.dumps({'schema':1,'run_id':'101','head':producer,
                'tree':self.git('rev-parse','HEAD^{tree}'),'outcome':'FLAKY','runner':'step',
                'shard':slot,'flaky':[],'attempts':[]}))
            saved=composite('save',slot,status)
            name=saved['artifact']
            shutil.copytree(self.repo/f'target/full-gate-job-{slot}',self.root/slot)
            downloads[name]=slot
            artifacts.append({'id':len(artifacts)+1,'name':name,'expired':False,
                'workflow_run':{'id':101,'head_sha':producer}})
            jobs.append({'name':slot,'conclusion':status})
        (self.root/'forge.json').write_text(json.dumps({'job_artifacts':artifacts,'job_downloads':downloads,
            'job_jobs':jobs,'job_run':{'id':101,'head_sha':producer,'event':'merge_group',
                'path':'.github/workflows/full-validation.yml','status':'completed','conclusion':'failure'}}))
        self.git('commit','--allow-empty','-qm','same tree requeue')
        self.env.update(GITHUB_SHA=self.git('rev-parse','HEAD'),GITHUB_RUN_ID='102')
        shutil.rmtree(self.repo/'target')
        def check(slot, selection=inputs,requires=''):
            return composite('restore',slot,selection=selection,requires=requires)['reused']=='true'
        self.assertTrue(check('scripts'))
        self.assertFalse(check('rust'))
        restored=json.loads((self.repo/'target/queue-flakes/reused/0/receipt.json').read_text())
        self.assertEqual((restored['run_id'],restored['observation_run_id']),('102','101'))
        # Two jobs of one run record the same script step; publication keeps both.
        for job in ('scripts','ci-tiers'):
            self.run_cli('job-save','--repo',self.repo,'--job',job,'--job-name',job,
                '--inputs',inputs,'--status','success','--out',self.repo/f'target/full-gate-job-{job}')
        exported=self.run_cli('job-export','--repo',self.repo,'--evidence',self.repo/'target',
            '--out',self.root/'exported')
        self.assertEqual(exported['observations'],2)
        published=subprocess.run([sys.executable,str(SCRIPT.parent/'ci-queue-flakes.py'),'reconcile',
            '--issues-repo','fixture/repo','--receipts',str(self.root/'exported')],
            env={**self.env,'GITHUB_EVENT_NAME':'merge_group'},capture_output=True,text=True)
        self.assertEqual(published.returncode,0,published.stdout+published.stderr)
        self.assertFalse(check('scripts','{"selection":"changed"}'))
        self.assertFalse(check('scripts',requires='expired-suite-archive'))
        forge=self.root/'forge.json'
        provider=json.loads(forge.read_text())
        provider['job_outputs']=[{'name':'suite-archive','expired':False}]
        forge.write_text(json.dumps(provider))
        self.assertTrue(check('scripts',requires='suite-archive'))
        provider['job_outputs'][0]['expired']=True
        forge.write_text(json.dumps(provider))
        self.assertFalse(check('scripts',requires='suite-archive'))
        provider['job_jobs'][0]['conclusion']='failure'
        forge.write_text(json.dumps(provider))
        self.assertFalse(check('scripts'))
        provider['job_jobs'][0]['conclusion']='success'
        provider['job_run']['event']='push'
        forge.write_text(json.dumps(provider))
        self.assertFalse(check('scripts'))
        provider['job_run']['event']='merge_group'
        provider['job_run']['path']='.github/workflows/untrusted.yml'
        forge.write_text(json.dumps(provider))
        self.assertFalse(check('scripts'))
        provider['job_run']['path']='.github/workflows/full-validation.yml'
        forge.write_text(json.dumps(provider))
        self.commit('different tree')
        self.env['GITHUB_SHA']=self.git('rev-parse','HEAD')
        self.assertFalse(check('scripts'))
        self.assertFalse(check('rust'))


if __name__ == '__main__':
    unittest.main()
