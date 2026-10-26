#!/usr/bin/env python3
"""Selection of the script-tier tests a lane's diff touches (no subprocesses)."""
import importlib.util
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    "script_tests", Path(__file__).resolve().parent / "ci-script-tests-for-diff.py")
SELECTOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SELECTOR)
TIER = ["python3 scripts/test-worker-memory.py", "python3 scripts/test-example.py",
        "python3 scripts/test-release-portable.py"]


class Selection(unittest.TestCase):
    def test_terminal_qa_inputs_select_the_real_tier_command_once(self):
        terminal = "scripts/terminal-qa.test.mjs"
        builtin = "cas-cli/src/builtins/skills/cas-cli-craft/scripts/terminal-qa.mjs"
        paths = ["scripts/terminal-qa.mjs", terminal, builtin, "scripts/unrelated.txt",
                 "scripts/test-proof-target.py"]
        with tempfile.TemporaryDirectory(prefix="terminal-qa-selection-") as directory:
            repo = Path(directory)
            (repo / "cas-cli").mkdir()
            shutil.copyfile(Path(__file__).resolve().parents[1] / "cas-cli/Makefile",
                            repo / "cas-cli/Makefile")
            for path in paths:
                file = repo / path
                file.parent.mkdir(parents=True, exist_ok=True)
                file.write_text("baseline\n")
            def git(*args):
                subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True)
            git("init", "-q")
            git("add", ".")
            git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                "-c", "commit.gpgsign=false", "commit", "-qm", "base")
            for changed, expected in [
                ([paths[0]], True), ([terminal], True), ([builtin], True),
                ([paths[0], builtin, paths[4]], True), ([paths[3], paths[4]], False),
            ]:
                with self.subTest(changed=changed):
                    for path in changed:
                        (repo / path).write_text("changed\n")
                    output = ("ci-script-tests-changed: would run node --test " + terminal + "\n"
                              if expected else "ci-script-tests-changed: no script test covers this lane's changes\n")
                    modes = [[], ["--only", terminal]] if paths[4] not in changed else [["--only", terminal]]
                    for mode in modes:
                        result = subprocess.run(
                            [sys.executable, SELECTOR.__file__, "--repo", str(repo),
                             "--base", "HEAD", "--dry-run", *mode],
                            capture_output=True, text=True,
                        )
                        self.assertEqual(result.returncode, 0, result.stderr)
                        self.assertEqual(result.stdout, output)
                    git("restore", ".")

    def test_package_scripts_select_the_named_weight_assertion_not_the_suite_cas_eac2(self):
        # cas-2077 changed hub-web/package.json; the lane selected no script test
        # and the merge queue found 4 GiB != 2 GiB.
        selected = SELECTOR.select(TIER, ["hub-web/package.json"])
        self.assertEqual(selected, [("hub-web/package.json", SELECTOR.NAMED["hub-web/package.json"])])
        self.assertIn("AdmissionTests.test_cas_4cb9_weights_follow_shell_and_actual_package_scripts",
                      selected[0][1])
        self.assertEqual(SELECTOR.select(TIER, ["hub-web/src/app.ts"]), [])

    def test_release_train_sources_select_release_portability_tests(self):
        selected = SELECTOR.select(TIER, [
            "scripts/release-train.sh",
            "scripts/release-train.d/post-publication.sh",
        ])
        self.assertEqual(selected, [
            ("scripts/test-release-portable.py", "python3 scripts/test-release-portable.py")
        ])


if __name__ == "__main__":
    unittest.main()
