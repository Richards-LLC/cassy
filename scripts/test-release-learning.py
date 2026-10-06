#!/usr/bin/env python3
"""Receipt learning gate, durable mappings and unreleased tooling warnings."""
from pathlib import Path
import os
import shutil
import sqlite3
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
HELPER = ROOT / 'scripts/release-learning.py'


class Learning(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        self.run_dir = self.repo / 'run'
        self.run_dir.mkdir()
        self.log = self.repo / 'cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md'
        self.log.parent.mkdir(parents=True)
        self.log.write_text('- 2026-10-05 — **assemble-stale-base** — Symptom: fixture Root cause: fixture\n')
        self.env = dict(os.environ)
        self.command('git', 'init', '-q', '-b', 'main')
        self.command('git', 'config', 'user.name', 'Fixture')
        self.command('git', 'config', 'user.email', 'fixture@example.test')
        (self.repo / 'scripts').mkdir()
        shutil.copy(ROOT / 'scripts/release-gate.sh', self.repo / 'scripts/release-gate.sh')
        self.command('git', 'add', '.')
        self.command('git', 'commit', '-qm', 'fixture')

    def command(self, *args):
        return subprocess.run(args, cwd=self.repo, env=self.env, text=True,
                              stdout=subprocess.PIPE, stderr=subprocess.STDOUT)

    def check(self):
        return self.command('python3', str(HELPER), '--check', str(self.repo), str(self.run_dir))

    def test_unlearned_blocker_names_stage_and_command(self):
        (self.run_dir / 'blockers.log').write_text('assemble\n')
        result = self.check()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('assemble', result.stdout)
        self.assertIn('scripts/release-gate.sh --learn', result.stdout)
        self.assertIn('--evidence blockers.log:1', result.stdout)

    def test_existing_failure_row_mapping_passes(self):
        (self.run_dir / 'blockers.log').write_text('assemble learn=assemble-stale-base\n')
        self.assertEqual(self.check().returncode, 0)

    def test_unknown_and_manual_row_mapping_refused(self):
        for row in ['missing', 'manual:assemble']:
            (self.run_dir / 'blockers.log').write_text(f'assemble learn={row}\n')
            self.assertNotEqual(self.check().returncode, 0)

    def test_each_supervisor_intervention_requires_mapping(self):
        (self.run_dir / 'blockers.log').write_text('assemble learn=assemble-stale-base\n')
        (self.run_dir / 'supervisor-interventions.md').write_text('# Rescues\n\n- assemble: manual rebase\n')
        result = self.check()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('manual rebase', result.stdout)
        self.assertIn('supervisor-interventions.md:3', result.stdout)

    def test_open_task_accepted_closed_or_missing_refused(self):
        database = self.repo / 'tasks.db'
        with sqlite3.connect(database) as connection:
            connection.execute('CREATE TABLE tasks(id TEXT, status TEXT)')
            connection.executemany('INSERT INTO tasks VALUES (?,?)', [('cas-61dc', 'open'), ('cas-dead', 'closed')])
        self.env['CAS_RELEASE_LEARNING_TASK_DB'] = str(database)
        for identifier, expected in [('cas-61dc', 0), ('cas-dead', 1), ('cas-none', 1)]:
            (self.run_dir / 'blockers.log').write_text(f'pipeline task={identifier}\n')
            self.assertEqual(self.check().returncode, expected)

    def test_malformed_blocker_not_silently_ignored(self):
        (self.run_dir / 'blockers.log').write_text('garbage\n')
        self.assertNotEqual(self.check().returncode, 0)

    def test_clean_run_passes_without_db(self):
        self.assertEqual(self.check().returncode, 0)

    def test_receipts_commit_shortcut_still_checks_learning(self):
        (self.run_dir / 'blockers.log').write_text('assemble\n')
        (self.run_dir / 'receipts.commit').write_text('COMMIT_SHA=irrelevant\n')
        result = self.command('bash', '-c', '''
script_dir="$1/scripts"; worktree="$2"; run_dir="$2/run"
source "$script_dir/release-train.d/receipts.sh"
release_train_receipts
''', 'fixture', str(ROOT), str(self.repo))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('release-learning', result.stdout)
        self.assertIn('assemble', result.stdout)

    def test_cut_done_shortcut_still_checks_learning(self):
        (self.run_dir / 'blockers.log').write_text('publish\n')
        result = self.command('bash', '-c', '''
script_dir="$1/scripts"; worktree="$2"; run_dir="$2/run"
source "$script_dir/release-train.d/ledger.sh"
cut_stage_done() { return 0; }
cut_run_stage receipts
''', 'fixture', str(ROOT), str(self.repo))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('publish', result.stdout)

    def test_map_existing_entry_is_durable(self):
        (self.run_dir / 'blockers.log').write_text('assemble\n')
        result = self.command('python3', str(HELPER), '--map', str(self.repo), str(self.run_dir),
                              'assemble-stale-base', 'blockers.log:1')
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn('learn=assemble-stale-base', (self.run_dir / 'blockers.log').read_text())
        self.assertEqual(self.check().returncode, 0)

    def test_learn_command_records_mapping_and_rejects_invalid_reference(self):
        shutil.copy(HELPER, self.repo / 'scripts/release-learning.py')
        (self.run_dir / 'blockers.log').write_text('assemble\n')
        result = self.command('bash', 'scripts/release-gate.sh', '--learn',
                              'assemble rejected docs', 'journey docs omitted', 'assemble-stale-base',
                              '--run-dir', str(self.run_dir), '--evidence', 'blockers.log:1')
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual(self.check().returncode, 0)
        before = self.log.read_bytes()
        result = self.command('bash', 'scripts/release-gate.sh', '--learn',
                              'bad', 'bad', 'assemble-stale-base', '--run-dir', str(self.run_dir),
                              '--evidence', '../outside:1')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.log.read_bytes(), before)

    def warn(self):
        return self.command('python3', str(HELPER), '--warn-tooling', str(self.repo))

    def tooling_commit(self, text, date=None):
        path = self.repo / 'scripts/release-extra.py'
        path.write_text(text)
        self.command('git', 'add', str(path))
        env = dict(self.env)
        if date:
            self.env.update(GIT_AUTHOR_DATE=date, GIT_COMMITTER_DATE=date)
        result = self.command('git', 'commit', '-qm', 'tooling')
        self.env = env
        self.assertEqual(result.returncode, 0, result.stdout)
        return self.command('git', 'rev-parse', 'HEAD').stdout.strip()

    def live_tasks(self, rows):
        database = self.repo / 'tasks.db'
        with sqlite3.connect(database) as connection:
            connection.execute('CREATE TABLE tasks(id TEXT, status TEXT, branch TEXT, deliverables TEXT)')
            connection.executemany('INSERT INTO tasks VALUES (?,?,?,?)', rows)
        self.env['CAS_RELEASE_LEARNING_TASK_DB'] = str(database)

    def test_stale_branch_skipped_but_old_live_delivery_retained(self):
        self.command('git', 'checkout', '-qb', 'factory/dead-cas-dead')
        old = self.tooling_commit('# old tooling\n', '2020-01-01T00:00:00Z')
        self.command('git', 'branch', 'factory/parked', old)
        self.command('git', 'checkout', '-q', 'main')
        self.env.update(GIT_COMMITTER_DATE='2021-01-01T00:00:00Z')
        self.command('git', 'tag', '-am', 'release', 'v1.0.0')
        self.env.pop('GIT_COMMITTER_DATE')
        self.live_tasks([('cas-live', 'awaiting_merge', None,
                          '{"parked_branch":"factory/parked"}')])
        result = self.warn()
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertNotIn('factory/dead', result.stdout)
        self.assertIn('factory/parked', result.stdout)
        self.assertIn('1 stale', result.stdout)
        self.assertEqual(sum('skipped' in line for line in result.stdout.splitlines()), 1)

    def test_missing_task_state_retains_old_refs_and_ignores_unpublished_tag(self):
        self.command('git', 'checkout', '-qb', 'factory/uncertain')
        self.tooling_commit('# uncertain tooling\n', '2020-01-01T00:00:00Z')
        self.env.update(GIT_COMMITTER_DATE='2030-01-01T00:00:00Z')
        self.command('git', 'tag', '-am', 'not published', 'v99.0.0')
        self.env.pop('GIT_COMMITTER_DATE')
        self.command('git', 'checkout', '-q', 'main')
        self.env['CAS_RELEASE_LEARNING_TASK_DB'] = str(self.repo / 'missing.db')
        result = self.warn()
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn('factory/uncertain', result.stdout)
        self.assertIn('task state unavailable', result.stdout)
        self.assertFalse((self.repo / 'missing.db').exists())
        self.live_tasks([])
        result = self.warn()
        self.assertIn('factory/uncertain', result.stdout)
        self.assertIn('0 stale', result.stdout)

    def test_closed_task_and_lightweight_release_tag_prune_old_branch(self):
        self.command('git', 'checkout', '-qb', 'factory/closed-cas-closed')
        self.tooling_commit('# old closed tooling\n', '2020-01-01T00:00:00Z')
        self.command('git', 'checkout', '-q', 'main')
        self.command('git', 'tag', 'v1.0.0')
        self.live_tasks([('cas-closed', 'closed', 'factory/closed-cas-closed', '{}')])
        result = self.warn()
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertNotIn('off main:', result.stdout)
        self.assertIn('1 stale', result.stdout)
        self.command('git', 'branch', 'factory/already-merged', 'main')
        result = self.warn()
        self.assertIn('1 merged into main', result.stdout)

    def test_live_epic_covers_worker_and_remote_duplicates(self):
        self.command('git', 'checkout', '-qb', 'factory/worker-cas-live')
        worker = self.tooling_commit('# worker tooling\n')
        self.command('git', 'checkout', '-qb', 'epic/current-cas-epic')
        epic = self.tooling_commit('# worker + epic tooling\n')
        self.command('git', 'update-ref', 'refs/remotes/origin/epic/current-cas-epic', epic)
        self.command('git', 'checkout', '-q', 'main')
        self.live_tasks([('cas-epic', 'open', 'epic/current-cas-epic', '{}'),
                         ('cas-live', 'awaiting_merge', 'factory/worker-cas-live', '{}')])
        result = self.warn()
        self.assertEqual(result.returncode, 0, result.stdout)
        findings = [line for line in result.stdout.splitlines() if 'off main:' in line]
        self.assertEqual(len(findings), 1, result.stdout)
        self.assertIn('refs/heads/epic/current', findings[0])
        self.assertNotIn('factory/worker', result.stdout)
        self.assertIn('1 covered by live epic', result.stdout)
        self.assertIn('1 duplicate', result.stdout)

    def test_multiple_merge_bases_print_no_git_diagnostics(self):
        self.command('git', 'checkout', '-qb', 'factory/criss-cross')
        left = self.tooling_commit('# left tooling\n')
        self.command('git', 'checkout', '-q', 'main')
        (self.repo / 'main-file').write_text('right\n')
        self.command('git', 'add', '.')
        self.command('git', 'commit', '-qm', 'right')
        right = self.command('git', 'rev-parse', 'HEAD').stdout.strip()
        self.command('git', 'merge', '--no-ff', '-qm', 'merge left', left)
        self.command('git', 'checkout', '-q', 'factory/criss-cross')
        self.command('git', 'merge', '--no-ff', '-qm', 'merge right', right)
        self.tooling_commit('# after criss-cross\n')
        bases = self.command('git', 'merge-base', '--all', 'main', 'HEAD')
        self.assertEqual(len(bases.stdout.splitlines()), 2, bases.stdout)
        raw = self.command('git', 'diff', '--name-only', 'main...HEAD')
        self.assertIn('multiple merge bases', raw.stdout)
        result = self.warn()
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertNotIn('multiple merge bases', result.stdout)
        self.assertNotIn('fatal:', result.stdout)
        self.assertIn('factory/criss-cross', result.stdout)
        self.assertIn('release-extra.py', result.stdout)

    def test_off_main_tooling_branch_warns(self):
        self.command('git', 'checkout', '-qb', 'factory/parked-fix')
        (self.repo / 'scripts/release-extra.py').write_text('# changed tooling\n')
        self.command('git', 'add', '.')
        self.command('git', 'commit', '-qm', 'unreleased tooling')
        self.command('git', 'checkout', '-q', 'main')
        result = self.command('bash', '-c', '''
script_dir="$1/scripts"; worktree="$2"
source "$script_dir/release-train.d/preflight.sh"
cut_preflight_check_unreleased_tooling
''', 'fixture', str(ROOT), str(self.repo))
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn('preflight warning', result.stdout)
        self.assertIn('factory/parked-fix', result.stdout)
        self.assertIn('release-extra.py', result.stdout)
        self.command('git', 'merge', '-q', '--ff-only', 'factory/parked-fix')
        result = self.command('python3', str(HELPER), '--warn-tooling', str(self.repo))
        self.assertEqual(result.returncode, 0)
        self.assertNotIn('preflight warning', result.stdout)


if __name__ == '__main__':
    unittest.main()
