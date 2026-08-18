#!/usr/bin/env python3
"""Regression coverage for operational_index.py (cas-2556 / M2).

The fixtures are synthetic and offline: a miniature "frozen corpus" plus a
miniature CAS project store.  They exercise the three mechanisms the milestone
is accountable for - admission, namespace isolation in both directions, and the
semantic gate that must pass before any vector answer is surfaced - plus the
provenance contract on hybrid join rows.
"""

from __future__ import annotations

import argparse
import contextlib
import importlib.util
import io
import json
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("operational_index.py")
SPEC = importlib.util.spec_from_file_location("operational_index", SCRIPT)
assert SPEC and SPEC.loader
module = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = module
SPEC.loader.exec_module(module)

EMBEDDER = module.HashingEmbedder()

# The two probe queries deliberately share NO vocabulary with their gold rows:
# that is what makes the lexical and prefix baselines miss them, and it is the
# paraphrase-robustness the milestone requires the vector channel to demonstrate.
DRIFT_QUERY = "paraphrased probe about repeated confusion regarding a governing operating rule"
SYMPTOM_QUERY = "paraphrased probe about acknowledgement vanishing before handoff completed correctly"
DRIFT_TEXT = (
    "I re-read the standing operator directive and realised I had been running the full test suite "
    "on every push even though the worker policy says only the scoped target belongs to a worker, so "
    "I misread the instruction twice in a row before the supervisor corrected me."
)
SYMPTOM_TEXT = (
    "The close call kept returning a false confirmation while the message never reached the supervisor "
    "inbox, so the worker believed delivery had happened and moved on to another task without merging."
)
# The decoy repeats every probe term, so both lexical baselines rank it first.
DECOY_TEXT = (
    "A paraphrased probe about a governing rule and a paraphrased probe about acknowledgement: this "
    "line repeats the probe vocabulary - repeated confusion, governing operating rule, acknowledgement "
    "vanishing before handoff completed correctly - without describing any operational episode at all."
)
JSON_TEXT = json.dumps({"event": "task_assigned", "agent": "cosmic-bear-43", "role": "supervisor"})
TELEMETRY_TEXT = (
    "cas::factory::spawn: factory spawn lifecycle request_id=919 worker=proud-octopus-28 stage=launch "
    "outcome=started attempts=1 pid=4212 elapsed_ms=88"
)
TEMPLATED_TEXT = "Task completed: Harden the liveness gate"
MEMORY_TEXT = (
    "Durable memory: CAS system bugs are in-repo fixes; when a downstream project surfaces a verifier "
    "bug the fix lands in cas-src as a task assigned to a worker rather than being reported upstream, "
    "because other projects consume CAS and never modify it, and escalating such a bug outward wastes "
    "a round trip that the in-repo task would have finished already."
)
CODE_TEXT = (
    "pub fn resolve_commit_receipt(store: &Store, receipt: &str) -> Result<CommitId> { "
    "let resolved = store.resolve_partial(receipt)?; ensure_ancestor(store, &resolved)?; "
    "ensure_tree_effect_present(store, &resolved)?; Ok(resolved) } "
    "// resolves an abbreviated receipt to a full immutable commit id before validation"
)
KNOWLEDGE_TEXT = (
    "Knowledge page: the factory worker lifecycle covers claim, start, deliver, and close with a "
    "supervisor merge gate in between; this page is the canonical description of that lifecycle."
)


