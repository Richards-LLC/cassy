#!/usr/bin/env python3
"""Receipt invalidation tests; no Rust process is invoked."""
import copy
import importlib.util
from pathlib import Path
import tempfile
import time
import unittest

spec = importlib.util.spec_from_file_location("assembly_proof", Path(__file__).with_name("assembly-proof.py"))
proof = importlib.util.module_from_spec(spec)
spec.loader.exec_module(proof)


class ReceiptTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.git("init", "-q")
        self.git("config", "user.email", "fixture@example.invalid")
        self.git("config", "user.name", "Fixture")
        (self.root / ".gitignore").write_text(".cas/\n")
        (self.root / "src").mkdir()
        (self.root / "docs/release-reports").mkdir(parents=True)
        (self.root / "docs/release-notes").mkdir(parents=True)
        (self.root / "src/lib.rs").write_text('const DOC: &str = include_str!(\n "../docs/release-reports/embedded.md");\n')
        (self.root / "docs/release-reports/embedded.md").write_text("embedded\n")
        (self.root / "docs/release-notes/prose.md").write_text("prose\n")
        self.commit()
        self.expected = {"format": proof.FORMAT, "code_input": proof.code_input(self.root)}
        self.path = proof.receipt_path(self.root, self.expected)
        self.path.parent.mkdir(parents=True)
        self.tree = self.git("rev-parse", "HEAD^{tree}")
        self.record = {"inputs": self.expected, "status": "PASS", "head": self.git("rev-parse", "HEAD"),
                       "tree": self.tree, "completed_epoch": int(time.time()), "archive_size_bytes": 123,
                       "contexts": {name: {"status": "PASS", "tree": self.tree, "passed": 10}
                                    for name in ("worktree", "clone")}}
        self.save()

    def git(self, *args):
        return proof.git(self.root, *args).decode().strip()

    def commit(self):
        self.git("add", ".")
        self.git("-c", "commit.gpgsign=false", "commit", "-qm", "fixture")

    def save(self):
        proof.write(self.path, self.record)

    def test_matching_tree_passes(self):
        self.assertIsNotNone(proof.matching(self.root, self.expected))

    def test_release_prose_changes_tree_but_keeps_code_proof(self):
        (self.root / "docs/release-notes/prose.md").write_text("new prose\n")
        self.commit()
        self.assertNotEqual(self.tree, self.git("rev-parse", "HEAD^{tree}"))
        self.assertEqual(self.expected["code_input"], proof.code_input(self.root))
        self.assertIsNotNone(proof.matching(self.root, self.expected))

    def test_embedded_release_report_remains_a_code_input(self):
        (self.root / "docs/release-reports/embedded.md").write_text("new embedded content\n")
        self.commit()
        self.assertNotEqual(self.expected["code_input"], proof.code_input(self.root))

    def test_expired_future_and_invalid_receipts_miss(self):
        for epoch in (time.time() - proof.MAX_AGE - 10, time.time() + 100, "bad"):
            with self.subTest(epoch=epoch):
                self.record["completed_epoch"] = epoch
                self.save()
                self.assertIsNone(proof.matching(self.root, self.expected))

    def test_each_context_must_pass_the_recorded_tree_with_nonzero_tests(self):
        baseline = copy.deepcopy(self.record)
        for name in ("worktree", "clone"):
            for field, value in (("status", "FAIL"), ("tree", "0" * 40), ("passed", 0)):
                with self.subTest(name=name, field=field):
                    self.record = copy.deepcopy(baseline)
                    self.record["contexts"][name][field] = value
                    self.save()
                    self.assertIsNone(proof.matching(self.root, self.expected))

    def test_incomplete_corrupt_and_running_receipts_miss(self):
        baseline = copy.deepcopy(self.record)
        for key in ("contexts", "tree", "archive_size_bytes", "inputs"):
            self.record = copy.deepcopy(baseline)
            del self.record[key]
            self.save()
            self.assertIsNone(proof.matching(self.root, self.expected))
        self.record = baseline
        self.record["status"] = "RUNNING"
        self.save()
        self.assertIsNone(proof.matching(self.root, self.expected))
        self.path.write_text("{partial")
        self.assertIsNone(proof.matching(self.root, self.expected))

    def test_tree_provenance_must_exist_and_match(self):
        for field, value in (("head", "0" * 40), ("tree", "0" * 40)):
            saved = self.record[field]
            self.record[field] = value
            self.save()
            self.assertIsNone(proof.matching(self.root, self.expected))
            self.record[field] = saved

    def test_changed_environment_toolchain_and_code_keys_miss(self):
        for field in ("environment", "toolchain", "code_input"):
            changed = dict(self.expected, **{field: "changed"})
            self.assertIsNone(proof.matching(self.root, changed))

    def test_dirty_checkouts_are_detected(self):
        self.assertTrue(proof.clean(self.root))
        (self.root / "src/lib.rs").write_text("dirty\n")
        self.assertFalse(proof.clean(self.root))

    def test_plain_clone_refuses_a_cas_ancestor(self):
        child = self.root / "clone"
        child.mkdir()
        with self.assertRaises(ValueError):
            proof.no_cas_ancestor(child)


if __name__ == "__main__":
    unittest.main()
