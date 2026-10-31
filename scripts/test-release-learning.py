#!/usr/bin/env python3
"""Receipt learning gate, durable mappings and unreleased tooling warnings."""
from pathlib import Path
import json
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
        self.log.write_text('- 2026-10-05 — **release-script** — Symptom: fixture Root cause: fixture\n')
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
        (self.run_dir / 'blockers.log').write_text('assemble learn=release-script\n')
        self.assertEqual(self.check().returncode, 0)

    def test_unknown_and_manual_row_mapping_refused(self):
        for row in ['missing', 'manual:assemble']:
            (self.run_dir / 'blockers.log').write_text(f'assemble learn={row}\n')
            self.assertNotEqual(self.check().returncode, 0)

    def test_each_supervisor_intervention_requires_mapping(self):
        (self.run_dir / 'blockers.log').write_text('assemble learn=release-script\n')
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


    def test_map_existing_entry_is_durable(self):
        (self.run_dir / 'blockers.log').write_text('assemble\n')
        result = self.command('python3', str(HELPER), '--map', str(self.repo), str(self.run_dir),
                              'release-script', 'blockers.log:1')
        self.assertEqual(result.returncode, 0, result.stdout)
        # pin: the saved learn=<gate-id> field is the durable blocker-mapping wire format.
        self.assertIn('learn=release-script', (self.run_dir / 'blockers.log').read_text())
        self.assertEqual(self.check().returncode, 0)

    def test_learn_command_records_mapping_and_rejects_invalid_reference(self):
        shutil.copy(HELPER, self.repo / 'scripts/release-learning.py')
        (self.run_dir / 'blockers.log').write_text('assemble\n')
        result = self.command('bash', 'scripts/release-gate.sh', '--learn',
                              'assemble rejected docs', 'journey docs omitted', 'release-script',
                              '--run-dir', str(self.run_dir), '--evidence', 'blockers.log:1')
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual(self.check().returncode, 0)
        before = self.log.read_bytes()
        result = self.command('bash', 'scripts/release-gate.sh', '--learn',
                              'bad', 'bad', 'release-script', '--run-dir', str(self.run_dir),
                              '--evidence', '../outside:1')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.log.read_bytes(), before)

    def test_learn_task_maps_an_open_task_and_refuses_a_closed_one(self):
        """cas-297d: one supported command maps a blocker to an open task."""
        shutil.copy(HELPER, self.repo / 'scripts/release-learning.py')
        database = self.repo / 'tasks.db'
        with sqlite3.connect(database) as connection:
            connection.execute('CREATE TABLE tasks(id TEXT, status TEXT)')
            connection.executemany('INSERT INTO tasks VALUES (?,?)', [('cas-61dc', 'open'), ('cas-dead', 'closed')])
        self.env['CAS_RELEASE_LEARNING_TASK_DB'] = str(database)
        (self.run_dir / 'blockers.log').write_text('preflight\n')
        self.assertIn('--learn-task', self.check().stdout)

        def learn_task(identifier):
            return self.command('bash', 'scripts/release-gate.sh', '--learn-task', identifier,
                                '--run-dir', str(self.run_dir), '--evidence', 'blockers.log:1')

        result = learn_task('cas-dead')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('cas-dead is not an open task', result.stdout)
        self.assertEqual((self.run_dir / 'blockers.log').read_text(), 'preflight\n')
        self.assertNotEqual(self.check().returncode, 0)

        result = learn_task('cas-61dc')
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual((self.run_dir / 'blockers.log').read_text(), 'preflight task=cas-61dc\n')
        self.assertEqual(self.check().returncode, 0)

    # cas-9734: a lesson learned twice is not sticking. A third learn= mapping
    # or failure-log entry needs an open P0/P1 task that removes the cause.
    def recurring_log(self):
        self.log.write_text(
            '- 2026-10-01 — **release-script** — Symptom: first Root cause: fixture\n'
            '- 2026-10-05 — **release-script** — Symptom: second Root cause: fixture\n')

    def cause_tasks(self):
        database = self.repo / 'tasks.db'
        with sqlite3.connect(database) as connection:
            connection.execute('CREATE TABLE tasks(id TEXT, status TEXT, priority INTEGER)')
            connection.executemany('INSERT INTO tasks VALUES (?,?,?)', [
                ('cas-p1', 'open', 1), ('cas-p0', 'in_progress', 0), ('cas-p3', 'open', 3),
                ('cas-done', 'closed', 1)])
        self.env['CAS_RELEASE_LEARNING_TASK_DB'] = str(database)

    def test_recurring_lesson_mapping_needs_an_open_p1_cause_task(self):
        self.recurring_log()
        self.cause_tasks()
        (self.run_dir / 'blockers.log').write_text('assemble learn=release-script\n')
        result = self.check()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn('release-script has been learned 2 times', result.stdout)
        self.assertIn('--task "${RELEASE_CAUSE_TASK:?', result.stdout)
        for task, expected in [('cas-p3', 1), ('cas-done', 1), ('cas-none', 1), ('cas-p1', 0), ('cas-p0', 0)]:
            (self.run_dir / 'blockers.log').write_text(f'assemble learn=release-script task={task}\n')
            self.assertEqual(self.check().returncode, expected, (task, self.check().stdout))
        # The row's own --learn entry is not a prior learning; a forged marker gains one, not a pass.
        (self.run_dir / 'blockers.log').write_text('assemble learn=release-script recurrence=2\n')
        self.assertEqual(self.check().returncode, 0)
        self.log.write_text(self.log.read_text() + '- 2026-10-09 — **release-script** — Symptom: third Root cause: fixture\n')
        self.assertNotEqual(self.check().returncode, 0)
        # A first-time lesson still maps by itself.
        self.log.write_text('- 2026-10-01 — **release-script** — Symptom: once Root cause: fixture\n')
        (self.run_dir / 'blockers.log').write_text('assemble learn=release-script\n')
        self.assertEqual(self.check().returncode, 0)

    def test_learn_task_on_a_recurring_lesson_needs_the_p1_cause_task(self):
        """cas-297d with cas-9734: --learn-task gives escalation its task=."""
        shutil.copy(HELPER, self.repo / 'scripts/release-learning.py')
        self.recurring_log()
        self.cause_tasks()
        (self.run_dir / 'blockers.log').write_text('assemble learn=release-script\n')
        learn_task = lambda task: self.command(  # noqa: E731
            'bash', 'scripts/release-gate.sh', '--learn-task', task,
            '--run-dir', str(self.run_dir), '--evidence', 'blockers.log:1')
        result = learn_task('cas-p3')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('cas-p3 is not an open P0/P1 task', result.stdout)
        self.assertNotEqual(self.check().returncode, 0)
        self.assertEqual(learn_task('cas-p1').returncode, 0)
        self.assertEqual((self.run_dir / 'blockers.log').read_text(),
                         'assemble learn=release-script task=cas-p1\n')
        self.assertEqual(self.check().returncode, 0)

    def test_learn_refuses_a_third_entry_without_cause_task(self):
        shutil.copy(HELPER, self.repo / 'scripts/release-learning.py')
        self.recurring_log()
        self.cause_tasks()
        (self.run_dir / 'blockers.log').write_text('assemble\n')
        before = self.log.read_bytes()
        for extra in ([], ['--task', 'cas-p3']):
            result = self.command('bash', 'scripts/release-gate.sh', '--learn', 'stale base again',
                                  'rebase skipped', 'release-script', *extra,
                                  '--run-dir', str(self.run_dir), '--evidence', 'blockers.log:1')
            self.assertNotEqual(result.returncode, 0, result.stdout)
            self.assertIn('has been learned 2 times', result.stdout)
            self.assertEqual(self.log.read_bytes(), before)
            self.assertEqual((self.run_dir / 'blockers.log').read_text(), 'assemble\n')
        result = self.command('bash', 'scripts/release-gate.sh', '--learn', 'stale base again',
                              'rebase skipped', 'release-script', '--task', 'cas-p1',
                              '--run-dir', str(self.run_dir), '--evidence', 'blockers.log:1')
        self.assertEqual(result.returncode, 0, result.stdout)
        entry = self.log.read_text().splitlines()[-1]
        self.assertIn('Symptom: stale base again Root cause: rebase skipped Recurrence: 3. Rung: gate. '
                      'Task: cas-p1. Release: operator-reported.', entry)
        self.assertEqual((self.run_dir / 'blockers.log').read_text(),
                         'assemble learn=release-script recurrence=3 task=cas-p1\n')
        self.assertEqual(self.check().returncode, 0)

    def test_learn_records_recurrence_and_rung(self):
        shutil.copy(HELPER, self.repo / 'scripts/release-learning.py')
        result = self.command('bash', 'scripts/release-gate.sh', '--learn', 'literal version',
                              'no env macro', 'version-literals', '--rung', 'preflight')
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn('Recurrence: 1. Rung: preflight. Release:', self.log.read_text())
        result = self.command('bash', 'scripts/release-gate.sh', '--learn', 'slack route',
                              'assumed', 'manual:slack-route')
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn('**manual:slack-route** — Symptom: slack route Root cause: assumed '
                      'Recurrence: 1. Rung: prose.', self.log.read_text())
        before = self.log.read_bytes()
        for args in (['manual:slack-route', '--rung', 'gate'], ['version-literals', '--rung', 'memo']):
            result = self.command('bash', 'scripts/release-gate.sh', '--learn', 'x', 'y', *args)
            self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertEqual(self.log.read_bytes(), before)

    def test_receipt_measures_rescues_that_repeat_a_learned_lesson(self):
        # The recorded 3.51.0 run: two rescues mapped to existing lessons, one to a task.
        self.log.write_text(
            '- 2026-09-02 — **version-literals** — Symptom: literal Root cause: no env!\n'
            '- 2026-10-10 — **scratch-base** — Symptom: tmpfs Root cause: base\n'
            '- 2026-10-14 — **release-script** — Symptom: new Root cause: new\n')
        (self.run_dir / 'blockers.log').write_text(
            'preflight learn=scratch-base\ngate learn=version-literals\nreceipts task=cas-1543\n'
            'assemble learn=release-script recurrence=1\n')
        run_dir = self.repo / 'v9.98.0-release-9.98.0'
        self.run_dir.rename(run_dir)
        self.command('python3', str(HELPER), '--measure', str(self.repo), str(run_dir))
        self.assertEqual(json.loads((run_dir / 'lesson-recurrence.json').read_text()), {
            'version': '9.98.0', 'rescues': 4, 'recurrences': 2, 'rate': 0.5,
            'classes': ['scratch-base', 'version-literals']})

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


if __name__ == '__main__':
    unittest.main()