def frozen_corpus(path: Path) -> None:
    """A miniature stand-in for the frozen cas-c505 corpus."""
    db = sqlite3.connect(path)
    db.executescript(
        """
        CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE chunks(id INTEGER PRIMARY KEY, content_hash TEXT, source_kind TEXT, text TEXT,
                            duplicate_count INTEGER DEFAULT 1);
        CREATE TABLE occurrences(id INTEGER PRIMARY KEY, chunk_id INTEGER, source_path TEXT, session_id TEXT,
                                 task_id TEXT, worker TEXT, timestamp TEXT, epoch TEXT, privacy_scope TEXT);
        CREATE TABLE vectors(chunk_id INTEGER PRIMARY KEY, vector BLOB);
        """
    )
    db.execute("INSERT INTO meta VALUES('model', ?)", (json.dumps(module.HASHING_EMBEDDER),))
    db.execute("INSERT INTO meta VALUES('dimensions', ?)", (json.dumps(EMBEDDER.dims),))
    db.execute("INSERT INTO meta VALUES('vector_embedder', ?)", (json.dumps(module.HASHING_EMBEDDER),))

    rows = [
        # (chunk_id, kind, text, embedding_proxy).  The proxy stands in for what a
        # real sentence embedding would place near the paraphrased probe; the
        # offline hashing embedder cannot invent semantics, so the fixture makes
        # the semantic neighbourhood explicit and the test covers the machinery.
        (1, "claude_transcript", DRIFT_TEXT, DRIFT_QUERY),
        (2, "codex_transcript", SYMPTOM_TEXT, SYMPTOM_QUERY),
        (3, "daemon_log", DECOY_TEXT, DECOY_TEXT),
        (4, "supervisor_queue", JSON_TEXT, JSON_TEXT),
        (5, "daemon_log", TELEMETRY_TEXT, TELEMETRY_TEXT),
        (6, "event", TEMPLATED_TEXT, TEMPLATED_TEXT),
        (7, "memory", MEMORY_TEXT, MEMORY_TEXT),
        (8, "code_symbol", CODE_TEXT, CODE_TEXT),
        (9, "knowledge_page", KNOWLEDGE_TEXT, KNOWLEDGE_TEXT),
    ]
    import struct

    for chunk_id, kind, text, proxy in rows:
        db.execute("INSERT INTO chunks(id,content_hash,source_kind,text,duplicate_count) VALUES(?,?,?,?,?)",
                   (chunk_id, module.content_hash(text), kind, text, 1))
        vector = EMBEDDER.embed([proxy])[0]
        db.execute("INSERT INTO vectors VALUES(?,?)", (chunk_id, struct.pack(f"<{EMBEDDER.dims}f", *vector)))
        db.execute(
            "INSERT INTO occurrences(chunk_id,source_path,session_id,task_id,worker,timestamp,epoch,privacy_scope)"
            " VALUES(?,?,?,?,?,?,?,?)",
            (chunk_id, f"transcripts/{chunk_id}.jsonl", "session-77", "cas-1234", "proud-raven-97",
             "2026-08-16T12:25:00Z", "2.71.0", "project-private/redacted-before-embedding"),
        )
    db.commit()
    db.close()


def project_store(path: Path) -> None:
    """A miniature stand-in for .cas/cas.db (memory, knowledge, code, task, history)."""
    db = sqlite3.connect(path)
    db.executescript(
        """
        CREATE TABLE entries(id TEXT PRIMARY KEY, type TEXT, tags TEXT, created TEXT, title TEXT,
                             content TEXT, session_id TEXT);
        CREATE TABLE knowledge_pages(id TEXT PRIMARY KEY, page_type TEXT, title TEXT, rel_path TEXT, snippet TEXT);
        CREATE TABLE code_symbols(id TEXT PRIMARY KEY, qualified_name TEXT, name TEXT, kind TEXT, language TEXT,
                                  file_path TEXT, line_start INTEGER, line_end INTEGER, source TEXT,
                                  documentation TEXT, repository TEXT, commit_hash TEXT);
        CREATE TABLE tasks(id TEXT PRIMARY KEY, title TEXT, status TEXT, task_type TEXT, created_at TEXT,
                           closed_at TEXT, external_ref TEXT);
        CREATE TABLE history_commits(sha TEXT PRIMARY KEY, short_sha TEXT, subject TEXT, committed_at TEXT,
                                     repository TEXT);
        CREATE TABLE commit_links(commit_hash TEXT PRIMARY KEY, session_id TEXT, branch TEXT, message TEXT,
                                  committed_at TEXT, link_method TEXT);
        CREATE TABLE history_epochs(id INTEGER PRIMARY KEY, epoch_kind TEXT, version TEXT, started_at TEXT,
                                    ended_at TEXT);
        """
    )
    db.execute("INSERT INTO entries VALUES('mem-1','learning','cas','2026-08-01T00:00:00Z','In-repo fixes',?,'session-77')",
               (MEMORY_TEXT,))
    db.execute("INSERT INTO knowledge_pages VALUES('kp-1','reference','Worker lifecycle','docs/worker.md',?)",
               (KNOWLEDGE_TEXT,))
    db.execute(
        "INSERT INTO code_symbols VALUES('sym-1','cas::store::resolve_commit_receipt','resolve_commit_receipt',"
        "'function','rust','crates/cas-store/src/receipts.rs',10,24,?,'Resolve a commit receipt','cas-src','abc1234')",
        (CODE_TEXT,),
    )
    db.execute("INSERT INTO tasks VALUES('cas-1234','Fix false delivery confirmations','closed','bug',"
               "'2026-08-10T00:00:00Z','2026-08-16T00:00:00Z','https://github.com/Richards-LLC/cas/issues/401')")
    db.execute("INSERT INTO history_commits VALUES('abc1234def5678','abc1234','fix(factory): confirm delivery',"
               "'2026-08-16T11:00:00Z','cas-src')")
    db.execute("INSERT INTO commit_links VALUES('abc1234def5678','session-77','factory/proud-raven-97',"
               "'fix(factory): confirm delivery','2026-08-16T11:00:00Z','trailer')")
    db.execute("INSERT INTO history_epochs VALUES(1,'daemon_start','2.71.0','2026-08-16T12:20:00Z',NULL)")
    db.commit()
    db.close()


