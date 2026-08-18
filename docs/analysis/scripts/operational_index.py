#!/usr/bin/env python3
"""Operational index namespace (cas-2556 / M2 of cas-0cda).

A physically separate index that holds ONLY genuinely semantic operational text
(worker reasoning, instruction-drift phrasing, confusion themes, failure
narratives).  Three properties are enforced mechanically rather than asserted in
prose:

1. Namespace isolation - the operational namespace lives in its own artifact,
   every row carries the namespace, and `isolation-check` probes both directions
   (operational text invisible to the memory/knowledge/code search paths, and
   memory/knowledge/code text absent from the operational namespace).
2. Evaluated semantics - `evaluate` scores the vector channel against the
   lexical (BM25) and prefix baselines on a human-reviewed labelled set and
   records a gate receipt.  `query`/`join` refuse to surface vector or hybrid
   answers unless a current, passing gate exists for this corpus.
3. Hybrid joins with provenance - `join` cross-references each operational event
   with tasks, code symbols, commits, issues, memories, and deployed-binary
   epochs; every row (and every joined entity) carries provenance, and the join
   fails loudly if it does not.

All CAS stores are opened read-only.  Nothing here writes to memory, knowledge,
code, or task stores.  Standard library only.
"""

from __future__ import annotations

import argparse
import array
import collections
import hashlib
import json
import math
import operator
import os
import re
import sqlite3
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterable, Sequence

NAMESPACE = "operational/v2"
PROVIDER_MODEL = "cas-embed-v1"
HASHING_EMBEDDER = "hashing-test"
HASHING_DIMS = 64

DEFAULT_FROZEN = Path("/home/pippenz/.cas/artifacts/cas-c505/frozen-index/index.sqlite3")
DEFAULT_INDEX = Path("/home/pippenz/.cas/artifacts/cas-2556/operational-index/index.sqlite3")
DEFAULT_PROJECT_DB = Path("/home/pippenz/Petrastella/cas-src/.cas/cas.db")
DEFAULT_USER_DB = Path("/home/pippenz/.cas/cas.db")

# Source kinds admissible into the operational namespace.  Memory, knowledge and
# code kinds are deliberately absent: those corpora have their own indexes and
# must never be mirrored here.
OPERATIONAL_KINDS = (
    "claude_transcript",
    "codex_transcript",
    "grok_transcript",
    "daemon_log",
    "event",
    "task",
    "prompt_queue",
    "supervisor_queue",
)
FOREIGN_KINDS = ("memory", "entry", "knowledge", "knowledge_page", "code_symbol", "code_file", "rule", "skill")

TASK_ID = re.compile(r"\bcas-[0-9a-f]{4,}\b", re.I)
SHA = re.compile(r"\b[0-9a-f]{7,40}\b")
ISSUE_REF = re.compile(r"(?:\bGH-|(?<![\w/])#)(\d{2,5})\b")
PATH_REF = re.compile(r"\b[\w./-]+\.(?:rs|py|ts|tsx|js|toml|md|sh)\b")
IDENT = re.compile(r"\b[a-z][a-z0-9_]{5,}\b")
WORD = re.compile(r"[A-Za-z][A-Za-z0-9_'-]*")

# Structured envelopes that carry a real prose tail.  The envelope itself is a
# SQL-derived fact (M1/structured lane); only the prose tail is semantic.
TEMPLATE_PREFIX = re.compile(
    r"^(?:Task (?:created|completed|closed|assigned|updated|note added)[^:]*:"
    r"|Worker [\w-]+ (?:died|idle|spawned)[^:]*:"
    r"|Session (?:start|end)[^:]*:"
    r"|\[[^\]]+\]\s*(?:PROGRESS|DISCOVERY|DECISION|BLOCKER)?)\s*",
    re.I,
)
KEY_VALUE = re.compile(r"\b[a-z_][a-z0-9_]*=")
PROSE_TAIL = re.compile(r"(?:detail|message|notes|summary|reason|prompt)=(.+)$", re.S)

GATE_THRESHOLDS = {
    "vector_mode": "the raw vector channel must beat BOTH baselines on recall@k AND MRR in every "
                   "required family",
    "hybrid_mode": "the vector channel must beat both baselines on recall@k, and the surfaced hybrid "
                   "channel must beat both baselines on recall@k AND MRR, in every required family",
    "baselines": ["prefix (FTS5 prefix match, BM25 ordered)", "lexical (FTS5 BM25)"],
    "require_all_families": True,
}


# --------------------------------------------------------------------------
# helpers
# --------------------------------------------------------------------------


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def open_ro(path: Path) -> sqlite3.Connection:
    """Every CAS store is opened through here, read-only, no exceptions."""
    if not Path(path).exists():
        raise SystemExit(f"store not found: {path}")
    return sqlite3.connect(f"file:{path}?mode=ro", uri=True)


def normalized(text: str) -> str:
    value = text.lower()
    value = re.sub(r"\b[0-9a-f]{8,40}\b", "<hex>", value)
    value = re.sub(r"\b\d+(?:\.\d+)?(?:ms|s|mb|gb|bytes?)?\b", "<n>", value)
    value = re.sub(r"/home/[^\s\"']+", "<path>", value)
    return re.sub(r"\s+", " ", value).strip()


def content_hash(text: str) -> str:
    return hashlib.sha256(normalized(text).encode()).hexdigest()


def terms_of(text: str) -> list[str]:
    return list(dict.fromkeys(w for w in WORD.findall(text.lower()) if len(w) >= 3))


# --------------------------------------------------------------------------
# semantic admission policy
# --------------------------------------------------------------------------


def semantic_payload(text: str) -> str:
    """Strip structured envelopes, returning the prose that remains (may be '')."""
    candidate = text.strip()
    if candidate.startswith(("{", "[")):
        try:
            json.loads(candidate)
        except json.JSONDecodeError:
            pass
        else:
            return ""
    tail = PROSE_TAIL.search(candidate)
    if tail and len(KEY_VALUE.findall(candidate)) >= 2:
        candidate = tail.group(1).strip()
    stripped = TEMPLATE_PREFIX.sub("", candidate, count=1).strip()
    return stripped or candidate


def prose_score(text: str) -> float:
    tokens = text.split()
    if not tokens:
        return 0.0
    words = [t for t in tokens if re.fullmatch(r"[A-Za-z][A-Za-z'-]{2,}", t.strip(".,;:!?()\"'"))]
    return len(words) / len(tokens)


def admit(text: str, min_words: int = 12, min_prose: float = 0.55) -> tuple[bool, str, str]:
    """Return (admitted, payload, reason).  Rejections name their rule."""
    payload = semantic_payload(text)
    if not payload:
        return False, "", "structured-json"
    if len(KEY_VALUE.findall(payload)) >= 3 and prose_score(payload) < 0.7:
        return False, payload, "key-value-telemetry"
    if len(payload.split()) < min_words:
        return False, payload, "too-short"
    score = prose_score(payload)
    if score < min_prose:
        return False, payload, "low-prose-density"
    if len(set(re.findall(r"[a-z]{3,}", payload.lower()))) < 8:
        return False, payload, "low-lexical-variety"
    return True, payload, "admitted"


