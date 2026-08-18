#!/usr/bin/env python3
"""Acceptance coverage for evidence_units.py (cas-b78b / M1).

Each test maps to a stated acceptance criterion:

* read-only + zero writes to memory/knowledge  -> ReadOnlyTests
* incremental, resumable, scoped               -> ResumabilityTests, ScopeTests
* full provenance and joins                    -> ProvenanceJoinTests
* cas-9d92 reproduction without withdrawn claims -> ReproductionTests
* retention + deletion/redaction receipts      -> RetentionTests
"""

import argparse
import hashlib
import importlib.util
import json
import shutil
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("evidence_units.py")
SPEC = importlib.util.spec_from_file_location("evidence_units", SCRIPT)
assert SPEC and SPEC.loader
module = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = module
SPEC.loader.exec_module(module)

CLAIMS = Path(__file__).parents[1] / "evidence_claims.json"

# Long enough to clear the meaningful() floor of 48 chars / 7 words.
WITHDRAWN_CLAIM_TEXT = (
    "Investigation update for cas-9d92: there is no reconciliation code path for "
    "hook surfaced prompts anywhere in the delivery pipeline, so every queued row stays pending forever."
)
CORRECTION_TEXT = (
    "Correction to the cas-9d92 report: the claim that there is no reconciliation code path "
    "is withdrawn. Current source handles SurfacingSource HookSurfaced and stamps acked_via hook_surfaced."
)
COMMIT_TEXT = (
    "Delivered the queue reconciliation fix in commit abc1234def5678901234567890abcdef12345678 "
    "which touched the prompt queue store and its regression coverage for surfaced rows."
)
PLAIN_TEXT = (
    "Supervisor decision recorded for cas-1234: the worker rebased onto the epic branch and "
    "re-ran the scoped validation lane before requesting a merge review."
)
SECRET_TEXT = (
    "Deployment note for cas-5678: exported api_key=supersecretvalue123 and mailed operator@example.com "
    "before restarting the daemon on the build host tonight."
)


