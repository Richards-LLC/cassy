#!/usr/bin/env python3
"""Exercise train receipts through their CLI, with real Git and native reports."""
import json
import os
import runpy
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile
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
        with zipfile.ZipFile(self.root/'logs.zip', 'w') as archive:
            archive.writestr('complete.log', b'entire job output\n'*10000)

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.repo), *args], stderr=subprocess.PIPE, text=True).strip()

    def commit(self, text):
        (self.repo/'value').write_text(text)
        self.git('add', 'value')
        self.git('commit', '-qm', text)

    def run_cli(self, *args, expected=0):
        result = subprocess.run([sys.executable, str(SCRIPT), *map(str, args)], env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, expected, result.stdout+result.stderr)
        return json.loads(result.stdout) if result.stdout and args[0] != 'reuse' else None

    def evidence(self, name, failed=False):
        out = self.root/name
        for slot in (1, 2, 3):
            log, junit = self.root/'native.log', self.root/'junit.xml'
            log.write_bytes(b'first line\n'+b'untruncated output\n'*20000+b'last line\n')
            bad = failed and slot == 2
            junit.write_text('<testsuites><testsuite name="fixture::lib"><testcase name="regression">'+
                             ('<failure>deterministic red</failure>' if bad else '')+'</testcase></testsuite></testsuites>')
            self.run_cli('shard', '--log', log, '--junit', junit, '--status', int(bad), '--shard', f'{slot}/3', '--out', out/str(slot))
            self.assertEqual((out/str(slot)/'native.log').read_bytes(), log.read_bytes())
        self.run_cli('collect', '--repo', self.repo, '--evidence', out,
                     '--build-status', 'success', '--shards-status', 'failure' if failed else 'success')
        jobs = {name: {'result': 'success'} for name in (
            'fast-validation-preflight', 'fast-validation-ci-tiers', 'fast-validation-web',
            'fast-validation-suite', 'fast-validation-docs', 'queue-flake-publication')}
        jobs['ci-diff'] = {'outputs': {'journeys-check-needed': 'false', 'bridge-check-needed': 'false'}}
        if failed: jobs['fast-validation-suite']['result'] = 'failure'
        full = subprocess.run([sys.executable, str(SCRIPT), 'full-collect', '--repo', str(self.repo),
                               '--evidence', str(out), '--jobs-json', json.dumps(jobs),
                               '--main-baseline', self.git('rev-parse', 'origin/main'),
                               '--started-at', '2026-01-01T00:00:00Z'], env=self.env, capture_output=True, text=True)
        self.assertEqual(full.returncode, int(failed), full.stdout+full.stderr)
        return json.loads(full.stdout)


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
            reports=self.repo/'target/train-suite/1'
            reports.mkdir(parents=True,exist_ok=True)
            (reports/'shard.json').write_text(json.dumps({'schema':1,'run_id':'101','run_attempt':1,
                'branch':self.env['GITHUB_REF_NAME'],'head':producer,'shard':'1/3','status':0,'tests':1,'failures':[]}))
            (reports/'native.log').write_text('complete original output')
            original_log=(reports/'native.log').read_bytes()
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
        self.assertEqual(json.loads((reports/'shard.json').read_text())['head'],self.env['GITHUB_SHA'])
        self.assertEqual((reports/'native.log').read_bytes(),original_log)
        self.run_cli('job-save','--repo',self.repo,'--job','scripts','--job-name','scripts',
            '--inputs',inputs,'--status','success','--out',self.repo/'target/full-gate-job-scripts')
        exported=self.run_cli('job-export','--repo',self.repo,'--evidence',self.repo/'target/full-gate-job-scripts',
            '--out',self.root/'exported')
        self.assertEqual(exported['observations'],1)
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
        provider['job_run']['path']='.github/workflows/untrusted.yml'
        forge.write_text(json.dumps(provider))
        self.assertFalse(check('scripts'))
        provider['job_run']['path']='.github/workflows/full-validation.yml'
        forge.write_text(json.dumps(provider))
        self.commit('different tree')
        self.env['GITHUB_SHA']=self.git('rev-parse','HEAD')
        self.assertFalse(check('scripts'))
        self.assertFalse(check('rust'))

    def forge(self, artifact, status='completed', conclusion='success', fanin='success'):
        (self.root/'forge.json').write_text(json.dumps({'artifact': artifact, 'fanin': fanin, 'runs': [{
            'id': int(self.env['GITHUB_RUN_ID']), 'run_attempt': 1, 'head_sha': self.env['GITHUB_SHA'],
            'head_branch': self.env['GITHUB_REF_NAME'], 'event': 'push', 'status': status, 'conclusion': conclusion}]}))

    def observe(self, check=False, expected=0):
        return self.run_cli('check' if check else 'observe', '--repo', self.repo, '--head', self.env['GITHUB_SHA'],
                            '--out', self.root/'cache', '--gh', self.gh, expected=expected)

    def test_red_wake_data_exact_cut_receipt_and_cancelled_tip(self):
        self.assertEqual(self.evidence('green')['outcome'], 'PASS')
        self.forge('green')
        green = self.observe(check=True)
        self.assertEqual(green['last_green'], self.env['GITHUB_SHA'])
        self.git('checkout', '-qb', 'factory/cas-b222')
        self.commit('cas-b222 bad change')
        self.git('checkout', '-q', self.env['GITHUB_REF_NAME'])
        self.git('merge', '--no-ff', '-qm', 'Merge reviewed delivery\n\nTask: cas-b222', 'factory/cas-b222')
        self.env.update(GITHUB_SHA=self.git('rev-parse', 'HEAD'), GITHUB_RUN_ID='102')
        self.assertEqual(self.evidence('red', failed=True)['failures'], ['fast-validation-suite'])
        self.forge('red', conclusion='failure', fanin='failure')
        red = self.observe()
        self.assertEqual(red['introducing_merge'], self.env['GITHUB_SHA'])
        self.assertEqual(red['merge_range'][0]['tasks'], ['cas-b222'])
        self.assertTrue(Path(red['complete_log']).is_file())
        self.observe(check=True, expected=1)
        # A newer cancelled tip cannot clear the durable red hold or authorize a cut.
        self.commit('cas-c333 later change')
        self.env.update(GITHUB_SHA=self.git('rev-parse', 'HEAD'), GITHUB_RUN_ID='103')
        self.forge('red', conclusion='cancelled')
        pending = self.observe(check=True, expected=1)
        self.assertEqual(pending['outcome'], 'PENDING')
        self.assertEqual(pending['red_since'], red['head'])
        self.evidence('later-red', failed=True)
        self.forge('later-red', conclusion='failure', fanin='failure')
        later = self.observe()
        self.assertIsNone(later['introducing_merge'])
        self.assertEqual({task for merge in later['merge_range'] for task in merge['tasks']}, {'cas-b222', 'cas-c333'})
        self.env['GITHUB_RUN_ID'] = '104'
        self.evidence('fixed')
        self.forge('fixed')
        fixed = self.observe(check=True)
        self.assertNotIn('red_since', fixed)
        self.assertEqual(fixed['last_green'], self.env['GITHUB_SHA'])

    def test_missing_or_foreign_shard_cannot_produce_green(self):
        self.evidence('evidence')
        out = self.root/'evidence'
        shutil.rmtree(out/'3')
        result = self.run_cli('collect', '--repo', self.repo, '--evidence', out,
                              '--build-status', 'success', '--shards-status', 'success')
        self.assertEqual(result['outcome'], 'FAIL')
        row = out/'1/shard.json'
        data = json.loads(row.read_text())
        data['head'] = '0'*40
        row.write_text(json.dumps(data))
        self.run_cli('collect', '--repo', self.repo, '--evidence', out,
                     '--build-status', 'success', '--shards-status', 'success', expected=1)

    def test_full_receipt_refuses_partial_legacy_and_wrong_tree(self):
        self.evidence('full')
        self.forge('full')
        receipt = self.root/'full/receipt.json'
        original = json.loads(receipt.read_text())
        for mutation in ('legacy', 'tree', 'partial', 'omitted'):
            with self.subTest(mutation=mutation):
                row = json.loads(json.dumps(original))
                if mutation == 'legacy': row['schema'] = 1
                if mutation == 'tree': row['tree'] = '0'*40
                if mutation == 'partial': row['jobs']['fast-validation-ci-tiers'] = 'skipped'
                if mutation == 'omitted': row['required_jobs'].remove('fast-validation-web')
                receipt.write_text(json.dumps(row))
                shutil.rmtree(self.root/'cache', ignore_errors=True)
                self.observe(check=True, expected=1)
        receipt.write_text(json.dumps(original))
        shutil.rmtree(self.root/'cache', ignore_errors=True)
        self.observe(check=True)

    def test_queue_reuses_plumbing_only_with_unchanged_main(self):
        manifest = self.repo/'cas-cli/Cargo.toml'
        manifest.parent.mkdir()
        manifest.write_text('[package]\nname="cas"\nversion="1.0.0"\n')
        self.git('add', '.')
        self.git('commit', '-qm', 'manifest baseline')
        source = self.git('rev-parse', 'HEAD')
        self.env['GITHUB_SHA'] = source
        self.evidence('full')
        self.forge('full')
        manifest.write_text('[package]\nname="cas"\nversion="1.1.0"\n')
        (self.repo/'CHANGELOG.md').write_text('Release notes')
        self.git('add', '.')
        self.git('commit', '-qm', f'release: v1.1.0\n\nRelease-Version: 1.1.0\nTrain-CI-Head: {source}')
        release_head = self.git('rev-parse', 'HEAD')
        data_path = self.root/'forge.json'
        data = json.loads(data_path.read_text())
        main = self.git('rev-parse', 'origin/main')
        data['pr'] = {'head': {'sha': release_head, 'ref': self.env['GITHUB_REF_NAME']}, 'base': {'sha': main}}
        data['statuses'] = [{'context': 'cas/full-gate', 'state': 'success',
                             'description': 'PASS tree='+self.git('rev-parse', source+'^{tree}') }]
        data_path.write_text(json.dumps(data))
        self.env.update(GITHUB_EVENT_NAME='merge_group', GITHUB_REF='refs/heads/gh-readonly-queue/main/pr-1-'+'a'*40,
                        QUEUE_BASE_SHA=main)
        output = self.root/'queue-output'
        def attempt(expected=0):
            output.write_text('run-fast-validation=true\n')
            self.run_cli('reuse', '--repo', self.repo, '--gh', self.gh,
                         '--out', self.root/'reuse-cache', '--github-output', output, expected=expected)
            return 'run-fast-validation=false' in output.read_text()
        self.assertTrue(attempt())
        # Explicit hotfixes cannot borrow a train's result, and a status alone
        # cannot substitute for a complete artifact.
        data['pr']['head']['ref'] = 'hotfix-fixture'
        data_path.write_text(json.dumps(data))
        self.assertFalse(attempt())
        data['pr']['head']['ref'] = self.env['GITHUB_REF_NAME']
        saved_statuses = data['statuses']
        data['statuses'] = []
        data_path.write_text(json.dumps(data))
        self.assertFalse(attempt())
        data['statuses'] = saved_statuses
        data['artifact'] = 'absent'
        data_path.write_text(json.dumps(data))
        shutil.rmtree(self.root/'reuse-cache')
        self.assertFalse(attempt(expected=1))
        data['artifact'] = 'full'
        data_path.write_text(json.dumps(data))
        (self.repo/'.claude').mkdir()
        (self.repo/'.claude/CODEMAP.md').write_text('Release navigation')
        self.git('add', '.')
        self.git('commit', '-qm', 'release plumbing continuation')
        data['pr']['head']['sha'] = self.git('rev-parse', 'HEAD')
        data_path.write_text(json.dumps(data))
        self.assertTrue(attempt())
        self.env['QUEUE_BASE_SHA'] = 'f'*40
        self.assertFalse(attempt())
        self.env['QUEUE_BASE_SHA'] = main
        # Renaming product input into an allowed directory still deletes source.
        plumbing_head = self.git('rev-parse', 'HEAD')
        (self.repo/'changelog.d').mkdir()
        self.git('mv', 'value', 'changelog.d/moved-value.md')
        self.git('commit', '-qm', 'move product input into release plumbing')
        data['pr']['head']['sha'] = self.git('rev-parse', 'HEAD')
        data_path.write_text(json.dumps(data))
        self.assertFalse(attempt())
        self.git('reset', '--hard', plumbing_head)
        self.commit('product change')
        data['pr']['head']['sha'] = self.git('rev-parse', 'HEAD')
        data_path.write_text(json.dumps(data))
        self.assertFalse(attempt())

    def test_queue_wall_clock_requires_the_completed_matching_tree(self):
        queue_timing = runpy.run_path(str(SCRIPT))['queue_timing']
        head, tree = self.git('rev-parse', 'HEAD'), self.git('rev-parse', 'HEAD^{tree}')
        committed = {'tree': {'sha': tree}}
        def api(endpoint, *fields):
            if endpoint.startswith('pulls?'): return []
            if endpoint.startswith('actions/artifacts?'):
                return {'artifacts': [{'id': 1, 'expired': False, 'workflow_run': {'id': 123}}]}
            if endpoint == 'actions/runs/123':
                return {'event': 'merge_group', 'status': 'completed', 'conclusion': 'success',
                        'head_sha': head, 'run_started_at': '2026-01-01T00:00:00Z',
                        'html_url': 'https://github.com/fixture/repo/actions/runs/123'}
            if endpoint == 'git/commits/'+head: return committed
            if endpoint == 'actions/runs/123/jobs':
                return {'jobs': [{'name': 'Shared / Full-set gate', 'conclusion': 'success',
                                  'completed_at': '2026-01-01T00:02:00Z'}]}
            self.fail(endpoint)
        observed = queue_timing(api, self.repo, self.env['GITHUB_REF_NAME'], head)
        self.assertEqual(observed['queue_seconds'], 120)
        self.assertEqual(observed['queue_run_id'], '123')
        committed['tree']['sha'] = '0'*40
        self.assertEqual(queue_timing(api, self.repo, self.env['GITHUB_REF_NAME'], head), {})

    def test_preparation_reuses_only_its_immediate_starting_tip(self):
        manifest = self.repo/'cas-cli/Cargo.toml'
        manifest.parent.mkdir()
        manifest.write_text('[package]\nname="cas"\nversion="1.0.0"\n')
        self.git('add', '.')
        self.git('commit', '-qm', 'manifest baseline')
        source = self.git('rev-parse', 'HEAD')

        def preparation():
            result = subprocess.run([sys.executable, str(SCRIPT), 'prepared', '--repo', str(self.repo)],
                                    capture_output=True, text=True)
            return result.returncode, result.stdout.strip()

        trailers = f'release: v1.1.0\n\nRelease-Version: 1.1.0\nTrain-CI-Head: {source}'
        # Copied release trailers on a docs commit cannot suppress the suite.
        self.git('commit', '--allow-empty', '-qm', trailers)
        self.assertEqual(preparation()[0], 1)
        self.git('reset', '--hard', source)
        manifest.write_text('[package]\nname="cas"\nversion="1.1.0"\n')
        self.git('add', '.')
        self.git('commit', '-qm', trailers)
        self.assertEqual(preparation(), (0, source))
        # A genuine version bump cannot launder an older ancestor's receipt.
        stale = self.git('rev-parse', f'{source}^')
        self.git('commit', '--amend', '-qm', trailers.replace(source, stale))
        self.assertEqual(preparation()[0], 1)


if __name__ == '__main__':
    unittest.main()
