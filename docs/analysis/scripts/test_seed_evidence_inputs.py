#!/usr/bin/env python3
"""Tests for the M1/M2 → M3 join (`seed_evidence_inputs.py`).

The properties under test are the ones that decide whether a verdict can be
trusted: the exposure denominator is not silently filtered, a withdrawn claim
never becomes evidence, a semantic score can never attach to something M3
cannot see, and an unlabelled or rejected candidate never contributes one.
"""

from __future__ import annotations

import json
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import seed_evidence_inputs as sei  # noqa: E402


def make_units_db(path: Path, rows: list[tuple]) -> None:
    """rows: (id, content_hash, unit_type, text, correction_state, claim_key, timestamp, source_key)"""
    conn = sqlite3.connect(path)
    conn.executescript(
        """
        CREATE TABLE evidence_units(
          id INTEGER PRIMARY KEY, content_hash TEXT NOT NULL UNIQUE, unit_type TEXT NOT NULL,
          text TEXT NOT NULL, occurrence_count INTEGER NOT NULL DEFAULT 1,
          claim_key TEXT, correction_state TEXT NOT NULL DEFAULT 'current');
        CREATE TABLE evidence_provenance(
          id INTEGER PRIMARY KEY, unit_id INTEGER NOT NULL, source_key TEXT NOT NULL,
          timestamp TEXT NOT NULL DEFAULT '');
        """
    )
    for unit_id, content_hash, unit_type, text, correction_state, claim_key, timestamp, source_key in rows:
        conn.execute(
            "INSERT INTO evidence_units(id, content_hash, unit_type, text, claim_key, correction_state)"
            " VALUES (?,?,?,?,?,?)",
            (unit_id, content_hash, unit_type, text, claim_key, correction_state),
        )
        conn.execute(
            "INSERT INTO evidence_provenance(unit_id, source_key, timestamp) VALUES (?,?,?)",
            (unit_id, source_key, timestamp),
        )
    conn.commit()
    conn.close()


class ExportEvidenceTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.units = Path(self.tmp.name) / "units.sqlite3"
        make_units_db(
            self.units,
            [
                (1, "h1", "daemon_log", "worker stalled after merge", "current", "claim-a",
                 "2026-08-10T00:00:00Z", "daemon"),
                (2, "h2", "task", "epic refresh after push failure", "current", None,
                 "2026-08-17T15:00:00Z", "tasks"),
                (3, "h3", "daemon_log", "withdrawn claim", "withdrawn", None,
                 "2026-08-17T16:00:00Z", "daemon"),
            ],
        )
        self.addCleanup(self.tmp.cleanup)

    def test_exports_m3_contract_fields(self) -> None:
        evidence, manifest = sei.export_evidence(self.units, None)
        self.assertEqual([item["id"] for item in evidence], ["1", "2"])
        first = evidence[0]
        # M3 indexes these directly; a missing one is a silent dropped unit.
        for key in ("id", "timestamp", "source", "text", "structured"):
            self.assertIn(key, first)
        self.assertEqual(first["source"], "daemon")
        self.assertEqual(manifest["exported"], 2)

    def test_withdrawn_claims_never_become_evidence(self) -> None:
        evidence, manifest = sei.export_evidence(self.units, None)
        self.assertNotIn("3", [item["id"] for item in evidence])
        self.assertEqual(manifest["dropped_corrected_or_withdrawn"], 1)

    def test_structured_tags_carry_type_and_claim_key(self) -> None:
        evidence, _ = sei.export_evidence(self.units, None)
        self.assertEqual(evidence[0]["structured"], ["daemon_log", "claim-a"])
        self.assertEqual(evidence[1]["structured"], ["task"])

    def test_since_filters_only_by_time_and_is_reported(self) -> None:
        evidence, manifest = sei.export_evidence(self.units, "2026-08-15")
        self.assertEqual([item["id"] for item in evidence], ["2"])
        self.assertEqual(manifest["dropped_before_since"], 1)

    def test_export_is_sorted_by_observation_time(self) -> None:
        evidence, _ = sei.export_evidence(self.units, None)
        self.assertEqual(evidence, sorted(evidence, key=lambda item: item["timestamp"]))

    def test_source_database_is_opened_read_only(self) -> None:
        conn = sei.connect_ro(self.units)
        with self.assertRaises(sqlite3.OperationalError):
            conn.execute("DELETE FROM evidence_units")
        conn.close()


class SemanticFromLabelsTest(unittest.TestCase):
    def labels(self, **candidate) -> dict:
        base = {
            "event_id": 7, "evidence_id": "42", "label": True,
            "label_confidence": 0.9, "text": "t", "labelled_by": "reviewer",
            "labelled_at": "2026-08-18T15:00:00Z",
        }
        base.update(candidate)
        return {"pools": [{"fix_id": "cas-aa2b", "candidates": [base]}]}

    def test_positive_label_becomes_a_score_on_its_evidence_id(self) -> None:
        semantic, report = sei.semantic_from_labels(self.labels())
        self.assertEqual(semantic["cas-aa2b"]["scores"], {"42": 0.9})
        self.assertTrue(semantic["cas-aa2b"]["evaluated"])
        self.assertEqual(semantic["cas-aa2b"]["candidates_reviewed"], 1)
        self.assertEqual(semantic["cas-aa2b"]["reviewer"], "reviewer")
        self.assertEqual(report["positive_labels"], 1)

    def test_rejected_candidate_is_an_evaluation_with_zero_positives(self) -> None:
        # The whole point of the declared shape: "we looked and found nothing"
        # is a result M3 may act on, and it must not read as "nobody looked".
        semantic, report = sei.semantic_from_labels(self.labels(label=False))
        self.assertEqual(semantic["cas-aa2b"]["scores"], {})
        self.assertTrue(semantic["cas-aa2b"]["evaluated"])
        self.assertEqual(semantic["cas-aa2b"]["candidates_reviewed"], 1)
        self.assertEqual(report["negative_labels"], 1)
        self.assertEqual(report["fixes_evaluated_with_zero_positives"], 1)

    def test_unlabelled_candidate_contributes_nothing(self) -> None:
        semantic, report = sei.semantic_from_labels(self.labels(label=None))
        self.assertEqual(semantic, {})
        self.assertEqual(report["unlabelled_candidates"], 1)

    def test_positive_label_without_an_evidence_id_withholds_the_whole_fix(self) -> None:
        # M3 looks scores up by evidence id. A positive it cannot attach must
        # not be published as "evaluated, nothing matched" — that would state
        # the opposite of what the reviewer found.
        semantic, report = sei.semantic_from_labels(self.labels(evidence_id=None))
        self.assertEqual(semantic, {})
        self.assertEqual(report["positive_but_unmapped"], 1)
        self.assertEqual(report["fixes_withheld_unmappable_positive"], ["cas-aa2b"])

    def test_positive_label_without_confidence_is_an_error(self) -> None:
        with self.assertRaises(ValueError):
            sei.semantic_from_labels(self.labels(label_confidence=None))

    def test_label_without_a_named_reviewer_is_an_error(self) -> None:
        with self.assertRaises(ValueError):
            sei.semantic_from_labels(self.labels(labelled_by=None))


class LoadSeedsTest(unittest.TestCase):
    def test_missing_required_key_is_named(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "seeds.json"
            path.write_text(json.dumps([{"id": "cas-x", "title": "t"}]))
            with self.assertRaises(ValueError) as ctx:
                sei.load_seeds(path)
            self.assertIn("fix_built_at", str(ctx.exception))


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
