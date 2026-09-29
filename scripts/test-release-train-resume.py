#!/usr/bin/env python3
"""Dirty-output guard tests; the cut-stage journey lives in test-release-train.sh."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


HELPER = Path(__file__).with_name("release-train-resume.py")


class ResumeOutputs(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "release"
        self.root.mkdir()
        self.run = Path(self.temp.name) / "run"
        self.run.mkdir()
        self.draft = self.root / "docs/release-notes/2026-09-29-v9.99.25-slack.md"
        self.draft.parent.mkdir(parents=True)
        self.draft.write_text("Linux {{LINUX_SHA256}}; macOS {{MACOS_SHA256}}\n")
        (self.root / "source.txt").write_text("source\n")
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Resume Test")
        self.git("config", "user.email", "test@example.invalid")
        self.git("config", "core.hooksPath", "/dev/null")
        self.git("add", ".")
        self.git("-c", "commit.gpgsign=false", "commit", "-qm", "seed")
        (self.run / "landed-main.sha").write_bytes(self.git("rev-parse", "HEAD"))
        (self.run / "release-published.receipt").write_text(
            "LINUX_SHA256=" + "a" * 64 + "\nMACOS_SHA256=" + "b" * 64 + "\n")
        self.draft.write_text("Linux " + "a" * 64 + "; macOS " + "b" * 64 + "\n")

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.root), *args], stderr=subprocess.PIPE)

    def helper(self, action, success=True):
        result = subprocess.run([sys.executable, str(HELPER), action, str(self.root), str(self.run),
                                 "9.99.25", str(self.draft)], capture_output=True, text=True)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stderr)
            self.assertIn("ERROR resume outputs:", result.stderr)
        return result

    def test_exact_outputs_and_clean_committed_outputs(self):
        self.helper("record")
        self.helper("check")
        self.git("add", ".")
        self.git("-c", "commit.gpgsign=false", "commit", "-qm", "receipts")
        self.helper("check")

    def test_unrelated_tracked_and_untracked_files_never_recorded(self):
        (self.root / "source.txt").write_text("unrelated edit\n")
        extra = self.root / "unknown.txt"
        extra.write_text("unknown\n")
        self.helper("record")
        self.helper("check", False)
        self.git("checkout", "--", "source.txt")
        self.helper("check", False)
        extra.unlink()
        self.helper("check")

    def test_draft_tampering(self):
        self.helper("record")
        self.draft.write_text(self.draft.read_text() + "unrecorded prose\n")
        self.helper("check", False)

    def test_index_tampering_even_with_unchanged_working_bytes(self):
        self.helper("record")
        self.git("add", ".")
        self.helper("check", False)

    def test_mode_tampering(self):
        self.helper("record")
        self.draft.chmod(0o755)
        self.helper("check", False)

    def test_partial_report_and_staged_receipts_retry(self):
        report = self.root / "docs/release-reports/v9.99.25.html"
        report.parent.mkdir()
        report.write_text("partial report\n")
        self.git("add", ".")
        self.helper("record")
        self.helper("check")
        report.write_text("modified partial report\n")
        self.helper("check", False)

    def test_foreign_run_receipt(self):
        self.helper("record")
        path = self.run / "post-publication-outputs.json"
        data = json.loads(path.read_text())
        for key in ("version", "run", "landed", "draft"):
            altered = dict(data, **{key: "different"})
            path.write_text(json.dumps(altered))
            self.helper("check", False)

    def test_malformed_receipt(self):
        for value in ("{", "[]", '{"outputs": []}'):
            (self.run / "post-publication-outputs.json").write_text(value)
            self.helper("check", False)

    def test_removed_output_and_symlink_refused(self):
        self.helper("record")
        self.draft.unlink()
        self.helper("check", False)
        self.draft.symlink_to(self.root / "source.txt")
        self.helper("check", False)

    def test_legacy_checksum_delta_only(self):
        self.helper("check")
        self.draft.write_text(self.draft.read_text() + "unrecorded prose\n")
        self.helper("check", False)

    def test_legacy_staging_and_mode_changes_refused(self):
        self.git("add", ".")
        self.helper("check", False)
        self.git("reset", "-q", "HEAD", "--", ".")
        self.draft.chmod(0o755)
        self.helper("check", False)

    def test_legacy_receipt_required(self):
        (self.run / "release-published.receipt").unlink()
        self.helper("check", False)


if __name__ == "__main__":
    unittest.main()
