#!/usr/bin/env python3
"""Regression coverage for deployed_epoch_verdicts.py."""

import importlib.util
import json
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("deployed_epoch_verdicts.py")
SPEC = importlib.util.spec_from_file_location("deployed_epoch_verdicts", SCRIPT)
assert SPEC and SPEC.loader
module = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = module
SPEC.loader.exec_module(module)


def epoch_db(path: Path) -> None:
    db = sqlite3.connect(path)
    db.execute("CREATE TABLE history_epochs(epoch_kind TEXT, started_at TEXT, ended_at TEXT, binary_mtime TEXT, version TEXT, exe_deleted INTEGER)")
    # The old process lives through 12:20. A merge at 12:00 and a new daemon at
    # 12:10 therefore do not make the 12:12 evidence post-fix.
    db.execute("INSERT INTO history_epochs VALUES(?,?,?,?,?,?)", ("daemon_start", "2026-08-16T11:00:00Z", "2026-08-16T12:20:00Z", "2026-08-16T11:00:00Z", "2.70.0", 0))
    db.execute("INSERT INTO history_epochs VALUES(?,?,?,?,?,?)", ("daemon_start", "2026-08-16T12:10:00Z", "2026-08-16T12:30:00Z", "2026-08-16T12:00:00Z", "2.71.0", 0))
    db.commit(); db.close()


class DeployedEpochVerdictsTests(unittest.TestCase):
    def test_three_states_and_merge_time_trap(self):
        root = Path(__file__).parents[1] / "fixtures"
        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / "cas.db"; epoch_db(database)
            epochs = module.load_epochs(database)
            evidence = module.load_evidence(root / "deployed_epoch_verdicts_evidence.json")
            semantic = module.load_semantic(root / "deployed_epoch_verdicts_semantic.json")
            seeds = json.loads((root / "deployed_epoch_verdicts_seed.json").read_text())
            results = [module.verdict_for(seed, epochs, evidence, semantic) for seed in seeds]
        self.assertEqual([result["state"] for result in results], ["fixed", "recurred", "insufficient-post-fix-data"])
        self.assertEqual(results[0]["exposure"]["mixed"], 1)
        self.assertFalse(any(card["evidence_id"] == "mixed-1" and card["epoch_class"] == "clean-post" for card in results[0]["evidence_cards"]))
        self.assertIsNotNone(results[1]["proposal_draft"])
        self.assertIn("Human review required", results[1]["proposal_draft"]["body"])

    def test_no_epoch_never_becomes_fixed(self):
        fix = {"id": "x", "title": "x", "fix_built_at": "2026-08-16T12:00:00Z", "sample_threshold": 0}
        result = module.verdict_for(fix, [], [], {})
        self.assertEqual(result["state"], "insufficient-post-fix-data")
        self.assertEqual(result["reason"], "no deployed binary epoch contains the fix")


if __name__ == "__main__":
    unittest.main()