def build_source_db(path: Path) -> None:
    """A miniature coordination DB with the tables ingestion reads, plus the
    memory (`entries`) and knowledge (`knowledge_pages`) stores it must not touch."""
    db = sqlite3.connect(path)
    db.executescript(
        """
        CREATE TABLE tasks(id TEXT PRIMARY KEY, title TEXT, description TEXT, design TEXT,
          acceptance_criteria TEXT, notes TEXT, close_reason TEXT, assignee TEXT, team_id TEXT,
          created_at TEXT, updated_at TEXT);
        CREATE TABLE events(id INTEGER PRIMARY KEY, event_type TEXT, entity_id TEXT, summary TEXT,
          created_at TEXT, session_id TEXT);
        CREATE TABLE prompt_queue(id INTEGER PRIMARY KEY, dedupe_key TEXT, target TEXT, summary TEXT,
          prompt TEXT, created_at TEXT, processed_at TEXT, delivery_attempts INTEGER,
          last_pending_reason TEXT);
        CREATE TABLE supervisor_queue(id INTEGER PRIMARY KEY, supervisor_id TEXT, event_type TEXT,
          payload TEXT, created_at TEXT, processed_at TEXT);
        CREATE TABLE history_epochs(id INTEGER PRIMARY KEY, epoch_kind TEXT, binary_path TEXT,
          binary_mtime TEXT, version TEXT, started_at TEXT, ended_at TEXT, pid INTEGER,
          exe_deleted INTEGER, recorded_at TEXT);
        CREATE TABLE history_commits(sha TEXT PRIMARY KEY, short_sha TEXT, committed_at TEXT,
          subject TEXT, repository TEXT, indexed_at TEXT);
        CREATE TABLE history_commit_files(sha TEXT, file_path TEXT, change_type TEXT, PRIMARY KEY(sha, file_path));
        CREATE TABLE history_commit_symbols(sha TEXT, symbol_id TEXT, qualified_name TEXT, file_path TEXT,
          PRIMARY KEY(sha, symbol_id));
        CREATE TABLE code_files(id TEXT PRIMARY KEY, path TEXT, repository TEXT);
        CREATE TABLE code_symbols(id TEXT PRIMARY KEY, qualified_name TEXT, name TEXT, kind TEXT,
          language TEXT, file_path TEXT, repository TEXT);
        CREATE TABLE entries(id TEXT PRIMARY KEY, content TEXT);
        CREATE TABLE knowledge_pages(id TEXT PRIMARY KEY, body TEXT);
        """
    )
    db.execute(
        "INSERT INTO tasks VALUES('cas-9d92','Delivery audit',?,'','','','','patient-sparrow-46','',"
        "'2026-08-10T10:00:00Z','2026-08-10T10:00:00Z')",
        (WITHDRAWN_CLAIM_TEXT,),
    )
    db.execute(
        "INSERT INTO tasks VALUES('cas-4444','Team scoped task','A team visible planning note about "
        "cross project coordination and the shared release calendar for the quarter.','','','','',"
        "'worker-a','team-alpha','2026-08-10T10:05:00Z','2026-08-10T10:05:00Z')"
    )
    db.execute(
        "INSERT INTO events VALUES(1,'task_note_added','cas-9d92',?, '2026-08-16T12:00:00Z','session-1')",
        (CORRECTION_TEXT,),
    )
    db.execute(
        "INSERT INTO events VALUES(2,'task_note_added','cas-7788',?, '2026-08-16T12:05:00Z','session-2')",
        (COMMIT_TEXT,),
    )
    db.execute(
        "INSERT INTO events VALUES(3,'task_closed','cas-1234',?, '2026-08-16T12:06:00Z','session-3')",
        (PLAIN_TEXT,),
    )
    for row in range(1, 11):
        db.execute(
            "INSERT INTO prompt_queue VALUES(?,?,'worker-a','summary',?, '2026-08-16T12:00:00Z',?,?,?)",
            (
                row,
                f"key-{row}",
                f"Queued delivery payload number {row} for the supervisor relay lane with retry accounting.",
                None if row <= 3 else "2026-08-16T12:10:00Z",
                0 if row <= 8 else 2,
                "awaiting_ack" if row <= 3 else None,
            ),
        )
    for row in range(1, 5):
        db.execute(
            "INSERT INTO supervisor_queue VALUES(?, 'sup-1','worker_died',?, '2026-08-16T12:00:00Z', ?)",
            (row, f"Worker died notice number {row} raised by the factory supervisor lane.", None if row <= 3 else "2026-08-16T12:20:00Z"),
        )
    db.execute(
        "INSERT INTO history_epochs VALUES(1,'daemon_start','/bin/cas','2026-08-16T11:00:00Z','2.70.0',"
        "'2026-08-16T11:00:00Z','2026-08-16T12:20:00Z',1,0,'2026-08-16T11:00:00Z')"
    )
    db.execute(
        "INSERT INTO history_epochs VALUES(2,'daemon_start','/bin/cas','2026-08-16T12:00:00Z','2.71.0',"
        "'2026-08-16T12:10:00Z','2026-08-16T12:30:00Z',2,0,'2026-08-16T12:10:00Z')"
    )
    db.execute(
        "INSERT INTO history_commits VALUES('abc1234def5678901234567890abcdef12345678','abc1234',"
        "'2026-08-16T11:30:00Z','fix queue reconciliation','cas-src','2026-08-16T11:31:00Z')"
    )
    db.execute(
        "INSERT INTO history_commit_files VALUES('abc1234def5678901234567890abcdef12345678',"
        "'crates/cas-store/src/prompt_queue_store.rs','modified')"
    )
    db.execute(
        "INSERT INTO history_commit_symbols VALUES('abc1234def5678901234567890abcdef12345678','sym-1',"
        "'prompt_queue_store::reconcile_surfaced','crates/cas-store/src/prompt_queue_store.rs')"
    )
    db.execute("INSERT INTO code_files VALUES('f1','crates/cas-store/src/prompt_queue_store.rs','cas-src')")
    db.execute(
        "INSERT INTO code_symbols VALUES('sym-1','prompt_queue_store::reconcile_surfaced',"
        "'reconcile_surfaced','function','rust','crates/cas-store/src/prompt_queue_store.rs','cas-src')"
    )
    db.execute("INSERT INTO entries VALUES('m1','a durable memory that ingestion must never touch')")
    db.execute("INSERT INTO knowledge_pages VALUES('k1','a knowledge page that ingestion must never touch')")
    db.commit()
    db.close()


