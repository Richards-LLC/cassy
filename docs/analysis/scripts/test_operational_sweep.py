#!/usr/bin/env python3
"""Focused tests for the artifact-only M1 -> M4 operational sweep."""

from __future__ import annotations

import json
import sqlite3
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import operational_sweep as sweep  # noqa: E402


def make_units(path: Path) -> None:
    db = sqlite3.connect(path)
    db.executescript(
        """
        CREATE TABLE evidence_units(
          id INTEGER PRIMARY KEY, content_hash TEXT UNIQUE, unit_type TEXT, text TEXT,
          occurrence_count INTEGER, redaction_secrets INTEGER, redaction_emails INTEGER,
          first_seen_at TEXT, correction_state TEXT);
        CREATE TABLE evidence_provenance(
          id INTEGER PRIMARY KEY, unit_id INTEGER, source_key TEXT, source_path TEXT,
          source_locator TEXT, session_id TEXT DEFAULT '', task_id TEXT DEFAULT '', worker TEXT DEFAULT '',
          commit_sha TEXT DEFAULT '', file_path TEXT DEFAULT '', symbol TEXT DEFAULT '', timestamp TEXT,
          epoch TEXT DEFAULT '', epoch_version TEXT DEFAULT '', privacy_scope TEXT DEFAULT '', host TEXT DEFAULT '',
          project TEXT DEFAULT '', team TEXT DEFAULT '');
        CREATE TABLE ingest_watermarks(
          source_key TEXT PRIMARY KEY, source_path TEXT, cursor_kind TEXT, byte_offset INTEGER,
          line_offset INTEGER, row_cursor INTEGER, size_bytes INTEGER, inode TEXT, updated_at TEXT);
        CREATE TABLE redaction_receipts(run_at TEXT, source_key TEXT, secrets INTEGER, emails INTEGER, receipt_hash TEXT);
        CREATE TABLE retention_receipts(run_at TEXT, privacy_scope TEXT, cutoff TEXT, provenance_deleted INTEGER,
          units_deleted INTEGER, receipt_hash TEXT);
        """
    )
    rows = [
        (1, "hash-current", "task", "suite passed but zero tests ran " + "x" * 400, 7, 2, 1, "2026-08-12T00:00:00Z", "current"),
        (2, "hash-withdrawn", "task", "withdrawn claim", 99, 0, 0, "2026-08-13T00:00:00Z", "withdrawn"),
        (3, "hash-old", "task", "old evidence", 2, 0, 0, "2026-08-01T00:00:00Z", "current"),
    ]
    db.executemany("INSERT INTO evidence_units VALUES (?,?,?,?,?,?,?,?,?)", rows)
    db.executemany(
        "INSERT INTO evidence_provenance(id,unit_id,source_key,source_path,task_id,timestamp) VALUES (?,?,?,?,?,?)",
        [
            (1, 1, "db:task", "/private/live.db", "cas-a111", "2026-08-12T00:00:00Z"),
            (2, 1, "db:task", "/private/live.db", "cas-a111", "2026-08-14T00:00:00Z"),
            (3, 2, "db:task", "/private/live.db", "cas-b222", "2026-08-13T00:00:00Z"),
            (4, 3, "db:task", "/private/live.db", "cas-c333", "2026-08-01T00:00:00Z"),
        ],
    )
    db.execute("INSERT INTO redaction_receipts VALUES ('2026-08-12','db:task',2,1,'redacted')")
    db.execute("INSERT INTO retention_receipts VALUES ('2026-08-12','project','2026-08-01',4,2,'retained')")
    db.commit()
    db.close()


def make_stub(path: Path) -> None:
    path.write_text(textwrap.dedent(
        """
        import json, pathlib, sys
        args = sys.argv[1:]
        if 'query' in args:
            if 'FAIL_GATE' in args:
                print(json.dumps({'gate_available': False, 'rows': []}))
            else:
                print(json.dumps({'gate_available': True, 'latency_ms': 3.5, 'rows': [{
                    'event_id': 9, 'content_hash': 'hash-current', 'duplicate_count': 7,
                    'score': 0.03, 'text': 'suite passed but zero tests ran',
                    'provenance': [{'timestamp': '2026-08-12T00:00:00Z', 'task_id': 'cas-a111', 'worker': 'w'}]
                }]}))
        elif 'evidence' in args:
            output = pathlib.Path(args[args.index('--output') + 1]); output.write_text('[]\\n')
            print(json.dumps({'units': 0}))
        elif '--output-json' in args:
            output = pathlib.Path(args[args.index('--output-json') + 1])
            report = pathlib.Path(args[args.index('--output-report') + 1])
            output.write_text(json.dumps({'verdicts': [{
                'id': 'cas-fix1', 'state': 'fixed', 'reason': 'clean observed window',
                'epoch_evidence': {'clean_post_from': '2026-08-12T00:00:00Z'},
                'exposure': {'clean_post': 100, 'threshold': 100}, 'evidence_cards': []
            }]}))
            report.write_text('private intermediate')
        elif 'queue' in args:
            output = pathlib.Path(args[args.index('--output') + 1])
            output.write_text(json.dumps({'counts': {'items': 0}, 'items': []}))
            print(json.dumps({'items': 0}))
        else:
            raise SystemExit(2)
        """
    ))