def run(func, **kwargs) -> dict:
    buffer = io.StringIO()
    with contextlib.redirect_stdout(buffer):
        func(argparse.Namespace(**kwargs))
    return json.loads(buffer.getvalue())


def build_index(root: Path) -> tuple[Path, Path]:
    # The namespace artifact lives outside the CAS store directory on purpose;
    # isolation_check treats any path inside a store directory as a violation.
    frozen, index, store = root / "frozen.sqlite3", root / "namespace" / "op-index.sqlite3", root / "store" / "cas.db"
    store.parent.mkdir(parents=True, exist_ok=True)
    frozen_corpus(frozen)
    project_store(store)
    run(module.build, frozen=frozen, index=index, limit=0)
    return index, store


def labels_file(root: Path, name: str, items: list[dict], families: list[str]) -> Path:
    path = root / name
    path.write_text(json.dumps({
        "revision": "test", "method": "pooled candidates, hand-reviewed", "required_families": families,
        "items": items,
    }))
    return path


WINNING_LABELS = [
    {"id": "drift-1", "family": "instruction-drift", "query": DRIFT_QUERY,
     "gold_content_hashes": [module.content_hash(DRIFT_TEXT)]},
    {"id": "symptom-1", "family": "symptom-to-fix", "query": SYMPTOM_QUERY,
     "gold_content_hashes": [module.content_hash(SYMPTOM_TEXT)]},
]
# A label the vector channel loses: the gold row is the term-stuffed decoy, which
# both lexical baselines rank first and the vector channel does not.
LOSING_LABELS = [
    {"id": "drift-lex", "family": "instruction-drift", "query": DRIFT_QUERY,
     "gold_content_hashes": [module.content_hash(DECOY_TEXT)]},
]


class AdmissionTests(unittest.TestCase):
    def test_structured_payloads_are_rejected(self):
        self.assertEqual(module.admit(JSON_TEXT)[2], "structured-json")
        self.assertEqual(module.admit(TELEMETRY_TEXT)[2], "key-value-telemetry")

    def test_templated_envelope_is_stripped_then_judged_on_its_prose(self):
        admitted, payload, reason = module.admit(TEMPLATED_TEXT)
        self.assertFalse(admitted)
        self.assertEqual(payload, "Harden the liveness gate")
        self.assertEqual(reason, "too-short")
        note = ("[2026-08-17 19:29] PROGRESS the worker kept rerunning the whole suite because it read the "
                "scoped-test policy as advice rather than a rule, which is the drift we care about here")
        admitted, payload, _ = module.admit(note)
        self.assertTrue(admitted)
        self.assertFalse(payload.startswith("["))

    def test_semantic_prose_is_admitted(self):
        self.assertTrue(module.admit(DRIFT_TEXT)[0])
        self.assertTrue(module.admit(SYMPTOM_TEXT)[0])


