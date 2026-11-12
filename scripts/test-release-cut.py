#!/usr/bin/env python3
"""Real Git/process proof of the shared manual/automatic lease boundary."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

OWNER = Path(__file__).with_name('release-cut.py')


class CutOwnerTest(unittest.TestCase):
    def test_held_branch_and_competing_owner(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            def git(*args):
                return subprocess.check_output(['git', '-C', str(repo), '-c', 'user.name=Fixture',
                    '-c', 'user.email=fixture@example.test', *args], text=True).strip()
            git('init', '-q', '-b', 'train')
            (repo / 'scripts').mkdir()
            (repo / 'draft.md').write_text('notes\n')
            (repo / 'scripts/release.sh').write_text('''printf 'READY\\n'
read -r finish
git rm --quiet -- "$2" || exit
python3 -c 'import json,os,subprocess; json.dump([subprocess.check_output(["git","rev-parse","HEAD"],text=True).strip(),subprocess.run(["git","symbolic-ref","--quiet","HEAD"]).returncode,os.environ["CAS_RELEASE_CUT_BRANCH"]],open(os.environ["CUT_CAPTURE"],"w"))'
''')
            git('add', '.')
            git('commit', '-qm', 'cut fixture')
            tip = git('rev-parse', 'HEAD')
            command = [sys.executable, str(OWNER), '--repo', str(repo), '--branch', 'train', '--tip', tip, '--', '--notes', str(repo / 'draft.md')]
            capture = repo / 'result.json'
            first = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                stderr=subprocess.PIPE, text=True, env=dict(os.environ, CUT_CAPTURE=str(capture)))
            try:
                self.assertEqual(first.stdout.readline().strip(), 'READY')
                second = subprocess.run(command, capture_output=True, text=True, timeout=10)
                self.assertEqual(second.returncode, 71)
                first.communicate('finish\n', timeout=10)
                self.assertEqual(first.returncode, 0)
            finally:
                if first.poll() is None:
                    first.kill()
                    first.wait()
            self.assertEqual(json.loads(capture.read_text()), [tip, 1, 'train'])
            self.assertEqual(git('branch', '--show-current'), 'train')
            self.assertEqual(len(git('worktree', 'list', '--porcelain').split('worktree ')) - 1, 1)


if __name__ == '__main__':
    unittest.main()
