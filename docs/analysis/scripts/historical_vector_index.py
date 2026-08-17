#!/usr/bin/env python3
"""Build and query the frozen cas-c505 operational-evidence vector index.

The source stores are read-only.  The output is a standalone SQLite database;
it is intentionally not registered with any CAS memory, knowledge, history, or
code-vector store.  Python's standard library is the only runtime dependency.
"""

from __future__ import annotations

import argparse
import array
import collections
import hashlib
import json
import math
import os
import re
import sqlite3
import struct
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterable, Iterator


DEFAULT_CUTOFF = "2026-08-17T13:44:57Z"
DEFAULT_SNAPSHOT = Path("/home/pippenz/.cas/artifacts/cas-c505/snapshot/cas.db")
DEFAULT_INDEX = Path("/home/pippenz/.cas/artifacts/cas-c505/frozen-index/index.sqlite3")
CAS_REPO = Path("/home/pippenz/Petrastella/cas-src")
MODEL = "cas-embed-v1"
DIMS = 1024
MAX_BATCH = 32
PRICE_PER_MILLION_TOKENS = 0.13  # provider model text-embedding-3-large

ISO_PREFIX = re.compile(r"^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z?)")
TASK_ID = re.compile(r"\bcas-[0-9a-f]{4,}\b", re.I)
WORKER = re.compile(r"/\.cas/worktrees/([^/\s]+)")
SECRET_PATTERNS = (
    re.compile(r"(?i)(authorization\s*:\s*bearer\s+)[A-Za-z0-9._~+/=-]+"),
    re.compile(r"(?i)\b(sk|pk|ghp|github_pat|xox[baprs])[-_A-Za-z0-9]{12,}\b"),
    re.compile(r"(?i)((?:api[_-]?key|token|password|secret)\s*[=:]\s*)[^\s,;\"']{6,}"),
    re.compile(r"-----BEGIN [^-]+ PRIVATE KEY-----.*?-----END [^-]+ PRIVATE KEY-----", re.S),
)
BOILERPLATE_BLOCKS = (
    "skills_instructions", "permissions instructions", "environment_context",
    "user_info", "git_status", "apps_instructions", "plugins_instructions",
)


def parse_time(value: object) -> datetime | None:
    if isinstance(value, (int, float)):
        value = value / 1000 if value > 10_000_000_000 else value
        return datetime.fromtimestamp(value, timezone.utc)
    if not isinstance(value, str) or not value:
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00")).astimezone(timezone.utc)
    except ValueError:
        return None


def privacy_redact(text: str) -> tuple[str, int]:
    count = 0
    for pattern in SECRET_PATTERNS:
        text, n = pattern.subn(lambda m: (m.group(1) if m.lastindex else "") + "[REDACTED_SECRET]", text)
        count += n
    text, n = re.subn(r"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b", "[REDACTED_EMAIL]", text)
    return text, count + n


def strip_boilerplate(text: str) -> str:
    for tag in BOILERPLATE_BLOCKS:
        text = re.sub(rf"<{re.escape(tag)}\b[^>]*>.*?</{re.escape(tag)}>", " ", text, flags=re.I | re.S)
    text = re.sub(r"<system-reminder>.*?</system-reminder>", " ", text, flags=re.I | re.S)
    text = re.sub(r"<!--.*?-->", " ", text, flags=re.S)
    text = re.sub(r"(?m)^CAS provenance: notification_id=.*$", " ", text)
    text = re.sub(r"(?m)^# AGENTS\.md instructions.*?(?=\n\S|\Z)", " ", text, flags=re.S)
    return re.sub(r"\n{3,}", "\n\n", text).strip()


def normalized(text: str) -> str:
    value = text.lower()
    value = re.sub(r"\b[0-9a-f]{8,40}\b", "<hex>", value)
    value = re.sub(r"\b\d+(?:\.\d+)?(?:ms|s|mb|gb|bytes?)?\b", "<n>", value)
    value = re.sub(r"/home/pippenz/[^\s\"']+", "<path>", value)
    return re.sub(r"\s+", " ", value).strip()


