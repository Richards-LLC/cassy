#!/usr/bin/env python3
"""Receipt/runner fixtures: no browser, server or live task-store writes."""
import importlib.util
import json
import os
import shutil
import sqlite3
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('receipt', ROOT / 'scripts/journey-receipt.py')
receipt = importlib.util.module_from_spec(spec)
spec.loader.exec_module(receipt)


def native(*tests):
    return {'suites': [{'suites': [{'specs': [
        {'title': title, 'tests': [{'status': 'expected' if statuses == ['passed'] else 'unexpected',
                                  'expectedStatus': 'passed', 'results': [{'status': s} for s in statuses]}]}
        for title, statuses in tests
    ]}]}]}


def run_plan(ids=('HUB-J7',), scope='affected'):
    return {'selection_ids': list(ids), 'scope': scope, 'head_sha': 'a' * 40, 'base_sha': 'b' * 40}


class Folding(unittest.TestCase):
    def test_native_supplemental_parts_fold_by_actual_title(self):
        out, status = receipt.fold(run_plan(), native(('HUB-J7 main', ['passed']), ('HUB-J7 supplemental part', ['passed'])), 0, '1.63')
        self.assertEqual(status, 0)
        self.assertEqual(out['results'], [{'id': 'HUB-J7', 'status': 'PASS', 'passed': 2, 'failed': 0, 'skipped': 0}])

    def test_failure_skip_and_retry_never_become_green(self):
        for states in [['failed'], ['skipped'], ['timedOut'], ['failed', 'passed']]:
            out, status = receipt.fold(run_plan(), native(('HUB-J7 part', states)), 7, '1.63')
            self.assertEqual(status, 7)
            self.assertEqual(out['suite_exit'], 7)
            self.assertEqual(out['results'][0]['status'], 'FAIL')
            self.assertEqual(out['results'][0]['skipped'], int(states == ['skipped']))
            self.assertEqual(out['results'][0]['failed'], int(states != ['skipped']))

    def test_missing_unmapped_and_runner_error_refuse(self):
        for report in [None, native(('HUB-J1 foreign', ['passed'])), {'suites': [], 'errors': ['startup']}]:
            out, status = receipt.fold(run_plan(), report, 0, '1.63')
            self.assertEqual(status, 1)
            self.assertEqual(out['suite_exit'], 0)
            self.assertEqual(out['results'][0]['passed'], 0)
            self.assertTrue(out['errors'])

    def test_explicit_empty_impact(self):
        out, status = receipt.fold(run_plan(()), None, 0, 'not run: zero impact')
        self.assertEqual((status, out['selection_ids'], out['results'], out['native_report']), (0, [], [], None))