def build_log(path: Path) -> None:
    path.write_text(
        "2026-08-16T12:00:01Z cas INFO delivery lane processed the queued relay message for "
        "/home/pippenz/Petrastella/cas-src/.cas/worktrees/patient-sparrow-46 without any retry backoff\n"
        "2026-08-16T12:00:02Z cas WARN " + SECRET_TEXT + "\n"
    )


def build_claude(root: Path) -> None:
    """Two sessions relay the same text, which is what dedupe-before-embed must collapse."""
    root.mkdir(parents=True, exist_ok=True)
    for index, session in enumerate(("sess-claude-1", "sess-claude-2"), start=1):
        row = {
            "sessionId": session,
            "cwd": "/home/pippenz/Petrastella/cas-src/.cas/worktrees/patient-sparrow-46",
            "timestamp": f"2026-08-16T12:1{index}:00Z",
            "message": {"role": "assistant", "content": [{"type": "text", "text": PLAIN_TEXT}]},
        }
        (root / f"{session}.jsonl").write_text(json.dumps(row) + "\n")


def ingest_args(root: Path, source_db: Path, log_root: Path, claude_root: Path, **overrides):
    values = dict(
        namespace_root=root,
        source_db=source_db,
        log_root=log_root,
        claude_root=[claude_root],
        codex_root=[],
        grok_root=[],
        claims=CLAIMS,
        scopes="host,project,team",
        tables="all",
        project="cas-src",
        project_marker="",
        host="test-host",
        until="",
        batch=500,
        max_rows=5000,
        max_bytes=8 << 20,
    )
    values.update(overrides)
    return argparse.Namespace(**values)


