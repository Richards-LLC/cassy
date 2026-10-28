#!/usr/bin/env python3
"""Receipt integrity regressions; no browser or operator artifacts required."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("journey_bundles", Path(__file__).with_name("journey-bundles.py"))
bundles = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bundles)


class BundleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.artifacts = Path(self.temp.name)

    def receipt(self, ident, title, part=None, status="PASS", stages=1):
        directory = self.artifacts / "journeys" / ident
        if part:
            directory = directory / "parts" / part
        directory.mkdir(parents=True, exist_ok=True)
        output = self.artifacts / "playwright" / (part or ident)
        output.mkdir(parents=True, exist_ok=True)
        (output / "trace.zip").write_bytes(title.encode())
        for name in ["receipt.webm", "final.aria.yml", "final.aria.json"]:
            (directory / name).write_text(title)
        captures = []
        for index in range(1, stages + 1):
            name = f"J{index:02}.png"
            (directory / name).write_text(title)
            captures.append({"title": title, "screenshot": name, "ms": 100})
        (directory / "result.json").write_text(json.dumps({
            "id": ident, "title": title, "status": status,
            "stages": captures, "output_dir": str(output),
        }))
        return directory

    def fold(self, suite_exit=0, commit="a" * 40):
        actions = '1. 0:00.1 Expect "toHaveText" 2ms'
        with patch.object(bundles, "trace_actions", return_value=actions), contextlib.redirect_stdout(io.StringIO()):
            return bundles.main([str(self.artifacts), "tree", commit, str(suite_exit), "hub", "1.63"])

    def test_refuses_overwritten_result_with_stale_stage_screenshots(self):
        # A previous longer attempt left one screenshot beyond this passing result.
        self.receipt("HUB-J13", "start a new session", stages=8)
        self.receipt("HUB-J13", "reconnecting machine", stages=7)
        with self.assertRaisesRegex(ValueError, "HUB-J13.*stale|mixed"):
            self.fold()

    def test_keeps_only_one_trace_for_a_timeout_with_incomplete_stage_evidence(self):
        self.receipt("HUB-J19", "machine alerts", part="forced-colors", status="FAIL", stages=7)
        part = self.artifacts / "journeys/HUB-J19/parts/forced-colors"
        (part / "J08.png").write_text("screenshot completed before the timed-out stage receipt")
        main = self.receipt("HUB-J19", "machine alerts")

        self.assertEqual(self.fold(suite_exit=1), 0)
        bundle = json.loads((main / "bundle.json").read_text())
        self.assertEqual(bundle["verdict"], "FAIL")
        self.assertIn("parts/forced-colors/J08.png", bundle["files"]["cells"])
        self.assertEqual(bundle["incomplete_stages"], [{
            "screenshot": "parts/forced-colors/J08.png",
            "status": "INCOMPLETE",
            "reason": "suite timed out after screenshot write and before stage receipt",
        }])
        failed_part = bundle["files"]["parts"][0]
        self.assertEqual(failed_part["trace"], "parts/forced-colors/trace.zip")
        self.assertTrue((part / "trace.zip").is_file())
        self.assertFalse((self.artifacts / "playwright/HUB-J19/trace.zip").exists())
        self.assertFalse((self.artifacts / "playwright/forced-colors/trace.zip").exists())
        passing_trace = main / bundle["files"]["trace"]
        failed_trace = main / failed_part["trace"]
        self.assertEqual(sorted(main.rglob(passing_trace.name)), sorted([passing_trace, failed_trace]))
        self.assertTrue((main / bundle["files"]["trace_actions"]).is_file())

    def test_parts_keep_each_tests_media_and_cells_and_one_catalog_row(self):
        main = self.receipt("HUB-J13", "start a new session", stages=10)
        part = self.receipt("HUB-J13", "reconnecting machine", part="reconnecting", stages=3)
        for ident in ["HUB-J14", "HUB-J15", "HUB-J16"]:
            self.receipt(ident, ident)
        self.assertEqual(self.fold(), 0)
        data = json.loads((main / "bundle.json").read_text())
        self.assertEqual(data["verdict"], "PASS")
        self.assertEqual(len(data["files"]["cells"]), 13)
        self.assertEqual(data["files"]["parts"][0]["receipt"], "parts/reconnecting/receipt.webm")
        passing_trace = main / data["files"]["trace"]
        actions = main / data["files"]["trace_actions"]
        # pin: qa_evidence requires at least one trace-actions Expect row.
        self.assertIn('Expect "toHaveText"', actions.read_text())
        self.assertTrue(passing_trace.is_file())
        self.assertEqual(list(main.rglob(passing_trace.name)), [passing_trace])
        self.assertFalse((self.artifacts / "playwright/HUB-J13/trace.zip").exists())
        self.assertFalse((self.artifacts / "playwright/reconnecting/trace.zip").exists())
        rows = (self.artifacts / "journeys/JOURNEYS.md").read_text()
        for ident in ["HUB-J13", "HUB-J14", "HUB-J15", "HUB-J16"]:
            self.assertEqual(rows.count(f"| {ident} |"), 1)
        self.assertIn("| HUB-J13 | start a new session | PASS |", rows)

    def test_doc_only_rebundle_keeps_the_executed_trace_and_its_revision_cas_167c(self):
        main = self.receipt("HUB-J5", "reply by typing")
        executed, documented = "a" * 40, "b" * 40
        self.assertEqual(self.fold(commit=executed), 0)
        trace = main / "trace.zip"
        ran_at = trace.stat().st_mtime_ns
        # A documentation-only commit rebundles the same run: no browser, so
        # the trace already sits at its canonical path.
        self.assertEqual(self.fold(commit=documented), 0)
        data = json.loads((main / "bundle.json").read_text())
        self.assertEqual(data["files"]["trace"], "trace.zip")
        self.assertEqual(data["files"]["trace_actions"], "trace-actions.txt")
        self.assertEqual(data["head_sha"], documented)
        self.assertEqual(data["executed_head_sha"], executed)
        self.assertEqual(trace.stat().st_mtime_ns, ran_at)

    def test_failed_part_makes_the_whole_journey_fail(self):
        main = self.receipt("HUB-J13", "start a new session")
        self.receipt("HUB-J13", "reconnecting machine", part="reconnecting", status="FAIL")
        self.fold()
        self.assertEqual(json.loads((main / "bundle.json").read_text())["verdict"], "FAIL")

    def test_same_leaf_title_variants_keep_both_receipts_and_full_title_paths(self):
        for theme in ["light", "dark"]:
            part = self.receipt("HUB-J17", "phone feedback", part=f"phone-feedback-{theme}")
            result = json.loads((part / "result.json").read_text())
            result.update(project="journeys", title_path=["fleet-ops.journey.ts", f"phone feedback {theme}", "HUB-J17 phone feedback"])
            (part / "result.json").write_text(json.dumps(result))
            (part / "receipt.webm").write_text(theme)
        self.assertEqual(self.fold(), 0)
        directory = self.artifacts / "journeys/HUB-J17"
        data = json.loads((directory / "bundle.json").read_text())
        parts = data["files"]["parts"]
        self.assertEqual(len(parts), 2)
        self.assertEqual(len(data["files"]["cells"]), 2)
        self.assertEqual({part["title"] for part in parts}, {"phone feedback"})
        self.assertEqual({part["project"] for part in parts}, {"journeys"})
        self.assertEqual({part["title_path"][1] for part in parts}, {"phone feedback light", "phone feedback dark"})
        self.assertEqual({(directory / part["receipt"]).read_text() for part in parts}, {"light", "dark"})

    def test_real_hub_part_preserves_transport(self):
        part = self.receipt("HUB-J11", "real disposable hub", part="real-hub-run-0")
        result = json.loads((part / "result.json").read_text())
        result.update(label="real-bundle, real-hub, real-factory-daemon", output_dir=str(part))
        (part / "result.json").write_text(json.dumps(result))
        self.assertEqual(self.fold(), 0)
        data = json.loads((part.parent.parent / "bundle.json").read_text())
        self.assertIn("real-hub", data["label"])
        self.assertNotIn("protocol-double", data["label"])
        self.assertIn("real-hub", data["files"]["parts"][0]["label"])


if __name__ == "__main__":
    unittest.main()
