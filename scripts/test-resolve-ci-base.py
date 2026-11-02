#!/usr/bin/env python3
"""Exercise delivery-range classification against real Git epic and lane history."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[1]
ACTION = ROOT / '.github/actions/classify-required-diff/action.yml'


class LaneBaseTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        self.git('init', '-q', '-b', 'main')
        self.git('config', 'user.email', 'fixture@example.test')
        self.git('config', 'user.name', 'Fixture')
        # No detached background git (auto maintenance, fsmonitor) may write
        # into .git while TemporaryDirectory cleanup removes it.
        self.git('config', 'maintenance.auto', 'false')
        self.git('config', 'core.fsmonitor', 'false')
        for name in ('classify-ci-diff.sh', 'classify-fast-admission.sh', 'classify-web-inputs.py',
                     'resolve-ci-base.sh'):
            source = ROOT / 'scripts' / name
            destination = self.repo / 'scripts' / name
            destination.parent.mkdir(exist_ok=True)
            shutil.copy2(source, destination)
        self.write('Cargo.lock', 'base\n')
        self.commit('main')
        self.main = self.git('rev-parse', 'HEAD')
        self.git('update-ref', 'refs/remotes/origin/main', self.main)
        self.write('Cargo.lock', 'epic dependency\n')
        self.write('cas-cli/src/lib.rs', '// epic Rust change\n')
        self.commit('unreleased epic Rust')
        self.epic = self.git('rev-parse', 'HEAD')
        self.git('update-ref', 'refs/remotes/origin/epic/approved', self.epic)
        self.write('hub-web/src/conversation-view.ts', 'export const fixed = true;\n')
        self.commit('web-only task')

    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.repo, text=True).strip()

    def write(self, path, text):
        file = self.repo / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text)

    def commit(self, message):
        self.git('add', '.')
        self.git('commit', '-qm', message)

    def classify(self, before='0' * 40, fallback='origin/main', ref='refs/heads/factory/worker'):
        output = self.repo / '.git' / 'classification-outputs'
        output.write_text('')
        code = yaml.safe_load(ACTION.read_text())['runs']['steps'][0]['run']
        env = dict(os.environ, BASE_SHA=before, ZERO_BASE_REF=fallback,
                   GITHUB_EVENT_NAME='push', GITHUB_REF=ref, GITHUB_OUTPUT=str(output))
        result = subprocess.run(['bash', '-e', '-c', code], cwd=self.repo, env=env,
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return dict(line.split('=', 1) for line in output.read_text().splitlines())

    def test_first_web_push_excludes_unreleased_epic_rust(self):
        out = self.classify()
        self.assertEqual(out['class'], 'hub-web-only')
        self.assertEqual(out['rust-unaffected'], 'true')
        self.assertEqual(out['changed-files'], '1')

    def test_docs_followup_push_preserves_delivery_code_coverage(self):
        for path, text, output, expected_class in [
            ('cas-cli/src/lib.rs', '// worker Rust change\n', 'rust-unaffected', 'rust-touched'),
            ('hub-web/src/conversation-view.ts', 'export const fixed = true;\n', 'web-check-needed', 'hub-web-only'),
            ('slack-bridge/src/index.ts', 'export const fixed = true;\n', 'bridge-check-needed', 'slack-bridge-only'),
        ]:
            with self.subTest(path=path):
                self.git('reset', '--hard', self.epic)
                self.write(path, text)
                self.commit('worker code push')
                previous = self.git('rev-parse', 'HEAD')
                self.write('docs/task.md', '# Delivery notes\n')
                self.commit('docs followup push')
                out = self.classify(previous)
                self.assertEqual(out['comparison-base'], self.epic)
                self.assertEqual(out['class'], expected_class)
                self.assertEqual(out[output], 'false' if output == 'rust-unaffected' else 'true')
                self.assertEqual(out['changed-files'], '2')
                # Non-factory pushes still validate only their event delta.
                direct = self.classify(previous, ref='refs/heads/main')
                self.assertEqual(direct['class'], 'docs-only')
                self.assertEqual(direct['changed-files'], '1')

    def test_unprovable_bases_fail_closed(self):
        for name, kwargs in {
            'missing base': {'before': '1' * 40},
            'untrusted fallback': {'fallback': 'HEAD'},
            'missing default': {'fallback': 'origin/missing'},
            'tag creation': {'ref': 'refs/tags/v0.0.0'},
            'non-factory first push': {'ref': 'refs/heads/epic/release'},
        }.items():
            with self.subTest(name):
                self.assertEqual(self.classify(**kwargs)['rust-unaffected'], 'false')

    def test_no_shared_epic_keeps_default_branch_coverage(self):
        self.git('update-ref', '-d', 'refs/remotes/origin/epic/approved')
        self.assertEqual(self.classify()['rust-unaffected'], 'false')

    def test_equal_aliases_and_older_ancestor_epics_keep_the_newest_fork(self):
        self.git('update-ref', 'refs/remotes/origin/epic/alias', self.epic)
        self.git('update-ref', 'refs/remotes/origin/epic/older', self.main)
        self.assertEqual(self.classify()['class'], 'hub-web-only')

    def test_epic_advancing_after_spawn_preserves_task_fork(self):
        lane = self.git('rev-parse', 'HEAD')
        self.git('checkout', '-qb', 'epic-progress', self.epic)
        self.write('cas-cli/src/other.rs', '// sibling merged while worker runs\n')
        self.commit('epic advances after spawn')
        self.git('update-ref', 'refs/remotes/origin/epic/approved', 'HEAD')
        self.git('checkout', '-q', lane)
        out = self.classify()
        self.assertEqual(out['class'], 'hub-web-only')
        self.assertEqual(out['comparison-base'], self.epic)

    def other_epic(self, name):
        back = self.git('rev-parse', 'HEAD')
        self.git('checkout', '-qb', name, self.main)
        self.write('other.rs', '// other epic\n')
        self.commit('other')
        self.git('update-ref', f'refs/remotes/origin/epic/{name}', 'HEAD')
        self.git('checkout', '-q', back)

    def test_unrelated_epic_is_not_a_parent(self):
        self.other_epic('unrelated')
        self.assertEqual(self.classify()['class'], 'hub-web-only')

    def test_incomparable_parent_epics_fail_closed(self):
        self.other_epic('other')
        self.git('merge', '--no-edit', 'other')
        self.assertEqual(self.classify()['rust-unaffected'], 'false')


if __name__ == '__main__':
    unittest.main()
