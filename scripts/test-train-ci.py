#!/usr/bin/env python3
"""Exercise train receipts through their CLI, with real Git and native reports."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile

SCRIPT = Path(__file__).with_name('train-ci.py').resolve()
GH = '''#!/usr/bin/env python3
import json, os, pathlib, shutil, sys
root=pathlib.Path(os.environ['FORGE']); args=sys.argv[1:]
data=json.loads((root/'forge.json').read_text())
if args[:2]==['repo','view']: print('fixture/repo')
elif args[:2]==['run','download']:
 shutil.copytree(root/data['artifact'],pathlib.Path(args[args.index('--dir')+1]),dirs_exist_ok=True)
elif args[0]=='api':
 endpoint=next(a for a in args if a.startswith('repos/'))
 if endpoint.endswith('/logs'): sys.stdout.buffer.write((root/'logs.zip').read_bytes())
 elif endpoint.endswith('/jobs'): print(json.dumps({'jobs':[{'name':'Fast Validation — full suite','conclusion':data['fanin']}]}))
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
        return json.loads(result.stdout) if result.stdout else None

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
        return self.run_cli('collect', '--repo', self.repo, '--evidence', out,
                            '--build-status', 'success', '--shards-status', 'failure' if failed else 'success')

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
        self.assertEqual(self.evidence('red', failed=True)['failures'], ['fixture::lib::regression'])
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
