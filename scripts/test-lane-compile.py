#!/usr/bin/env python3
"""Real Git merge/receipt fixtures; the capped runner is a local stand-in."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import shutil
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("check-lane-compile.py").resolve()
spec = importlib.util.spec_from_file_location("lane_compile", SCRIPT)
lane = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lane)


class LaneCompile(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name) / "repo with spaces"
        self.repo.mkdir()
        self.write(".gitignore", "/.cas/\n/target/\n/.context/\n")
        self.write("Cargo.toml", '[workspace]\nmembers=["first", "second"]\n')
        for package in ("first", "second"):
            self.write(f"{package}/Cargo.toml", f'[package]\nname="{package}"\nversion="0.1.0"\n')
            self.write(f"{package}/src/lib.rs", "pub fn original() {}\n")
        self.write("crates/ghostty_vt_sys/build.rs", "// Zig toolchain consumer\n")
        self.write("scripts/check-lane-compile.py", SCRIPT.read_text())
        self.git("init", "-q", "-b", "target")
        self.git("config", "user.email", "fixture@example.invalid")
        self.git("config", "user.name", "Fixture")
        self.commit()
        self.base = self.git("rev-parse", "HEAD")
        (self.repo / ".cas").mkdir()
        self.log = Path(self.temp.name) / "calls.jsonl"
        self.fake = Path(self.temp.name) / "fake-cas"
        self.fake.write_text("""#!/usr/bin/env python3