class Harness(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.live = self.root / "live"
        self.live.mkdir()
        (self.live / "logs").mkdir()
        self.source_db = self.live / "cas.db"
        build_source_db(self.source_db)
        build_log(self.live / "logs" / "daemon.log")
        build_claude(self.live / "claude")
        self.namespace = self.root / "namespace"
        self.addCleanup(self.tmp.cleanup)

    def args(self, **overrides):
        return ingest_args(
            self.namespace, self.source_db, self.live / "logs", self.live / "claude", **overrides
        )

    def namespace_db(self) -> sqlite3.Connection:
        return sqlite3.connect(self.namespace / "units.sqlite3")

    @staticmethod
    def fingerprint(root: Path) -> dict[str, tuple[str, int]]:
        result: dict[str, tuple[str, int]] = {}
        for path in sorted(root.rglob("*")):
            if path.is_file():
                digest = hashlib.sha256(path.read_bytes()).hexdigest()
                result[str(path.relative_to(root))] = (digest, path.stat().st_mtime_ns)
        return result


class ReadOnlyTests(Harness):
    def test_ingest_leaves_every_live_source_byte_identical(self):
        before = self.fingerprint(self.live)
        summary = module.ingest(self.args())
        after = self.fingerprint(self.live)
        self.assertEqual(before, after, "ingestion mutated a live source")
        self.assertGreater(summary["unique_units"], 0)
        self.assertEqual(summary["embedded"], 0)

    def test_memory_and_knowledge_rows_are_never_read_into_the_namespace(self):
        module.ingest(self.args())
        db = self.namespace_db()
        tables = {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        for forbidden in module.FORBIDDEN_WRITE_TABLES:
            self.assertNotIn(forbidden, tables)
        texts = " ".join(row[0] for row in db.execute("SELECT text FROM evidence_units"))
        self.assertNotIn("durable memory that ingestion must never touch", texts)
        self.assertNotIn("knowledge page that ingestion must never touch", texts)
        db.close()

    def test_source_handles_are_pinned_read_only(self):
        connection = module.connect_readonly(self.source_db)
        with self.assertRaises(sqlite3.OperationalError):
            connection.execute("INSERT INTO entries VALUES('m2','injected')")
        connection.close()

    def test_write_guard_refuses_paths_outside_the_namespace(self):
        module.set_writable_root(self.namespace)
        module.assert_writable(self.namespace / "units.sqlite3")
        with self.assertRaises(module.ReadOnlyViolation):
            module.assert_writable(self.source_db)
        with self.assertRaises(module.ReadOnlyViolation):
            module.connect_namespace(self.live / "sneaky.sqlite3")

    def test_write_guard_refuses_the_frozen_cas_c505_artifact(self):
        module.set_writable_root(Path("/home/pippenz/.cas/artifacts"))
        with self.assertRaises(module.ReadOnlyViolation):
            module.assert_writable(Path("/home/pippenz/.cas/artifacts/cas-c505/frozen-index/index.sqlite3"))

    def test_source_contains_no_write_statement_against_a_protected_store(self):
        source = SCRIPT.read_text().lower()
        for forbidden in module.FORBIDDEN_WRITE_TABLES:
            for verb in ("insert into", "update", "delete from"):
                self.assertNotIn(f"{verb} {forbidden}", source)


class ResumabilityTests(Harness):
    def test_second_run_with_no_new_data_is_a_no_op(self):
        first = module.ingest(self.args())
        second = module.ingest(self.args())
        self.assertGreater(first["candidates"], 0)
        self.assertEqual(second["candidates"], 0)
        self.assertEqual(second["unique_units"], 0)

    def test_appended_log_lines_are_the_only_thing_reread(self):
        module.ingest(self.args())
        db = self.namespace_db()
        before = db.execute("SELECT COUNT(*) FROM evidence_provenance").fetchone()[0]
        db.close()
        log = self.live / "logs" / "daemon.log"
        with log.open("a") as handle:
            handle.write(
                "2026-08-16T12:00:03Z cas INFO a freshly appended operational line describing "
                "the supervisor merge gate outcome for the current lane\n"
            )
        result = module.ingest(self.args())
        db = self.namespace_db()
        after = db.execute("SELECT COUNT(*) FROM evidence_provenance").fetchone()[0]
        offset = db.execute(
            "SELECT byte_offset FROM ingest_watermarks WHERE source_key LIKE 'daemon_log:%'"
        ).fetchone()[0]
        db.close()
        self.assertEqual(result["candidates"], 1)
        self.assertEqual(after - before, 1)
        self.assertEqual(offset, log.stat().st_size)

    def test_rotated_log_restarts_from_zero_instead_of_seeking_past_the_end(self):
        module.ingest(self.args())
        log = self.live / "logs" / "daemon.log"
        log.unlink()
        log.write_text(
            "2026-08-16T13:00:00Z cas INFO rotated log file now carries a brand new operational "
            "line about the reconciliation sweep completing cleanly\n"
        )
        result = module.ingest(self.args())
        self.assertEqual(result["candidates"], 1)
        db = self.namespace_db()
        offset = db.execute(
            "SELECT byte_offset FROM ingest_watermarks WHERE source_key LIKE 'daemon_log:%'"
        ).fetchone()[0]
        db.close()
        self.assertEqual(offset, log.stat().st_size)

    def test_row_cursors_advance_per_table(self):
        module.ingest(self.args())
        db = self.namespace_db()
        cursors = dict(
            db.execute("SELECT source_key, row_cursor FROM ingest_watermarks WHERE cursor_kind='row-cursor'")
        )
        db.close()
        self.assertEqual(cursors["db:lifecycle_event"], 3)
        self.assertEqual(cursors["db:prompt_queue"], 10)
        self.assertEqual(cursors["db:supervisor_queue"], 4)


class ScopeTests(Harness):
    def test_host_scope_can_be_excluded_from_a_run(self):
        module.ingest(self.args(scopes="project,team"))
        db = self.namespace_db()
        scopes = {row[0] for row in db.execute("SELECT DISTINCT privacy_scope FROM evidence_provenance")}
        db.close()
        self.assertNotIn("host", scopes)

    def test_every_scope_is_represented_and_daemon_logs_are_host_scoped(self):
        module.ingest(self.args())
        db = self.namespace_db()
        by_scope = dict(
            db.execute("SELECT privacy_scope, COUNT(*) FROM evidence_provenance GROUP BY privacy_scope")
        )
        log_scopes = {
            row[0]
            for row in db.execute(
                "SELECT DISTINCT privacy_scope FROM evidence_provenance WHERE source_key LIKE 'log:%'"
            )
        }
        db.close()
        self.assertEqual(set(by_scope), {"host", "project", "team"})
        self.assertEqual(log_scopes, {"host"})

    def test_secrets_and_emails_are_redacted_before_storage(self):
        module.ingest(self.args())
        db = self.namespace_db()
        texts = " ".join(row[0] for row in db.execute("SELECT text FROM evidence_units"))
        secrets, emails = db.execute(
            "SELECT SUM(secrets_redacted), SUM(emails_redacted) FROM redaction_receipts"
        ).fetchone()
        db.close()
        self.assertNotIn("supersecretvalue123", texts)
        self.assertNotIn("operator@example.com", texts)
        self.assertIn("[REDACTED_SECRET]", texts)
        self.assertIn("[REDACTED_EMAIL]", texts)
        self.assertGreaterEqual(secrets, 1)
        self.assertGreaterEqual(emails, 1)


class ProvenanceJoinTests(Harness):
    def test_unit_joins_across_task_session_worker_commit_file_symbol_and_epoch(self):
        module.ingest(self.args())
        db = self.namespace_db()
        provenance_id = db.execute(
            "SELECT id FROM evidence_provenance WHERE task_id='cas-7788'"
        ).fetchone()[0]
        links = {
            (row[0], row[1])
            for row in db.execute(
                "SELECT link_type, link_value FROM evidence_links WHERE provenance_id=?", (provenance_id,)
            )
        }
        db.close()
        self.assertIn(("task", "cas-7788"), links)
        self.assertIn(("session", "session-2"), links)
        self.assertIn(("commit", "abc1234def5678901234567890abcdef12345678"), links)
        self.assertIn(("file", "crates/cas-store/src/prompt_queue_store.rs"), links)
        self.assertIn(("symbol", "prompt_queue_store::reconcile_surfaced"), links)
        self.assertTrue(any(kind == "epoch" for kind, _ in links))

    def test_worker_is_recovered_from_a_worktree_path(self):
        module.ingest(self.args())
        db = self.namespace_db()
        workers = {
            row[0]
            for row in db.execute("SELECT DISTINCT worker FROM evidence_provenance WHERE worker<>''")
        }
        db.close()
        self.assertIn("patient-sparrow-46", workers)

    def test_mixed_epoch_windows_are_labelled_and_never_single_version(self):
        module.ingest(self.args())
        db = self.namespace_db()
        epochs = {
            row[0]
            for row in db.execute(
                "SELECT DISTINCT epoch FROM evidence_provenance WHERE timestamp LIKE '2026-08-16T12:1%'"
            )
        }
        versions = {
            row[0]
            for row in db.execute(
                "SELECT DISTINCT epoch_version FROM evidence_provenance WHERE epoch LIKE 'mixed:%'"
            )
        }
        db.close()
        self.assertTrue(any(value.startswith("mixed:") for value in epochs), epochs)
        self.assertEqual(versions, {""}, "a mixed window must not resolve to one version")

    def test_identical_text_from_two_sources_collapses_to_one_unit_with_two_provenances(self):
        module.ingest(self.args())
        db = self.namespace_db()
        rows = db.execute(
            "SELECT u.id, u.occurrence_count FROM evidence_units u WHERE u.text LIKE 'Supervisor decision recorded%'"
        ).fetchall()
        db.close()
        self.assertEqual(len(rows), 1, "the same text must dedupe to a single unit")
        self.assertEqual(rows[0][1], 2, "both observations must survive as provenance")


class CorrectionTests(Harness):
    def test_withdrawn_claim_is_marked_and_its_correction_is_not(self):
        module.ingest(self.args())
        db = self.namespace_db()
        states = dict(
            db.execute(
                "SELECT correction_state, COUNT(*) FROM evidence_units "
                "WHERE claim_key='cas-9d92/no-reconciliation-code-path' GROUP BY correction_state"
            )
        )
        db.close()
        self.assertEqual(states.get("withdrawn"), 1)
        self.assertEqual(states.get("correction"), 1)

    def test_registry_and_corpus_corrections_are_both_recorded(self):
        module.ingest(self.args())
        db = self.namespace_db()
        authorities = {
            row[0]
            for row in db.execute(
                "SELECT authority FROM evidence_corrections WHERE claim_key='cas-9d92/no-reconciliation-code-path'"
            )
        }
        db.close()
        self.assertEqual(authorities, {"current-source", "corpus"})

    def test_query_downranks_the_withdrawn_claim_below_its_correction(self):
        module.ingest(self.args())
        db = self.namespace_db()
        results = module.search_units(db, "no reconciliation code path hook surfaced prompts", top=8)
        db.close()
        states = [item["correction_state"] for item in results if item["claim_key"]]
        self.assertIn("withdrawn", states)
        self.assertIn("correction", states)
        self.assertLess(
            states.index("correction"), states.index("withdrawn"),
            "the authoritative correction must outrank the claim it retired",
        )

    def test_a_withdrawn_claim_can_never_be_returned_without_its_correction(self):
        module.ingest(self.args())
        db = self.namespace_db()
        results = module.search_units(db, "no reconciliation code path hook surfaced prompts", top=1)
        db.close()
        withdrawn = [item for item in results if item["correction_state"] == "withdrawn"]
        corrections = [item for item in results if item["correction_state"] == "correction"]
        for item in withdrawn:
            self.assertTrue(item["corrections"], "a withdrawn unit must carry its correction records")
            self.assertEqual(item["corrections"][0]["relation"], "withdraws")
        if withdrawn:
            self.assertTrue(corrections, "the correcting unit must be force-attached to the result set")

    def test_real_cas_9d92_correction_phrasing_is_adjudicated_as_a_correction(self):
        """The phrasing that actually appears in the corpus, not a tidied paraphrase.

        Both halves of cas-9d92's retraction note are plural ("Two claims
        WITHDRAWN", "two withdrawn claims"); a singular-only marker silently
        files the retraction as another assertion of the claim it retires.
        """
        claims = module.load_claims(CLAIMS)
        retraction = (
            'Two claims WITHDRAWN: (a) "no reconciliation code path" - false, prompt_queue_store.rs '
            'stamps acked_via=hook_surfaced in the same transaction as the receipt; (b) "rows 7924/7926 '
            'were consumed yet unacked, so the ack is broken" - invalid, inbox_poll deliberately does not ack.'
        )
        brief = (
            "Reproduce cas-9d92's corrected findings WITHOUT reviving its two withdrawn claims "
            "(the no reconciliation code path and rows 7924/7926 prove ack broken errors)."
        )
        assertion = (
            "Includes my own spawn brief rows 7924/7926, which I demonstrably received and acted on, "
            "yet the database still shows acked_at NULL for both of them."
        )
        for text in (retraction, brief):
            claim_key, is_correction = module.classify_claim(text, claims)
            self.assertIsNotNone(claim_key)
            self.assertTrue(is_correction, f"not adjudicated as a correction: {text[:60]}")
        claim_key, is_correction = module.classify_claim(assertion, claims)
        self.assertEqual(claim_key, "cas-9d92/inbox-poll-rows-prove-ack-broken")
        self.assertFalse(is_correction, "an assertion of the claim must not pass as its correction")

    def test_unclaimed_units_are_untouched_by_correction_logic(self):
        module.ingest(self.args())
        db = self.namespace_db()
        state = db.execute(
            "SELECT correction_state FROM evidence_units WHERE text LIKE 'Supervisor decision recorded%'"
        ).fetchone()[0]
        db.close()
        self.assertEqual(state, "current")


class ReproductionTests(Harness):
    def test_cas_9d92_findings_reproduce_from_sql_without_reviving_withdrawn_claims(self):
        module.ingest(self.args())
        snapshot = self.namespace / "snapshot" / "cas.db"
        report = module.reproduce(
            argparse.Namespace(namespace_root=self.namespace, snapshot=snapshot, days=7)
        )
        findings = {item["finding"]: item for item in report["findings"]}

        self.assertEqual(findings["worker_died supervisor notices unprocessed"]["measured"],
                         {"notices": 4, "unprocessed": 3})
        self.assertEqual(findings["prompt_queue.delivery_attempts = 0"]["measured"],
                         {"rows": 10, "at_zero": 8})
        day = findings["undelivered rate by day"]["measured"][0]
        self.assertEqual((day["rows"], day["undelivered"]), (10, 3))
        reasons = {row["last_pending_reason"]: row["rows"] for row in
                   findings["pending reason attribution"]["measured"]}
        self.assertEqual(reasons["awaiting_ack"], 3)

        self.assertTrue(all(item["method"] == "sql" for item in report["findings"]))
        keys = {entry["claim_key"] for entry in report["withdrawn_claims_not_reproduced"]}
        self.assertIn("cas-9d92/no-reconciliation-code-path", keys)
        self.assertIn("cas-9d92/inbox-poll-rows-prove-ack-broken", keys)
        self.assertNotIn("cas-9d92/no-reconciliation-code-path", report["claims_still_live"])
        marked = {
            entry["claim_key"]: entry["units_marked_withdrawn"]
            for entry in report["withdrawn_claims_not_reproduced"]
        }
        self.assertEqual(marked["cas-9d92/no-reconciliation-code-path"], 1)

    def test_epoch_stratification_never_calls_a_mixed_window_post_fix(self):
        module.ingest(self.args())
        snapshot = self.namespace / "snapshot" / "cas.db"
        report = module.reproduce(
            argparse.Namespace(namespace_root=self.namespace, snapshot=snapshot, days=7)
        )
        strata = next(
            item for item in report["findings"]
            if item["finding"].startswith("deployed-binary epoch stratification")
        )["measured"]
        self.assertTrue(strata)
        for row in strata:
            if str(row["epoch"]).startswith("mixed:"):
                self.assertFalse(row["post_fix_eligible"])


class RetentionTests(Harness):
    def test_retention_deletes_by_scope_and_writes_a_hashed_receipt(self):
        module.ingest(self.args())
        db = self.namespace_db()
        before = db.execute(
            "SELECT COUNT(*) FROM evidence_provenance WHERE privacy_scope='host'"
        ).fetchone()[0]
        db.close()
        self.assertGreater(before, 0)

        result = module.retention(
            argparse.Namespace(
                namespace_root=self.namespace,
                policy="host=1,project=3650,team=3650",
                now="2026-09-30T00:00:00Z",
            )
        )
        by_scope = {item["scope"]: item for item in result["receipts"]}
        self.assertEqual(by_scope["host"]["provenance_deleted"], before)
        self.assertEqual(by_scope["project"]["provenance_deleted"], 0)
        self.assertEqual(len(by_scope["host"]["receipt_hash"]), 64)

        db = self.namespace_db()
        remaining = db.execute(
            "SELECT COUNT(*) FROM evidence_provenance WHERE privacy_scope='host'"
        ).fetchone()[0]
        receipts = db.execute("SELECT COUNT(*) FROM retention_receipts").fetchone()[0]
        orphan_links = db.execute(
            "SELECT COUNT(*) FROM evidence_links l LEFT JOIN evidence_provenance p ON p.id=l.provenance_id "
            "WHERE p.id IS NULL"
        ).fetchone()[0]
        db.close()
        self.assertEqual(remaining, 0)
        self.assertEqual(receipts, 3)
        self.assertEqual(orphan_links, 0, "retention must not leave dangling join rows")

    def test_retention_never_deletes_a_correction_unit(self):
        module.ingest(self.args())
        module.retention(
            argparse.Namespace(
                namespace_root=self.namespace, policy="host=1,project=1,team=1", now="2027-01-01T00:00:00Z"
            )
        )
        db = self.namespace_db()
        corrections = db.execute(
            "SELECT COUNT(*) FROM evidence_units WHERE correction_state='correction'"
        ).fetchone()[0]
        db.close()
        self.assertGreaterEqual(corrections, 1)

    def test_receipt_hash_is_deterministic_for_identical_deletions(self):
        payload = {
            "scope": "host", "days": 1, "cutoff": "2026-09-29T00:00:00Z",
            "provenance_deleted": 2, "links_deleted": 4, "units_deleted": 2, "oldest_retained": "",
        }
        expected = hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()
        self.assertEqual(len(expected), 64)


class StatusTests(Harness):
    def test_status_reports_watermarks_receipts_and_pending_embedding(self):
        module.ingest(self.args())
        payload = module.status(argparse.Namespace(namespace_root=self.namespace))
        self.assertTrue(payload["exists"])
        self.assertGreater(payload["units"], 0)
        self.assertTrue(payload["watermarks"])
        self.assertTrue(payload["runs"])
        self.assertEqual([row["state"] for row in payload["embed_states"]], ["pending"])


if __name__ == "__main__":
    unittest.main()