class Tooling(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.repo = Path(self.temp.name) / 'repo'
        self.repo.mkdir()
        self.git('init', '-q', '-b', 'main')
        self.git('config', 'user.email', 'fixture@example.test')
        self.git('config', 'user.name', 'fixture')
        (self.repo / 'scripts').mkdir()
        for name in ['journey-eval.sh', 'journey-receipt.py', 'journey-bundles.py']:
            shutil.copyfile(ROOT / 'scripts' / name, self.repo / 'scripts' / name)
        # Public selector CLI seam. Deliberately fail on positional diff API,
        # arbitrary selection IDs, wrong reviewed-revision env or bad base.
        (self.repo / 'scripts/journeys-for-diff.py').write_text('''import json, os, sys
assert sys.argv[1] in ('--paths', '--all')
assert len(os.environ['CAS_JOURNEYS_HEAD']) == 40
assert len(os.environ['CAS_JOURNEYS_BASE']) == 40
rows = [{'id': 'HUB-J7', 'suite': 'hub-web/e2e/main.journey.ts'}] if '--all' in sys.argv or 'hub-web/src/ask.ts' in sys.argv else []
print(json.dumps({'journeys': rows}))
''')
        (self.repo / 'hub-web/dist').mkdir(parents=True)
        (self.repo / 'hub-web/dist/app.js').write_text('fixture bundle')
        (self.repo / 'hub-web/package.json').write_text(json.dumps({'devDependencies': {'@playwright/test': '1.63.0'}}))
        self.git('add', '.')
        self.git('commit', '-q', '-m', 'fixture base')
        self.base = self.git('rev-parse', 'HEAD')
        self.artifacts = Path(self.temp.name) / 'artifacts' / 'cas-fixture'
        self.env = dict(os.environ, CAS_ROOT=str(self.repo / '.cas'), CAS_AGENT_NAME='fixture', CAS_AGENT_ROLE='worker')
        self.env.pop('GITHUB_ACTIONS', None)
        (self.repo / '.cas').mkdir()
        db = sqlite3.connect(self.repo / '.cas/cas.db')
        db.execute('CREATE TABLE tasks (id,deliverables,notes,assignee,status)')
        db.execute('INSERT INTO tasks VALUES (?,?,?,?,?)', ('cas-fixture', json.dumps({'work_target': {'target_branch': 'main'}}), '', 'fixture', 'in_progress'))
        db.commit()
        db.close()
        (self.repo / '.git/info/exclude').write_text('.cas/\nnode_modules/\n')

    def tearDown(self):
        self.temp.cleanup()

    def git(self, *args):
        return subprocess.run(['git', *args], cwd=self.repo, check=True, capture_output=True, text=True).stdout.strip()

    def plan(self, *args):
        return subprocess.run(['python3', self.repo / 'scripts/journey-receipt.py', 'plan', '--repo', self.repo, self.artifacts, *args], env=self.env, capture_output=True, text=True)

    def test_default_target_and_explicit_base_and_worker_count(self):
        self.git('switch', '-q', '-c', 'factory/fixture')
        (self.repo / 'hub-web/src').mkdir()
        (self.repo / 'hub-web/src/ask.ts').write_text('export const ask = 1')
        self.git('add', '.')
        self.git('commit', '-q', '-m', 'fix ask (cas-fixture)')
        out = self.plan()
        self.assertEqual(out.returncode, 0, out.stderr)
        plan = json.loads(out.stdout)
        self.assertEqual((plan['base_sha'], plan['workers'], plan['selection_ids']), (self.base, 4, ['HUB-J7']))
        self.assertEqual(json.loads(self.plan('--affected', self.base).stdout)['base_sha'], self.base)
        self.assertNotEqual(self.plan('--workers=8').returncode, 0)
        self.assertNotEqual(self.plan('--grep=HUB-J1').returncode, 0)

    def test_recorded_base_survives_target_merge(self):
        head = self.git('rev-parse', 'HEAD')
        task = {'id': 'cas-fixture', 'notes': f'JOURNEY_SELECTION: head={head} base={self.base} ids=', 'deliverables': '{}'}
        self.assertEqual(receipt.task_base(self.repo, head, task), self.base)

    def test_full_worker_refused_supervisor_allowed(self):
        self.assertIn('supervisor-only', self.plan('--full').stderr)
        self.env['CAS_AGENT_ROLE'] = 'supervisor'
        out = self.plan('--full')
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(json.loads(out.stdout)['scope'], 'full')

    def test_selector_error_and_unreadable_base_fail_closed(self):
        self.assertNotEqual(self.plan('--affected', '0' * 40).returncode, 0)
        (self.repo / 'scripts/journeys-for-diff.py').write_text('raise SystemExit(7)\n')
        self.git('add', '.')
        self.git('commit', '-q', '-m', 'broken committed selector')
        self.assertNotEqual(self.plan('--affected', self.base).returncode, 0)

    def test_selector_is_bound_to_reviewed_head_across_checkout(self):
        selector = self.repo / 'scripts/journeys-for-diff.py'
        selector.write_text(selector.read_text().replace('HUB-J7', 'HUB-J12'))
        (self.repo / 'hub-web/src').mkdir()
        (self.repo / 'hub-web/src/ask.ts').write_text('export const ask = 1')
        self.git('add', '.')
        self.git('commit', '-q', '-m', 'reviewed selector and source')
        head = self.git('rev-parse', 'HEAD')
        expected = receipt.selection(self.repo, self.base, head, False)
        self.assertEqual([row['id'] for row in expected], ['HUB-J12'])
        self.git('checkout', '-q', self.base)
        self.assertEqual(receipt.selection(self.repo, self.base, head, False), expected)
        self.assertEqual(receipt.selection(self.repo, self.base, head, True), expected)
        selector.write_text('raise SystemExit(7)\n')
        self.assertEqual(receipt.selection(self.repo, self.base, head, False), expected)

    def test_missing_reviewed_selector_cannot_use_checkout_copy(self):
        self.git('rm', 'scripts/journeys-for-diff.py')
        self.git('commit', '-q', '-m', 'remove reviewed selector')
        head = self.git('rev-parse', 'HEAD')
        self.git('checkout', '-q', self.base)
        with self.assertRaises(subprocess.CalledProcessError):
            receipt.selection(self.repo, self.base, head, False)

    def test_empty_wrapper_no_browser_and_archives_previous_receipt(self):
        # No node_modules exists. Both default and explicit empty selections
        # must still write receipts; repeat evidence is archived, not erased.
        for args in [[], ['--affected', self.base]]:
            out = subprocess.run(['bash', self.repo / 'scripts/journey-eval.sh', self.artifacts, *args], env=self.env, capture_output=True, text=True)
            self.assertEqual(out.returncode, 0, out.stderr)
            self.assertIn('no browser run', out.stdout)
            value = json.loads((self.artifacts / 'journey-receipt.json').read_text())
            self.assertEqual((value['selection_ids'], value['results']), ([], []))
        self.assertEqual(len(list((self.artifacts / 'journey-runs').glob('*/journey-receipt.json'))), 1)

    def test_nonempty_wrapper_owns_filter_workers_and_preserves_failure(self):
        self.git('switch', '-q', '-c', 'factory/fixture')
        (self.repo / 'hub-web/src').mkdir()
        (self.repo / 'hub-web/src/ask.ts').write_text('ask')
        self.git('add', '.')
        self.git('commit', '-q', '-m', 'ask (cas-fixture)')
        (self.repo / 'hub-web/node_modules/@playwright/test').mkdir(parents=True)
        tools = Path(self.temp.name) / 'tools'
        tools.mkdir()
        (tools / 'node').write_text('#!/bin/sh\nprintf "1.63.0\\n"\n')
        (tools / 'npm').write_text("#!/usr/bin/env python3\n" + "import json, os, sys\n" +
            "from pathlib import Path\n" +
            "Path(os.environ['FIXTURE_ARGS']).write_text(json.dumps(sys.argv[1:]))\n" +
            "code = int(os.environ.get('FIXTURE_NATIVE_EXIT', '0'))\n" +
            "row = {'title':'HUB-J7 main','tests':[{'status':'unexpected' if code else 'expected','expectedStatus':'passed','results':[{'status':'failed' if code else 'passed'}]}]}\n" +
            "Path(os.environ['JOURNEY_OUTPUT'], 'report.json').write_text(json.dumps({'suites':[{'specs':[row]}]}))\n" +
            "sys.exit(code)\n")
        for tool in tools.iterdir():
            tool.chmod(0o755)
        self.env.update(PATH=str(tools) + os.pathsep + self.env['PATH'], FIXTURE_ARGS=str(Path(self.temp.name) / 'args.json'))
        for code in [7, 0]:
            self.env['FIXTURE_NATIVE_EXIT'] = str(code)
            out = subprocess.run(['bash', self.repo / 'scripts/journey-eval.sh', self.artifacts], env=self.env, capture_output=True, text=True)
            self.assertEqual(out.returncode, code, out.stderr)
            args = json.loads(Path(self.env['FIXTURE_ARGS']).read_text())
            self.assertIn('--workers=4', args)
            self.assertIn('--grep', args)
            self.assertRegex('HUB-J7 main', args[-1])
            value = json.loads((self.artifacts / 'journey-receipt.json').read_text())
            self.assertEqual(value['suite_exit'], code)
            self.assertEqual(value['results'][0]['failed'], int(code != 0))
        old = next((self.artifacts / 'journey-runs').glob('*/journey-receipt.json'))
        self.assertEqual(json.loads(old.read_text())['suite_exit'], 7)

    def test_writer_recomputes_selection_and_refuses_hand_picked_plan(self):
        (self.repo / 'hub-web/src').mkdir()
        (self.repo / 'hub-web/src/ask.ts').write_text('ask')
        self.git('add', '.')
        self.git('commit', '-q', '-m', 'ask (cas-fixture)')
        run = json.loads(self.plan('--affected', self.base).stdout)
        self.artifacts.mkdir(parents=True)
        plan_file = self.artifacts / 'plan.json'
        run['selection_ids'] = []
        plan_file.write_text(json.dumps(run))
        output = self.artifacts / 'forged.json'
        out = subprocess.run(['python3', self.repo / 'scripts/journey-receipt.py', 'write', '--plan', plan_file,
                              '--report', self.artifacts / 'absent.json', '--suite-exit', '0', '--tool-version', '1.63',
                              '--output', output], env=self.env, capture_output=True, text=True)
        self.assertNotEqual(out.returncode, 0)
        self.assertIn('caller IDs cannot substitute', out.stderr)
        self.assertFalse(output.exists())

    def test_no_arguments_resolve_task_and_configured_artifacts(self):
        base = Path(self.temp.name) / 'configured-artifacts'
        (self.repo / '.cas/config.toml').write_text(f'[factory]\nartifacts_root = "{base}"\n')
        out = subprocess.run(['bash', self.repo / 'scripts/journey-eval.sh'], env=self.env, capture_output=True, text=True)
        self.assertEqual(out.returncode, 0, out.stderr)
        receipts = list(base.glob('*/cas-fixture/journey-receipt.json'))
        self.assertEqual(len(receipts), 1)
        self.assertEqual(json.loads(receipts[0].read_text())['base_sha'], self.base)

    def test_wrapper_rejects_dirty_product(self):
        (self.repo / 'hub-web/dist/app.js').write_text('uncommitted')
        out = subprocess.run(['bash', self.repo / 'scripts/journey-eval.sh', self.artifacts, '--affected', self.base], env=self.env, capture_output=True, text=True)
        self.assertEqual(out.returncode, 2)
        self.assertIn('commit product', out.stderr)
        self.assertFalse((self.artifacts / 'journey-receipt.json').exists())


if __name__ == '__main__':
    unittest.main()