class EvidenceTest(unittest.TestCase):
    def test_new_evidence_uses_first_observation_and_honors_corrections(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "units.db"
            make_units(path)
            cards, metadata = sweep.evidence_rows(path, "2026-08-10T00:00:00Z", 10)
            self.assertEqual([card["card_id"] for card in cards], ["eu:1"])
            self.assertEqual(cards[0]["timestamp"], "2026-08-12T00:00:00Z")
            self.assertLessEqual(len(cards[0]["excerpt"]), 280)
            self.assertNotIn("source_path", cards[0]["provenance"][0])
            self.assertEqual(metadata["receipts_honored"]["redaction"][0]["receipt_hash"], "redacted")
            self.assertEqual(metadata["receipts_honored"]["retention"][0]["receipt_hash"], "retained")

    def test_issue_draft_has_house_headings_and_no_filing_path(self) -> None:
        finding = {"id": "x", "title": "candidate", "statement": "observed", "expected": "expected", "evidence_card_ids": ["eu:1"]}
        drafts = sweep.proposals([finding], {"eu:1": {"card_id": "eu:1", "excerpt": "actual"}}, "python sweep.py run --config c.json")
        body = drafts[0]["issue_body"]
        self.assertEqual([line[3:] for line in body.splitlines() if line.startswith("## ")], list(sweep.ISSUE_HEADINGS))
        self.assertTrue(drafts[0]["never_auto_file"])
        source = Path(sweep.__file__).read_text()
        self.assertNotIn("gh issue create", source)
        self.assertNotIn("mcp__cs__task", source)


class EndToEndTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        root = Path(self.tmp.name)
        self.units = root / "units.db"; make_units(self.units)
        self.index = root / "index.db"; sqlite3.connect(self.index).close()
        self.epochs = root / "epochs.db"; sqlite3.connect(self.epochs).close()
        self.memories = root / "memories.db"; sqlite3.connect(self.memories).close()
        self.seeds = root / "seeds.json"; self.seeds.write_text("[]\n")
        self.stub = root / "stub.py"; make_stub(self.stub)
        self.artifacts = root / "artifacts"

    def config(self) -> dict:
        return {
            "artifact_root": str(self.artifacts), "units_db": str(self.units), "index": str(self.index),
            "epochs_db": str(self.epochs), "memories_db": str(self.memories), "seeds": str(self.seeds),
            "m2_script": str(self.stub), "m3_script": str(self.stub), "m4_script": str(self.stub),
            "join_script": str(self.stub), "since": "2026-08-10T00:00:00Z", "window_start": "2026-08-10T00:00:00Z",
            "probes": [{"id": "suite", "family": "recurring_failure", "title": "silent suite",
                        "query": "suite", "expected": "tests execute"}],
        }

    def test_success_records_receipts_and_advances_artifact_state(self) -> None:
        report = sweep.run(self.config(), ["python", "operational_sweep.py", "run", "--config", "config.json"])
        payload = json.loads(report.with_name("report.json").read_text())
        self.assertTrue(payload["claims"])
        self.assertTrue(all(claim["evidence_card_ids"] for claim in payload["claims"]))
        self.assertEqual(payload["run"]["cost"]["model_turns"], 0)
        self.assertEqual(payload["run"]["cost"]["m2_queries"], 1)
        self.assertGreater(payload["run"]["latency_ms"], 0)
        self.assertGreater(payload["run"]["storage_bytes"], 0)
        self.assertEqual(len(payload["run"]["stage_receipts"]), 4)
        self.assertTrue((self.artifacts / "state.json").exists())
        self.assertFalse(any("private-stage" in str(path) for path in report.parent.iterdir()))
        markdown = report.read_text()
        self.assertIn("Top recurring failure narratives", markdown)
        self.assertIn("never auto-filed", markdown)

    def test_m2_gate_failure_is_fail_closed_and_does_not_advance_state(self) -> None:
        config = self.config()
        config["probes"][0]["query"] = "FAIL_GATE"
        with self.assertRaises(sweep.SweepError):
            sweep.run(config, ["sweep"])
        self.assertFalse((self.artifacts / "state.json").exists())
        failure = next((self.artifacts / "runs").glob("*/failed-run.json"))
        self.assertFalse(json.loads(failure.read_text())["state_advanced"])


if __name__ == "__main__":
    unittest.main()