class BuildAndIsolationTests(unittest.TestCase):
    def test_build_namespaces_rows_and_excludes_foreign_kinds(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            index, _ = build_index(root)
            db = sqlite3.connect(index)
            kinds = {k for (k,) in db.execute("SELECT DISTINCT source_kind FROM op_events")}
            self.assertTrue(kinds <= set(module.OPERATIONAL_KINDS))
            self.assertFalse(kinds & set(module.FOREIGN_KINDS))
            for table in ("op_events", "op_occurrences", "op_vectors"):
                outside = db.execute(f"SELECT COUNT(*) FROM {table} WHERE namespace<>?", (module.NAMESPACE,)).fetchone()[0]
                self.assertEqual(outside, 0, table)
            self.assertEqual(db.execute("SELECT COUNT(*) FROM op_vectors").fetchone()[0],
                             db.execute("SELECT COUNT(*) FROM op_events").fetchone()[0])
            self.assertEqual(db.execute("SELECT COUNT(*) FROM op_occurrences WHERE source_path=''").fetchone()[0], 0)
            db.close()

    def test_isolation_check_passes_on_a_clean_namespace(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            index, store = build_index(root)
            verdict = run(module.isolation_check, index=index, project_db=store, user_db=None, canaries=5,
                          live_fts=False, live_fts_canaries=0)
        self.assertTrue(verdict["passed"], verdict["failed_checks"])
        names = {check["check"] for check in verdict["checks"]}
        self.assertIn("artifact_is_not_a_cas_store_or_store_neighbour", names)
        self.assertIn("namespace_not_registered_in_any_cas_store", names)
        self.assertIn("operational_text_absent_from_memory_knowledge_code", names)
        self.assertIn("memory_knowledge_code_text_absent_from_namespace", names)
        self.assertIn("cas_stores_opened_read_only", names)

    def test_isolation_check_rejects_an_artifact_parked_next_to_a_store(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            index, store = build_index(root)
            beside = store.parent / "op-index.sqlite3"
            beside.write_bytes(index.read_bytes())
            with self.assertRaises(SystemExit) as raised:
                run(module.isolation_check, index=beside, project_db=store, user_db=None, canaries=5,
                    live_fts=False, live_fts_canaries=0)
            self.assertEqual(raised.exception.code, 2)

    def test_isolation_check_detects_memory_text_smuggled_into_the_namespace(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            index, store = build_index(root)
            db = sqlite3.connect(index)
            db.execute("INSERT INTO op_events(namespace,content_hash,source_kind,text,admitted_reason)"
                       " VALUES(?,?,?,?,?)",
                       (module.NAMESPACE, module.content_hash(MEMORY_TEXT), "claude_transcript", MEMORY_TEXT, "admitted"))
            db.commit()
            db.close()
            with self.assertRaises(SystemExit) as raised:
                run(module.isolation_check, index=index, project_db=store, user_db=None, canaries=5,
                    live_fts=False, live_fts_canaries=0)
            self.assertEqual(raised.exception.code, 2)

    def test_isolation_check_detects_operational_text_leaking_into_memory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            index, store = build_index(root)
            db = sqlite3.connect(store)
            db.execute("INSERT INTO entries VALUES('mem-leak','learning','x','2026-08-17T00:00:00Z','leak',?,'s')",
                       (DRIFT_TEXT,))
            db.commit()
            db.close()
            with self.assertRaises(SystemExit) as raised:
                run(module.isolation_check, index=index, project_db=store, user_db=None, canaries=5,
                    live_fts=False, live_fts_canaries=0)
            self.assertEqual(raised.exception.code, 2)

    def test_cas_stores_are_opened_read_only(self):
        with tempfile.TemporaryDirectory() as directory:
            store = Path(directory) / "cas.db"
            project_store(store)
            connection = module.open_ro(store)
            with self.assertRaises(sqlite3.OperationalError):
                connection.execute("UPDATE entries SET content='rewritten'")
            connection.close()


class SemanticGateTests(unittest.TestCase):
    def test_vector_answers_are_blocked_until_the_gate_passes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            index, _ = build_index(root)

            with self.assertRaises(SystemExit) as raised:
                run(module.query_command, query=DRIFT_QUERY, index=index, mode="vector", top=3)
            self.assertEqual(raised.exception.code, 3)
            # the lexical baseline stays available with no gate at all
            baseline = run(module.query_command, query="standing directive", index=index, mode="lexical", top=3)
            self.assertTrue(baseline["rows"])

            passing = run(module.evaluate, index=index,
                          labels=labels_file(root, "win.json", WINNING_LABELS, ["instruction-drift", "symptom-to-fix"]),
                          top=3, record_gate=True)
            self.assertTrue(passing["passed"], passing["families"])
            for family in passing["families"].values():
                self.assertTrue(family["vector_beats_lexical_recall"])
                self.assertTrue(family["vector_beats_prefix_recall"])
            answered = run(module.query_command, query=DRIFT_QUERY, index=index, mode="vector", top=3)
            self.assertTrue(answered["gate_available"])
            self.assertEqual(answered["rows"][0]["content_hash"], module.content_hash(DRIFT_TEXT))

    def test_a_failing_evaluation_keeps_vector_answers_blocked(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            index, _ = build_index(root)
            receipt = run(module.evaluate, index=index,
                          labels=labels_file(root, "lose.json", LOSING_LABELS, ["instruction-drift"]),
                          top=1, record_gate=True)
            self.assertFalse(receipt["passed"])
            with self.assertRaises(SystemExit) as raised:
                run(module.query_command, query=DRIFT_QUERY, index=index, mode="hybrid", top=3)
            self.assertEqual(raised.exception.code, 3)

    def test_gate_goes_stale_when_the_corpus_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            index, _ = build_index(root)
            run(module.evaluate, index=index,
                labels=labels_file(root, "win.json", WINNING_LABELS, ["instruction-drift", "symptom-to-fix"]),
                top=3, record_gate=True)
            db = sqlite3.connect(index)
            db.execute("INSERT INTO op_events(namespace,content_hash,source_kind,text,admitted_reason)"
                       " VALUES(?,?,?,?,?)", (module.NAMESPACE, "new-hash", "daemon_log", "a new operational line", "admitted"))
            db.commit()
            db.close()
            with self.assertRaises(SystemExit) as raised:
                run(module.query_command, query=DRIFT_QUERY, index=index, mode="vector", top=3)
            self.assertEqual(raised.exception.code, 3)
            state = None
            try:
                run(module.gate_command, index=index)
            except SystemExit:
                buffer = io.StringIO()
                with contextlib.redirect_stdout(buffer), contextlib.suppress(SystemExit):
                    module.gate_command(argparse.Namespace(index=index))
                state = json.loads(buffer.getvalue())
            self.assertIsNotNone(state)
            self.assertIn("stale", state["reason"])


class AuthorizationTests(unittest.TestCase):
    """The mode authorisation rule, pinned to the real 2026-08-18 measurement."""

    # Measured on the 46,654-row operational namespace, top_k=10, 7 labelled probes.
    MEASURED = {
        "instruction-drift": {"prefix_recall": 0.1667, "prefix_mrr": 0.1667, "lexical_recall": 0.1667,
                              "lexical_mrr": 0.3333, "vector_recall": 0.8333, "vector_mrr": 0.2889,
                              "hybrid_recall": 0.5, "hybrid_mrr": 0.381},
        "symptom-to-fix": {"prefix_recall": 0.2708, "prefix_mrr": 0.3125, "lexical_recall": 0.2708,
                           "lexical_mrr": 0.25, "vector_recall": 0.6667, "vector_mrr": 0.5083,
                           "hybrid_recall": 0.3541, "hybrid_mrr": 0.5695},
    }

    def test_drift_family_authorises_hybrid_but_not_raw_vector(self):
        summary = module.authorize(dict(self.MEASURED["instruction-drift"]))
        # vector wins recall against both baselines but loses MRR to BM25...
        self.assertTrue(summary["vector_recall_beats_both_baselines"])
        self.assertFalse(summary["vector_strictly_beats_both_baselines"])
        # ...so only the fused channel that does beat both baselines is offered.
        self.assertEqual(summary["authorized_modes"], ["hybrid"])

    def test_symptom_family_authorises_both_modes(self):
        summary = module.authorize(dict(self.MEASURED["symptom-to-fix"]))
        self.assertEqual(summary["authorized_modes"], ["vector", "hybrid"])

    def test_a_channel_that_loses_recall_is_never_authorised(self):
        losing = dict(self.MEASURED["symptom-to-fix"], vector_recall=0.1, hybrid_recall=0.1)
        summary = module.authorize(losing)
        self.assertEqual(summary["authorized_modes"], [])
        self.assertFalse(summary["passed"])


class HybridJoinTests(unittest.TestCase):
    def test_join_returns_provenance_on_every_row_and_entity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            index, store = build_index(root)
            run(module.evaluate, index=index,
                labels=labels_file(root, "win.json", WINNING_LABELS, ["instruction-drift", "symptom-to-fix"]),
                top=3, record_gate=True)
            payload = run(module.join_command, query=SYMPTOM_QUERY, index=index, project_db=store, mode="hybrid",
                          top=2, join_limit=3, allow_missing_provenance=False)
        self.assertEqual(payload["provenance_violations"], [])
        row = payload["rows"][0]
        self.assertEqual(row["namespace"], module.NAMESPACE)
        self.assertTrue(row["provenance"][0]["source_path"])
        self.assertEqual(row["provenance"][0]["epoch"], "2.71.0")
        self.assertEqual(row["epoch_context"]["store"], "history_epochs")
        joins = row["joins"]
        self.assertEqual(joins["tasks"][0]["id"], "cas-1234")
        self.assertEqual(joins["tasks"][0]["join_method"], "task-id-mention")
        self.assertTrue(any(entity["join_method"] == "session-provenance" for entity in joins["commits"]))
        self.assertTrue(any(entity["issue"] == 401 for entity in joins["issues"]))
        self.assertTrue(joins["memories"])
        self.assertIn("M4", joins["memories"][0]["adjudication"])

    def test_join_fails_loudly_when_provenance_is_missing(self):
        problems = module.validate_provenance([
            {"event_id": 1, "namespace": module.NAMESPACE, "provenance": [], "epoch_context": {},
             "joins": {"tasks": [{"store": "tasks", "join_key": "", "join_method": "x"}]}},
        ])
        self.assertTrue(any("no provenance occurrences" in p for p in problems))
        self.assertTrue(any("missing join_key" in p for p in problems))

    def test_generic_identifiers_and_common_words_do_not_create_joins(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            index, store = build_index(root)
            connection = module.open_ro(store)
            # `process` matches half the codebase by name; only distinctive
            # identifiers (snake_case or long) are allowed to create a join.
            self.assertEqual(module.joinable_identifiers("the process update called factory"), [])
            self.assertIn("resolve_commit_receipt", module.joinable_identifiers("resolve_commit_receipt failed"))
            noisy = module.join_memories(connection, "the worker installed processes material", [], "", 3,
                                         distinctive=[])
            self.assertEqual(noisy, [])
            connection.close()
            db = sqlite3.connect(f"file:{index}?mode=ro", uri=True)
            # every fixture row mentions "supervisor"/"worker", so those are not rare
            self.assertNotIn("supervisor", module.rare_terms(db, SYMPTOM_TEXT))
            db.close()

    def test_symbol_join_uses_file_paths_and_symbol_names(self):
        with tempfile.TemporaryDirectory() as directory:
            store = Path(directory) / "cas.db"
            project_store(store)
            connection = module.open_ro(store)
            by_path = module.join_symbols(connection, "the panic came from crates/cas-store/src/receipts.rs", 5)
            by_name = module.join_symbols(connection, "resolve_commit_receipt returned the wrong id", 5)
            connection.close()
        self.assertEqual(by_path[0]["join_method"], "file-path-mention")
        self.assertEqual(by_name[0]["join_method"], "symbol-name-mention")
        self.assertEqual(by_name[0]["qualified_name"], "cas::store::resolve_commit_receipt")


if __name__ == "__main__":
    unittest.main()
