#!/usr/bin/env python3
"""Hook-exported Git state must never redirect a fixture into its parent repo."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRUB = ROOT / "scripts/release-test-env.sh"


class GitFixtureIsolationTests(unittest.TestCase):
    def setUp(self):
        names = subprocess.check_output(["git", "rev-parse", "--local-env-vars"], text=True).splitlines()
        self.env = {key: value for key, value in os.environ.items() if key not in names}
        self.env["GIT_CONFIG_GLOBAL"] = "/dev/null"
        self.real_bare = self.git(ROOT, "config", "--local", "--get", "core.bare").stdout
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.parent = self.root / "parent"
        self.parent.mkdir()
        self.git(self.parent, "init", "-q").check_returncode()
        self.git(self.parent, "-c", "user.name=Parent", "-c", "user.email=parent@example.invalid",
                 "commit", "-q", "--allow-empty", "-m", "parent").check_returncode()
        self.linked = self.root / "linked"
        self.git(self.parent, "worktree", "add", "-q", "--detach", str(self.linked)).check_returncode()
        self.git_dir = self.parent / ".git/worktrees/linked"
        self.config = self.parent / ".git/config"
        self.before = self.config.read_bytes()
        self.index_before = (self.parent / ".git/index").read_bytes()
        self.fixture = self.root / "fixture.git"
        self.fixture.mkdir()

    def tearDown(self):
        # Fail the suite if it changed the actual checkout's bare setting.
        self.assertEqual(self.git(ROOT, "config", "--local", "--get", "core.bare").stdout,
                         self.real_bare, "fixture run changed the real repo's core.bare")

    def git(self, cwd, *args):
        return subprocess.run(["git", *args], cwd=cwd, env=self.env,
                              capture_output=True, text=True)

    def leaked(self):
        return dict(self.env, GIT_DIR=str(self.git_dir),
                    GIT_COMMON_DIR=str(self.parent / ".git"),
                    GIT_WORK_TREE=str(self.parent),
                    GIT_INDEX_FILE=str(self.parent / ".git/index"))

    def run_fixture(self, source=False, scrub=SCRUB, env=None):
        # Exercise the two production entry points, including the fixture's
        # identity write that contaminated the host config in the incident.
        fixture_command = ('cd "$1" && git init -q --bare && '
                           'git -C "$1" config user.name "Test fixture" && '
                           'git -C "$1" config user.email fixture@example.invalid')
        if source:
            argv = ["bash", "-c", 'source "$1"; release_test_child bash -c "$2" _ "$3"',
                    "_", str(scrub), fixture_command, str(self.fixture)]
        else:
            argv = ["bash", str(scrub), "bash", "-c", fixture_command, "_", str(self.fixture)]
        return subprocess.run(argv, cwd=self.root, env=env or self.leaked(),
                              capture_output=True, text=True)

    def assert_isolated(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.config.read_bytes(), self.before,
                         "bare fixture rewrote the parent repo config")
        self.assertTrue((self.fixture / "HEAD").is_file())
        self.assertEqual(self.git(self.fixture, "config", "--get", "core.bare").stdout.strip(), "true")
        self.assertEqual(self.git(self.fixture, "config", "--get", "user.name").stdout.strip(), "Test fixture")
        self.assertEqual((self.parent / ".git/index").read_bytes(), self.index_before)

    def test_executable_child_scrubs_hook_git_environment(self):
        self.assert_isolated(self.run_fixture())

    def test_sourced_child_scrubs_hook_git_environment(self):
        self.assert_isolated(self.run_fixture(source=True))
        # The parent shell's hook state is intact; only its suite child loses it.
        result = subprocess.run(["bash", "-c",
                                 'source "$1"; release_test_child true; test "$GIT_DIR" = "$2"',
                                 "_", str(SCRUB), self.leaked()["GIT_DIR"]],
                                cwd=self.root, env=self.leaked(), capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_mutation_without_scrub_reproduces_parent_config_corruption(self):
        mutant = self.root / "unscrubbed.sh"
        text = SCRUB.read_text()
        line = "    unset $(git rev-parse --local-env-vars 2>/dev/null)"
        self.assertIn(line, text)
        mutant.write_text(text.replace(line, "    : # regression mutation: leaked hook environment"))
        result = self.run_fixture(scrub=mutant,
                                  env=dict(self.env, GIT_DIR=str(self.git_dir)))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotEqual(self.config.read_bytes(), self.before)
        self.assertEqual(self.git(self.parent, "config", "--get", "core.bare").stdout.strip(), "true")
        self.assertEqual(self.git(self.parent, "config", "--get", "user.name").stdout.strip(), "Test fixture")
        self.assertFalse((self.fixture / "HEAD").exists())


if __name__ == "__main__":
    unittest.main()