import fcntl, hashlib, json, os, pathlib, subprocess, sys
args = sys.argv[1:]
assert args[:2] == ['factory', 'worker-check']
assert args[2] == '--cas-root' and args[4] == '--'
root = pathlib.Path(args[3]).resolve()
cwd = pathlib.Path.cwd().resolve()
assert cwd.parent == root / 'worktrees'
assert args[-1] in ['--lib', '--tests']
owner_verified = False
if os.environ.get('LANE_FIXTURE_OWNER_PROBE'):
    metadata = cwd.with_name(cwd.name.removesuffix('-preview'))
    marker = json.loads((metadata / '.cas-lane-compile.json').read_text())
    assert marker['version'] == 1
    assert marker['head'] == subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    assert marker['git_common_dir'] == str((root.parent / '.git').resolve())
    assert marker['worktree'] == str(cwd)
    with (metadata / '.cas-lane-compile.lock').open('a') as owner:
        try:
            fcntl.flock(owner, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            owner_verified = True
        else:
            raise AssertionError('live preview owner lock was not held')
with open(os.environ['LANE_FIXTURE_LOG'], 'a') as log:
    log.write(json.dumps({'args': args, 'cwd': str(cwd), 'zig': os.environ.get('ZIG'),
                          'continues': os.environ.get('CAS_WORKER_CHECK_CONTINUES_PASS'),
                          'owner_verified': owner_verified}) + '\\n')
if args[-1] == os.environ.get('LANE_FIXTURE_FAIL'):
    sys.exit(7)
# cas-f616: the load the --lib step raised is above the cap when --tests starts;
# like worker-check, refuse unless this step continues a fresh PASS here.
if args[-1] == '--tests' and os.environ.get('LANE_FIXTURE_LOAD_AFTER_LIB'):
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    key = hashlib.sha256(os.fsencode(cwd)).hexdigest()
    passed = (root / 'worker-checks' / key / (head + '.json')).is_file()
    if not (os.environ.get('CAS_WORKER_CHECK_CONTINUES_PASS') == '1' and passed):
        sys.stderr.write('Worker check refused: 1-minute load 41.50 exceeds 32 CPUs; retry later\\n')
        sys.exit(1)
if args[-1] == '--tests' and os.environ.get('LANE_FIXTURE_MOVE'):
    subprocess.check_call(['git', 'update-ref', 'refs/heads/target', os.environ['LANE_FIXTURE_MOVE']])
if args[-1] == '--tests' and os.environ.get('LANE_FIXTURE_DIRTY'):
    (cwd / 'first/src/lib.rs').write_text('changed during check')
if not os.environ.get('LANE_FIXTURE_MISSING'):
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    key = hashlib.sha256(os.fsencode(cwd)).hexdigest()
    receipt = root / 'worker-checks' / key / (head + '.json')
    receipt.parent.mkdir(parents=True, exist_ok=True)
    receipt.write_text(json.dumps({'head': head, 'repo': str(cwd), 'packages': args[6:-1:2]}))
""")
        self.fake.chmod(0o755)
        self.bin = Path(self.temp.name) / "bin"
        self.bin.mkdir()
        for name, path in (("git", shutil.which("git")), ("bash", shutil.which("bash")),
                           ("python3", sys.executable)):
            (self.bin / name).symlink_to(path)
        self.zig = self.make_zig(self.repo / ".context/zig/zig")
        self.env = dict(os.environ, ZIG="", PATH=str(self.bin),
                        CAS_LANE_CHECK_CAS=str(self.fake), LANE_FIXTURE_LOG=str(self.log))

    def make_zig(self, path):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("#!/bin/sh\nexit 0\n")
        path.chmod(0o755)
        return path.resolve()

    def assert_preview_zig(self, expected):
        self.rust_lane()
        self.prove()
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertEqual([call["zig"] for call in calls], [str(expected)] * 2)
        self.assertTrue(all(not (Path(call["cwd"]) / ".context/zig/zig").exists()
                            for call in calls))

    def test_unrelated_rust_repo_needs_no_zig(self):
        self.git("rm", "crates/ghostty_vt_sys/build.rs")
        self.commit()
        self.zig.unlink()
        self.rust_lane()
        self.prove()
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertEqual([call["zig"] for call in calls], [""] * 2)

    def test_source_repo_zig_is_exported_to_fresh_preview_without_path_zig(self):
        self.assert_preview_zig(self.zig)

    def test_explicit_zig_precedes_path_and_source_repo(self):
        expected = self.make_zig(self.repo / ".context/custom-zig")
        self.make_zig(self.bin / "zig")
        self.env["ZIG"] = ".context/custom-zig"
        self.assert_preview_zig(expected)

    def test_path_zig_precedes_source_repo_and_invalid_configured_zig(self):
        expected = self.make_zig(self.bin / "zig")
        self.env["ZIG"] = "missing-zig"
        self.assert_preview_zig(expected)

    def test_linked_source_repo_resolves_main_checkout_zig(self):
        self.rust_lane()
        checkout = Path(self.temp.name) / "source-worktree"
        self.git("worktree", "add", "--detach", str(checkout), "HEAD")
        self.repo = checkout
        self.prove()
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertEqual([call["zig"] for call in calls], [str(self.zig)] * 2)

    def test_missing_zig_refuses_before_runner_and_invalidates_previous_proof(self):
        self.rust_lane()
        path = self.prove()
        self.zig.chmod(0o644)
        result = self.run_check("--prove")
        self.assertEqual(result.returncode, 1)
        self.assertIn("Zig compiler", result.stderr)
        self.assertIn("bootstrap-zig.sh", result.stderr)
        self.assertEqual(len(self.log.read_text().splitlines()), 2)
        self.assertFalse(path.exists())
        self.assertNotIn("lane-compile-", self.git("worktree", "list", "--porcelain"))

    def write(self, path, text):
        target = self.repo / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.repo), *args], stderr=subprocess.STDOUT).decode().strip()

    def commit(self):
        self.git("add", ".")
        self.git("commit", "-qm", "fixture")

    def rust_lane(self):
        self.git("checkout", "-qb", "factory/lane")
        self.write("first/src/lib.rs", "pub fn changed() {}\n")
        self.commit()
        return self.git("rev-parse", "HEAD")

    def run_check(self, *extra, env=None):
        return subprocess.run([sys.executable, str(SCRIPT), str(self.repo), "target", "factory/lane", *extra],
                              env=env or self.env, capture_output=True, text=True)

    def prove(self):
        result = self.run_check("--prove")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return lane.receipt_path(self.repo, lane.merged_tree(self.repo, "target", "factory/lane")[2])

    def test_missing_receipt_refuses_without_changing_refs_and_prints_executable_command(self):
        source = self.rust_lane()
        result = self.run_check()
        self.assertEqual(result.returncode, 1)
        self.assertIn("cargo check -p first --lib", result.stderr)
        self.assertIn("cargo check -p first --tests", result.stderr)
        self.assertEqual(self.git("rev-parse", "target"), self.base)
        self.assertEqual(self.git("rev-parse", "factory/lane"), source)
        self.assertFalse(self.log.exists())
        remedy = result.stderr.split("Run: ", 1)[1].strip()
        proof = subprocess.run(["bash", "-o", "pipefail", "-c", remedy], env=self.env,
                               capture_output=True, text=True)
        self.assertEqual(proof.returncode, 0, proof.stdout + proof.stderr)
        self.assertEqual(self.run_check().returncode, 0)

    def test_docs_only_needs_no_receipt_or_capped_runner(self):
        self.git("checkout", "-qb", "factory/lane")
        self.write("notes.md", "# Notes\n")
        self.commit()
        result = self.run_check()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("not required", result.stdout)
        self.assertFalse(self.log.exists())

    def test_proof_is_for_combined_tree_not_lane_tip(self):
        source = self.rust_lane()
        self.prove()
        self.git("checkout", "target")
        self.write("second/src/lib.rs", "pub fn another_lane() {}\n")
        self.commit()
        target = self.git("rev-parse", "HEAD")
        self.assertNotEqual(lane.merged_tree(self.repo, "target", "factory/lane")[2], self.git("rev-parse", f"{source}^{{tree}}"))
        result = self.run_check()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(self.git("rev-parse", "target"), target)
        path = self.prove()
        proof = json.loads(path.read_text())
        self.assertEqual(proof["tree"], lane.merged_tree(self.repo, "target", "factory/lane")[2])
        self.assertEqual(proof["packages"], ["first"])
        self.assertEqual(self.run_check().returncode, 0)

    def test_both_selectors_run_through_existing_cap_on_private_preview(self):
        self.rust_lane()
        path = self.prove()
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertEqual([call["args"][-1] for call in calls], ["--lib", "--tests"])
        self.assertTrue(all(call["args"][5:-1] == ["-p", "first"] for call in calls))
        self.assertFalse(any(Path(call["cwd"]).exists() for call in calls))
        self.assertNotIn("lane-compile-", self.git("worktree", "list", "--porcelain"))
        self.assertEqual(json.loads(path.read_text())["targets"], ["--lib", "--tests"])

    def test_preview_has_provenance_and_owner_lock_for_both_steps_cas_29b0(self):
        self.rust_lane()
        result = self.run_check("--prove", env=dict(self.env, LANE_FIXTURE_OWNER_PROBE="1"))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertEqual([call["owner_verified"] for call in calls], [True, True])
        self.assertNotIn("lane-compile-", self.git("worktree", "list", "--porcelain"))

    def test_load_raised_by_lib_step_does_not_refuse_its_tests_step_cas_f616(self):
        self.rust_lane()
        result = self.run_check("--prove", env=dict(self.env, LANE_FIXTURE_LOAD_AFTER_LIB="1"))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertEqual([call["args"][-1] for call in calls], ["--lib", "--tests"])
        # Only the continuation asks to carry the admission; the first step is gated.
        self.assertEqual([call["continues"] for call in calls], [None, "1"])
        proof = json.loads(lane.receipt_path(self.repo, lane.merged_tree(self.repo, "target", "factory/lane")[2]).read_text())
        self.assertEqual(proof["targets"], ["--lib", "--tests"])
        self.assertEqual(proof["capped_runner"], "cas factory worker-check")

    def test_failed_retry_erases_earlier_pass(self):
        self.rust_lane()
        path = self.prove()
        result = self.run_check("--prove", env=dict(self.env, LANE_FIXTURE_FAIL="--tests"))
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(path.exists())
        self.assertEqual(self.run_check().returncode, 1)
        self.assertEqual(self.git("rev-parse", "target"), self.base)

    def test_exit_success_without_worker_receipt_is_not_compile_proof(self):
        self.rust_lane()
        result = self.run_check("--prove", env=dict(self.env, LANE_FIXTURE_MISSING="1"))
        self.assertEqual(result.returncode, 1)
        self.assertIn("capped worker PASS receipt missing", result.stderr)
        self.assertEqual(self.run_check().returncode, 1)

    def test_missing_or_corrupt_receipt_fields_refuse(self):
        self.rust_lane()
        path = self.prove()
        proof = json.loads(path.read_text())
        for key, value in (("tree", self.base), ("git_common_dir", "another/repo"),
                           ("targets", ["--lib"]), ("packages", []), ("result", "FAIL"),
                           ("version", 0), ("capped_runner", "cargo")):
            with self.subTest(field=key):
                path.write_text(json.dumps(dict(proof, **{key: value})))
                self.assertEqual(self.run_check().returncode, 1)
        path.write_text("broken JSON")
        self.assertEqual(self.run_check().returncode, 1)

    def test_dirty_preview_does_not_publish_pass(self):
        self.rust_lane()
        result = self.run_check("--prove", env=dict(self.env, LANE_FIXTURE_DIRTY="1"))
        self.assertEqual(result.returncode, 1)
        self.assertIn("preview changed", result.stderr)
        self.assertEqual(self.run_check().returncode, 1)

    def test_target_movement_during_compile_refuses_pass(self):
        self.rust_lane()
        source = self.git("rev-parse", "factory/lane")
        result = self.run_check("--prove", env=dict(self.env, LANE_FIXTURE_MOVE=source))
        self.assertEqual(result.returncode, 1)
        self.assertIn("moved during compile", result.stderr)
        self.assertFalse(list((lane.common_dir(self.repo) / "lane-compile").glob("*.json")))

    def test_actual_candidate_must_have_its_own_tree_receipt(self):
        self.rust_lane()
        self.prove()
        self.write("second/src/lib.rs", "pub fn manual_resolution() {}\n")
        self.commit()
        actual_tree = self.git("rev-parse", "HEAD^{tree}")
        result = self.run_check("--tree", actual_tree)
        self.assertEqual(result.returncode, 1)
        self.assertIn("first,second", result.stderr)

    def test_renamed_and_deleted_rust_paths_retain_package_scope(self):
        self.rust_lane()
        self.git("mv", "first/src/lib.rs", "second/src/moved.rs")
        self.commit()
        base, _, tree = lane.merged_tree(self.repo, "target", "factory/lane")
        self.assertEqual(lane.required_packages(self.repo, base, tree), ["first", "second"])


if __name__ == "__main__":
    unittest.main()
