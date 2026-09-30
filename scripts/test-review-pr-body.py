#!/usr/bin/env python3
"""Shared renderer contract and real release-stage fixtures (no Rust build)."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("review_body", ROOT / "scripts/review-pr-body.py")
review = importlib.util.module_from_spec(spec)
spec.loader.exec_module(review)

CHANGELOG = """# Changelog
## [Unreleased]
## [3.29.1] - 2026-09-25
- later fix
## [3.29.0] - 2026-09-24
- literal {{risk}}, `$(touch escaped)` and spaces
## [3.2900] - 2026-01-01
- not this one
"""


class ReviewBodyTests(unittest.TestCase):
    def test_shared_delivery_release_and_literal_fixtures(self):
        for fixture in json.loads((ROOT / "docs/review/pr-body-fixtures.json").read_text()):
            with self.subTest(fixture["name"]):
                self.assertEqual(review.render(fixture["fields"]), fixture["expected"])

    def test_release_preserves_literal_section_gate_rows_and_metadata(self):
        body = review.release_body(CHANGELOG, "3.29.0", "noise\nPASS workspace-tests 12\nFAIL macos-check reason\n", "platform", "one-way")
        self.assertIn("## [3.29.0] - 2026-09-24", body)
        self.assertIn('literal {{risk}}, `$(touch escaped)`', body)
        self.assertNotIn("later fix", body)
        self.assertNotIn("not this one", body)
        self.assertIn("PASS workspace-tests 12", body)
        self.assertIn("FAIL macos-check reason", body)
        self.assertIn("**Risk:** platform", body)
        self.assertIn("**Door:** one-way", body)

    def test_missing_evidence_never_claims_green(self):
        body = review.release_body(CHANGELOG, "3.29.0")
        self.assertIn("Release gate evidence not supplied", body)
        self.assertIn("**Risk:** not declared", body)
        self.assertIn("**Door:** not declared", body)

    def test_missing_version_refused(self):
        with self.assertRaisesRegex(ValueError, "could not derive"):
            review.release_body(CHANGELOG, "3.29.2")

    def test_real_release_stage_output_and_untrusted_changelog(self):
        with tempfile.TemporaryDirectory(prefix="cas-review-body-") as directory:
            root = Path(directory)
            worktree = root / "worktree with spaces"
            run = root / "run with spaces"
            worktree.mkdir()
            run.mkdir()
            (worktree / "CHANGELOG.md").write_text(CHANGELOG)
            (run / "gate.log").write_text("PASS workspace-tests\nFAIL nextest\n")
            env = dict(os.environ, REVIEW_ROOT=str(ROOT), REVIEW_WT=str(worktree), REVIEW_RUN=str(run),
                       CAS_RELEASE_TRAIN_TASK_RISK="platform, concurrency", CAS_RELEASE_TRAIN_TASK_DOOR="two-way")
            result = subprocess.run(["bash", "-c", '''
script_dir="$REVIEW_ROOT/scripts"
worktree="$REVIEW_WT"
run_dir="$REVIEW_RUN"
version=3.29.0
source "$script_dir/release-train.d/pr-body.sh"
cut_has_external_stage() { return 1; }
cut_stage_pr_body
'''], env=env, cwd=root, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            body = (run / "pr-body.md").read_text()
            self.assertIn("## Summary\n", body)
            self.assertIn("## Evidence\n", body)
            self.assertIn("## Merge Danger\n", body)
            self.assertIn("**Door:** two-way", body)
            self.assertIn("**Risk:** platform, concurrency", body)
            self.assertIn("FAIL nextest", body)
            self.assertFalse((root / "escaped").exists())

    def test_cli_rejects_invalid_door_and_risk_without_output(self):
        with tempfile.TemporaryDirectory(prefix="cas-review-validation-") as directory:
            root = Path(directory)
            changelog = root / "CHANGELOG.md"
            output = root / "output.md"
            changelog.write_text(CHANGELOG)
            for flags in (("--door", "unknown"), ("--risk", "none,platform"), ("--risk", "platform,platform")):
                result = subprocess.run(["python3", str(ROOT / "scripts/review-pr-body.py"),
                    "--version", "3.29.0", "--changelog", str(changelog), "--output", str(output), *flags], capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
