#!/usr/bin/env python3
"""Receipt invalidation tests; no Rust process is invoked."""
import copy
import contextlib
import io
import importlib.util
import json
from pathlib import Path
import tempfile
import time
import unittest
from unittest import mock

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
        (self.root / "Cargo.toml").write_text('[workspace]\nmembers = ["member-one", "member-two"]\n')
        for name in ("member-one", "member-two"):
            (self.root / name).mkdir()
            (self.root / name / "Cargo.toml").write_text(
                f'[package]\nname = "{name}"\nversion = "1.0.0" # preserve comment\n'
                '[dependencies]\nthird-party = "1.0.0"\n')
        (self.root / "Cargo.lock").write_text('version = 4\n' + "".join(
            f'\n[[package]]\nname = "{name}"\nversion = "1.0.0"\n'
            for name in ("member-one", "member-two", "third-party")))
        ledger = self.root / "cas-cli/src/builtins/reference-history.json"
        ledger.parent.mkdir(parents=True)
        ledger.write_text('{}\n')
        self.commit()
        self.expected = {"format": proof.FORMAT, "code_input": proof.code_input(self.root)}
        self.path = proof.receipt_path(self.root, self.expected)
        self.path.parent.mkdir(parents=True)
        self.tree = self.git("rev-parse", "HEAD^{tree}")
        self.record = {"inputs": self.expected, "status": "PASS", "head": self.git("rev-parse", "HEAD"),
                       "tree": self.tree, "completed_epoch": int(time.time()), "archive_size_bytes": 123,
                       "script_tests": {"status": "PASS", "row": "ci-script-tests", "tree": self.tree},
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

    def assert_code_miss(self):
        self.commit()
        changed = dict(self.expected, code_input=proof.code_input(self.root))
        self.assertNotEqual(self.expected["code_input"], changed["code_input"])
        self.assertIsNone(proof.matching(self.root, changed))

    def test_train_output_locations_do_not_change_environment_fingerprint(self):
        base = {"RUSTFLAGS": "-C debuginfo=1", "HOME": "/home/fixture", "PATH": "/bin"}
        output = dict(base, CAS_RELEASE_ARTIFACTS_ROOT="/output/train",
                      CAS_RELEASE_RECEIPTS_RUN_DIR="/output/receipts")
        self.assertEqual(proof.environment_material(self.root, base),
                         proof.environment_material(self.root, output))
        changed = dict(output, RUSTFLAGS="-C debuginfo=2")
        self.assertNotEqual(proof.environment_material(self.root, base),
                            proof.environment_material(self.root, changed))

    def test_miss_reports_first_differing_input_without_environment_values(self):
        self.record["inputs"]["environment"] = "old"
        self.save()
        expected = dict(self.expected, environment="new")
        stream = io.StringIO()
        with contextlib.redirect_stderr(stream):
            self.assertIsNone(proof.matching(self.root, expected, diagnostic=True))
        self.assertIn("key=environment reason=different", stream.getvalue())

    def test_environment_detail_uses_names_and_hashes_only(self):
        self.record["inputs"]["environment"] = "old"
        self.record["environment_keys"] = {
            key: proof.digest(value.encode())
            for key, value in proof.environment_material(self.root, proof.test_environment(self.root)).items()}
        self.record["environment_keys"]["RUSTFLAGS"] = proof.digest(b"secret-compiler-flag")
        self.save()
        stream = io.StringIO()
        with contextlib.redirect_stderr(stream):
            self.assertIsNone(proof.matching(self.root, dict(self.expected, environment="new"), diagnostic=True))
        self.assertIn("environment_key=RUSTFLAGS", stream.getvalue())
        self.assertNotIn("secret-compiler-flag", stream.getvalue())

    def test_invalid_receipt_names_its_validation_key(self):
        self.record["completed_epoch"] = 0
        self.save()
        stream = io.StringIO()
        with contextlib.redirect_stderr(stream):
            self.assertIsNone(proof.matching(self.root, self.expected, diagnostic=True))
        self.assertIn("key=completed_epoch reason=future_or_expired", stream.getvalue())

    def test_prep_member_versions_lock_and_ledger_reuse_proof(self):
        for name in ("member-one", "member-two"):
            path = self.root / name / "Cargo.toml"
            path.write_text(path.read_text().replace('version = "1.0.0"', 'version = "2.0.0"'))
        lock = self.root / "Cargo.lock"
        lock.write_text(lock.read_text().replace('version = "1.0.0"', 'version = "2.0.0"', 2))
        (self.root / "cas-cli/src/builtins/reference-history.json").write_text('{"new": ["hash"]}\n')
        self.commit()
        self.assertEqual(self.expected["code_input"], proof.code_input(self.root))
        found = proof.matching(self.root, self.expected)
        self.assertIsNotNone(found)
        self.assertEqual(found[0]["head"], self.record["head"])

    def test_manifest_dependency_version_change_misses(self):
        path = self.root / "member-one/Cargo.toml"
        path.write_text(path.read_text().replace('third-party = "1.0.0"', 'third-party = "2.0.0"'))
        self.assert_code_miss()

    def test_lock_nonmember_dependency_version_change_misses(self):
        path = self.root / "Cargo.lock"
        path.write_text(path.read_text().replace('name = "third-party"\nversion = "1.0.0"',
                                                'name = "third-party"\nversion = "2.0.0"'))
        self.assert_code_miss()

    def test_lock_member_version_must_correspond_to_manifest(self):
        path = self.root / "Cargo.lock"
        path.write_text(path.read_text().replace('name = "member-one"\nversion = "1.0.0"',
                                                'name = "member-one"\nversion = "2.0.0"'))
        self.assert_code_miss()

    def test_other_member_manifest_lines_still_invalidate(self):
        path = self.root / "member-one/Cargo.toml"
        path.write_text(path.read_text().replace('# preserve comment', '# changed comment'))
        self.assert_code_miss()

    def test_other_lock_lines_still_invalidate(self):
        path = self.root / "Cargo.lock"
        path.write_text(path.read_text().replace('name = "member-one"', 'name = "renamed"'))
        self.assert_code_miss()

    def test_registry_package_colliding_with_member_name_is_not_masked(self):
        path = self.root / "Cargo.lock"
        path.write_text(path.read_text() + '\n[[package]]\nname = "member-one"\n'
                        'version = "1.0.0"\nsource = "registry+https://example.invalid/index"\n')
        self.commit()
        baseline = proof.code_input(self.root)
        path.write_text(path.read_text().replace(
            'version = "1.0.0"\nsource =', 'version = "2.0.0"\nsource ='))
        self.commit()
        self.assertNotEqual(baseline, proof.code_input(self.root))

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

    def test_journey_markdown_keeps_code_proof(self):
        path = self.root / "docs/qa/journey-evaluations/fixture.md"
        path.parent.mkdir(parents=True)
        path.write_text("journey passed\n")
        self.commit()
        self.assertEqual(self.expected["code_input"], proof.code_input(self.root))
        self.assertIsNotNone(proof.matching(self.root, self.expected))

    def test_non_markdown_journey_file_remains_a_code_input(self):
        path = self.root / "docs/qa/journey-evaluations/fixture.json"
        path.parent.mkdir(parents=True)
        path.write_text('{}\n')
        self.assert_code_miss()

    def test_embedded_journey_markdown_remains_a_code_input(self):
        path = self.root / "docs/qa/journey-evaluations/fixture.md"
        path.parent.mkdir(parents=True)
        path.write_text("journey passed\n")
        (self.root / "src/lib.rs").write_text('const DOC: &str = include_str!(\n'
                                               ' "../docs/qa/journey-evaluations/fixture.md");\n')
        self.commit()
        before = proof.code_input(self.root)
        path.write_text("changed embedded journey\n")
        self.commit()
        self.assertNotEqual(before, proof.code_input(self.root))

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

    def test_script_tier_must_pass_the_same_tree(self):
        baseline = copy.deepcopy(self.record)
        for field, value in (("status", "FAIL"), ("tree", "0" * 40), ("row", "nextest")):
            with self.subTest(field=field):
                self.record = copy.deepcopy(baseline)
                self.record["script_tests"][field] = value
                self.save()
                self.assertIsNone(proof.matching(self.root, self.expected))

    def run_producer(self, fail_script=False):
        self.path.unlink()
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        rows = []

        def run(root, row, env, logs):
            rows.append(row)
            self.assertFalse(proof.IDENTITY & env.keys())
            if row == "ci-script-tests" and fail_script:
                raise ValueError("test_seeded_ci_script_failure")
            if row == "archive-mode":
                (logs / "archive-size-bytes").write_text("123")
            return {"status": "PASS", "row": row, "tree": self.tree, "passed": 10}

        with mock.patch.object(proof, "clone_scratch", return_value=Path(scratch.name) / "base"), \
                mock.patch.object(proof, "inputs", return_value=(self.expected, proof.test_environment(self.root))), \
                mock.patch.object(proof, "run_row", side_effect=run):
            if fail_script:
                with self.assertRaisesRegex(ValueError, "test_seeded_ci_script_failure"):
                    proof.prove(self.root)
                self.assertEqual(rows, ["ci-script-tests"])
                self.assertIsNone(proof.matching(self.root, self.expected))
                self.assertEqual(json.loads(self.path.read_text())["status"], "RUNNING")
            else:
                record, _ = proof.prove(self.root)
                self.assertEqual(rows, ["ci-script-tests", "nextest", "archive-mode"])
                self.assertEqual(record["script_tests"]["status"], "PASS")
                self.assertIsNotNone(proof.matching(self.root, self.expected))
                proof.prove(self.root)
                self.assertEqual(rows, ["ci-script-tests", "nextest", "archive-mode"])

    def test_script_failure_blocks_rust_suites_and_pass_publication(self):
        self.run_producer(fail_script=True)

    def test_script_pass_precedes_both_contexts_and_receipt_reuse(self):
        self.run_producer()

    def test_incomplete_corrupt_and_running_receipts_miss(self):
        baseline = copy.deepcopy(self.record)
        for key in ("contexts", "tree", "archive_size_bytes", "inputs", "script_tests"):
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

    def test_prove_refuses_disposable_clone_scratch_before_any_tool_or_suite(self):
        cases = [{}, {"CAS_RELEASE_GATE_HOME_DIR": ""}] + [
            {"CAS_RELEASE_GATE_HOME_DIR": root + "/cas-release-gate/base"}
            for root in ("/tmp", "/var/tmp", "/private/tmp", "/private/var/tmp")]
        cases.append({"TMPDIR": str(self.root),
                      "CAS_RELEASE_GATE_HOME_DIR": str(self.root / "gate/base")})
        for env in cases:
            with self.subTest(env=env), mock.patch.dict(proof.os.environ, env, clear=True), \
                    mock.patch.object(proof, "inputs", side_effect=AssertionError("tools must not run")):
                with self.assertRaisesRegex(ValueError, "CAS_RELEASE_GATE_HOME_DIR"):
                    proof.prove(self.root)

    def test_prove_refuses_a_symlink_into_disposable_clone_scratch(self):
        alias = self.root / "scratch-link"
        alias.symlink_to("/var/tmp", target_is_directory=True)
        with mock.patch.dict(proof.os.environ, {"CAS_RELEASE_GATE_HOME_DIR": str(alias / "base")}, clear=True), \
                mock.patch.object(proof, "inputs", side_effect=AssertionError("tools must not run")):
            with self.assertRaisesRegex(ValueError, "CAS_RELEASE_GATE_HOME_DIR"):
                proof.prove(self.root)

    def test_non_disposable_override_reaches_inputs_and_retains_ancestry_guard(self):
        for base in ("/home/cas-release-gate/base", "/Users/Shared/cas-release-gate/base",
                     "/var/tmp-neighbour/cas-release-gate/base"):
            with self.subTest(base=base), \
                    mock.patch.dict(proof.os.environ, {"CAS_RELEASE_GATE_HOME_DIR": base}, clear=True), \
                    mock.patch.object(proof, "no_cas_ancestor") as ancestry, \
                    mock.patch.object(proof, "inputs", side_effect=RuntimeError("guard accepted")):
                with self.assertRaisesRegex(RuntimeError, "guard accepted"):
                    proof.prove(self.root)
                ancestry.assert_called_once_with(Path(base).resolve().parent)

    def test_prove_preserves_cas_ancestor_refusal_before_inputs(self):
        with mock.patch.dict(proof.os.environ, {"CAS_RELEASE_GATE_HOME_DIR": "/home/cas-release-gate/base"}, clear=True), \
                mock.patch.object(proof, "no_cas_ancestor", side_effect=ValueError(".cas ancestor")), \
                mock.patch.object(proof, "inputs", side_effect=AssertionError("tools must not run")):
            with self.assertRaisesRegex(ValueError, r"\.cas ancestor"):
                proof.prove(self.root)


if __name__ == "__main__":
    unittest.main()
