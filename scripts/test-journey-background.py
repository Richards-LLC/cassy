#!/usr/bin/env python3
"""Background journey evaluation scheduling (cas-bb5e); no browser runs."""
import importlib.util
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('background', ROOT / 'scripts/journey-background.py')
bg = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bg)

HOUR = 3600
INPUTS = {'hub_web': 'h' * 40, 'catalog': 'c' * 40, 'runner': 'r' * 64}


def fresh():
    return bg.empty_state()


class Offer(unittest.TestCase):
    def test_changed_tree_on_green_integration_is_pending(self):
        state, outcome = bg.offer(fresh(), 'tip1', 'tree1', INPUTS, green=True, now=100)
        self.assertEqual(outcome, 'pending')
        self.assertEqual(state['pending']['tree'], 'tree1')
        self.assertEqual(state['pending']['tip'], 'tip1')

    def test_red_integration_offers_nothing(self):
        state, outcome = bg.offer(fresh(), 'tip1', 'tree1', INPUTS, green=False, now=100)
        self.assertEqual(outcome, 'skip: integration not green')
        self.assertIsNone(state['pending'])

    def test_unchanged_tree_is_not_evaluated_again(self):
        state = fresh()
        state['receipts']['tree1'] = {'status': 'PASS', 'tip': 'tip0', 'inputs': INPUTS}
        state, outcome = bg.offer(state, 'tip1', 'tree1', INPUTS, green=True, now=100)
        self.assertEqual(outcome, 'skip: dist tree already evaluated')
        self.assertIsNone(state['pending'])
        # The tree being evaluated right now is not queued twice either.
        state['running'] = {'tree': 'tree2', 'tip': 'tip2', 'pid': 1, 'started_epoch': 50}
        state, outcome = bg.offer(state, 'tip3', 'tree2', INPUTS, green=True, now=100)
        self.assertEqual(outcome, 'skip: dist tree is being evaluated')
        self.assertIsNone(state['pending'])

    def test_newer_tree_replaces_older_pending_one(self):
        state, _ = bg.offer(fresh(), 'tip1', 'tree1', INPUTS, green=True, now=100)
        state, _ = bg.offer(state, 'tip2', 'tree2', INPUTS, green=True, now=200)
        state, _ = bg.offer(state, 'tip3', 'tree3', INPUTS, green=True, now=300)
        self.assertEqual(state['pending']['tree'], 'tree3')
        self.assertEqual(state['pending']['skipped'], ['tree1', 'tree2'])


class Decide(unittest.TestCase):
    def pending(self, now=1000):
        state, _ = bg.offer(fresh(), 'tip1', 'tree1', INPUTS, green=True, now=now)
        return state

    def test_idle_host_starts_the_pending_tree(self):
        action, detail = bg.decide(self.pending(), now=10 * HOUR, host_idle=True,
                                   min_interval=1800, alive=lambda pid: False)
        self.assertEqual((action, detail['tree'], detail['tip']), ('start', 'tree1', 'tip1'))

    def test_busy_host_waits(self):
        action, reason = bg.decide(self.pending(), now=10 * HOUR, host_idle=False,
                                   min_interval=1800, alive=lambda pid: False)
        self.assertEqual((action, reason), ('wait', 'host busy'))

    def test_rate_cap_waits_until_the_interval_passes(self):
        state = self.pending()
        state['last_started_epoch'] = 10 * HOUR - 600
        action, reason = bg.decide(state, now=10 * HOUR, host_idle=True,
                                   min_interval=1800, alive=lambda pid: False)
        self.assertEqual(action, 'wait')
        self.assertTrue(reason.startswith('rate cap'), reason)
        action, _ = bg.decide(state, now=10 * HOUR + 1200, host_idle=True,
                              min_interval=1800, alive=lambda pid: False)
        self.assertEqual(action, 'start')

    def test_a_running_evaluation_is_never_cancelled_or_doubled(self):
        state = self.pending()
        state['running'] = {'tree': 'tree0', 'tip': 'tip0', 'pid': 42, 'started_epoch': 10 * HOUR - 60}
        action, reason = bg.decide(state, now=10 * HOUR, host_idle=True,
                                   min_interval=0, alive=lambda pid: pid == 42)
        self.assertEqual((action, reason), ('wait', 'evaluation running'))
        self.assertEqual(state['running']['tree'], 'tree0')

    def test_after_a_run_finishes_only_the_newest_pending_tree_starts(self):
        state = fresh()
        state['running'] = {'tree': 'tree0', 'tip': 'tip0', 'pid': 42, 'started_epoch': 0}
        for index, tree in enumerate(('tree1', 'tree2', 'tree3'), start=1):
            state, _ = bg.offer(state, f'tip{index}', tree, INPUTS, green=True, now=100 * index)
        state = bg.record_finish(state, 'tree0', 'PASS', 'tip0', '/artifacts/tree0', INPUTS, 'ok', now=760)
        self.assertIsNone(state['running'])
        self.assertEqual(state['receipts']['tree0']['status'], 'PASS')
        action, detail = bg.decide(state, now=HOUR, host_idle=True, min_interval=1800,
                                   alive=lambda pid: False)
        self.assertEqual((action, detail['tree']), ('start', 'tree3'))

    def test_dead_runner_without_a_result_is_recorded_failed(self):
        state = fresh()
        state['running'] = {'tree': 'tree0', 'tip': 'tip0', 'pid': 42, 'started_epoch': 0,
                            'artifacts': '/a', 'inputs': INPUTS}
        state = bg.reap(state, alive=lambda pid: False, now=900)
        self.assertIsNone(state['running'])
        self.assertEqual(state['receipts']['tree0']['status'], 'FAIL')
        self.assertIn('without a result', state['receipts']['tree0']['detail'])

    def test_nothing_pending_waits(self):
        action, reason = bg.decide(fresh(), now=HOUR, host_idle=True, min_interval=0,
                                   alive=lambda pid: False)
        self.assertEqual((action, reason), ('wait', 'nothing pending'))


