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

    def evaluated_case(self, semantic):
        """A fix with adequate clean-post exposure and no symptom match.

        Everything except the semantic evaluation is held constant, so the
        verdict below is decided by that evaluation alone.
        """
        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / "cas.db"; epoch_db(database)
            epochs = module.load_epochs(database)
        evidence = [module.Evidence("e1", module.parse_time("2026-08-16T12:25:00Z"), "unrelated chatter", "log")]
        fix = {"id": "cas-x", "title": "x", "fix_built_at": "2026-08-16T12:00:00Z",
               "lexical_terms": ["symptom"], "sample_threshold": 1, "semantic_threshold": 0.8}
        return module.verdict_for(fix, epochs, evidence, semantic)

    def test_empty_map_still_reads_as_not_evaluated(self):
        # The legacy bare shape cannot say "reviewed, nothing matched", so an
        # empty one must keep its fail-closed meaning.
        result = self.evaluated_case({"cas-x": {}})
        self.assertEqual(result["state"], "insufficient-post-fix-data")
        self.assertEqual(result["reason"], "M2 semantic evidence has not been evaluated for this fix")

    def test_declared_evaluation_with_zero_positives_can_reach_fixed(self):
        result = self.evaluated_case({
            "cas-x": {"evaluated": True, "candidates_reviewed": 15, "reviewer": "daniel", "scores": {}}
        })
        self.assertEqual(result["state"], "fixed")
        self.assertEqual(result["matching"]["semantic_evaluation"],
                         {"evaluated": True, "positives": 0, "candidates_reviewed": 15,
                          "reviewer": "daniel", "reviewed_at": None})

    def test_declared_evaluation_of_nothing_is_refused(self):
        # "evaluated" with no candidates behind it would release the one guard
        # between an unobserved fix and a `fixed` verdict.
        with self.assertRaises(ValueError):
            self.evaluated_case({"cas-x": {"evaluated": True, "candidates_reviewed": 0, "reviewer": "daniel", "scores": {}}})

    def test_declared_evaluation_without_a_reviewer_is_refused(self):
        with self.assertRaises(ValueError):
            self.evaluated_case({"cas-x": {"evaluated": True, "candidates_reviewed": 3, "scores": {}}})

    def test_scores_under_evaluated_false_are_contradictory(self):
        with self.assertRaises(ValueError):
            self.evaluated_case({"cas-x": {"evaluated": False, "scores": {"e1": 0.9}}})

    def test_declared_evaluation_still_matches_its_positives(self):
        result = self.evaluated_case({
            "cas-x": {"evaluated": True, "candidates_reviewed": 4, "reviewer": "daniel", "scores": {"e1": 0.95}}
        })
        self.assertEqual(result["state"], "recurred")
        self.assertEqual([card["evidence_id"] for card in result["evidence_cards"]], ["e1"])

    def test_loader_round_trips_both_shapes(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "semantic.json"
            path.write_text(json.dumps({
                "legacy": {"e1": 0.9},
                "declared": {"evaluated": True, "candidates_reviewed": 2, "reviewer": "daniel",
                             "reviewed_at": "2026-08-18T15:00:00Z", "scores": {}},
            }))
            loaded = module.load_semantic(path)
        self.assertEqual(loaded["legacy"], module.SemanticEvaluation(True, {"e1": 0.9}, 1))
        self.assertEqual(loaded["declared"].summary["candidates_reviewed"], 2)
        self.assertTrue(loaded["declared"].evaluated)

    def test_no_epoch_never_becomes_fixed(self):
        fix = {"id": "x", "title": "x", "fix_built_at": "2026-08-16T12:00:00Z", "sample_threshold": 0}
        result = module.verdict_for(fix, [], [], {})
        self.assertEqual(result["state"], "insufficient-post-fix-data")
        self.assertEqual(result["reason"], "no deployed binary epoch contains the fix")


if __name__ == "__main__":
    unittest.main()