# --------------------------------------------------------------------------
# embedders
# --------------------------------------------------------------------------


class HashingEmbedder:
    """Deterministic, offline, TEST-ONLY embedder.

    Never used for a production gate: the gate receipt records the embedder and
    `query` refuses when it does not match the embedder that produced the index.
    """

    name = HASHING_EMBEDDER
    dims = HASHING_DIMS

    def embed(self, texts: Sequence[str]) -> list[list[float]]:
        out = []
        for text in texts:
            vector = [0.0] * self.dims
            for token in terms_of(text):
                digest = hashlib.sha256(token.encode()).digest()
                slot = digest[0] % self.dims
                sign = 1.0 if digest[1] % 2 else -1.0
                vector[slot] += sign
            norm = math.sqrt(sum(v * v for v in vector)) or 1.0
            out.append([v / norm for v in vector])
        return out


class ProviderEmbedder:
    def __init__(self, model: str = PROVIDER_MODEL, dims: int = 1024):
        self.name = f"provider:{model}"
        self.model = model
        self.dims = dims
        self.endpoint = os.environ.get("CAS_CLOUD_ENDPOINT", "")
        self.token = os.environ.get("CAS_CLOUD_TOKEN", "")

    def embed(self, texts: Sequence[str]) -> list[list[float]]:
        if not self.endpoint or not self.token:
            raise SystemExit("CAS_CLOUD_ENDPOINT and CAS_CLOUD_TOKEN are required for provider embeddings")
        body = json.dumps({"model": self.model, "input": list(texts)}).encode()
        request = urllib.request.Request(
            self.endpoint.rstrip("/") + "/api/embeddings",
            data=body,
            headers={"Authorization": "Bearer " + self.token, "Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(request, timeout=90) as response:
                payload = json.load(response)
        except urllib.error.HTTPError as exc:
            raise RuntimeError(f"embedding HTTP {exc.code}: {exc.read(400).decode(errors='replace')}") from exc
        vectors = payload.get("embeddings")
        if not isinstance(vectors, list) or len(vectors) != len(texts):
            raise RuntimeError("embedding response count mismatch")
        if any(len(v) != self.dims for v in vectors):
            raise RuntimeError("embedding dimension mismatch")
        return vectors


def embedder_for(index_db: sqlite3.Connection):
    name = json.loads(meta_get(index_db, "vector_embedder", '""'))
    dims = int(json.loads(meta_get(index_db, "dimensions", "0")))
    if name == HASHING_EMBEDDER:
        embedder = HashingEmbedder()
        embedder.dims = dims or HASHING_DIMS
        return embedder
    if name.startswith("provider:"):
        return ProviderEmbedder(name.split(":", 1)[1], dims or 1024)
    raise SystemExit(f"unknown vector embedder in index meta: {name!r}")


# --------------------------------------------------------------------------
# index schema / build
# --------------------------------------------------------------------------

SCHEMA = """
CREATE TABLE IF NOT EXISTS op_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS op_events(
  id INTEGER PRIMARY KEY,
  namespace TEXT NOT NULL,
  content_hash TEXT NOT NULL UNIQUE,
  source_kind TEXT NOT NULL,
  text TEXT NOT NULL,
  duplicate_count INTEGER NOT NULL DEFAULT 1,
  admitted_reason TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS op_occurrences(
  id INTEGER PRIMARY KEY,
  event_id INTEGER NOT NULL REFERENCES op_events(id),
  namespace TEXT NOT NULL,
  source_path TEXT NOT NULL,
  session_id TEXT NOT NULL DEFAULT '',
  task_id TEXT NOT NULL DEFAULT '',
  worker TEXT NOT NULL DEFAULT '',
  timestamp TEXT NOT NULL DEFAULT '',
  epoch TEXT NOT NULL DEFAULT 'unattributed',
  privacy_scope TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS op_vectors(
  event_id INTEGER PRIMARY KEY REFERENCES op_events(id),
  namespace TEXT NOT NULL,
  vector BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS op_rejections(reason TEXT PRIMARY KEY, count INTEGER NOT NULL DEFAULT 0);
CREATE INDEX IF NOT EXISTS idx_op_occ_event ON op_occurrences(event_id);
CREATE INDEX IF NOT EXISTS idx_op_occ_task ON op_occurrences(task_id);
CREATE INDEX IF NOT EXISTS idx_op_occ_session ON op_occurrences(session_id);
CREATE VIRTUAL TABLE IF NOT EXISTS op_events_fts USING fts5(text, content='op_events', content_rowid='id');
"""


def meta_set(db: sqlite3.Connection, key: str, value: object) -> None:
    db.execute("INSERT OR REPLACE INTO op_meta(key,value) VALUES(?,?)", (key, json.dumps(value, sort_keys=True)))


def meta_get(db: sqlite3.Connection, key: str, default: str = "null") -> str:
    row = db.execute("SELECT value FROM op_meta WHERE key=?", (key,)).fetchone()
    return row[0] if row else default


def corpus_fingerprint(db: sqlite3.Connection) -> str:
    events, vectors = db.execute("SELECT COUNT(*), (SELECT COUNT(*) FROM op_vectors) FROM op_events").fetchone()
    digest = db.execute("SELECT COALESCE(GROUP_CONCAT(content_hash), '') FROM (SELECT content_hash FROM op_events ORDER BY id LIMIT 256)").fetchone()[0]
    payload = f"{NAMESPACE}|{events}|{vectors}|{hashlib.sha256(digest.encode()).hexdigest()}"
    return hashlib.sha256(payload.encode()).hexdigest()


def build(args: argparse.Namespace) -> None:
    frozen = open_ro(args.frozen)
    frozen_meta = {k: v for k, v in frozen.execute("SELECT key,value FROM meta").fetchall()}
    dims = int(json.loads(frozen_meta.get("dimensions", "1024")))
    embedder_name = json.loads(frozen_meta.get("vector_embedder", json.dumps(f"provider:{json.loads(frozen_meta.get('model', json.dumps(PROVIDER_MODEL)))}")))

    args.index.parent.mkdir(parents=True, exist_ok=True)
    if args.index.exists():
        args.index.unlink()
    db = sqlite3.connect(args.index)
    db.executescript(SCHEMA)

    stats = collections.Counter()
    rejections: collections.Counter = collections.Counter()
    sql = "SELECT id,content_hash,source_kind,text,duplicate_count FROM chunks ORDER BY id"
    if args.limit:
        sql += f" LIMIT {int(args.limit)}"
    mapping: dict[int, int] = {}
    for chunk_id, chunk_hash, kind, text, duplicates in frozen.execute(sql):
        stats["considered"] += 1
        if kind in FOREIGN_KINDS or kind not in OPERATIONAL_KINDS:
            rejections["foreign-or-unknown-kind"] += 1
            continue
        admitted, payload, reason = admit(text)
        if not admitted:
            rejections[reason] += 1
            continue
        payload_hash = content_hash(payload)
        try:
            cursor = db.execute(
                "INSERT INTO op_events(namespace,content_hash,source_kind,text,duplicate_count,admitted_reason)"
                " VALUES(?,?,?,?,?,?)",
                (NAMESPACE, payload_hash, kind, payload, duplicates, reason),
            )
        except sqlite3.IntegrityError:
            existing = db.execute("SELECT id FROM op_events WHERE content_hash=?", (payload_hash,)).fetchone()[0]
            db.execute("UPDATE op_events SET duplicate_count=duplicate_count+? WHERE id=?", (duplicates, existing))
            mapping[chunk_id] = existing
            stats["collapsed_after_envelope_strip"] += 1
            continue
        event_id = int(cursor.lastrowid)
        mapping[chunk_id] = event_id
        db.execute("INSERT INTO op_events_fts(rowid,text) VALUES(?,?)", (event_id, payload))
        stats["admitted"] += 1
        _ = chunk_hash

    # One sequential scan per source table: per-row lookups against the frozen
    # corpus are ~30x slower at full corpus size.
    for chunk_id, path, session, task, worker, ts, epoch, scope in frozen.execute(
        "SELECT chunk_id,source_path,session_id,task_id,worker,timestamp,epoch,privacy_scope FROM occurrences"
    ):
        event_id = mapping.get(chunk_id)
        if event_id is None:
            continue
        db.execute(
            "INSERT INTO op_occurrences(event_id,namespace,source_path,session_id,task_id,worker,timestamp,epoch,privacy_scope)"
            " VALUES(?,?,?,?,?,?,?,?,?)",
            (event_id, NAMESPACE, path or "unknown", session or "", task or "", worker or "",
             ts or "", epoch or "unattributed", scope or "project-private/redacted-before-embedding"),
        )
        stats["occurrences"] += 1
    for chunk_id, vector in frozen.execute("SELECT chunk_id,vector FROM vectors"):
        event_id = mapping.get(chunk_id)
        if event_id is None:
            continue
        db.execute("INSERT OR REPLACE INTO op_vectors(event_id,namespace,vector) VALUES(?,?,?)",
                   (event_id, NAMESPACE, vector))
    stats["vectors_inherited"] = db.execute("SELECT COUNT(*) FROM op_vectors").fetchone()[0]
    stats["events_without_vector"] = db.execute(
        "SELECT COUNT(*) FROM op_events e LEFT JOIN op_vectors v ON v.event_id=e.id WHERE v.event_id IS NULL"
    ).fetchone()[0]

    for reason, count in rejections.items():
        db.execute("INSERT OR REPLACE INTO op_rejections(reason,count) VALUES(?,?)", (reason, count))

    receipt = {
        "namespace": NAMESPACE,
        "built_at": now_iso(),
        "source_corpus": str(args.frozen),
        "source_access": "read-only (mode=ro)",
        "isolation": "standalone artifact; not registered with any CAS memory/knowledge/code/history store",
        "admission_policy": "structured envelopes stripped; JSON payloads, telemetry key=value lines, "
                            "short or low-prose text rejected; structured metrics stay SQL-derived",
        "vector_provenance": "inherited from the frozen cas-c505 corpus; no re-embedding, no new provider cost",
        "counts": dict(stats),
        "rejections": dict(rejections),
    }
    meta_set(db, "namespace", NAMESPACE)
    meta_set(db, "dimensions", dims)
    meta_set(db, "vector_embedder", embedder_name)
    meta_set(db, "build_receipt", receipt)
    db.commit()
    meta_set(db, "corpus_fingerprint", corpus_fingerprint(db))
    db.commit()
    frozen.close()
    print(json.dumps(receipt, indent=2, sort_keys=True))
    db.close()


# --------------------------------------------------------------------------
# rankers
# --------------------------------------------------------------------------


def unpack(blob: bytes) -> array.array:
    values = array.array("f")
    values.frombytes(blob)
    if sys.byteorder != "little":
        values.byteswap()
    return values


def fts_ranking(db: sqlite3.Connection, query_text: str, prefix: bool = False, limit: int = 200) -> list[tuple[float, int]]:
    terms = terms_of(query_text)[:24]
    if not terms:
        return []
    if prefix:
        expression = " OR ".join(f'"{t}"*' for t in terms)
    else:
        expression = " OR ".join(f'"{t}"' for t in terms)
    try:
        rows = db.execute(
            "SELECT rowid,bm25(op_events_fts) FROM op_events_fts WHERE op_events_fts MATCH ? "
            "ORDER BY bm25(op_events_fts) LIMIT ?",
            (expression, limit),
        ).fetchall()
    except sqlite3.OperationalError:
        return []
    return [(-float(score), int(rowid)) for rowid, score in rows]


def vector_ranking(db: sqlite3.Connection, vector: Sequence[float], limit: int = 200) -> list[tuple[float, int]]:
    scored: list[tuple[float, int]] = []
    query = array.array("f", vector)
    for event_id, blob in db.execute("SELECT event_id,vector FROM op_vectors"):
        candidate = unpack(blob)
        if len(candidate) != len(query):
            continue
        scored.append((sum(map(operator.mul, query, candidate)), int(event_id)))
    scored.sort(reverse=True)
    return scored[:limit]


def fuse(*rankings: list[tuple[float, int]], k: int = 60) -> list[tuple[float, int]]:
    fused: dict[int, float] = collections.defaultdict(float)
    for ranking in rankings:
        for rank, (_, event_id) in enumerate(ranking, 1):
            fused[event_id] += 1 / (k + rank)
    return sorted(((score, event_id) for event_id, score in fused.items()), reverse=True)


def rank(db: sqlite3.Connection, query_text: str, mode: str, vector: Sequence[float] | None) -> list[tuple[float, int]]:
    if mode == "lexical":
        return fts_ranking(db, query_text)
    if mode == "prefix":
        return fts_ranking(db, query_text, prefix=True)
    if vector is None:
        raise SystemExit(f"mode {mode} requires a query vector")
    semantic = vector_ranking(db, vector)
    if mode == "vector":
        return semantic
    return fuse(semantic, fts_ranking(db, query_text))


def hydrate(db: sqlite3.Connection, ranked: list[tuple[float, int]], top: int) -> list[dict]:
    rows = []
    for score, event_id in ranked[:top]:
        record = db.execute(
            "SELECT namespace,content_hash,source_kind,text,duplicate_count FROM op_events WHERE id=?", (event_id,)
        ).fetchone()
        if not record:
            continue
        occurrences = db.execute(
            "SELECT source_path,session_id,task_id,worker,timestamp,epoch,privacy_scope"
            " FROM op_occurrences WHERE event_id=? ORDER BY timestamp LIMIT 8",
            (event_id,),
        ).fetchall()
        rows.append({
            "event_id": event_id,
            "score": round(float(score), 6),
            "namespace": record[0],
            "content_hash": record[1],
            "source_kind": record[2],
            "text": record[3],
            "duplicate_count": record[4],
            "provenance": [
                {"source_path": o[0], "session_id": o[1], "task_id": o[2], "worker": o[3],
                 "timestamp": o[4], "epoch": o[5], "privacy_scope": o[6]}
                for o in occurrences
            ],
        })
    return rows


# --------------------------------------------------------------------------
# semantic gate
# --------------------------------------------------------------------------


def gate_state(db: sqlite3.Connection) -> dict:
    receipt = json.loads(meta_get(db, "semantic_gate", "null"))
    if not receipt:
        return {"available": False, "reason": "no gate receipt recorded; run `evaluate --record-gate` first"}
    current = corpus_fingerprint(db)
    if receipt.get("corpus_fingerprint") != current:
        return {"available": False, "reason": "gate receipt is stale (corpus changed since evaluation)",
                "receipt_fingerprint": receipt.get("corpus_fingerprint"), "current_fingerprint": current}
    if not receipt.get("passed"):
        return {"available": False, "reason": "semantic gate FAILED: no semantic mode beat the "
                                              "lexical/prefix baselines", "receipt": receipt}
    index_embedder = json.loads(meta_get(db, "vector_embedder", '""'))
    if receipt.get("embedder") != index_embedder:
        return {"available": False, "reason": "gate embedder does not match the index embedder",
                "gate_embedder": receipt.get("embedder"), "index_embedder": index_embedder}
    return {"available": True, "authorized_modes": receipt.get("authorized_modes", []), "receipt": receipt}


def require_gate(db: sqlite3.Connection, mode: str) -> dict:
    if mode in ("lexical", "prefix"):
        return {"available": True, "reason": "baseline channel needs no gate", "authorized_modes": [mode]}
    state = gate_state(db)
    if state["available"] and mode not in state.get("authorized_modes", []):
        state = {"available": False,
                 "reason": f"mode {mode!r} is not authorised by the recorded evaluation",
                 "authorized_modes": state.get("authorized_modes", []),
                 "per_family": {f: v.get("authorized_modes") for f, v in state["receipt"].get("families", {}).items()}}
    if not state["available"]:
        print(json.dumps({"error": "semantic answers are not authorised for this corpus in this mode",
                          "mode": mode, "gate": state}, indent=2, sort_keys=True))
        raise SystemExit(3)
    return state


def metrics_for(ranked: list[tuple[float, int]], gold: set[int], top: int) -> dict:
    ids = [event_id for _, event_id in ranked[:top]]
    hits = [i for i, event_id in enumerate(ids, 1) if event_id in gold]
    recall = len(set(ids) & gold) / len(gold) if gold else 0.0
    return {
        "recall_at_k": round(recall, 4),
        "mrr": round(1 / hits[0], 4) if hits else 0.0,
        "hits": len(hits),
        "first_hit_rank": hits[0] if hits else None,
    }


def gold_ids(db: sqlite3.Connection, hashes: Iterable[str]) -> tuple[set[int], list[str]]:
    found: set[int] = set()
    missing: list[str] = []
    for value in hashes:
        row = db.execute("SELECT id FROM op_events WHERE content_hash=?", (value,)).fetchone()
        if row:
            found.add(int(row[0]))
        else:
            missing.append(value)
    return found, missing


def authorize(summary: dict) -> dict:
    """Decide, from one family's averaged metrics, which modes may be surfaced.

    `vector` is the raw semantic channel; `hybrid` is what `query`/`join` actually
    return by default (reciprocal-rank fusion of vector and lexical).  Each is
    authorised on its own evidence: a channel that does not beat both baselines
    is not offered, whatever the other channel does.
    """
    for channel in ("vector", "hybrid"):
        for metric in ("recall", "mrr"):
            summary[f"{channel}_beats_lexical_{metric}"] = summary[f"{channel}_{metric}"] > summary[f"lexical_{metric}"]
            summary[f"{channel}_beats_prefix_{metric}"] = summary[f"{channel}_{metric}"] > summary[f"prefix_{metric}"]
    vector_recall_wins = summary["vector_beats_lexical_recall"] and summary["vector_beats_prefix_recall"]
    vector_strict = vector_recall_wins and summary["vector_beats_lexical_mrr"] and summary["vector_beats_prefix_mrr"]
    hybrid_dominates = all(summary[f"hybrid_beats_{b}_{m}"] for b in ("lexical", "prefix") for m in ("recall", "mrr"))
    modes = []
    if vector_strict:
        modes.append("vector")
    if vector_recall_wins and hybrid_dominates:
        modes.append("hybrid")
    summary["vector_recall_beats_both_baselines"] = vector_recall_wins
    summary["vector_strictly_beats_both_baselines"] = vector_strict
    summary["hybrid_dominates_both_baselines"] = hybrid_dominates
    summary["authorized_modes"] = modes
    summary["passed"] = bool(modes)
    return summary


def evaluate(args: argparse.Namespace) -> None:
    db = sqlite3.connect(args.index)
    labels = json.loads(args.labels.read_text())
    embedder = embedder_for(db)
    queries = [item["query"] for item in labels["items"]]
    vectors = embedder.embed(queries)

    per_item = []
    families: dict[str, dict[str, list[float]]] = collections.defaultdict(lambda: collections.defaultdict(list))
    for item, vector in zip(labels["items"], vectors):
        gold, missing = gold_ids(db, item["gold_content_hashes"])
        if not gold:
            raise SystemExit(f"label {item['id']}: no gold rows present in this corpus (missing={len(missing)})")
        started = time.monotonic()
        channels = {
            "prefix": fts_ranking(db, item["query"], prefix=True),
            "lexical": fts_ranking(db, item["query"]),
            "vector": vector_ranking(db, vector),
        }
        channels["hybrid"] = fuse(channels["vector"], channels["lexical"])
        scored = {name: metrics_for(ranking, gold, args.top) for name, ranking in channels.items()}
        for name, values in scored.items():
            families[item["family"]][f"{name}_recall"].append(values["recall_at_k"])
            families[item["family"]][f"{name}_mrr"].append(values["mrr"])
        per_item.append({
            "id": item["id"], "family": item["family"], "query": item["query"],
            "gold_rows": len(gold), "gold_missing": missing,
            "latency_ms": round((time.monotonic() - started) * 1000, 1),
            "channels": scored,
        })

    family_summary = {family: authorize({key: round(sum(v) / len(v), 4) for key, v in values.items()})
                      for family, values in families.items()}

    required = set(labels.get("required_families", list(family_summary)))
    covered = required <= set(family_summary)
    modes = sorted(set.intersection(*[set(family_summary[f]["authorized_modes"]) for f in required])) if covered and required else []
    passed = bool(modes)
    receipt = {
        "namespace": NAMESPACE,
        "evaluated_at": now_iso(),
        "labels_file": str(args.labels),
        "labels_revision": labels.get("revision"),
        "labelling_method": labels.get("method"),
        "top_k": args.top,
        "embedder": embedder.name if embedder.name != HASHING_EMBEDDER else HASHING_EMBEDDER,
        "corpus_fingerprint": corpus_fingerprint(db),
        "thresholds": GATE_THRESHOLDS,
        "required_families": sorted(required),
        "families": family_summary,
        "authorized_modes": modes,
        "passed": passed,
        "items": per_item,
    }
    if args.record_gate:
        meta_set(db, "semantic_gate", receipt)
        db.commit()
    print(json.dumps(receipt, indent=2, sort_keys=True))
    db.close()


# --------------------------------------------------------------------------
# isolation
# --------------------------------------------------------------------------


def probe(store: sqlite3.Connection, sql: str, needle: str, slots: int) -> int | str:
    """Count matches, or report an absent table (itself evidence of separation)."""
    try:
        return int(store.execute(sql, tuple([f"%{needle}%"] * slots)).fetchone()[0])
    except sqlite3.OperationalError as exc:
        return f"table-absent: {exc}"


def store_probe_memory(store: sqlite3.Connection, needle: str) -> int | str:
    return probe(store, "SELECT COUNT(*) FROM entries WHERE content LIKE ? OR COALESCE(title,'') LIKE ?", needle, 2)


def store_probe_knowledge(store: sqlite3.Connection, needle: str) -> int | str:
    return probe(store, "SELECT COUNT(*) FROM knowledge_pages WHERE snippet LIKE ? OR title LIKE ?", needle, 2)


def store_probe_code(store: sqlite3.Connection, needle: str) -> int | str:
    return probe(store, "SELECT COUNT(*) FROM code_symbols WHERE source LIKE ? OR qualified_name LIKE ?"
                        " OR COALESCE(documentation,'') LIKE ?", needle, 3)


def distinctive_phrase(text: str, words: int = 9) -> str:
    tokens = [t for t in text.split() if t.strip()]
    if len(tokens) < words:
        return " ".join(tokens)
    start = max(0, len(tokens) // 2 - words // 2)
    return " ".join(tokens[start:start + words])


def isolation_check(args: argparse.Namespace) -> None:
    db = sqlite3.connect(f"file:{args.index}?mode=ro", uri=True)
    stores = {"project": open_ro(args.project_db)}
    if args.user_db and Path(args.user_db).exists():
        stores["user"] = open_ro(args.user_db)

    findings: list[dict] = []
    failures: list[str] = []

    def record(check: str, ok: bool, detail: object) -> None:
        findings.append({"check": check, "ok": bool(ok), "detail": detail})
        if not ok:
            failures.append(check)

    # 1. physical separation ------------------------------------------------
    # The artifact may live under the sanctioned artifacts root; what it may not
    # do is be a CAS store, sit in a store's own directory (where sidecars, sync
    # and backup globs operate), or be registered by any store.
    index_path = Path(args.index).resolve()
    store_paths = [Path(p).resolve() for p in [args.project_db, args.user_db] if p]
    collisions = [str(p) for p in store_paths
                  if index_path == p or str(index_path).startswith(str(p) + "-") or index_path.parent == p.parent]
    record("artifact_is_not_a_cas_store_or_store_neighbour", not collisions,
           {"index": str(index_path), "stores": [str(p) for p in store_paths], "violations": collisions})

    registration = []
    for label, store in stores.items():
        op_tables = [r[0] for r in store.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'op\\_%' ESCAPE '\\'")]
        mentions = []
        for table, column in (("metadata", "value"), ("knowledge_sources", "rel_path"),
                              ("code_index_state", "repository"), ("history_index_state", "repository")):
            try:
                mentions += [f"{table}.{column}" for (count,) in store.execute(
                    f"SELECT COUNT(*) FROM {table} WHERE {column} LIKE ?", (f"%{index_path.name}%",)) if count]
            except sqlite3.OperationalError:
                continue
        registration.append({"store": label, "operational_tables": op_tables, "path_mentions": mentions})
    record("namespace_not_registered_in_any_cas_store",
           not any(r["operational_tables"] or r["path_mentions"] for r in registration), registration)

    # 2. schema separation --------------------------------------------------
    tables = {r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type IN ('table','view')")}
    foreign_tables = sorted(t for t in tables if t.split("_")[0] in {"entries", "knowledge", "code", "rules", "skills", "tasks"})
    record("no_memory_knowledge_code_tables", not foreign_tables,
           {"tables": sorted(tables), "foreign": foreign_tables})

    # 3. every row is namespaced, kinds are operational ----------------------
    bad_ns = db.execute("SELECT COUNT(*) FROM op_events WHERE namespace<>?", (NAMESPACE,)).fetchone()[0]
    bad_ns += db.execute("SELECT COUNT(*) FROM op_occurrences WHERE namespace<>?", (NAMESPACE,)).fetchone()[0]
    bad_ns += db.execute("SELECT COUNT(*) FROM op_vectors WHERE namespace<>?", (NAMESPACE,)).fetchone()[0]
    record("all_rows_namespaced", bad_ns == 0, {"namespace": NAMESPACE, "rows_outside_namespace": bad_ns})
    kinds = {r[0]: r[1] for r in db.execute("SELECT source_kind,COUNT(*) FROM op_events GROUP BY 1")}
    foreign_kinds = sorted(k for k in kinds if k not in OPERATIONAL_KINDS)
    record("no_foreign_source_kinds", not foreign_kinds, {"kinds": kinds, "foreign": foreign_kinds})

    # 4. outbound: operational text is invisible to memory/knowledge/code -----
    sample = db.execute(
        "SELECT id,content_hash,text FROM op_events WHERE LENGTH(text)>200 ORDER BY id LIMIT ?",
        (args.canaries,),
    ).fetchall()
    outbound = []
    for event_id, chash, text in sample:
        phrase = distinctive_phrase(text)
        hits = {
            f"{label}.{name}": probe(store, needle)
            for label, store in stores.items()
            for name, probe in (("memory", store_probe_memory), ("knowledge", store_probe_knowledge), ("code", store_probe_code))
            for needle in [phrase]
        }
        hash_hits = {f"{label}.memory_hash": store_probe_memory(store, chash) for label, store in stores.items()}
        outbound.append({"event_id": event_id, "phrase": phrase, "hits": hits | hash_hits})
    leaked = [o for o in outbound if any(isinstance(v, int) and v for v in o["hits"].values())]
    record("operational_text_absent_from_memory_knowledge_code", not leaked,
           {"canaries_probed": len(outbound), "leaks": leaked[:5]})

    # 5. outbound against every live full-text index in the stores -----------
    # `cas` has no CLI search verb (search is an MCP surface), so the probe goes
    # at the substrate those surfaces read: every FTS5 index in the CAS stores.
    if args.live_fts and sample:
        fts_hits = []
        for label, store in stores.items():
            for (table,) in store.execute(
                "SELECT name FROM sqlite_master WHERE type='table' AND sql LIKE '%fts5%'"
                " AND name NOT LIKE '%\\_data' ESCAPE '\\' AND name NOT LIKE '%\\_idx' ESCAPE '\\'"
                " AND name NOT LIKE '%\\_docsize' ESCAPE '\\' AND name NOT LIKE '%\\_config' ESCAPE '\\'"
                " AND name NOT LIKE '%\\_content' ESCAPE '\\'"
            ):
                for event_id, _, text in sample[: args.live_fts_canaries]:
                    phrase = distinctive_phrase(text, words=6).replace('"', " ")
                    try:
                        count = store.execute(
                            f'SELECT COUNT(*) FROM {table} WHERE {table} MATCH ?', (f'"{phrase}"',)
                        ).fetchone()[0]
                    except sqlite3.OperationalError as exc:
                        count = f"unqueryable: {exc}"
                    fts_hits.append({"store": label, "fts_index": table, "event_id": event_id,
                                     "phrase": phrase, "hits": count})
        record("live_full_text_indexes_return_no_operational_text",
               not any(isinstance(h["hits"], int) and h["hits"] for h in fts_hits),
               {"indexes_probed": sorted({h["fts_index"] for h in fts_hits}), "probes": len(fts_hits),
                "hits": [h for h in fts_hits if isinstance(h["hits"], int) and h["hits"]]})

    # 6. inbound: memory/knowledge/code text is absent from the namespace -----
    inbound = []
    project = stores["project"]
    samples: list[tuple[str, str]] = []
    for label, sql in (
        ("memory", "SELECT COALESCE(title,'')||' '||content FROM entries WHERE LENGTH(content)>200 LIMIT ?"),
        ("knowledge", "SELECT title||' '||snippet FROM knowledge_pages WHERE LENGTH(snippet)>120 LIMIT ?"),
        ("code", "SELECT qualified_name||' '||source FROM code_symbols WHERE LENGTH(source)>200 LIMIT ?"),
    ):
        for (text,) in project.execute(sql, (args.canaries,)):
            samples.append((label, text))
    for label, text in samples:
        phrase = distinctive_phrase(text)
        exact = db.execute("SELECT COUNT(*) FROM op_events WHERE content_hash=?", (content_hash(text),)).fetchone()[0]
        like = db.execute("SELECT COUNT(*) FROM op_events WHERE text LIKE ?", (f"%{phrase}%",)).fetchone()[0]
        inbound.append({"store": label, "phrase": phrase[:90], "exact_hash_rows": exact, "substring_rows": like})
    contaminated = [i for i in inbound if i["exact_hash_rows"] or i["substring_rows"]]
    record("memory_knowledge_code_text_absent_from_namespace", not contaminated,
           {"canaries_probed": len(inbound), "contamination": contaminated[:5]})

    # 7. store handles are genuinely read-only -------------------------------
    write_attempts = []
    for label, store in stores.items():
        try:
            store.execute("CREATE TABLE cas_2556_write_probe(x INTEGER)")
            write_attempts.append({"store": label, "rejected": False})
        except sqlite3.OperationalError as exc:
            write_attempts.append({"store": label, "rejected": True, "error": str(exc)})
    record("cas_stores_opened_read_only", all(a["rejected"] for a in write_attempts), write_attempts)

    verdict = {
        "namespace": NAMESPACE,
        "checked_at": now_iso(),
        "index": str(index_path),
        "passed": not failures,
        "failed_checks": failures,
        "checks": findings,
    }
    print(json.dumps(verdict, indent=2, sort_keys=True))
    for store in stores.values():
        store.close()
    db.close()
    if failures:
        raise SystemExit(2)


# --------------------------------------------------------------------------
# hybrid join
# --------------------------------------------------------------------------


def epoch_at(store: sqlite3.Connection, timestamp: str) -> dict:
    if not timestamp:
        return {"epoch": "unattributed", "reason": "event carries no timestamp", "store": "history_epochs"}
    rows = store.execute(
        "SELECT version,epoch_kind,started_at,ended_at FROM history_epochs"
        " WHERE started_at<=? AND (ended_at IS NULL OR ended_at>=?) ORDER BY started_at DESC LIMIT 4",
        (timestamp, timestamp),
    ).fetchall()
    if not rows:
        return {"epoch": "unattributed", "reason": "no deployed-binary epoch covers this timestamp",
                "store": "history_epochs", "join_key": timestamp, "join_method": "timestamp-window"}
    versions = sorted({r[0] or "unknown" for r in rows})
    return {
        "epoch": versions[0] if len(versions) == 1 else "mixed:" + "+".join(versions),
        "kinds": sorted({r[1] for r in rows}),
        "window": {"started_at": rows[0][2], "ended_at": rows[0][3]},
        "store": "history_epochs", "join_key": timestamp, "join_method": "timestamp-window",
    }


def join_tasks(store: sqlite3.Connection, ids: Iterable[str]) -> list[dict]:
    out = []
    for task_id in sorted(set(ids)):
        row = store.execute(
            "SELECT id,title,status,task_type,created_at,closed_at,COALESCE(external_ref,'') FROM tasks WHERE id=?",
            (task_id,),
        ).fetchone()
        if row:
            out.append({"store": "tasks", "join_key": task_id, "join_method": "task-id-mention",
                        "id": row[0], "title": row[1], "status": row[2], "task_type": row[3],
                        "created_at": row[4], "closed_at": row[5], "external_ref": row[6]})
    return out


def rare_terms(index_db: sqlite3.Connection, text: str, limit: int = 6, max_share: float = 0.002) -> list[str]:
    """Terms rare enough in the operational corpus to be worth joining on.

    Joining memories on words like "installed" or "processes" produces noise that
    looks like evidence; document frequency is what separates a distinctive term
    from a common one.
    """
    total = index_db.execute("SELECT COUNT(*) FROM op_events").fetchone()[0] or 1
    ceiling = max(1, int(total * max_share))
    chosen = []
    for term in (t for t in terms_of(text) if len(t) >= 8):
        try:
            count = index_db.execute(
                "SELECT COUNT(*) FROM op_events_fts WHERE op_events_fts MATCH ?", (f'"{term}"',)
            ).fetchone()[0]
        except sqlite3.OperationalError:
            continue
        if count and count <= ceiling:
            chosen.append(term)
        if len(chosen) >= limit:
            break
    return chosen


def joinable_identifiers(text: str) -> list[str]:
    """Only distinctive identifiers: bare words like `process` match everything."""
    return [i for i in dict.fromkeys(IDENT.findall(text)) if "_" in i or len(i) >= 12]


def join_symbols(store: sqlite3.Connection, text: str, limit: int, ambiguity: int = 3) -> list[dict]:
    out: list[dict] = []
    seen: set[str] = set()
    for path in list(dict.fromkeys(PATH_REF.findall(text)))[:6]:
        for row in store.execute(
            "SELECT id,qualified_name,kind,file_path,line_start,line_end,COALESCE(commit_hash,''),repository"
            # path-segment match: `pty.rs` must not also match `crypty.rs`
            " FROM code_symbols WHERE file_path=? OR file_path LIKE ? LIMIT ?",
            (path, f"%/{path.lstrip('/')}", limit),
        ):
            if row[0] in seen:
                continue
            seen.add(row[0])
            out.append({"store": "code_symbols", "join_key": path, "join_method": "file-path-mention",
                        "id": row[0], "qualified_name": row[1], "kind": row[2], "file_path": row[3],
                        "lines": [row[4], row[5]], "commit_hash": row[6], "repository": row[7]})
    for ident in joinable_identifiers(text)[:24]:
        if len(out) >= limit * 2:
            break
        matches = store.execute("SELECT COUNT(*) FROM code_symbols WHERE name=?", (ident,)).fetchone()[0]
        if not matches or matches > ambiguity:
            continue
        for row in store.execute(
            "SELECT id,qualified_name,kind,file_path,line_start,line_end,COALESCE(commit_hash,''),repository"
            " FROM code_symbols WHERE name=? LIMIT 3",
            (ident,),
        ):
            if row[0] in seen:
                continue
            seen.add(row[0])
            out.append({"store": "code_symbols", "join_key": ident, "join_method": "symbol-name-mention",
                        "id": row[0], "qualified_name": row[1], "kind": row[2], "file_path": row[3],
                        "lines": [row[4], row[5]], "commit_hash": row[6], "repository": row[7]})
    return out[: limit * 2]


def join_commits(store: sqlite3.Connection, text: str, session_id: str, limit: int) -> list[dict]:
    out: list[dict] = []
    seen: set[str] = set()
    for sha in list(dict.fromkeys(SHA.findall(text)))[:8]:
        row = store.execute(
            "SELECT sha,short_sha,subject,committed_at,repository FROM history_commits"
            " WHERE sha=? OR short_sha=? OR sha LIKE ? LIMIT 1",
            (sha, sha, sha + "%"),
        ).fetchone()
        if row and row[0] not in seen:
            seen.add(row[0])
            out.append({"store": "history_commits", "join_key": sha, "join_method": "sha-mention",
                        "sha": row[0], "short_sha": row[1], "subject": row[2],
                        "committed_at": row[3], "repository": row[4]})
    if session_id:
        for row in store.execute(
            "SELECT commit_hash,branch,message,committed_at,COALESCE(link_method,'') FROM commit_links"
            " WHERE session_id=? ORDER BY committed_at DESC LIMIT ?",
            (session_id, limit),
        ):
            if row[0] in seen:
                continue
            seen.add(row[0])
            out.append({"store": "commit_links", "join_key": session_id, "join_method": "session-provenance",
                        "sha": row[0], "branch": row[1], "subject": row[2], "committed_at": row[3],
                        "link_method": row[4] or "unrecorded"})
    return out


def join_issues(text: str, tasks: list[dict]) -> list[dict]:
    out = []
    for number in list(dict.fromkeys(ISSUE_REF.findall(text)))[:6]:
        out.append({"store": "operational-text", "join_key": f"#{number}", "join_method": "issue-ref-mention",
                    "issue": int(number), "resolution": "reference only; no network lookup performed"})
    for task in tasks:
        ref = task.get("external_ref") or ""
        for number in ISSUE_REF.findall(ref) or re.findall(r"/issues/(\d+)", ref):
            out.append({"store": "tasks.external_ref", "join_key": task["id"], "join_method": "task-external-ref",
                        "issue": int(number), "resolution": ref})
    return out


def join_memories(store: sqlite3.Connection, text: str, task_ids: Iterable[str], session_id: str,
                  limit: int, distinctive: Sequence[str] = ()) -> list[dict]:
    out: list[dict] = []
    seen: set[str] = set()

    def add(row, method: str, key: str) -> None:
        if row[0] in seen:
            return
        seen.add(row[0])
        out.append({"store": "entries", "join_key": key, "join_method": method,
                    "id": row[0], "type": row[1], "title": row[2] or "", "created": row[3],
                    "snippet": (row[4] or "")[:280],
                    "adjudication": "surfaced only; contradiction adjudication is M4 (cas-2332)"})

    select = "SELECT id,type,COALESCE(title,''),created,content FROM entries"
    if session_id:
        for row in store.execute(f"{select} WHERE session_id=? LIMIT ?", (session_id, limit)):
            add(row, "session-provenance", session_id)
    for task_id in sorted(set(task_ids)):
        for row in store.execute(f"{select} WHERE content LIKE ? OR COALESCE(title,'') LIKE ? LIMIT ?",
                                 (f"%{task_id}%", f"%{task_id}%", limit)):
            add(row, "task-id-mention", task_id)
    # A single shared rare word is coincidence, not evidence: require at least two
    # of the distinctive terms to co-occur in the same memory before surfacing it.
    overlap: dict[str, list] = {}
    hits: collections.Counter = collections.Counter()
    for term in list(distinctive)[:6]:
        for row in store.execute(f"{select} WHERE content LIKE ? LIMIT 8", (f"%{term}%",)):
            overlap.setdefault(row[0], [row, []])[1].append(term)
            hits[row[0]] += 1
    for entry_id, (row, terms) in sorted(overlap.items(), key=lambda kv: -hits[kv[0]]):
        if hits[entry_id] < 2 or len(out) >= limit * 3:
            continue
        add(row, "distinctive-term-cooccurrence", "+".join(sorted(terms)))
    return out[: limit * 3]


PROVENANCE_KEYS = ("source_path", "session_id", "task_id", "worker", "timestamp", "epoch", "privacy_scope")


def validate_provenance(rows: list[dict]) -> list[str]:
    problems = []
    for row in rows:
        if row.get("namespace") != NAMESPACE:
            problems.append(f"event {row.get('event_id')}: missing/incorrect namespace")
        if not row.get("provenance"):
            problems.append(f"event {row.get('event_id')}: no provenance occurrences")
        for occurrence in row.get("provenance", []):
            for key in PROVENANCE_KEYS:
                if key not in occurrence:
                    problems.append(f"event {row.get('event_id')}: provenance missing {key}")
            if not occurrence.get("source_path"):
                problems.append(f"event {row.get('event_id')}: empty source_path")
            if not occurrence.get("epoch"):
                problems.append(f"event {row.get('event_id')}: empty epoch")
        if "epoch_context" not in row:
            problems.append(f"event {row.get('event_id')}: no epoch context")
        for name, entities in row.get("joins", {}).items():
            for entity in entities:
                for key in ("store", "join_key", "join_method"):
                    if not entity.get(key):
                        problems.append(f"event {row.get('event_id')}: {name} entity missing {key}")
    return problems


def join_command(args: argparse.Namespace) -> None:
    db = sqlite3.connect(f"file:{args.index}?mode=ro", uri=True)
    gate = require_gate(db, args.mode)
    store = open_ro(args.project_db)
    started = time.monotonic()

    vector = None
    if args.mode in ("vector", "hybrid"):
        vector = embedder_for(db).embed([args.query])[0]
    rows = hydrate(db, rank(db, args.query, args.mode, vector), args.top)

    for row in rows:
        occurrences = row["provenance"]
        task_ids = {o["task_id"] for o in occurrences if o["task_id"]}
        task_ids |= {m.group(0).lower() for m in TASK_ID.finditer(row["text"])}
        session_id = next((o["session_id"] for o in occurrences if o["session_id"]), "")
        timestamp = next((o["timestamp"] for o in occurrences if o["timestamp"]), "")
        tasks = join_tasks(store, task_ids)
        row["epoch_context"] = epoch_at(store, timestamp)
        row["joins"] = {
            "tasks": tasks,
            "symbols": join_symbols(store, row["text"], args.join_limit),
            "commits": join_commits(store, row["text"], session_id, args.join_limit),
            "issues": join_issues(row["text"], tasks),
            "memories": join_memories(store, row["text"], task_ids, session_id, args.join_limit,
                                      rare_terms(db, row["text"])),
        }
        row["join_coverage"] = {name: len(entities) for name, entities in row["joins"].items()}

    problems = validate_provenance(rows)
    payload = {
        "namespace": NAMESPACE,
        "query": args.query,
        "mode": args.mode,
        "gate": {"available": gate.get("available"), "passed": gate.get("receipt", {}).get("passed")
                 if gate.get("receipt") else None},
        "latency_ms": round((time.monotonic() - started) * 1000, 1),
        "stores_read": ["op namespace artifact", str(args.project_db) + " (mode=ro)"],
        "rows": rows,
        "provenance_violations": problems,
    }
    print(json.dumps(payload, indent=2, sort_keys=True))
    store.close()
    db.close()
    if problems and not args.allow_missing_provenance:
        raise SystemExit(4)


def query_command(args: argparse.Namespace) -> None:
    db = sqlite3.connect(f"file:{args.index}?mode=ro", uri=True)
    gate = require_gate(db, args.mode)
    started = time.monotonic()
    vector = embedder_for(db).embed([args.query])[0] if args.mode in ("vector", "hybrid") else None
    rows = hydrate(db, rank(db, args.query, args.mode, vector), args.top)
    print(json.dumps({"namespace": NAMESPACE, "query": args.query, "mode": args.mode,
                      "gate_available": gate.get("available"),
                      "latency_ms": round((time.monotonic() - started) * 1000, 1),
                      "rows": rows}, indent=2, sort_keys=True))
    db.close()


def candidates_command(args: argparse.Namespace) -> None:
    """Pooled candidate dump for human labelling (pooled relevance, all channels)."""
    db = sqlite3.connect(f"file:{args.index}?mode=ro", uri=True)
    vector = embedder_for(db).embed([args.query])[0]
    pooled = {}
    for name, ranking in (("prefix", fts_ranking(db, args.query, prefix=True)),
                          ("lexical", fts_ranking(db, args.query)),
                          ("vector", vector_ranking(db, vector))):
        for score, event_id in ranking[: args.top]:
            entry = pooled.setdefault(event_id, {"event_id": event_id, "channels": []})
            entry["channels"].append({"channel": name, "score": round(float(score), 5)})
    for event_id, entry in pooled.items():
        row = db.execute("SELECT content_hash,source_kind,text FROM op_events WHERE id=?", (event_id,)).fetchone()
        entry["content_hash"], entry["source_kind"] = row[0], row[1]
        entry["text"] = row[2][: args.chars]
    print(json.dumps({"query": args.query, "pooled_candidates": list(pooled.values())}, indent=2, sort_keys=True))
    db.close()


def gate_command(args: argparse.Namespace) -> None:
    db = sqlite3.connect(f"file:{args.index}?mode=ro", uri=True)
    state = gate_state(db)
    print(json.dumps(state, indent=2, sort_keys=True))
    db.close()
    if not state["available"]:
        raise SystemExit(3)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)

    build_cmd = sub.add_parser("build", help="build the operational namespace from a read-only corpus")
    build_cmd.add_argument("--frozen", type=Path, default=DEFAULT_FROZEN)
    build_cmd.add_argument("--index", type=Path, default=DEFAULT_INDEX)
    build_cmd.add_argument("--limit", type=int, default=0)
    build_cmd.set_defaults(func=build)

    iso = sub.add_parser("isolation-check", help="prove namespace isolation in both directions")
    iso.add_argument("--index", type=Path, default=DEFAULT_INDEX)
    iso.add_argument("--project-db", type=Path, default=DEFAULT_PROJECT_DB)
    iso.add_argument("--user-db", type=Path, default=DEFAULT_USER_DB)
    iso.add_argument("--canaries", type=int, default=25)
    iso.add_argument("--live-fts", action="store_true",
                     help="also probe every live FTS5 index in the CAS stores with operational canaries")
    iso.add_argument("--live-fts-canaries", type=int, default=5)
    iso.set_defaults(func=isolation_check)

    eva = sub.add_parser("evaluate", help="score vector vs lexical/prefix baselines on the labelled set")
    eva.add_argument("--index", type=Path, default=DEFAULT_INDEX)
    eva.add_argument("--labels", type=Path, required=True)
    eva.add_argument("--top", type=int, default=10)
    eva.add_argument("--record-gate", action="store_true")
    eva.set_defaults(func=evaluate)

    gate = sub.add_parser("gate", help="show whether vector answers are authorised")
    gate.add_argument("--index", type=Path, default=DEFAULT_INDEX)
    gate.set_defaults(func=gate_command)

    qry = sub.add_parser("query", help="search the operational namespace")
    qry.add_argument("query")
    qry.add_argument("--index", type=Path, default=DEFAULT_INDEX)
    qry.add_argument("--mode", choices=("prefix", "lexical", "vector", "hybrid"), default="hybrid")
    qry.add_argument("--top", type=int, default=8)
    qry.set_defaults(func=query_command)

    cand = sub.add_parser("candidates", help="pooled candidates for human labelling")
    cand.add_argument("query")
    cand.add_argument("--index", type=Path, default=DEFAULT_INDEX)
    cand.add_argument("--top", type=int, default=10)
    cand.add_argument("--chars", type=int, default=600)
    cand.set_defaults(func=candidates_command)

    joi = sub.add_parser("join", help="hybrid query joined to tasks, symbols, commits, issues, memories")
    joi.add_argument("query")
    joi.add_argument("--index", type=Path, default=DEFAULT_INDEX)
    joi.add_argument("--project-db", type=Path, default=DEFAULT_PROJECT_DB)
    joi.add_argument("--mode", choices=("prefix", "lexical", "vector", "hybrid"), default="hybrid")
    joi.add_argument("--top", type=int, default=5)
    joi.add_argument("--join-limit", type=int, default=5)
    joi.add_argument("--allow-missing-provenance", action="store_true")
    joi.set_defaults(func=join_command)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
