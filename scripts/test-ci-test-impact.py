#!/usr/bin/env python3
"""Exercise impact selection against real Git fixtures; never invokes Cargo."""
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import urllib.request
import zipfile

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("impact", HERE / "ci-test-impact.py")
impact = importlib.util.module_from_spec(spec)
spec.loader.exec_module(impact)


class ImpactTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.put(".gitignore", "history.json\noutput/\nfake-cargo\n")
        self.put("Cargo.toml", '[workspace]\nmembers=["cas-cli", "crates/shared", "crates/consumer"]\n')
        self.put("cas-cli/Cargo.toml", '[package]\nname="cas"\nversion="0.1.0"\n[dependencies]\nshared={path="../crates/shared"}\n')
        self.put("crates/shared/Cargo.toml", '[package]\nname="shared"\nversion="0.1.0"\n')
        self.put("crates/consumer/Cargo.toml", '[package]\nname="consumer"\nversion="0.1.0"\n[dependencies]\nshared={path="../shared"}\n')
        self.put("cas-cli/src/lib.rs", 'pub mod alpha; pub mod beta; pub mod gamma;\n')
        self.put("cas-cli/src/alpha.rs", 'pub fn value() {}\n')
        self.put("cas-cli/src/beta.rs", 'use crate::alpha::value;\n')
        self.put("cas-cli/src/gamma.rs", 'pub fn other() {}\n')
        self.put("cas-cli/tests/alpha_test.rs", 'use cas::alpha::value;\n')
        self.put("cas-cli/tests/beta_test.rs", 'use cas::beta::value;\n')
        self.put("cas-cli/tests/gamma_test.rs", 'use cas::gamma::other;\n')
        self.put("cas-cli/tests/opaque_test.rs", 'use std::process::Command;\n')
        self.put("cas-cli/tests/builtin_archive_portability_test.rs", '// archive\n')
        self.put("crates/shared/src/lib.rs", '// shared\n')
        self.put("scripts/run-verified-tests.sh", (HERE / "run-verified-tests.sh").read_text(), executable=True)
        self.git("init", "-q")
        self.git("config", "user.email", "test@example.com")
        self.git("config", "user.name", "Test")
        self.commit()
        self.base = self.git("rev-parse", "HEAD")
        self.history = self.root / "history.json"

    def put(self, path, text, executable=False):
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)
        if executable:
            target.chmod(0o755)

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.root, text=True).strip()

    def commit(self):
        self.git("add", ".")
        self.git("commit", "-qm", "fixture")

    def select(self, path="cas-cli/src/alpha.rs"):
        with (self.root / path).open("a") as handle:
            handle.write('// changed\n')
        self.commit()
        return impact.plan(self.root, self.base, "", self.history)

    def test_module_consumers_and_opaque_entry_points_are_retained(self):
        plan = self.select()
        command = plan["commands"][0]
        self.assertEqual(command["package"], "cas")
        self.assertIn("--lib", command["args"])
        self.assertIn("alpha_test", command["stems"])
        self.assertIn("beta_test", command["stems"])
        self.assertIn("opaque_test", command["stems"])
        self.assertNotIn("gamma_test", command["stems"])

    def test_reverse_crate_dependencies_widen_to_all_consumers(self):
        plan = self.select("crates/shared/src/lib.rs")
        self.assertEqual({c["package"] for c in plan["commands"]}, {"shared", "consumer", "cas"})
        self.assertTrue(all(c["stems"] is None for c in plan["commands"]))

    def test_test_only_edit_selects_suite_and_archive_guard(self):
        command = self.select("cas-cli/tests/alpha_test.rs")["commands"][0]
        self.assertEqual(command["stems"], ["alpha_test", "builtin_archive_portability_test"])
        self.assertNotIn("--lib", command["args"])

    def test_missing_diff_base_runs_workspace(self):
        self.assertEqual(impact.plan(self.root, "missing-base", "", None)["mode"], "workspace")

    def test_zero_base_uses_trusted_fallback(self):
        self.select()
        self.assertEqual(impact.plan(self.root, "0" * 40, self.base, None)["mode"], "scoped")

    def test_unmapped_input_runs_workspace(self):
        self.put("unknown.bin", 'new input\n')
        self.commit()
        self.assertEqual(impact.plan(self.root, self.base, "", None)["mode"], "workspace")

    def test_bad_history_widens_scope(self):
        self.history.write_text('{broken')
        self.assertEqual(self.select()["mode"], "workspace")

    def test_recorded_failure_only_adds_scope(self):
        self.history.write_text(json.dumps({"failures": [{"package": "cas", "binary": "gamma_test", "test": "x"}], "uncertain": False}))
        self.assertIn("gamma_test", self.select()["commands"][0]["stems"])

    def test_grouped_inventory_uses_stem_module_filters(self):
        helper = '#!/usr/bin/env python3\nimport sys\nif "--check" in sys.argv: sys.exit(0)\nprint("alpha_test|integration_contracts\\nbeta_test|integration_contracts\\ngamma_test|integration_cli\\nopaque_test|integration_factory\\nbuiltin_archive_portability_test|builtin_archive_portability_test")\n'
        self.put("scripts/cas-test-targets.py", helper)
        self.commit()
        self.base = self.git("rev-parse", "HEAD")
        command = self.select("cas-cli/tests/alpha_test.rs")["commands"][0]
        expression = command["args"][command["args"].index("-E") + 1]
        self.assertIn("binary(integration_contracts) and test(alpha_test::)", expression)
        self.assertNotIn("gamma_test::", expression)

    def test_full_failure_outside_plan_is_a_recall_miss(self):
        plan = self.select()
        log = self.root / "run.log"
        log.write_text('FAIL [ 0.01s] cas::gamma_test missing\nSummary [ 1.0s] 4 tests run: 3 passed, 1 failed\n')
        receipt = impact.record(plan, [log], 1, 1.0, self.root / "receipt.json", "full")
        self.assertEqual(receipt["selected_test_count"], 4)
        self.assertEqual(len(receipt["post_merge_missed"]), 1)
        self.assertEqual(receipt["recall"], 0)

    def test_success_without_failures_does_not_claim_perfect_recall(self):
        plan = self.select()
        log = self.root / "run.log"
        log.write_text('Summary [ 1.0s] 4 tests run: 4 passed\n')
        receipt = impact.record(plan, [log], 0, 1.0, self.root / "receipt.json", "full")
        self.assertIsNone(receipt["recall"])

    def test_history_api_failure_widens_future_selection(self):
        with patch.dict(os.environ, {}, clear=True):
            impact.collect_history(self.history)
        self.assertEqual(self.select()["mode"], "workspace")

    def test_history_download_adds_failure_and_strips_cross_host_auth(self):
        failure = {"package": "cas", "binary": "gamma_test", "test": "example"}
        archive = io.BytesIO()
        with zipfile.ZipFile(archive, "w") as zipped:
            zipped.writestr("receipt.json", json.dumps({"failures": [failure], "uncertain": False}))
        payloads = [json.dumps({"artifacts": [{"name": "ci-test-impact-fixture", "id": 123, "expired": False}]}).encode(), archive.getvalue()]
        class Opener:
            def open(self, request, timeout):
                if "artifacts/123/zip" not in request.full_url:
                    assert request.get_header("Authorization") == "Bearer fixture-token"
                return io.BytesIO(payloads.pop(0))
        handlers = []
        def build_opener(handler):
            handlers.append(handler)
            return Opener()
        with patch.dict(os.environ, {"GITHUB_REPOSITORY": "fixture/repo", "GH_TOKEN": "fixture-token"}), patch.object(impact.urllib.request, "build_opener", build_opener):
            impact.collect_history(self.history)
        self.assertEqual(json.loads(self.history.read_text())["failures"], [failure])
        request = urllib.request.Request("https://api.github.com/path", headers={"Authorization": "Bearer fixture-token"})
        redirected = handlers[0].redirect_request(request, None, 302, "", {}, "https://storage.example/receipt")
        self.assertIsNone(redirected.get_header("Authorization"))
        self.assertIn("gamma_test", self.select()["commands"][0]["stems"])

    def test_opaque_source_import_widens_to_its_consumer_module(self):
        self.put("cas-cli/src/gamma.rs", "use crate::*;\n")
        self.commit()
        self.base = self.git("rev-parse", "HEAD")
        command = self.select()["commands"][0]
        self.assertIn("gamma_test", command["stems"])
        self.assertIn("test(gamma::)", command["args"][-1])

    def test_real_runner_records_count_time_and_failed_no_test_run(self):
        self.select("cas-cli/tests/alpha_test.rs")
        fake = self.root / "fake-cargo"
        fake.write_text('#!/bin/sh\necho "Summary [ 0.01s] 2 tests run: 2 passed"\n')
        fake.chmod(0o755)
        env = dict(os.environ, CARGO=str(fake))
        destination = self.root / "output/receipt.json"
        command = [str(HERE / "ci-test-impact.py"), "run", "--base-sha", self.base, "--history", str(self.history), "--out", str(destination)]
        result = subprocess.run(command, cwd=self.root, env=env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        receipt = json.loads(destination.read_text())
        self.assertEqual(receipt["selected_test_count"], 2)
        self.assertGreater(receipt["elapsed_seconds"], 0)
        fake.write_text('#!/bin/sh\ncase "$*" in *-E*) echo "Summary [ 0.01s] 0 tests run: 0 passed" ;; *) echo "Summary [ 0.01s] 2 tests run: 2 passed" ;; esac\n')
        result = subprocess.run(command, cwd=self.root, env=env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIsNone(json.loads(destination.read_text())["plan"]["commands"][0]["stems"])
        fake.write_text('#!/bin/sh\necho "Summary [ 0.01s] 0 tests run: 0 passed"\n')
        result = subprocess.run(command, cwd=self.root, env=env, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(json.loads(destination.read_text())["uncertain"])


if __name__ == "__main__":
    unittest.main()