def meaningful(text: str) -> bool:
    if len(text) < 48 or len(text.split()) < 7:
        return False
    low = text.lower()
    if low.startswith(("you are an ai", "you are codex", "you are grok", "you are claude")):
        return False
    if "<skills_instructions>" in low or "toolsearch(query=" in low:
        return False
    return len(set(re.findall(r"[a-z]{3,}", low))) >= 5


def chunks(text: str, limit: int = 3600, overlap: int = 240) -> Iterator[str]:
    text = re.sub(r"[ \t]+", " ", text).strip()
    if len(text) <= limit:
        if meaningful(text):
            yield text
        return
    start = 0
    while start < len(text):
        end = min(len(text), start + limit)
        if end < len(text):
            split = text.rfind("\n", start + limit // 2, end)
            if split > start:
                end = split
        piece = text[start:end].strip()
        if meaningful(piece):
            yield piece
        if end >= len(text):
            break
        start = max(start + 1, end - overlap)


def strings(value: object) -> Iterator[str]:
    if isinstance(value, str):
        yield value
    elif isinstance(value, list):
        for item in value:
            if isinstance(item, dict) and item.get("type") in {"text", "input_text", "output_text"}:
                yield str(item.get("text", ""))


@dataclass
class Evidence:
    source_kind: str
    source_path: str
    session_id: str = ""
    task_id: str = ""
    worker: str = ""
    timestamp: str = ""
    epoch: str = "unknown"
    privacy_scope: str = "project-private/redacted-before-embedding"
    text: str = ""


class Epochs:
    def __init__(self, db: sqlite3.Connection):
        self.rows = []
        try:
            rows = db.execute("SELECT version,started_at,ended_at FROM history_epochs ORDER BY started_at").fetchall()
            self.rows = [(v, parse_time(s), parse_time(e)) for v, s, e in rows]
        except sqlite3.Error:
            pass

    def at(self, value: str) -> str:
        ts = parse_time(value)
        if not ts:
            return "unknown"
        if ts < parse_time("2026-08-07T21:02:26Z"):
            return "pre-v2.49.0"
        if ts < parse_time("2026-08-07T21:36:35Z"):
            return "mixed-v2.49.0"
        active = sorted({v for v, start, end in self.rows if start and start <= ts and (not end or ts <= end)})
        if len(active) > 1:
            return "mixed:" + "+".join(active)
        if active:
            return active[0]
        return "post-v2.49.0/unattributed"


def db_evidence(path: Path, cutoff: datetime, row_limit: int = 0) -> Iterator[Evidence]:
    db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    epochs = Epochs(db)
    cutoff_s = cutoff.isoformat().replace("+00:00", "Z")
    queries = [
        ("task", "SELECT id,created_at,COALESCE(assignee,''),title||'\n'||description||'\n'||design||'\n'||acceptance_criteria||'\n'||notes FROM tasks WHERE created_at<=?"),
        ("event", "SELECT entity_id,created_at,COALESCE(session_id,''),summary FROM events WHERE created_at<=?"),
        ("prompt_queue", "SELECT COALESCE(summary,''),created_at,target,prompt FROM prompt_queue WHERE created_at<=?"),
        ("supervisor_queue", "SELECT event_type,created_at,supervisor_id,payload FROM supervisor_queue WHERE created_at<=?"),
    ]
    for kind, query in queries:
        sql = query + (" LIMIT ?" if row_limit else "")
        params = (cutoff_s, row_limit) if row_limit else (cutoff_s,)
        for ident, ts, actor, text in db.execute(sql, params):
            task = ident if isinstance(ident, str) and ident.startswith("cas-") else ""
            match = TASK_ID.search(str(text))
            task = task or (match.group(0).lower() if match else "")
            yield Evidence(kind, "snapshot:cas.db", str(actor) if kind == "event" else "", task,
                           str(actor) if kind != "event" else "", str(ts), epochs.at(str(ts)), text=str(text))
    db.close()


def log_evidence(root: Path, cutoff: datetime, epochs: Epochs, row_limit: int = 0) -> Iterator[Evidence]:
    emitted = 0
    for path in sorted(root.glob("*.log")):
        with path.open(errors="replace") as handle:
            for line in handle:
                match = ISO_PREFIX.match(line)
                ts = match.group(1) if match else ""
                parsed = parse_time(ts)
                if parsed and parsed > cutoff:
                    continue
                body = re.sub(r"^\S+\s+(?:TRACE|DEBUG|INFO|WARN|ERROR)\s+", "", line).strip()
                task = (TASK_ID.search(body).group(0).lower() if TASK_ID.search(body) else "")
                worker_m = WORKER.search(body)
                yield Evidence("daemon_log", f"logs/{path.name}", task_id=task,
                               worker=worker_m.group(1) if worker_m else "", timestamp=ts,
                               epoch=epochs.at(ts), text=body)
                emitted += 1
                if row_limit and emitted >= row_limit:
                    return


def claude_evidence(path: Path, cutoff: datetime, epochs: Epochs) -> Iterator[Evidence]:
    session = path.stem
    cwd = ""
    with path.open(errors="replace") as handle:
        for row in handle:
            try:
                obj = json.loads(row)
            except json.JSONDecodeError:
                continue
            ts = parse_time(obj.get("timestamp"))
            if ts and ts > cutoff:
                continue
            session = str(obj.get("sessionId") or session)
            cwd = str(obj.get("cwd") or cwd)
            msg = obj.get("message") if isinstance(obj.get("message"), dict) else {}
            role = msg.get("role")
            if role not in {"user", "assistant"}:
                continue
            for text in strings(msg.get("content")):
                stamp = str(obj.get("timestamp", ""))
                yield Evidence("claude_transcript", str(path), session, worker=worker_from(cwd),
                               timestamp=stamp, epoch=epochs.at(stamp), text=text)


def codex_evidence(path: Path, cutoff: datetime, epochs: Epochs) -> Iterator[Evidence]:
    session = path.stem
    cwd = ""
    is_cas = False
    pending: list[Evidence] = []
    with path.open(errors="replace") as handle:
        for row in handle:
            try:
                obj = json.loads(row)
            except json.JSONDecodeError:
                continue
            payload = obj.get("payload") if isinstance(obj.get("payload"), dict) else {}
            if obj.get("type") == "session_meta":
                session = str(payload.get("session_id") or payload.get("id") or session)
                cwd = str(payload.get("cwd") or cwd)
                is_cas = "/Petrastella/cas-src" in cwd
            ts = parse_time(obj.get("timestamp"))
            if ts and ts > cutoff:
                continue
            if obj.get("type") == "response_item" and payload.get("type") == "message" and payload.get("role") in {"user", "assistant"}:
                for text in strings(payload.get("content")):
                    stamp = str(obj.get("timestamp", ""))
                    pending.append(Evidence("codex_transcript", str(path), session, worker=worker_from(cwd),
                                            timestamp=stamp, epoch=epochs.at(stamp), text=text))
    if is_cas:
        yield from pending


def grok_evidence(path: Path, cutoff: datetime, epochs: Epochs) -> Iterator[Evidence]:
    session = path.parent.name
    cwd = path.parents[1].name.replace("%2F", "/")
    with path.open(errors="replace") as handle:
        for row in handle:
            try:
                obj = json.loads(row)
            except json.JSONDecodeError:
                continue
            if obj.get("type") not in {"user", "assistant"}:
                continue
            # chat_history lacks reliable per-row timestamps; the file cutoff is
            # enforced by inventory and the frozen scan date is retained in meta.
            for text in strings(obj.get("content")):
                yield Evidence("grok_transcript", str(path), session, worker=worker_from(cwd), text=text)


def worker_from(cwd: str) -> str:
    match = WORKER.search(cwd)
    return match.group(1) if match else ""


def source_files() -> dict[str, list[Path]]:
    claude = [p for root in (Path.home()/".claude", Path.home()/".claude-alt")
              for p in root.rglob("*.jsonl") if "cas-src" in str(p)]
    grok = [p for p in (Path.home()/".grok/sessions").rglob("chat_history.jsonl") if "cas-src" in str(p)]
    codex = [p for p in (Path.home()/".codex/sessions").rglob("*.jsonl") if codex_is_cas(p)]
    logs = list((CAS_REPO/".cas/logs").glob("*.log"))
    return {"claude": claude, "codex_candidates": codex, "grok": grok, "logs": logs}


def codex_is_cas(path: Path) -> bool:
    try:
        with path.open(errors="replace") as handle:
            return any("/Petrastella/cas-src" in handle.readline() for _ in range(8))
    except OSError:
        return False


def inventory(snapshot: Path, cutoff: datetime) -> dict:
    groups = source_files()
    result = {"cutoff": cutoff.isoformat(), "sources": {}}
    for name, paths in groups.items():
        result["sources"][name] = {"files": len(paths), "bytes": sum(p.stat().st_size for p in paths)}
    snap = [snapshot, Path(str(snapshot)+"-wal"), Path(str(snapshot)+"-shm")]
    result["sources"]["snapshot"] = {"files": sum(p.exists() for p in snap), "bytes": sum(p.stat().st_size for p in snap if p.exists())}
    all_codex = list((Path.home()/".codex/sessions").rglob("*.jsonl"))
    result["discovery_scan"] = {"codex_candidate_files": len(all_codex),
                                "codex_candidate_bytes": sum(p.stat().st_size for p in all_codex),
                                "selection": "session_meta cwd under /Petrastella/cas-src"}
    result["eligible_bytes"] = sum(v["bytes"] for v in result["sources"].values())
    return result


SCHEMA = """
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS chunks(
 id INTEGER PRIMARY KEY, content_hash TEXT NOT NULL UNIQUE, source_kind TEXT NOT NULL,
 text TEXT NOT NULL, duplicate_count INTEGER NOT NULL DEFAULT 1, redaction_count INTEGER NOT NULL DEFAULT 0,
 embedded INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS occurrences(
 id INTEGER PRIMARY KEY, chunk_id INTEGER NOT NULL REFERENCES chunks(id), source_path TEXT NOT NULL,
 session_id TEXT, task_id TEXT, worker TEXT, timestamp TEXT, epoch TEXT NOT NULL, privacy_scope TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS vectors(chunk_id INTEGER PRIMARY KEY REFERENCES chunks(id), vector BLOB NOT NULL);
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(text, content='chunks', content_rowid='id');
"""


def prepare(args: argparse.Namespace) -> None:
    cutoff = parse_time(args.cutoff)
    assert cutoff
    args.index.parent.mkdir(parents=True, exist_ok=True)
    db = sqlite3.connect(args.index)
    db.executescript(SCHEMA)
    db.executescript("DELETE FROM vectors; DELETE FROM occurrences; DELETE FROM chunks_fts; DELETE FROM chunks; DELETE FROM meta;")
    inv = inventory(args.snapshot, cutoff)
    seen: dict[str, int] = {}
    stats = collections.Counter()

    def add(ev: Evidence) -> None:
        raw = strip_boilerplate(ev.text)
        raw, redactions = privacy_redact(raw)
        for piece in chunks(raw):
            stats["semantic_candidates"] += 1
            key = hashlib.sha256(normalized(piece).encode()).hexdigest()
            chunk_id = seen.get(key)
            if chunk_id is None:
                cur = db.execute("INSERT INTO chunks(content_hash,source_kind,text,redaction_count) VALUES(?,?,?,?)",
                                 (key, ev.source_kind, piece, redactions))
                chunk_id = cur.lastrowid
                seen[key] = chunk_id
                db.execute("INSERT INTO chunks_fts(rowid,text) VALUES(?,?)", (chunk_id, piece))
                stats["unique_chunks"] += 1
                stats["chars"] += len(piece)
                stats["redactions"] += redactions
            else:
                db.execute("UPDATE chunks SET duplicate_count=duplicate_count+1 WHERE id=?", (chunk_id,))
                stats["duplicates_collapsed"] += 1
            task = ev.task_id or (TASK_ID.search(piece).group(0).lower() if TASK_ID.search(piece) else "")
            db.execute("INSERT INTO occurrences(chunk_id,source_path,session_id,task_id,worker,timestamp,epoch,privacy_scope) VALUES(?,?,?,?,?,?,?,?)",
                       (chunk_id, ev.source_path, ev.session_id, task, ev.worker, ev.timestamp,
                        ev.epoch if ev.epoch != "unknown" else "unattributed", ev.privacy_scope))

    epoch_db = sqlite3.connect(f"file:{args.snapshot}?mode=ro", uri=True)
    epochs = Epochs(epoch_db)
    for ev in db_evidence(args.snapshot, cutoff, args.sample_rows):
        add(ev)
    for ev in log_evidence(CAS_REPO/".cas/logs", cutoff, epochs, args.sample_rows):
        add(ev)
    groups = source_files()
    limits = {name: args.sample_files for name in groups} if args.sample_files else {}
    for path in groups["claude"][:limits.get("claude", None)]:
        for ev in claude_evidence(path, cutoff, epochs): add(ev)
    for path in groups["codex_candidates"][:limits.get("codex_candidates", None)]:
        for ev in codex_evidence(path, cutoff, epochs): add(ev)
    for path in groups["grok"][:limits.get("grok", None)]:
        for ev in grok_evidence(path, cutoff, epochs): add(ev)
    epoch_db.close()

    estimate_tokens = math.ceil(stats["chars"] / 4)
    meta = {
        "cutoff": args.cutoff, "model": MODEL, "dimensions": DIMS,
        "privacy_scope": "project-private; deterministic secret/email redaction before cloud embedding",
        "source_inventory": inv, "reduction": dict(stats), "estimated_tokens": estimate_tokens,
        "estimated_provider_cost_usd": round(estimate_tokens / 1_000_000 * PRICE_PER_MILLION_TOKENS, 4),
        "isolation": "standalone artifact; not registered in CAS live search",
        "prepared_at": datetime.now(timezone.utc).isoformat(),
    }
    for key, value in meta.items():
        db.execute("INSERT INTO meta(key,value) VALUES(?,?)", (key, json.dumps(value, sort_keys=True)))
    db.commit()
    print(json.dumps(meta, indent=2, sort_keys=True))
    db.close()


def post_embeddings(endpoint: str, token: str, texts: list[str]) -> list[list[float]]:
    body = json.dumps({"model": MODEL, "input": texts}).encode()
    req = urllib.request.Request(endpoint.rstrip("/")+"/api/embeddings", data=body,
                                 headers={"Authorization": "Bearer "+token, "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=90) as response:
            payload = json.load(response)
    except urllib.error.HTTPError as exc:
        raise RuntimeError(f"embedding HTTP {exc.code}: {exc.read(500).decode(errors='replace')}") from exc
    vectors = payload.get("embeddings")
    if not isinstance(vectors, list) or len(vectors) != len(texts):
        raise RuntimeError("embedding response count mismatch")
    if any(len(vector) != DIMS for vector in vectors):
        raise RuntimeError("embedding dimension mismatch")
    return vectors


def embed(args: argparse.Namespace) -> None:
    endpoint = os.environ.get("CAS_CLOUD_ENDPOINT", "")
    token = os.environ.get("CAS_CLOUD_TOKEN", "")
    if not endpoint or not token:
        raise SystemExit("CAS_CLOUD_ENDPOINT and CAS_CLOUD_TOKEN are required")
    db = sqlite3.connect(args.index)
    started = time.monotonic()
    count = requests = retries = 0
    while args.limit == 0 or count < args.limit:
        budget = MAX_BATCH if args.limit == 0 else min(MAX_BATCH, args.limit-count)
        rows = db.execute("SELECT id,text FROM chunks WHERE embedded=0 ORDER BY id LIMIT ?", (budget,)).fetchall()
        if not rows: break
        for attempt in range(5):
            try:
                vectors = post_embeddings(endpoint, token, [text for _, text in rows])
                break
            except RuntimeError:
                if attempt == 4:
                    raise
                retries += 1
                delay = 2 ** attempt
                print(f"embedding request failed; retry={attempt + 1}/4 delay={delay}s", file=sys.stderr)
                time.sleep(delay)
        for (chunk_id, _), vector in zip(rows, vectors):
            norm = math.sqrt(sum(v*v for v in vector))
            if norm < 0.5: raise RuntimeError(f"zero/invalid vector for {chunk_id}")
            db.execute("INSERT OR REPLACE INTO vectors(chunk_id,vector) VALUES(?,?)",
                       (chunk_id, struct.pack(f"<{DIMS}f", *vector)))
            db.execute("UPDATE chunks SET embedded=1 WHERE id=?", (chunk_id,))
        db.commit()
        count += len(rows); requests += 1
        if args.pause and len(rows) == MAX_BATCH: time.sleep(args.pause)
        if requests % 20 == 0: print(f"embedded={count} requests={requests}", file=sys.stderr)
    duration = time.monotonic()-started
    db.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('embedding_receipt',?)",
               (json.dumps({"embedded_this_run": count, "requests": requests, "retries": retries, "duration_seconds": round(duration,3),
                            "completed_at": datetime.now(timezone.utc).isoformat(), "server_retention": "no vectors/text; request metadata only per committed cloud response"}),))
    db.commit(); db.close()
    print(json.dumps({"embedded": count, "requests": requests, "retries": retries, "duration_seconds": round(duration, 3)}))


def unpack(blob: bytes) -> array.array:
    result = array.array("f"); result.frombytes(blob)
    if sys.byteorder != "little": result.byteswap()
    return result


def hydrate_results(db: sqlite3.Connection, ranked: list[tuple[float, int]], k: int) -> list[dict]:
    result = []
    for score, chunk_id in ranked[:k]:
        row = db.execute("SELECT source_kind,text,duplicate_count FROM chunks WHERE id=?", (chunk_id,)).fetchone()
        prov = db.execute("SELECT source_path,session_id,task_id,worker,timestamp,epoch,privacy_scope FROM occurrences WHERE chunk_id=? ORDER BY timestamp LIMIT 8", (chunk_id,)).fetchall()
        result.append({"chunk_id": chunk_id, "score": round(score, 6), "source_kind": row[0],
                       "text": row[1], "duplicate_count": row[2], "provenance": prov})
    return result


def vector_ranking(db: sqlite3.Connection, vector: list[float]) -> list[tuple[float, int]]:
    scored = []
    for chunk_id, blob in db.execute("SELECT chunk_id,vector FROM vectors"):
        candidate = unpack(blob)
        score = sum(a*b for a, b in zip(vector, candidate))
        scored.append((score, chunk_id))
    scored.sort(reverse=True)
    return scored


def lexical_ranking(db: sqlite3.Connection, query_text: str) -> list[tuple[float, int]]:
    terms = list(dict.fromkeys(re.findall(r"[A-Za-z][A-Za-z0-9_-]{2,}", query_text.lower())))
    if not terms:
        return []
    expression = " OR ".join('"'+term.replace('"', '')+'"' for term in terms[:24])
    rows = db.execute("SELECT rowid,bm25(chunks_fts) FROM chunks_fts WHERE chunks_fts MATCH ? ORDER BY bm25(chunks_fts) LIMIT 200", (expression,)).fetchall()
    return [(-float(score), int(rowid)) for rowid, score in rows]


def query(args: argparse.Namespace) -> None:
    db = sqlite3.connect(args.index)
    started = time.monotonic()
    lexical = lexical_ranking(db, args.query)
    if args.mode == "lexical":
        ranked = lexical
    else:
        endpoint = os.environ.get("CAS_CLOUD_ENDPOINT", ""); token = os.environ.get("CAS_CLOUD_TOKEN", "")
        if not endpoint or not token: raise SystemExit("CAS cloud credentials required for vector query")
        vector = post_embeddings(endpoint, token, [args.query])[0]
        semantic = vector_ranking(db, vector)
        if args.mode == "vector":
            ranked = semantic
        else:
            fused = collections.defaultdict(float)
            for rank, (_, chunk_id) in enumerate(semantic[:200], 1): fused[chunk_id] += 1/(60+rank)
            for rank, (_, chunk_id) in enumerate(lexical[:200], 1): fused[chunk_id] += 1/(60+rank)
            ranked = sorted(((score, chunk_id) for chunk_id, score in fused.items()), reverse=True)
    result = hydrate_results(db, ranked, args.top)
    print(json.dumps({"query": args.query, "mode": args.mode, "latency_ms": round((time.monotonic()-started)*1000, 2), "results": result}, indent=2))
    db.close()


def evaluate(args: argparse.Namespace) -> None:
    endpoint = os.environ.get("CAS_CLOUD_ENDPOINT", ""); token = os.environ.get("CAS_CLOUD_TOKEN", "")
    if not endpoint or not token: raise SystemExit("CAS cloud credentials required for evaluation")
    labels = json.loads(args.queries.read_text())
    query_vectors = post_embeddings(endpoint, token, [item["query"] for item in labels])
    db = sqlite3.connect(args.index)
    output = []
    for item, vector in zip(labels, query_vectors):
        started = time.monotonic()
        semantic = vector_ranking(db, vector)
        lexical = lexical_ranking(db, item["query"])
        fused = collections.defaultdict(float)
        for rank, (_, chunk_id) in enumerate(semantic[:200], 1): fused[chunk_id] += 1/(60+rank)
        for rank, (_, chunk_id) in enumerate(lexical[:200], 1): fused[chunk_id] += 1/(60+rank)
        hybrid = sorted(((score, chunk_id) for chunk_id, score in fused.items()), reverse=True)
        lexical_ids = {chunk_id for _, chunk_id in lexical[:50]}
        semantic_unique = sum(chunk_id not in lexical_ids for _, chunk_id in semantic[:args.top])
        output.append({"label": item["label"], "query": item["query"],
                       "hypothesis": item["hypothesis"], "latency_ms_local_rankers": round((time.monotonic()-started)*1000, 2),
                       "semantic_top_k_absent_from_lexical_top_50": semantic_unique,
                       "lexical": hydrate_results(db, lexical, args.top),
                       "vector": hydrate_results(db, semantic, args.top),
                       "hybrid": hydrate_results(db, hybrid, args.top)})
    print(json.dumps({"evaluated_at": datetime.now(timezone.utc).isoformat(), "top_k": args.top, "queries": output}, indent=2))
    db.close()


def command_inventory(args: argparse.Namespace) -> None:
    cutoff = parse_time(args.cutoff); assert cutoff
    print(json.dumps(inventory(args.snapshot, cutoff), indent=2, sort_keys=True))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    inv = sub.add_parser("inventory"); inv.add_argument("--snapshot", type=Path, default=DEFAULT_SNAPSHOT); inv.add_argument("--cutoff", default=DEFAULT_CUTOFF); inv.set_defaults(func=command_inventory)
    prep = sub.add_parser("prepare"); prep.add_argument("--snapshot", type=Path, default=DEFAULT_SNAPSHOT); prep.add_argument("--index", type=Path, default=DEFAULT_INDEX); prep.add_argument("--cutoff", default=DEFAULT_CUTOFF); prep.add_argument("--sample-files", type=int, default=0); prep.add_argument("--sample-rows", type=int, default=0); prep.set_defaults(func=prepare)
    emb = sub.add_parser("embed"); emb.add_argument("--index", type=Path, default=DEFAULT_INDEX); emb.add_argument("--limit", type=int, default=0, help="0 means all pending"); emb.add_argument("--pause", type=float, default=0.55); emb.set_defaults(func=embed)
    qry = sub.add_parser("query"); qry.add_argument("query"); qry.add_argument("--index", type=Path, default=DEFAULT_INDEX); qry.add_argument("--top", type=int, default=8); qry.add_argument("--mode", choices=("vector", "lexical", "hybrid"), default="hybrid"); qry.set_defaults(func=query)
    eva = sub.add_parser("evaluate"); eva.add_argument("--queries", type=Path, required=True); eva.add_argument("--index", type=Path, default=DEFAULT_INDEX); eva.add_argument("--top", type=int, default=5); eva.set_defaults(func=evaluate)
    args = parser.parse_args(); args.func(args)


if __name__ == "__main__":
    main()