class Reuse(unittest.TestCase):
    def test_lookup_needs_a_pass_for_the_same_tree_and_inputs(self):
        state = bg.record_finish(fresh(), 'tree1', 'PASS', 'tip1', '/artifacts/t1', INPUTS, 'ok', now=1)
        self.assertEqual(bg.lookup(state, 'tree1', INPUTS)['artifacts'], '/artifacts/t1')
        self.assertIsNone(bg.lookup(state, 'tree2', INPUTS), 'different dist tree')
        other = dict(INPUTS, catalog='d' * 40)
        self.assertIsNone(bg.lookup(state, 'tree1', other), 'catalog changed since the run')
        failed = bg.record_finish(fresh(), 'tree1', 'FAIL', 'tip1', '/artifacts/t1', INPUTS, 'x', now=1)
        self.assertIsNone(bg.lookup(failed, 'tree1', INPUTS))


def git(repo, *args):
    return subprocess.run(['git', '-C', str(repo), '-c', 'user.name=t', '-c', 'user.email=t@example.invalid',
                           '-c', 'commit.gpgsign=false', *args], check=True, capture_output=True,
                          text=True).stdout.strip()


class Fixture(unittest.TestCase):
    """The real CLI against a git fixture: offer, state on disk, reuse copy."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name) / 'repo'
        (self.repo / 'hub-web/dist').mkdir(parents=True)
        (self.repo / 'docs/qa').mkdir(parents=True)
        (self.repo / 'scripts').mkdir()
        (self.repo / 'hub-web/dist/app.js').write_text('one\n')
        (self.repo / 'docs/qa/journeys.md').write_text('catalog\n')
        for name in bg.RUNNER_INPUTS:
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(f'{name}\n')
        (self.repo / '.gitignore').write_text('.cas/\n')
        git(self.repo, 'init', '-q', '-b', 'main')
        git(self.repo, 'add', '.')
        git(self.repo, 'commit', '-qm', 'base')
        self.tip = git(self.repo, 'rev-parse', 'HEAD')

    def cli(self, *args, env=None):
        return subprocess.run(['python3', str(ROOT / 'scripts/journey-background.py'), *args],
                              capture_output=True, text=True, env=dict(os.environ, **(env or {})))

    def test_offer_and_reuse_through_the_cli(self):
        result = self.cli('offer', '--repo', str(self.repo), '--tip', self.tip, '--green')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('pending', result.stdout)
        state = json.loads((self.repo / '.cas/merge-sweeps/journey-background.json').read_text())
        tree = git(self.repo, 'rev-parse', 'HEAD:hub-web/dist')
        self.assertEqual(state['pending']['tree'], tree)

        # A finished background run for this tree and these inputs.
        source = Path(self.temp.name) / 'bg-artifacts'
        (source / 'journeys').mkdir(parents=True)
        (source / 'journey-receipt.json').write_text(json.dumps({'scope': 'full', 'suite_exit': 0}))
        inputs = bg.inputs(self.repo, 'HEAD')
        state = bg.record_finish(bg.load(self.repo), tree, 'PASS', self.tip, str(source), inputs, 'ok', now=1)
        bg.save(self.repo, state)

        # A release commit that keeps the dist tree reuses it.
        (self.repo / 'CHANGELOG.md').write_text('release\n')
        git(self.repo, 'add', '.')
        git(self.repo, 'commit', '-qm', 'release prep')
        target = Path(self.temp.name) / 'cut-artifacts'
        result = self.cli('reuse', '--repo', str(self.repo), '--artifacts', str(target))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('reused background evaluation', result.stdout)
        self.assertTrue((target / 'journey-receipt.json').is_file())
        marker = json.loads((target / 'journey-reuse.json').read_text())
        self.assertEqual((marker['tree'], marker['tip']), (tree, self.tip))

        # A changed dist tree does not.
        (self.repo / 'hub-web/dist/app.js').write_text('two\n')
        git(self.repo, 'add', '.')
        git(self.repo, 'commit', '-qm', 'new bundle')
        result = self.cli('reuse', '--repo', str(self.repo), '--artifacts', str(Path(self.temp.name) / 'x'))
        self.assertEqual(result.returncode, 1)
        self.assertIn('no background evaluation', result.stdout)
        # And JOURNEY_EVAL_FRESH forces a fresh run even on a match.
        git(self.repo, 'reset', '-q', '--hard', 'HEAD~1')
        result = self.cli('reuse', '--repo', str(self.repo), '--artifacts', str(Path(self.temp.name) / 'y'),
                          env={'JOURNEY_EVAL_FRESH': '1'})
        self.assertEqual(result.returncode, 1)

    def test_red_integration_through_the_cli_queues_nothing(self):
        result = self.cli('offer', '--repo', str(self.repo), '--tip', self.tip)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('integration not green', result.stdout)
        self.assertIsNone(bg.load(self.repo)['pending'])


if __name__ == '__main__':
    unittest.main()
