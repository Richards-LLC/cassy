#!/usr/bin/env python3
"""Real Git merge/receipt fixtures; the capped runner is a local stand-in."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
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
        self.write(".gitignore", "/.cas/\n/target/\n")
        self.write("Cargo.toml", '[workspace]\nmembers=["first", "second"]\n')
        for package in ("first", "second"):
            self.write(f"{package}/Cargo.toml", f'[package]\nname="{package}"\nversion="0.1.0"\n')
            self.write(f"{package}/src/lib.rs", "pub fn original() {}\n")
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
import hashlib, json, os, pathlib, subprocess, sys
args = sys.argv[1:]
assert args[:2] == ['factory', 'worker-check']
assert args[2] == '--cas-root' and args[4] == '--'
root = pathlib.Path(args[3]).resolve()
cwd = pathlib.Path.cwd().resolve()
assert cwd.is_relative_to(root / 'worktrees')
assert args[-1] in ['--lib', '--tests']
with open(os.environ['LANE_FIXTURE_LOG'], 'a') as log:
    log.write(json.dumps({'args': args, 'cwd': str(cwd)}) + '\\n')
if args[-1] == os.environ.get('LANE_FIXTURE_FAIL'):
    sys.exit(7)
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
        self.env = dict(os.environ, CAS_LANE_CHECK_CAS=str(self.fake), LANE_FIXTURE_LOG=str(self.log))

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
