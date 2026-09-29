#!/usr/bin/env python3
"""Runner orchestration fixtures; no Rust executable or build is launched."""
import contextlib
import csv
import io
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

SCRIPT = pathlib.Path(__file__).with_name("spike-crate-split-timings.sh")
SOURCE = SCRIPT.read_text().split("<<'PY'\n", 1)[1].rsplit("\nPY", 1)[0]
ARTIFACTS = pathlib.Path.home() / ".cas/artifacts/cas-a4b1"


class RunnerFixture(unittest.TestCase):
    def run_fixture(self, nightly=None, fail=None):
        ARTIFACTS.mkdir(parents=True, exist_ok=True)
        temp = tempfile.TemporaryDirectory(dir=ARTIFACTS)
        self.addCleanup(temp.cleanup)
        root = pathlib.Path(temp.name)
        repo, home = root / "repo", root / "home"
        repo.mkdir()
        (repo / ".git").mkdir()
        (repo / "Cargo.toml").write_text("[workspace]\n")
        (repo / ".cargo").mkdir()
        (repo / ".cargo/config.toml").write_text(
            '[build]\njobs = 2\n[target.x86_64-unknown-linux-gnu]\nrustflags = ["-C", "target-cpu=x86-64"]\n'
        )
        commits = {"base": "a" * 40, "spike": "b" * 40}
        invocations = []

        def output(command, **kwargs):
            if command[:3] == ["git", "rev-parse", "--path-format=absolute"]:
                return str(repo / ".git")
            if command[:2] == ["git", "rev-parse"]:
                return commits[command[-1].split("^")[0]]
            if command[:2] == ["git", "show"]:
                return "[workspace]\n"
            if command == ["rustc", "-vV"]:
                return "rustc fixture\nhost: x86_64-unknown-linux-gnu"
            if command == ["cargo", "--version"]:
                return "cargo fixture"
            if command == ["rustup", "toolchain", "list"]:
                return "nightly-x86_64-unknown-linux-gnu" if nightly == "installed" else "stable-x86_64-unknown-linux-gnu"
            if command[:3] == ["rustup", "component", "list"]:
                return "rustc-codegen-cranelift-preview-x86_64-unknown-linux-gnu"
            raise AssertionError(f"unexpected captured command: {command}")

        def run(command, **kwargs):
            if command[:3] == ["git", "worktree", "add"]:
                worktree = pathlib.Path(command[-2])
                (worktree / "cas-cli/src/hub").mkdir(parents=True)
                (worktree / "cas-cli/src/agent_id.rs").write_text("pub fn core() {}\n")
                (worktree / "cas-cli/src/hub/runtime.rs").write_text("pub fn hub() {}\n")
                if command[-1] == commits["spike"]:
                    (worktree / "crates/cas-hub-state/src").mkdir(parents=True)
                    (worktree / "crates/cas-hub-state/src/runtime.rs").write_text("pub fn hub() {}\n")
                return types.SimpleNamespace(returncode=0)
            if command[:3] == ["git", "worktree", "remove"]:
                shutil.rmtree(command[-1])
                return types.SimpleNamespace(returncode=0)
            if command[:2] == ["rustup", "run"]:
                # Exercise the modern flag fallback without launching rustc.
                return types.SimpleNamespace(returncode=int("-Zthreads=8" in command))
            self.assertEqual(command[0], "cargo")
            self.assertIn("--locked", command)
            self.assertIn("--timings", command)
            target = pathlib.Path(kwargs["env"]["CARGO_TARGET_DIR"])
            self.assertTrue(target.is_relative_to(repo / ".cas/scratch"))
            self.assertEqual(kwargs["env"]["RUSTC_WRAPPER"], "")
            self.assertEqual(kwargs["env"]["RUSTC_WORKSPACE_WRAPPER"], "")
            self.assertIn("target-cpu=x86-64", kwargs["env"]["CARGO_ENCODED_RUSTFLAGS"])
            invocations.append((command, kwargs["env"].copy(), str(kwargs["cwd"])))
            kwargs["stdout"].write(b"fixture: no Rust process launched\n")
            (target / "cargo-timings").mkdir(exist_ok=True)
            (target / "cargo-timings/cargo-timing.html").write_text("fixture timing")
            failed = fail == "all" or fail == len(invocations) - 1
            return types.SimpleNamespace(returncode=1 if failed else 0)

        argv = [str(SCRIPT), "base", "spike"] + (["--nightly"] if nightly else [])
        with patch.object(sys, "argv", argv), patch.object(subprocess, "check_output", side_effect=output), \
             patch.object(subprocess, "run", side_effect=run), patch.object(pathlib.Path, "home", return_value=home), \
             patch.object(shutil, "which", return_value="fixture-rustup"), \
             patch.dict(os.environ, {"CARGO_BUILD_JOBS": "2", "RUSTFLAGS": "", "CARGO_ENCODED_RUSTFLAGS": "-C\x1ftarget-cpu=x86-64"}), \
             contextlib.redirect_stdout(io.StringIO()):
            if fail is not None:
                with self.assertRaises(SystemExit) as error:
                    exec(compile(SOURCE, str(SCRIPT), "exec"), {"__name__": "__main__"})
                self.assertEqual(error.exception.code, 1)
            else:
                exec(compile(SOURCE, str(SCRIPT), "exec"), {"__name__": "__main__"})
        output_dir = next((home / ".cas/artifacts/cas-a4b1").iterdir())
        with (output_dir / "timings.tsv").open() as file:
            rows = list(csv.DictReader(file, delimiter="\t"))
        metadata = json.loads((output_dir / "metadata.json").read_text())
        self.assertFalse(any((repo / ".cas/scratch").rglob("target-*")), "targets are reclaimed")
        self.assertFalse(any((repo / ".cas/scratch").rglob("agent_id.rs")), "worktrees are removed")
        return rows, metadata, (output_dir / "summary.md").read_text(), invocations

    def test_three_samples_per_metric_and_equivalent_coverage_companions(self):
        rows, metadata, summary, calls = self.run_fixture()
        self.assertEqual(len(rows), 54)  # base 7 + spike 11 workloads, three times
        self.assertEqual({row["sample"] for row in rows}, {"1", "2", "3"})
        self.assertEqual(len({row["target_dir"] for row in rows}), 6)
        self.assertNotIn("INCOMPLETE", summary)
        self.assertIn("moved_tests_build_companion", summary)
        self.assertIn("cold_check_tests_equivalent_coverage", summary)
        self.assertEqual(int(metadata["jobs"]), 2)
        self.assertIn("base", calls[0][2])
        self.assertIn("spike", calls[18][2])  # second sample reverses order
        for command, _, _ in calls:
            if "cas-hub-state" in command:
                self.assertIn("--features", command)
                self.assertIn("test-support", command)

    def test_missing_nightly_skips_without_download(self):
        rows, metadata, summary, _ = self.run_fixture(nightly="missing")
        self.assertEqual(len(rows), 54)
        self.assertIn("toolchain missing", summary)
        self.assertEqual(metadata["variants"], ["default-llvm"])

    def test_installed_nightly_probes_frontend_flag_and_cranelift(self):
        rows, metadata, _, calls = self.run_fixture(nightly="installed")
        self.assertEqual(len(rows), 4 * 54)
        self.assertIn("nightly-frontend8", metadata["variants"])
        self.assertTrue(any("--jobs-frontend=8" in env["CARGO_ENCODED_RUSTFLAGS"] for _, env, _ in calls))
        self.assertTrue(any(env.get("CARGO_PROFILE_TEST_CODEGEN_BACKEND") == "cranelift" for _, env, _ in calls))

    def test_failed_workloads_continue_all_samples_and_cleanup(self):
        rows, _, summary, calls = self.run_fixture(fail="all")
        self.assertEqual(len(rows), 54)
        self.assertTrue(all(row["exit_code"] == "1" for row in rows))
        self.assertIn("exit 1", summary)
        self.assertIn("| base | default-llvm | cold_check_tests | 0 | 3 | 0 | INCOMPLETE |", summary)
        self.assertIn("| spike | default-llvm | cold_check_tests_equivalent_coverage | 0 | 3 | 0 | INCOMPLETE |", summary)
        self.assertEqual(len(calls), 54)

    def test_failed_warmup_invalidates_incremental_sample_then_recovers(self):
        rows, _, summary, _ = self.run_fixture(fail=0)
        self.assertEqual(len(rows), 54)
        self.assertEqual(rows[0]["exit_code"], "1")
        self.assertEqual(rows[1]["exit_code"], "0")
        self.assertEqual(rows[1]["prerequisites_ok"], "0")
        self.assertEqual(rows[2]["prerequisites_ok"], "1")
        self.assertIn("| base | default-llvm | incremental_check_split | 2 | 0 | 1 | INCOMPLETE |", summary)
        self.assertIn("| base | default-llvm | incremental_check_core | 3 | 0 | 0 |", summary)

    def test_failure_does_not_stop_later_nightly_variants(self):
        rows, _, summary, _ = self.run_fixture(nightly="installed", fail=7)
        self.assertEqual(len(rows), 216)
        self.assertEqual(sum(row["exit_code"] != "0" for row in rows), 1)
        self.assertIn("| spike | default-llvm | cold_check_tests_equivalent_coverage | 2 | 1 | 0 | INCOMPLETE |", summary)
        self.assertIn("nightly-cranelift", rows[-1]["variant"])


if __name__ == "__main__":
    unittest.main()
