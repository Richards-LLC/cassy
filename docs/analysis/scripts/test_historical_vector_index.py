#!/usr/bin/env python3
"""Focused regression tests for historical_vector_index.py."""

import importlib.util
import sqlite3
import struct
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("historical_vector_index.py")
SPEC = importlib.util.spec_from_file_location("historical_vector_index", SCRIPT)
assert SPEC and SPEC.loader
hvi = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = hvi
SPEC.loader.exec_module(hvi)


class HistoricalVectorIndexTests(unittest.TestCase):
    def test_redacts_credentials_and_email_before_chunking(self):
        text, count = hvi.privacy_redact(
            "Failure contacting service: Authorization: Bearer abcdefghijklmnop and api_key=secret-value; owner test@example.com"
        )
        self.assertNotIn("abcdefghijklmnop", text)
        self.assertNotIn("secret-value", text)
        self.assertNotIn("test@example.com", text)
        self.assertGreaterEqual(count, 3)

    def test_normalization_collapses_storm_ids_paths_and_timings(self):
        a = "message 7070 delivered from /home/pippenz/a in 81ms"
        b = "message 9999 delivered from /home/pippenz/b in 42ms"
        self.assertEqual(hvi.normalized(a), hvi.normalized(b))

    def test_boilerplate_blocks_are_removed(self):
        text = "<skills_instructions>very long repeated payload</skills_instructions>\nThe worker missed the merge receipt and retried delivery."
        result = hvi.strip_boilerplate(text)
        self.assertNotIn("repeated payload", result)
        self.assertIn("missed the merge receipt", result)

    def test_vector_query_returns_provenance(self):
        with tempfile.TemporaryDirectory() as directory:
            db = sqlite3.connect(Path(directory) / "index.db")
            db.executescript(hvi.SCHEMA)
            cur = db.execute("INSERT INTO chunks(content_hash,source_kind,text,embedded) VALUES('h','event','delivery retry failed after wake gate',1)")
            chunk_id = cur.lastrowid
            db.execute("INSERT INTO occurrences(chunk_id,source_path,session_id,task_id,worker,timestamp,epoch,privacy_scope) VALUES(?,?,?,?,?,?,?,?)",
                       (chunk_id, "snapshot:cas.db", "s", "cas-abcd", "worker", "2026-08-01T00:00:00Z", "2.49.0", "project-private"))
            vec = [0.0] * hvi.DIMS
            vec[0] = 1.0
            db.execute("INSERT INTO vectors(chunk_id,vector) VALUES(?,?)", (chunk_id, struct.pack(f"<{hvi.DIMS}f", *vec)))
            result = hvi.hydrate_results(db, hvi.vector_ranking(db, vec), 1)
            self.assertEqual(result[0]["chunk_id"], chunk_id)
            self.assertEqual(result[0]["provenance"][0][2], "cas-abcd")


if __name__ == "__main__":
    unittest.main()
