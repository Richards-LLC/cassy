#!/usr/bin/env python3
"""Receipt-backed Dev timing trailer; no Slack writes."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("announce", Path(__file__).with_name("release-train-announce.py"))
announce = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(announce)
DRAFT = '''User top-level
```text
*Live on production — User — Cassy v9.99.1*
Was: upgrades required work. → Now: upgrades are simple.
```
User reply
```text
• *Upgrade* — Installs the published release.
```
Dev top-level
```text
*Live on production — Dev — Cassy v9.99.1*
Was: releases required work. → Now: releases are measured.
```
Dev reply
```text
• *Release proof* — Measures publication.
```
Other prose retained.
'''


class PublicationTimingTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.draft = Path(self.tmp.name) / "draft.md"
        self.draft.write_text(DRAFT)
        self.draft.chmod(0o640)
        self.receipt = Path(self.tmp.name) / "latency.receipt"
        self.receipt.write_text("TAG=v9.99.1\nTAG_PUSHED_AT=2026-08-20T12:00:00Z\n"
                                "PUBLISHED_AT=2026-08-20T12:21:00Z\nPUBLISH_LATENCY_SECONDS=1260\n"
                                "BUDGET_SECONDS=600\nWITHIN_BUDGET=false\nINTERVENTIONS=8\n")

    def record(self):
        announce.record_latency("v9.99.1", str(self.receipt), str(self.draft))

    def test_timing_and_interventions_are_receipt_backed_and_idempotent(self):
        original = announce.extract_bodies(self.draft)
        self.record()
        after = self.draft.read_text()
        bodies = announce.extract_bodies(self.draft)
        self.assertEqual(bodies[0], original[0])
        self.assertEqual(bodies[2], original[2])
        for body in (bodies[1], bodies[3]):
            self.assertIn("8 manual interventions", body)
        self.assertIn("INTERVENTIONS=8", bodies[3])
        self.assertIn("1260s; over budget (600s); WITHIN_BUDGET=false", bodies[3])
        self.assertIn("Other prose retained.", after)
        self.assertEqual(self.draft.stat().st_mode & 0o777, 0o640)
        self.record()
        self.assertEqual(self.draft.read_text(), after)

    def test_fast_release_states_within_budget(self):
        self.receipt.write_text(self.receipt.read_text().replace("12:21:00", "12:04:10")
                                .replace("1260", "250").replace("false", "true"))
        self.record()
        self.assertIn("250s; within budget (600s); WITHIN_BUDGET=true", self.draft.read_text())

    def test_missing_or_incoherent_receipt_does_not_mutate_draft(self):
        original = self.receipt.read_text()
        for source in ("", original.replace("false", "true"), original.replace("1260", "1259"),
                       original.replace("v9.99.1", "v9.99.2"), original.replace("600", "-1"),
                       original.replace("INTERVENTIONS=8\n", ""),
                       original.replace("INTERVENTIONS=8", "INTERVENTIONS=-1"),
                       original.replace("INTERVENTIONS=8", "INTERVENTIONS=unknown"),
                       original + "WITHIN_BUDGET=true\n"):
            with self.subTest(source=source):
                self.receipt.write_text(source)
                with self.assertRaises(ValueError):
                    self.record()
                self.assertEqual(self.draft.read_text(), DRAFT)


if __name__ == "__main__":
    unittest.main()
