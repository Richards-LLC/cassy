#!/usr/bin/env python3
"""Receipt invalidation tests; no Rust process is invoked."""
import copy
import contextlib
import io
import importlib.util
import itertools
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import threading
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
        # A private admission pool: live worker suites on this host must not
        # make proof fixtures wait or time out.
        patcher = mock.patch.object(proof, "HOST_MEMORY_DIRECTORY", self.root / ".host-memory")
        patcher.start()
        self.addCleanup(patcher.stop)
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

    def inputs_for_environment(self, env):
        # Exercise the real projection and environment scrub, faking only tool
        # version probes so this script harness never starts Cargo or rustc.
        check_output = proof.subprocess.check_output

        def probe(command, **kwargs):
            if command[0] == "git":
                return check_output(command, **kwargs)
            self.assertIn(command, (["cargo", "--version"],
                                    ["cargo", "nextest", "--version"], ["rustc", "-Vv"]))
            return b"fixture tool version\n"

        with mock.patch.dict(proof.os.environ, env, clear=True), \
                mock.patch.object(proof.subprocess, "check_output", side_effect=probe):
            return proof.inputs(self.root)

    def harness_environment(self, base):
        return dict(base, AI_AGENT="claude", CLAUDECODE="1",
                    CLAUDE_CODE_CHILD_SESSION="fixture-child-session",
                    CAS_FACTORY_SESSION="fixture-factory", CAS_AGENT_ROLE="supervisor",
                    CAS_AGENT_NAME="fixture-agent", CAS_SUPERVISOR_NAME="fixture-supervisor",
                    CAS_AGENT_ID="fixture-agent-id", CAS_SESSION_ID="fixture-session",
                    CAS_ROOT="/fixture/operator/.cas", CAS_CLONE_PATH="/fixture/worktree",
                    CAS_FACTORY_MODE="1", CAS_FACTORY_SUPERVISOR_CLI="claude",
                    CAS_FACTORY_WORKER_CLI="codex", CAS_RELEASE_ENV_FILE="/fixture/release.env")

    def test_harness_sessions_share_fingerprint_and_scrubbed_test_environment(self):
        base = {"RUSTFLAGS": "-C debuginfo=1", "HOME": str(self.root), "PATH": "/usr/bin:/bin"}
        expected, baseline_env = self.inputs_for_environment(base)
        factory = self.harness_environment(base)
        other_session = dict(factory, AI_AGENT="codex", CAS_SESSION_ID="another-session",
                             CAS_CLONE_PATH="/another/worktree",
                             CAS_FACTORY_SUPERVISOR_CLI="codex",
                             CAS_RELEASE_ENV_FILE="/another/release.env")
        for ambient in (factory, other_session):
            with self.subTest(session=ambient["CAS_SESSION_ID"]):
                actual, test_env = self.inputs_for_environment(ambient)
                self.assertEqual(expected, actual)
                self.assertEqual(proof.receipt_path(self.root, expected),
                                 proof.receipt_path(self.root, actual))
                # Session context must not affect the actual test processes
                # either. The publish-only env-file locator may remain.
                self.assertEqual(baseline_env,
                                 {key: value for key, value in test_env.items()
                                  if key != "CAS_RELEASE_ENV_FILE"})

    def test_build_test_and_unknown_variables_still_invalidate_fingerprint(self):
        base = {"HOME": str(self.root), "PATH": "/usr/bin:/bin"}
        expected, _ = self.inputs_for_environment(base)
        variables = {
            "RUSTFLAGS": "-C debuginfo=2", "CARGO_ENCODED_RUSTFLAGS": "-C\x1fdebuginfo=2",
            "HOME": str(self.root / "other-home"), "PATH": "/other/bin:/usr/bin:/bin",
            "TMPDIR": "/other/tmp", "CAS_INIT_TIMEOUT_SECS": "30",
            "CAS_TEST_PROTECTED_DBS": "/fixture/operator.db",
            "CAS_TEST_PROTECTED_HOME": "/fixture/operator",
            "CAS_FACTORY_CARGO_BUILD_JOBS": "2", "CAS_FACTORY_BUILD_GUARD": "off",
            "CAS_FUTURE_TEST_INPUT": "enabled",
        }
        for name, value in variables.items():
            with self.subTest(variable=name):
                actual, test_env = self.inputs_for_environment(dict(base, **{name: value}))
                self.assertEqual(test_env[name], value)
                self.assertNotEqual(expected["environment"], actual["environment"])
                self.assertNotEqual(proof.receipt_path(self.root, expected),
                                    proof.receipt_path(self.root, actual))

    def test_scrubbed_train_reuses_factory_receipt_without_running_rows(self):
        # Keep scratch inventory inside the fixture: the real legacy bases may
        # hold a live proof's clone that churns while this test runs.
        base = {"HOME": str(self.root), "PATH": "/usr/bin:/bin",
                "CAS_RELEASE_GATE_HOME_DIR": str(self.root / "scratch-base"),
                "CAS_RELEASE_SCRATCH_EXTRA_BASES": ""}
        expected, _ = self.inputs_for_environment(self.harness_environment(base))
        self.record["inputs"] = expected
        self.path = proof.receipt_path(self.root, expected)
        self.save()
        train_inputs = self.inputs_for_environment(base)
        stream = io.StringIO()
        with mock.patch.dict(proof.os.environ, base, clear=True), \
                mock.patch.object(proof, "inputs", return_value=train_inputs), \
                mock.patch.object(proof, "clone_scratch", return_value=self.root / "scratch"), \
                mock.patch.object(proof, "run_row") as rows, \
                mock.patch.object(proof.sys, "argv", ["assembly-proof.py", "prove", str(self.root)]), \
                contextlib.redirect_stdout(stream):
            self.assertEqual(proof.main(), 0)
        rows.assert_not_called()
        self.assertIn("PASS assembly receipt=" + str(self.path), stream.getvalue())
        self.assertIn("source_sha=" + self.record["head"], stream.getvalue())

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

    def test_row_timing_rejects_a_nested_failed_self_test_row(self):
        logs = self.root / "row-logs"
        logs.mkdir()
        real_run = proof.subprocess.run

        def gate(command, **kwargs):
            if command[0] == "git":
                return real_run(command, **kwargs)
            rows = Path(kwargs["env"]["CAS_RELEASE_GATE_LOG_DIR"])
            rows.mkdir()
            (rows / "ci-script-tests.log").write_text("self-tests passed\n")
            (rows / "timing.tsv").write_text(
                "row\tstarted_utc\tended_utc\twall_s\tuser_s\tsystem_s\tstatus\tsource_sha\n"
                "hub-web-tests\tstart\tend\t1\t1\t0\t1\tsynthetic\n"
                "ci-script-tests\tstart\tend\t2\t1\t0\t0\touter\n")
            kwargs["stdout"].write("PASS ci-script-tests script fixtures\n")
            return proof.subprocess.CompletedProcess(command, 0)

        with mock.patch.object(proof.subprocess, "run", side_effect=gate):
            with self.assertRaisesRegex(ValueError, "invalid timing.tsv"):
                proof.run_row(self.root, "ci-script-tests", {}, logs)

    def test_run_row_uses_checkout_target_and_logs_source_identity(self):
        logs = self.root / '.cas/isolation-logs'
        logs.mkdir(parents=True)
        actual_run = proof.subprocess.run
        captured = {}
        link_rss = {"phase": "link-complete", "driver_pid": 101,
                    "peak_waited_driver_rss_bytes": 20 * proof.GIB // 1024,
                    "peak_mold_worker_rss_bytes": proof.GIB,
                    "peak_process_tree_rss_bytes": proof.GIB + 20 * proof.GIB // 1024,
                    "rss_sampling_status": "sampled", "rss_sample_count": 8,
                    "mold_worker_peak": {"pid": 102, "start_identity": "fixture-start"}}

        def gate(command, **kwargs):
            if command[0] == 'git':
                return actual_run(command, **kwargs)
            captured.update(kwargs['env'])
            rows = Path(kwargs['env']['CAS_RELEASE_GATE_LOG_DIR'])
            rows.mkdir()
            (rows / 'nextest.log').write_text('PASS: 1 test(s) passed\n')
            (rows / 'link-rss.jsonl').write_text(json.dumps(link_rss) + '\n')
            kwargs['stdout'].write('PASS nextest fixture\n')
            return proof.subprocess.CompletedProcess(command, 0)

        with mock.patch.object(proof.subprocess, 'run', side_effect=gate):
            result = proof.run_row(self.root, 'nextest', {'CARGO_TARGET_DIR': '/other/worktree/target'}, logs)
        self.assertEqual(captured['CARGO_TARGET_DIR'], str(self.root / 'target'))
        self.assertEqual(result['head'], self.git('rev-parse', 'HEAD'))
        self.assertEqual(result['link_rss'], [link_rss], 'assembly receipt retains both RSS scopes and worker identity')
        self.assertIn(str(self.root), (logs / 'nextest.log').read_text())
        self.assertIn(self.git('rev-parse', 'HEAD'), (logs / 'nextest.log').read_text())

    def test_merged_private_clone_target_keeps_bounded_owner_and_source_receipt(self):
        # Real Git clones/leases and shell rows; only Cargo/tool probes are fake.
        (self.root / '.gitignore').write_text('.cas/\ntarget/\n')
        scripts = self.root / 'scripts'
        scripts.mkdir()
        (scripts / 'release-gate.sh').write_text("""#!/bin/bash
set -eu
row=$3
mkdir -p "$CAS_RELEASE_GATE_LOG_DIR"
printf 'PASS: 1 test(s) passed\n' > "$CAS_RELEASE_GATE_LOG_DIR/$row.log"
printf 7 > "$CAS_RELEASE_GATE_ARCHIVE_SIZE_FILE"
printf 'PASS %s fixture\n' "$row"
""")
        self.commit()
        expected = {'format': proof.FORMAT, 'code_input': proof.code_input(self.root)}
        head = self.git('rev-parse', 'HEAD')
        path = proof.receipt_path(self.root, expected)
        legacy = path.parent.parent / 'assembly-target'
        leased = legacy.with_name(legacy.name + '-leased-v1')
        for directory in (legacy, leased):
            directory.mkdir(parents=True)
            (directory / 'opaque').write_text('preserve unknown artifacts')
        observed = {}

        def contexts(root, clone, env, logs, target, execution):
            observed['clone'] = clone
            self.assertEqual(target, clone / 'target')
            self.assertIsNotNone(proof.release_scratch.read_owner(target))
            owner = json.loads((target / proof.proof_target.OWNER).read_text())
            self.assertEqual(owner['worktree'], str(clone.resolve()))
            self.assertEqual(owner['head'], head)
            self.assertFalse(owner['dirty'])
            self.assertTrue(proof.release_scratch.CURRENT.leases)
            results = [proof.run_row(checkout, row, env, logs)
                       for checkout, row in ((root, 'ci-script-tests'), (root, 'nextest'),
                                             (clone, 'archive-mode'))]
            for result in results:
                source = json.loads(Path(result['log']).read_text().splitlines()[0].removeprefix('PROOF_SOURCE: '))
                self.assertEqual(source['head'], head)
                self.assertEqual(source['target'], str(Path(result['checkout']) / 'target'))
            return tuple(results)

        with tempfile.TemporaryDirectory() as directory:
            scratch = Path(directory) / 'base'
            env = {'CAS_RELEASE_GATE_HOME_DIR': str(scratch), 'CARGO_TARGET_DIR': '/foreign/target'}
            with mock.patch.object(proof, 'clone_scratch', return_value=scratch), \
                    mock.patch.object(proof, 'inputs', return_value=(expected, env)), \
                    mock.patch.object(proof, 'execution_plan', return_value={}), \
                    mock.patch.object(proof, 'run_contexts', side_effect=contexts):
                record, receipt = proof.prove(self.root)
        self.assertEqual(record['status'], 'PASS')
        self.assertFalse(observed['clone'].parent.exists(), 'owned clone removed after child teardown')
        self.assertTrue(record['cache'])
        self.assertTrue(all(row['path'] == str(observed['clone'] / 'target') for row in record['cache']))
        self.assertEqual({row['path'] for row in record['legacy_cache']}, {str(legacy), str(leased)})
        for directory in (legacy, leased):
            self.assertEqual((directory / 'opaque').read_text(), 'preserve unknown artifacts')
        self.assertEqual(json.loads(receipt.read_text())['status'], 'PASS')

    def run_producer(self, failure=None, serial=False, deny_test=False, recover_test=False):
        self.path.unlink()
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        rows = []
        tests = []
        script_done = threading.Event()
        native_done = threading.Event()
        producers = threading.Barrier(3)

        def run(root, row, env, logs):
            rows.append(row)
            self.assertFalse(proof.IDENTITY & env.keys())
            self.assertEqual(env['CARGO_TARGET_DIR'], str(root / 'target'))
            self.assertEqual(env['CARGO_BUILD_TARGET_DIR'], str(root / 'target'))
            if not serial:
                producers.wait(timeout=5)  # all three legs must overlap
            if row == "ci-script-tests":
                if failure == row:
                    raise ValueError("test_seeded_ci_script_failure")
                script_done.set()
            else:
                if failure == row:
                    raise ValueError("test_seeded_compile_failure")
                sync_dir = env.get("CAS_RELEASE_GATE_ASSEMBLY_SYNC_DIR")
                if sync_dir:
                    sync = Path(sync_dir)
                    (sync / ("compiled-" + row)).touch()
                    deadline = time.monotonic() + 5
                    while not (sync / ("release-" + row)).exists():
                        if (sync / "abort").exists():
                            raise ValueError("test_seeded_admission_aborted")
                        if time.monotonic() > deadline:
                            raise ValueError("test_admission_stranded")
                        time.sleep(0.01)
                self.assertTrue(script_done.is_set(), "tests preceded script PASS")
                if row == "archive-mode":
                    self.assertTrue(native_done.is_set(), "consumers overlapped")
                else:
                    native_done.set()
                tests.append(row)
            if row == "archive-mode":
                (logs / "archive-size-bytes").write_text("123")
            return {"status": "PASS", "row": row, "tree": self.tree, "passed": 10}

        memory = {"total_bytes": 64 * proof.GIB, "available_bytes": (36 if serial else 60) * proof.GIB,
                  "source": "fixture"}
        snapshots = itertools.chain([memory, memory], itertools.repeat(dict(memory, available_bytes=17 * proof.GIB))) if deny_test else None
        if recover_test:
            snapshots = itertools.chain([memory, memory, dict(memory, available_bytes=17 * proof.GIB)], itertools.repeat(memory))
        with mock.patch.object(proof, "clone_scratch", return_value=Path(scratch.name) / "base"), \
                mock.patch.dict(proof.os.environ, {"CAS_RELEASE_SCRATCH_EXTRA_BASES": "", "TMPDIR": scratch.name}), \
                mock.patch.object(proof, "inputs", return_value=(self.expected, dict(proof.test_environment(self.root), CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS="1"))), \
                mock.patch.object(proof, "memory_snapshot", return_value=memory, side_effect=snapshots), \
                mock.patch.object(proof, "cpu_count", return_value=32), \
                mock.patch.object(proof, "run_row", side_effect=run):
            if failure or deny_test:
                pattern = "cannot fit above memory reserve" if deny_test else "test_seeded_.*failure"
                with self.assertRaisesRegex(ValueError, pattern):
                    proof.prove(self.root)
                self.assertEqual(tests, [])
                self.assertCountEqual(rows, ["ci-script-tests", "nextest", "archive-mode"])
                self.assertIsNone(proof.matching(self.root, self.expected))
                self.assertEqual(json.loads(self.path.read_text())["status"], "RUNNING")
            else:
                record, _ = proof.prove(self.root)
                self.assertCountEqual(rows, ["ci-script-tests", "nextest", "archive-mode"])
                self.assertEqual(tests, ["nextest", "archive-mode"])
                self.assertEqual(record["script_tests"]["status"], "PASS")
                if recover_test:
                    self.assertEqual([event["admitted"] for event in record["execution"]["phases"]], [False, True, True])
                self.assertEqual(record["execution"]["mode"], "serial" if serial else "concurrent")
                self.assertIsNotNone(proof.matching(self.root, self.expected))
                proof.prove(self.root)
                self.assertEqual(len(rows), 3)

    def test_script_failure_blocks_rust_suites_and_pass_publication(self):
        self.run_producer(failure="ci-script-tests")

    def test_signal_tears_down_children_and_owned_scratch(self):
        # Real process/signal boundary; fake only tool probes and test work.
        script = '''
import importlib.util, pathlib, sys
spec = importlib.util.spec_from_file_location('proof', sys.argv[1])
p = importlib.util.module_from_spec(spec); spec.loader.exec_module(p)
root, scratch = map(pathlib.Path, sys.argv[2:4])
p.clone_scratch = lambda env: scratch / 'base'
p.inputs = lambda root: ({'format': p.FORMAT, 'code_input': 'signal-fixture'}, {})
p.execution_plan = lambda env: {'mode': 'serial', 'phases': []}
def contexts(root, clone, env, logs, target, execution):
    return p.run_row(clone, 'archive-mode', dict(env, SCRATCH=str(scratch)), logs)
p.run_contexts = contexts
p.prove(root)
'''
        (self.root / "scripts").mkdir(exist_ok=True)
        # A real gate row that blocks mid-archive; its own cleanup contract is
        # exercised separately by the shell fixture suite.
        (self.root / "scripts/release-gate.sh").write_text(
            '#!/bin/bash\n'
            'echo "$BASHPID" > "$SCRATCH/child-pid"\n'
            'touch "$SCRATCH/ready"\n'
            'exec python3 -c "import time; time.sleep(60)"\n')
        self.commit()
        with tempfile.TemporaryDirectory() as directory:
            scratch = Path(directory)
            # Keep the fixture's scratch sweep off the host's real scratch
            # bases: a live gate's clones there can take longer to scan than
            # the start-up deadline below.
            fixture_env = dict(os.environ, CAS_RELEASE_GATE_HOME_DIR=str(scratch / "home-base"),
                               CAS_RELEASE_SCRATCH_EXTRA_BASES="")
            child = subprocess.Popen([sys.executable, "-c", script,
                                      str(Path(proof.__file__).resolve()), str(self.root), str(scratch)],
                                     start_new_session=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                                     env=fixture_env)
            try:
                deadline = time.monotonic() + 10
                while not (scratch / "ready").exists() and child.poll() is None:
                    self.assertLess(time.monotonic(), deadline, "archive fixture did not start")
                    time.sleep(.02)
                self.assertIsNone(child.poll(), "proof exited before archive fixture")
                child.send_signal(signal.SIGTERM)
                child.communicate(timeout=10)
                self.assertFalse(list(scratch.glob("assembly-clone-*")), "SIGTERM leaked clone")
                pid = int((scratch / "child-pid").read_text())
                # The child must have been waited/reaped before scratch removal.
                with self.assertRaises(ProcessLookupError):
                    os.kill(pid, 0)
            finally:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.communicate(timeout=10)

    def test_script_pass_precedes_both_contexts_and_receipt_reuse(self):
        self.run_producer()

    def test_compile_failure_aborts_waiters_without_pass(self):
        for row in ("nextest", "archive-mode"):
            with self.subTest(row=row):
                self.run_producer(failure=row)

    def test_memory_constrained_proof_falls_back_to_serial_and_reuses(self):
        self.run_producer(serial=True)

    def test_memory_drop_after_compile_aborts_both_consumers(self):
        self.run_producer(deny_test=True)

    def test_refuse_then_recover_completes_proof_and_publishes_pass(self):
        self.run_producer(recover_test=True)

    def test_memory_and_cpu_caps_on_soundwave_and_prowl(self):
        for total, available, cores, expected_jobs in ((62, 50, 32, 16), (48, 40, 18, 9), (62, 35, 32, 0)):
            with self.subTest(cores=cores), \
                    mock.patch.object(proof, "memory_snapshot", return_value={
                        "total_bytes": total * proof.GIB, "available_bytes": available * proof.GIB,
                        "source": "fixture"}), mock.patch.object(proof, "cpu_count", return_value=cores):
                plan = proof.execution_plan({})
                self.assertEqual(plan["compile_jobs"], expected_jobs)
                self.assertEqual(plan["link_jobs"], 8)
                self.assertEqual(proof.execution_plan({"CAS_RELEASE_GATE_ASSEMBLY_LINK_JOBS": "2"})["link_jobs"], 2)
                self.assertEqual(proof.env_policy({"CAS_RELEASE_GATE_ASSEMBLY_LINK_JOBS": "2"})["CAS_RELEASE_GATE_ASSEMBLY_LINK_JOBS"], "2")
                estimated = 2 * (expected_jobs * proof.COMPILE_JOB_BYTES + proof.PRODUCER_BYTES) + proof.SCRIPT_BYTES + proof.LINK_BYTES + proof.GUARD_HEADROOM_BYTES
                if expected_jobs:
                    self.assertLessEqual(estimated, plan["budget_bytes"])
                self.assertEqual(proof.execution_plan({"CAS_RELEASE_GATE_ASSEMBLY_BUILD_JOBS": "99"})["compile_jobs"], expected_jobs)
                self.assertEqual(proof.execution_plan({"CAS_RELEASE_GATE_ASSEMBLY_BUILD_JOBS": "1"})["compile_jobs"], min(1, expected_jobs))
                reserved = proof.execution_plan({"CAS_RELEASE_GATE_ASSEMBLY_RESERVE_GIB": "48"})
                self.assertEqual(reserved["mode"], "serial")

    def test_reserve_floor_and_phase_refusal(self):
        with mock.patch.object(proof, "memory_snapshot", return_value={
                "total_bytes": 16 * proof.GIB, "available_bytes": 9 * proof.GIB, "source": "fixture"}):
            self.assertEqual(proof.execution_plan({})["reserve_bytes"], 8 * proof.GIB)
            execution = {"phases": []}
            with mock.patch.object(proof.time, "monotonic", side_effect=[0, 1]), \
                    self.assertRaisesRegex(ValueError, "cannot fit above memory reserve"):
                proof.admit_phase({"CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS": "1"}, execution, "nextest-compile", True)
            self.assertFalse(execution["phases"][0]["admitted"])

    def test_admission_waits_for_recovery_and_records_both_samples(self):
        low = {"total_bytes": 64 * proof.GIB, "available_bytes": 15 * proof.GIB, "source": "fixture"}
        high = dict(low, available_bytes=32 * proof.GIB)
        execution = {"phases": []}
        with mock.patch.object(proof, "memory_snapshot", side_effect=[low, high]), \
                mock.patch.object(proof.time, "sleep") as sleep:
            count = proof.admit_phase({"CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS": "1"},
                                      execution, "archive-mode-tests")
        self.assertGreater(int(count), 0)
        self.assertEqual([item["admitted"] for item in execution["phases"]], [False, True])
        sleep.assert_called_once()

    @unittest.skipUnless(Path("/proc/self/fd").is_dir(), "needs /proc")
    def test_cas_7b7b9_row_daemon_does_not_keep_the_proof_lease(self):
        # The release incident (cas-7b7b9): a row's build started an sccache server that
        # inherited the proof's intent/budget and held them after the proof.
        pool = self.root / ".host-memory"
        pidfile = self.root / "daemon-pid"
        daemon_program = (
            "import os,pathlib,time\n"
            "if os.fork() == 0:\n"
            "    os.setsid()\n"
            "    if os.fork() == 0:\n"
            f"        pathlib.Path({str(pidfile)!r}).write_text(str(os.getpid()))\n"
            "        time.sleep(30)\n"
            "    os._exit(0)\n"
            "os.wait()\n"
            f"while not pathlib.Path({str(pidfile)!r}).exists(): time.sleep(.01)\n")
        def rows(root, clone, env, *args):
            proof.release_scratch.child_run([sys.executable, "-c", daemon_program], env=env, check=True)
            return "ran"
        def stop_daemon():
            if pidfile.exists():
                try:
                    os.kill(int(pidfile.read_text()), signal.SIGKILL)
                except ProcessLookupError:
                    pass
        self.addCleanup(stop_daemon)
        env = {key: value for key, value in os.environ.items()
               if key not in ("RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WRAPPER", "CAS_HOST_MEMORY_LEASE")}
        with mock.patch.object(proof, "_run_contexts", side_effect=rows), \
                proof.release_scratch.ChildScope():
            self.assertEqual(proof.run_contexts(self.root, self.root, env, self.root, self.root, {}), "ran")
        daemon = int(pidfile.read_text())
        os.kill(daemon, 0)  # the daemon outlived its row and the proof
        held = {os.readlink(fd) for fd in Path(f"/proc/{daemon}/fd").iterdir()
                if os.path.exists(fd) and os.readlink(fd).startswith(str(pool))}
        self.assertEqual(held, set(), "the row daemon inherited the proof's admission descriptors")
        spec = importlib.util.spec_from_file_location("host_memory_7b7b9", Path(proof.__file__).with_name("host_memory.py"))
        host = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(host)
        with host.admission("proof", env, lambda _: {}, wait_secs=1, poll_secs=.05, directory=pool,
                            report=lambda event: None):
            pass  # a second proof is admitted although the daemon still runs

    def test_invalid_memory_and_job_knobs_fail_closed(self):
        for key in ("CAS_RELEASE_GATE_ASSEMBLY_BUILD_JOBS", "CAS_RELEASE_GATE_ASSEMBLY_RESERVE_GIB",
                    "CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS", "CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLL_SECS",
                    "CAS_RELEASE_GATE_ASSEMBLY_LINK_JOBS"):
            for value in ("", "0", "-1", "auto", "1.5"):
                with self.subTest(key=key, value=value), self.assertRaisesRegex(ValueError, key):
                    proof.execution_plan({key: value})
        with mock.patch.object(proof, "memory_snapshot", side_effect=OSError("unavailable")):
            with self.assertRaisesRegex(ValueError, "cannot safely admit"):
                proof.execution_plan({})

    def test_linux_memory_probe_uses_available_not_free(self):
        with mock.patch.object(proof.platform, "system", return_value="Linux"), \
                mock.patch.object(Path, "read_text", return_value="MemTotal: 64000 kB\nMemFree: 1 kB\nMemAvailable: 40000 kB\n"):
            self.assertEqual(proof.memory_snapshot()["available_bytes"], 40000 * 1024)

    def test_macos_memory_probe_uses_actual_page_size_without_double_counting(self):
        vm = ("Mach Virtual Memory Statistics: (page size of 16384 bytes)\n"
              "Pages free: 100.\nPages inactive: 200.\nPages speculative: 50.\n"
              "Pages purgeable: 80.\nPages occupied by compressor: 900.\n")
        with mock.patch.object(proof.platform, "system", return_value="Darwin"), \
                mock.patch.object(proof.subprocess, "check_output", side_effect=[str(48 * proof.GIB).encode(), vm]):
            self.assertEqual(proof.memory_snapshot()["available_bytes"], 350 * 16384)

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
                    mock.patch.object(proof.release_scratch, "sweep", return_value={}), \
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
