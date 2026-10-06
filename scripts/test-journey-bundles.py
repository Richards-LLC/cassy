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

    def fold(self):
        with patch.object(bundles, "trace_actions", return_value="trace actions"), contextlib.redirect_stdout(io.StringIO()):
            return bundles.main([str(self.artifacts), "tree", "commit", "0", "hub", "1.63"])

    def test_refuses_overwritten_result_with_stale_stage_screenshots(self):
        # F34: the extra three-stage test overwrote the ten-stage main result.
        self.receipt("HUB-J13", "start a new session", stages=10)
        self.receipt("HUB-J13", "reconnecting machine", stages=3)
        with self.assertRaisesRegex(ValueError, "HUB-J13.*stale|mixed"):
            self.fold()

    def test_parts_keep_each_tests_media_trace_and_cells_and_one_catalog_row(self):
        main = self.receipt("HUB-J13", "start a new session", stages=10)
        part = self.receipt("HUB-J13", "reconnecting machine", part="reconnecting", stages=3)
        for ident in ["HUB-J14", "HUB-J15", "HUB-J16"]:
            self.receipt(ident, ident)
        self.assertEqual(self.fold(), 0)
        data = json.loads((main / "bundle.json").read_text())
        self.assertEqual(data["verdict"], "PASS")
        self.assertEqual(len(data["files"]["cells"]), 13)
        self.assertEqual((main / "trace.zip").read_text(), "start a new session")
        self.assertEqual((part / "trace.zip").read_text(), "reconnecting machine")
        self.assertEqual(data["files"]["parts"][0]["receipt"], "parts/reconnecting/receipt.webm")
        rows = (self.artifacts / "journeys/JOURNEYS.md").read_text()
        for ident in ["HUB-J13", "HUB-J14", "HUB-J15", "HUB-J16"]:
            self.assertEqual(rows.count(f"| {ident} |"), 1)
        self.assertIn("| HUB-J13 | start a new session | PASS |", rows)

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

    def test_real_hub_part_preserves_transport_and_its_already_scrubbed_trace(self):
        part = self.receipt("HUB-J11", "real disposable hub", part="real-hub-run-0")
        result = json.loads((part / "result.json").read_text())
        result.update(label="real-bundle, real-hub, real-factory-daemon", output_dir=str(part))
        (part / "result.json").write_text(json.dumps(result))
        (part / "trace.zip").write_bytes(b"scrubbed real trace")
        self.assertEqual(self.fold(), 0)
        data = json.loads((part.parent.parent / "bundle.json").read_text())
        self.assertIn("real-hub", data["label"])
        self.assertNotIn("protocol-double", data["label"])
        self.assertEqual((part / "trace.zip").read_bytes(), b"scrubbed real trace")
        self.assertIn("real-hub", data["files"]["parts"][0]["label"])


if __name__ == "__main__":
    unittest.main()
