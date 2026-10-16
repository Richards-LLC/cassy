#!/usr/bin/env python3
"""Exercise first-push classification against real Git epic and lane history."""
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
        output = self.repo / 'outputs'
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

    def test_repair_push_uses_only_previous_push_delta(self):
        previous = self.git('rev-parse', 'HEAD')
        self.write('hub-web/src/conversation-view.test.ts', '// assertion repair\n')
        self.commit('test-only repair')
        out = self.classify(previous)
        self.assertEqual(out['class'], 'hub-web-only')
        self.assertEqual(out['changed-files'], '1')

    def test_missing_base_fails_closed(self):
        self.assertEqual(self.classify(before='1' * 40)['rust-unaffected'], 'false')

    def test_untrusted_fallback_fails_closed(self):
        self.assertEqual(self.classify(fallback='HEAD')['rust-unaffected'], 'false')

    def test_tag_creation_does_not_use_epic(self):
        self.assertEqual(self.classify(ref='refs/tags/v0.0.0')['rust-unaffected'], 'false')

    def test_missing_default_fails_closed(self):
        self.assertEqual(self.classify(fallback='origin/missing')['rust-unaffected'], 'false')

    def test_no_shared_epic_keeps_default_branch_coverage(self):
        self.git('update-ref', '-d', 'refs/remotes/origin/epic/approved')
        self.assertEqual(self.classify()['rust-unaffected'], 'false')

    def test_nonfactory_first_push_does_not_narrow_to_epic(self):
        self.assertEqual(self.classify(ref='refs/heads/epic/release')['rust-unaffected'], 'false')

    def test_equal_epic_aliases_do_not_create_false_ambiguity(self):
        self.git('update-ref', 'refs/remotes/origin/epic/alias', self.epic)
        self.assertEqual(self.classify()['class'], 'hub-web-only')

    def test_most_recent_ancestor_epic_wins(self):
        self.git('update-ref', 'refs/remotes/origin/epic/older', self.main)
        self.assertEqual(self.classify()['class'], 'hub-web-only')

    def test_unrelated_epic_is_not_a_parent(self):
        self.git('checkout', '-qb', 'other', self.main)
        self.write('other.rs', '// other epic\n')
        self.commit('other')
        self.git('update-ref', 'refs/remotes/origin/epic/unrelated', 'HEAD')
        self.git('checkout', '-q', 'main')
        self.assertEqual(self.classify()['class'], 'hub-web-only')

    def test_incomparable_parent_epics_fail_closed(self):
        lane = self.git('rev-parse', 'HEAD')
        self.git('checkout', '-qb', 'other', self.main)
        self.write('other.rs', '// other epic\n')
        self.commit('other')
        self.git('update-ref', 'refs/remotes/origin/epic/other', 'HEAD')
        self.git('checkout', '-q', lane)
        self.git('merge', '--no-edit', 'other')
        self.assertEqual(self.classify()['rust-unaffected'], 'false')


if __name__ == '__main__':
    unittest.main()
