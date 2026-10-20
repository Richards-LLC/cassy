#!/usr/bin/env python3
"""Selection of the script-tier tests a lane's diff touches (no subprocesses)."""
import importlib.util
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location(
    "script_tests", Path(__file__).resolve().parent / "ci-script-tests-for-diff.py")
SELECTOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SELECTOR)
TIER = ["python3 scripts/test-worker-memory.py", "python3 scripts/test-example.py",
        "python3 scripts/test-release-portable.py"]


class Selection(unittest.TestCase):
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
