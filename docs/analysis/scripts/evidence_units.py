#!/usr/bin/env python3
"""Continuous, read-only normalization of CAS operational evidence into typed units.

This is the M1 (cas-b78b) ingestion layer for the operational-intelligence v2
epic.  It does not build a new corpus from scratch: it normalizes the sources
that the existing corpora were built from (coordination DB, daemon logs,
Claude/Codex/Grok transcripts) into typed *evidence units* that carry full
provenance and join across session, task, worker, commit, file, symbol and
deployed-binary epoch.

Non-negotiable properties, each covered by a test in test_evidence_units.py:

* **Read-only sources.**  Every source handle is opened read-only and every
  write target is checked against the namespace root by :func:`assert_writable`.
  Nothing observed is ever written into CAS memories (``entries``) or the
  knowledge corpus (``knowledge_pages``); those table names are refused by the
  write guard as an extra belt-and-braces check.
* **Incremental and resumable.**  Append-only files carry byte/line watermarks,
  DB tables carry rowid watermarks.  Rotation and truncation are detected via
  inode + size and restart that source cleanly.
* **Correction-aware.**  A claim that a later authority withdrew is marked, is
  down-ranked at query time, and can never be returned without its correction
  attached.  This is the fix for the retrieval safety failure measured by
  cas-c505 (docs/analysis/2026-08-17-historical-operational-vector-index.md).
* **Scoped and retained.**  Every unit carries a host/project/team privacy
  scope, and retention deletions emit a hashed receipt.
* **SQL for metrics, vectors for text.**  Structured metrics (``reproduce``) are
  derived by SQL against a snapshot.  Ingestion never embeds anything; units are
  left ``embed_state='pending'`` for the M2 index lane.

The dedupe-before-embed pipeline (strip boilerplate, redact, normalize, hash)
is inherited from docs/analysis/scripts/historical_vector_index.py, which
measured a 92.17% structural reduction.  The window/candidate/adjudication
shape is inherited from docs/analysis/scripts/mine_failure_modes.py: lexical
matching only ever produces candidates, and an authority adjudicates them.

Python's standard library is the only runtime dependency.
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import json
import os
import re
import shutil
import sqlite3
import sys
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Iterable, Iterator, Sequence

NAMESPACE = "evidence_v1"
SCHEMA_VERSION = 1

DEFAULT_PROJECT = "cas-src"
DEFAULT_SOURCE_DB = Path("/home/pippenz/Petrastella/cas-src/.cas/cas.db")
DEFAULT_LOG_ROOT = Path("/home/pippenz/Petrastella/cas-src/.cas/logs")
DEFAULT_NAMESPACE_ROOT = Path("/home/pippenz/.cas/artifacts/cas-b78b/evidence")
DEFAULT_CLAIMS = Path(__file__).parents[1] / "evidence_claims.json"

# Stores that ingestion must never write into, at any path, ever.
FORBIDDEN_WRITE_TABLES = ("entries", "knowledge_pages", "knowledge_sources", "rules", "skills")
# The frozen cas-c505 artifact is a read-only input; the continuous lane owns
# its own namespace and must never append to that cutoff index.
FROZEN_ARTIFACTS = ("/artifacts/cas-c505/",)

PRIVACY_SCOPES = ("host", "project", "team")
# Most restrictive first: a unit observed in more than one scope keeps the
# narrowest audience of any of its observations.
SCOPE_RANK = {"host": 0, "project": 1, "team": 2}
DEFAULT_RETENTION_DAYS = {"host": 30, "project": 365, "team": 365}

ISO_PREFIX = re.compile(r"^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:?\d\d)?)")
TASK_ID = re.compile(r"\bcas-[0-9a-f]{4,}\b", re.I)
WORKER = re.compile(r"/\.cas/worktrees/([^/\s]+)")
UUID_RE = re.compile(r"\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b", re.I)
HEX_RE = re.compile(r"\b[0-9a-f]{7,40}\b")
PATH_RE = re.compile(r"\b(?:[\w.-]+/)+[\w.-]+\.(?:rs|py|ts|tsx|js|toml|md|sh|sql|json|yaml|yml)\b")
SYMBOL_RE = re.compile(r"\b(?:[A-Za-z_][A-Za-z0-9_]*::)+[A-Za-z_][A-Za-z0-9_]*\b")
IDENT_RE = re.compile(r"\b[a-z_][a-z0-9_]{4,}\b")

SECRET_PATTERNS = (
    re.compile(r"(?i)(authorization\s*:\s*bearer\s+)[A-Za-z0-9._~+/=-]+"),
    re.compile(r"(?i)\b(sk|pk|ghp|github_pat|xox[baprs])[-_A-Za-z0-9]{12,}\b"),
    re.compile(r"(?i)((?:api[_-]?key|token|password|secret)\s*[=:]\s*)[^\s,;\"']{6,}"),
    re.compile(r"-----BEGIN [^-]+ PRIVATE KEY-----.*?-----END [^-]+ PRIVATE KEY-----", re.S),
)
EMAIL_RE = re.compile(r"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b")
BOILERPLATE_BLOCKS = (
    "skills_instructions", "permissions instructions", "environment_context",
    "user_info", "git_status", "apps_instructions", "plugins_instructions",
)

# A correction marker turns a claim-matching unit from an assertion of the claim
# into the record that retires it.  Kept deliberately narrow: adjudication
# authority, not lexical enthusiasm.
CORRECTION_MARKERS = (
    re.compile(r"(?i)\b(?:is|are|was|were) withdrawn\b"),
    re.compile(r"(?i)\bwithdraw(?:n|s|ing)\b.{0,60}\bclaims?\b", re.S),
    re.compile(r"(?i)\bclaims?\b.{0,60}\bwithdraw(?:n|s|ing)\b", re.S),
    re.compile(r"(?i)\b(?:explicitly\s+)?not reproduced\b"),
    re.compile(r"(?i)\bdo(?:es)? not prove\b"),
    re.compile(r"(?i)\bcorrect(?:ion|ed|s)\b.{0,60}\b(?:claims?|findings?|reports?)\b", re.S),
    re.compile(r"(?i)\b(?:claims?|findings?)\b.{0,60}\bcorrect(?:ion|ed)\b", re.S),
)


class ReadOnlyViolation(RuntimeError):
    """Raised when ingestion attempts to write outside its own namespace."""


# --------------------------------------------------------------------------
# read-only guard
# --------------------------------------------------------------------------

_WRITABLE_ROOTS: list[Path] = []


def set_writable_root(root: Path) -> None:
    """Declare the single directory tree ingestion is allowed to write into."""
    resolved = Path(root).expanduser().resolve()
    _WRITABLE_ROOTS.clear()
    _WRITABLE_ROOTS.append(resolved)


def assert_writable(path: Path) -> Path:
    """Refuse any write target outside the declared namespace root."""
    resolved = Path(path).expanduser().resolve()
    text = str(resolved)
    for frozen in FROZEN_ARTIFACTS:
        if frozen in text:
            raise ReadOnlyViolation(f"refusing to write into frozen artifact: {resolved}")
    if not _WRITABLE_ROOTS:
        raise ReadOnlyViolation("no writable namespace root declared")
    for root in _WRITABLE_ROOTS:
        if resolved == root or root in resolved.parents:
            return resolved
    raise ReadOnlyViolation(f"write target {resolved} is outside namespace root {_WRITABLE_ROOTS[0]}")


def connect_readonly(path: Path) -> sqlite3.Connection:
    """Open a source database read-only, and pin it read-only at the handle."""
    connection = sqlite3.connect(f"file:{Path(path).expanduser()}?mode=ro", uri=True)
    connection.execute("PRAGMA query_only = ON")
    return connection


def connect_namespace(path: Path) -> sqlite3.Connection:
    """Open the evidence namespace for writing, after the guard approves it."""
    target = assert_writable(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    connection = sqlite3.connect(target)
    connection.execute("PRAGMA foreign_keys = ON")
    return connection


# --------------------------------------------------------------------------
# normalization (inherited from historical_vector_index.py)
# --------------------------------------------------------------------------


def parse_time(value: object) -> datetime | None:
    if isinstance(value, (int, float)):
        value = value / 1000 if value > 10_000_000_000 else value
        return datetime.fromtimestamp(value, timezone.utc)
    if not isinstance(value, str) or not value:
        return None
    text = value.strip().replace("Z", "+00:00")
    try:
        return datetime.fromisoformat(text).astimezone(timezone.utc)
    except ValueError:
        return None


def iso(value: datetime) -> str:
    return value.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")


def privacy_redact(text: str) -> tuple[str, int, int]:
    """Deterministic secret/email redaction; returns text plus counted receipts."""
    secrets = 0
    for pattern in SECRET_PATTERNS:
        text, count = pattern.subn(
            lambda m: (m.group(1) if m.lastindex else "") + "[REDACTED_SECRET]", text
        )
        secrets += count
    text, emails = EMAIL_RE.subn("[REDACTED_EMAIL]", text)
    return text, secrets, emails


def strip_boilerplate(text: str) -> str:
    for tag in BOILERPLATE_BLOCKS:
        text = re.sub(rf"<{re.escape(tag)}\b[^>]*>.*?</{re.escape(tag)}>", " ", text, flags=re.I | re.S)
    text = re.sub(r"<system-reminder>.*?</system-reminder>", " ", text, flags=re.I | re.S)
    text = re.sub(r"<!--.*?-->", " ", text, flags=re.S)
    text = re.sub(r"(?m)^CAS provenance: notification_id=.*$", " ", text)
    text = re.sub(r"(?m)^# AGENTS\.md instructions.*?(?=\n\S|\Z)", " ", text, flags=re.S)
    return re.sub(r"\n{3,}", "\n\n", text).strip()


def normalized(text: str) -> str:
    """Structural normal form used for dedupe-before-embed."""
    value = text.lower()
    value = UUID_RE.sub("<uuid>", value)
    value = re.sub(r"\b[0-9a-f]{8,40}\b", "<hex>", value)
    value = re.sub(r"\b\d+(?:\.\d+)?(?:ms|s|mb|gb|bytes?)?\b", "<n>", value)
    value = re.sub(r"/home/[^\s\"']+", "<path>", value)
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


def json_strings(value: object) -> Iterator[str]:
    if isinstance(value, str):
        yield value
    elif isinstance(value, list):
        for item in value:
            if isinstance(item, dict) and item.get("type") in {"text", "input_text", "output_text"}:
                yield str(item.get("text", ""))


def worker_from(text: str) -> str:
    match = WORKER.search(text or "")
    return match.group(1) if match else ""


# --------------------------------------------------------------------------
# deployed-binary epochs
# --------------------------------------------------------------------------


class Epochs:
    """Deployed-binary epoch attribution from the authoritative history_epochs.

    The mixed-epoch discipline from cas-9d92 is preserved: when more than one
    binary version was live at an instant, the evidence is labelled ``mixed:``
    and must not be counted as post-fix by anything downstream.
    """

    def __init__(self, rows: Sequence[tuple[str, datetime | None, datetime | None]]):
        self.rows = list(rows)

    @classmethod
    def load(cls, connection: sqlite3.Connection) -> "Epochs":
        try:
            rows = connection.execute(
                "SELECT COALESCE(version,''), started_at, ended_at FROM history_epochs ORDER BY started_at"
            ).fetchall()
        except sqlite3.Error:
            return cls([])
        return cls([(str(v), parse_time(s), parse_time(e)) for v, s, e in rows])

    def at(self, value: str) -> tuple[str, str]:
        """Return (epoch_label, single_version_or_empty) for a timestamp."""
        stamp = parse_time(value)
        if not stamp:
            return "unattributed", ""
        active = sorted(
            {
                version
                for version, start, end in self.rows
                if version and start and start <= stamp and (end is None or stamp <= end)
            }
        )
        if len(active) > 1:
            return "mixed:" + "+".join(active), ""
        if active:
            return active[0], active[0]
        return "unattributed", ""


# --------------------------------------------------------------------------
# reference index for commit/file/symbol joins
# --------------------------------------------------------------------------


@dataclass
class ReferenceIndex:
    """Known commits/files/symbols, used to promote text guesses into joins."""

    shas: dict[str, str] = field(default_factory=dict)
    files: set[str] = field(default_factory=set)
    file_basenames: dict[str, str] = field(default_factory=dict)
    symbols: set[str] = field(default_factory=set)
    symbol_names: dict[str, str] = field(default_factory=dict)
    commit_files: dict[str, list[str]] = field(default_factory=dict)
    commit_symbols: dict[str, list[str]] = field(default_factory=dict)

    @classmethod
    def load(cls, connection: sqlite3.Connection) -> "ReferenceIndex":
        index = cls()

        def try_query(sql: str) -> list[tuple]:
            try:
                return connection.execute(sql).fetchall()
            except sqlite3.Error:
                return []

        for sha, short in try_query("SELECT sha, short_sha FROM history_commits"):
            index.shas[str(sha).lower()] = str(sha)
            if short:
                index.shas[str(short).lower()] = str(sha)
        for (path,) in try_query("SELECT path FROM code_files"):
            index.files.add(str(path))
            index.file_basenames.setdefault(Path(str(path)).name, str(path))
        for (path,) in try_query("SELECT DISTINCT file_path FROM history_commit_files"):
            index.files.add(str(path))
            index.file_basenames.setdefault(Path(str(path)).name, str(path))
        for qualified, name in try_query("SELECT qualified_name, name FROM code_symbols"):
            index.symbols.add(str(qualified))
            index.symbol_names.setdefault(str(name), str(qualified))
        for sha, path in try_query("SELECT sha, file_path FROM history_commit_files"):
            index.commit_files.setdefault(str(sha).lower(), []).append(str(path))
        for sha, qualified in try_query("SELECT sha, qualified_name FROM history_commit_symbols"):
            index.commit_symbols.setdefault(str(sha).lower(), []).append(str(qualified))
        return index

    def resolve_commits(self, text: str) -> list[str]:
        found: list[str] = []
        for candidate in HEX_RE.findall(text.lower()):
            full = self.shas.get(candidate)
            if full and full not in found:
                found.append(full)
        return found

    def resolve_files(self, text: str) -> list[tuple[str, float, str]]:
        found: dict[str, tuple[float, str]] = {}
        for candidate in PATH_RE.findall(text):
            if candidate in self.files:
                found[candidate] = (1.0, "reference-join")
            else:
                known = self.file_basenames.get(Path(candidate).name)
                if known:
                    found.setdefault(known, (0.6, "basename-join"))
                else:
                    found.setdefault(candidate, (0.4, "text-extract"))
        return [(path, score, method) for path, (score, method) in found.items()]

    def resolve_symbols(self, text: str) -> list[tuple[str, float, str]]:
        found: dict[str, tuple[float, str]] = {}
        for candidate in SYMBOL_RE.findall(text):
            if candidate in self.symbols:
                found[candidate] = (1.0, "reference-join")
        for candidate in set(IDENT_RE.findall(text)):
            qualified = self.symbol_names.get(candidate)
            if qualified:
                found.setdefault(qualified, (0.6, "name-join"))
        return [(symbol, score, method) for symbol, (score, method) in found.items()]


# --------------------------------------------------------------------------
# evidence records
# --------------------------------------------------------------------------


@dataclass
class EvidenceRecord:
    """One observation of raw evidence, before normalization into a unit."""

    unit_type: str
    source_key: str
    source_path: str
    source_locator: str
    text: str
    timestamp: str = ""
    session_id: str = ""
    task_id: str = ""
    worker: str = ""
    privacy_scope: str = "project"
    project: str = DEFAULT_PROJECT
    team: str = ""


# --------------------------------------------------------------------------
# sources
# --------------------------------------------------------------------------

DB_SOURCES: dict[str, dict[str, str]] = {
    "task": {
        "table": "tasks",
        "cursor": "rowid",
        "sql": (
            "SELECT t.rowid, t.id, t.updated_at, COALESCE(t.assignee,''), COALESCE(t.team_id,''), "
            "COALESCE(t.title,'')||'\n'||COALESCE(t.description,'')||'\n'||COALESCE(t.design,'')||'\n'"
            "||COALESCE(t.acceptance_criteria,'')||'\n'||COALESCE(t.notes,'')||'\n'||COALESCE(t.close_reason,'') "
            "FROM tasks t WHERE t.rowid > ? ORDER BY t.rowid LIMIT ?"
        ),
    },
    "lifecycle_event": {
        "table": "events",
        "cursor": "id",
        "sql": (
            "SELECT e.id, COALESCE(e.entity_id,''), e.created_at, COALESCE(e.session_id,''), '', "
            "COALESCE(e.event_type,'')||': '||COALESCE(e.summary,'') "
            "FROM events e WHERE e.id > ? ORDER BY e.id LIMIT ?"
        ),
    },
    "prompt_queue": {
        "table": "prompt_queue",
        "cursor": "id",
        "sql": (
            "SELECT p.id, COALESCE(p.dedupe_key,''), p.created_at, COALESCE(p.target,''), '', "
            "COALESCE(p.summary,'')||'\n'||COALESCE(p.prompt,'') "
            "FROM prompt_queue p WHERE p.id > ? ORDER BY p.id LIMIT ?"
        ),
    },
    "supervisor_queue": {
        "table": "supervisor_queue",
        "cursor": "id",
        "sql": (
            "SELECT s.id, COALESCE(s.event_type,''), s.created_at, COALESCE(s.supervisor_id,''), '', "
            "COALESCE(s.event_type,'')||'\n'||COALESCE(s.payload,'') "
            "FROM supervisor_queue s WHERE s.id > ? ORDER BY s.id LIMIT ?"
        ),
    },
}


def db_records(
    connection: sqlite3.Connection,
    unit_type: str,
    source_path: str,
    cursor: int,
    batch: int,
    until: datetime | None,
) -> tuple[list[EvidenceRecord], int]:
    """Read one resumable batch of coordination-DB rows after ``cursor``."""
    spec = DB_SOURCES[unit_type]
    try:
        rows = connection.execute(spec["sql"], (cursor, batch)).fetchall()
    except sqlite3.Error:
        return [], cursor
    records: list[EvidenceRecord] = []
    highest = cursor
    for rowid, ident, timestamp, actor, team, text in rows:
        highest = max(highest, int(rowid))
        stamp = parse_time(timestamp)
        if until and stamp and stamp > until:
            continue
        task = str(ident) if str(ident).lower().startswith("cas-") else ""
        if not task:
            match = TASK_ID.search(str(text))
            task = match.group(0).lower() if match else ""
        session = str(actor) if unit_type == "lifecycle_event" else ""
        worker = "" if unit_type == "lifecycle_event" else str(actor)
        records.append(
            EvidenceRecord(
                unit_type=unit_type,
                source_key=f"db:{unit_type}",
                source_path=source_path,
                source_locator=f"{spec['table']}#{rowid}",
                text=str(text or ""),
                timestamp=str(timestamp or ""),
                session_id=session,
                task_id=task,
                worker=worker_from(worker) or worker,
                privacy_scope="team" if team else "project",
                team=str(team or ""),
            )
        )
    return records, highest


def log_records(path: Path, byte_offset: int, line_offset: int, budget: int) -> tuple[list[EvidenceRecord], int, int]:
    """Read appended daemon-log bytes only.  Host-scoped by construction."""
    records: list[EvidenceRecord] = []
    line_no = line_offset
    with path.open("rb") as handle:
        handle.seek(byte_offset)
        consumed = 0
        while consumed < budget:
            raw = handle.readline()
            if not raw:
                break
            consumed += len(raw)
            line_no += 1
            line = raw.decode("utf-8", errors="replace")
            match = ISO_PREFIX.match(line)
            timestamp = match.group(1) if match else ""
            body = re.sub(r"^\S+\s+(?:TRACE|DEBUG|INFO|WARN|ERROR)\s+", "", line).strip()
            if not body:
                continue
            task_match = TASK_ID.search(body)
            records.append(
                EvidenceRecord(
                    unit_type="daemon_log",
                    source_key=f"log:{path.name}",
                    source_path=str(path),
                    source_locator=str(line_no),
                    text=body,
                    timestamp=timestamp,
                    task_id=task_match.group(0).lower() if task_match else "",
                    worker=worker_from(body),
                    privacy_scope="host",
                )
            )
        return records, handle.tell(), line_no


def claude_records(path: Path, byte_offset: int, line_offset: int, budget: int) -> tuple[list[EvidenceRecord], int, int]:
    records: list[EvidenceRecord] = []
    session = path.stem
    cwd = ""
    line_no = line_offset
    with path.open("rb") as handle:
        handle.seek(byte_offset)
        consumed = 0
        while consumed < budget:
            raw = handle.readline()
            if not raw:
                break
            consumed += len(raw)
            line_no += 1
            try:
                obj = json.loads(raw.decode("utf-8", errors="replace"))
            except json.JSONDecodeError:
                continue
            session = str(obj.get("sessionId") or session)
            cwd = str(obj.get("cwd") or cwd)
            message = obj.get("message") if isinstance(obj.get("message"), dict) else {}
            if message.get("role") not in {"user", "assistant"}:
                continue
            worker = worker_from(cwd)
            for text in json_strings(message.get("content")):
                task_match = TASK_ID.search(text)
                records.append(
                    EvidenceRecord(
                        unit_type="claude_transcript",
                        source_key=f"claude:{path}",
                        source_path=str(path),
                        source_locator=str(line_no),
                        text=text,
                        timestamp=str(obj.get("timestamp", "")),
                        session_id=session,
                        task_id=task_match.group(0).lower() if task_match else "",
                        worker=worker,
                        privacy_scope="project" if worker or "cas-src" in cwd else "host",
                    )
                )
        return records, handle.tell(), line_no


def codex_records(path: Path, byte_offset: int, line_offset: int, budget: int) -> tuple[list[EvidenceRecord], int, int]:
    records: list[EvidenceRecord] = []
    session = path.stem
    cwd = ""
    line_no = line_offset
    with path.open("rb") as handle:
        handle.seek(byte_offset)
        consumed = 0
        while consumed < budget:
            raw = handle.readline()
            if not raw:
                break
            consumed += len(raw)
            line_no += 1
            try:
                obj = json.loads(raw.decode("utf-8", errors="replace"))
            except json.JSONDecodeError:
                continue
            payload = obj.get("payload") if isinstance(obj.get("payload"), dict) else {}
            if obj.get("type") == "session_meta":
                session = str(payload.get("session_id") or payload.get("id") or session)
                cwd = str(payload.get("cwd") or cwd)
            if obj.get("type") != "response_item" or payload.get("type") != "message":
                continue
            if payload.get("role") not in {"user", "assistant"}:
                continue
            worker = worker_from(cwd)
            for text in json_strings(payload.get("content")):
                task_match = TASK_ID.search(text)
                records.append(
                    EvidenceRecord(
                        unit_type="codex_transcript",
                        source_key=f"codex:{path}",
                        source_path=str(path),
                        source_locator=str(line_no),
                        text=text,
                        timestamp=str(obj.get("timestamp", "")),
                        session_id=session,
                        task_id=task_match.group(0).lower() if task_match else "",
                        worker=worker,
                        privacy_scope="project" if worker or "cas-src" in cwd else "host",
                    )
                )
        return records, handle.tell(), line_no


def grok_records(path: Path, byte_offset: int, line_offset: int, budget: int) -> tuple[list[EvidenceRecord], int, int]:
    records: list[EvidenceRecord] = []
    session = path.parent.name
    cwd = path.parents[1].name.replace("%2F", "/") if len(path.parents) > 1 else ""
    line_no = line_offset
    with path.open("rb") as handle:
        handle.seek(byte_offset)
        consumed = 0
        while consumed < budget:
            raw = handle.readline()
            if not raw:
                break
            consumed += len(raw)
            line_no += 1
            try:
                obj = json.loads(raw.decode("utf-8", errors="replace"))
            except json.JSONDecodeError:
                continue
            if obj.get("type") not in {"user", "assistant"}:
                continue
            worker = worker_from(cwd)
            for text in json_strings(obj.get("content")):
                task_match = TASK_ID.search(text)
                records.append(
                    EvidenceRecord(
                        unit_type="grok_transcript",
                        source_key=f"grok:{path}",
                        source_path=str(path),
                        # chat_history.jsonl carries no reliable per-row timestamp;
                        # the file mtime bounds the window and is recorded on the run.
                        source_locator=str(line_no),
                        text=text,
                        timestamp="",
                        session_id=session,
                        task_id=task_match.group(0).lower() if task_match else "",
                        worker=worker,
                        privacy_scope="project" if worker or "cas-src" in cwd else "host",
                    )
                )
        return records, handle.tell(), line_no


FILE_READERS = {
    "daemon_log": log_records,
    "claude_transcript": claude_records,
    "codex_transcript": codex_records,
    "grok_transcript": grok_records,
}


# --------------------------------------------------------------------------
# namespace store
# --------------------------------------------------------------------------

SCHEMA = """
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS evidence_units(
  id INTEGER PRIMARY KEY,
  content_hash TEXT NOT NULL UNIQUE,
  unit_type TEXT NOT NULL,
  text TEXT NOT NULL,
  occurrence_count INTEGER NOT NULL DEFAULT 0,
  redaction_secrets INTEGER NOT NULL DEFAULT 0,
  redaction_emails INTEGER NOT NULL DEFAULT 0,
  first_seen_at TEXT NOT NULL DEFAULT '',
  last_seen_at TEXT NOT NULL DEFAULT '',
  privacy_scope TEXT NOT NULL,
  claim_key TEXT,
  correction_state TEXT NOT NULL DEFAULT 'current',
  embed_state TEXT NOT NULL DEFAULT 'pending',
  ingested_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS evidence_provenance(
  id INTEGER PRIMARY KEY,
  unit_id INTEGER NOT NULL REFERENCES evidence_units(id) ON DELETE CASCADE,
  source_key TEXT NOT NULL,
  source_path TEXT NOT NULL,
  source_locator TEXT NOT NULL,
  session_id TEXT NOT NULL DEFAULT '',
  task_id TEXT NOT NULL DEFAULT '',
  worker TEXT NOT NULL DEFAULT '',
  commit_sha TEXT NOT NULL DEFAULT '',
  file_path TEXT NOT NULL DEFAULT '',
  symbol TEXT NOT NULL DEFAULT '',
  timestamp TEXT NOT NULL DEFAULT '',
  epoch TEXT NOT NULL DEFAULT 'unattributed',
  epoch_version TEXT NOT NULL DEFAULT '',
  privacy_scope TEXT NOT NULL,
  host TEXT NOT NULL DEFAULT '',
  project TEXT NOT NULL DEFAULT '',
  team TEXT NOT NULL DEFAULT '',
  UNIQUE(source_key, source_locator, unit_id)
);

CREATE TABLE IF NOT EXISTS evidence_links(
  provenance_id INTEGER NOT NULL REFERENCES evidence_provenance(id) ON DELETE CASCADE,
  unit_id INTEGER NOT NULL REFERENCES evidence_units(id) ON DELETE CASCADE,
  link_type TEXT NOT NULL,
  link_value TEXT NOT NULL,
  confidence REAL NOT NULL DEFAULT 1.0,
  method TEXT NOT NULL,
  PRIMARY KEY(provenance_id, link_type, link_value)
);

CREATE TABLE IF NOT EXISTS evidence_corrections(
  id INTEGER PRIMARY KEY,
  claim_key TEXT NOT NULL,
  relation TEXT NOT NULL,
  correcting_unit_id INTEGER REFERENCES evidence_units(id) ON DELETE SET NULL,
  authority TEXT NOT NULL,
  evidence_pointer TEXT NOT NULL,
  recorded_at TEXT NOT NULL,
  note TEXT NOT NULL DEFAULT '',
  UNIQUE(claim_key, relation, evidence_pointer)
);

CREATE TABLE IF NOT EXISTS ingest_watermarks(
  source_key TEXT PRIMARY KEY,
  source_path TEXT NOT NULL,
  cursor_kind TEXT NOT NULL,
  byte_offset INTEGER NOT NULL DEFAULT 0,
  line_offset INTEGER NOT NULL DEFAULT 0,
  row_cursor INTEGER NOT NULL DEFAULT 0,
  size_bytes INTEGER NOT NULL DEFAULT 0,
  inode TEXT NOT NULL DEFAULT '',
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS ingest_runs(
  id INTEGER PRIMARY KEY,
  started_at TEXT NOT NULL,
  finished_at TEXT NOT NULL DEFAULT '',
  window_until TEXT NOT NULL DEFAULT '',
  sources_scanned INTEGER NOT NULL DEFAULT 0,
  bytes_read INTEGER NOT NULL DEFAULT 0,
  candidates INTEGER NOT NULL DEFAULT 0,
  unique_units INTEGER NOT NULL DEFAULT 0,
  duplicates_collapsed INTEGER NOT NULL DEFAULT 0,
  scopes_ingested TEXT NOT NULL DEFAULT '',
  snapshot_sha256 TEXT NOT NULL DEFAULT '',
  receipt_hash TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS redaction_receipts(
  id INTEGER PRIMARY KEY,
  run_id INTEGER NOT NULL,
  unit_type TEXT NOT NULL,
  secrets_redacted INTEGER NOT NULL DEFAULT 0,
  emails_redacted INTEGER NOT NULL DEFAULT 0,
  recorded_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS retention_receipts(
  id INTEGER PRIMARY KEY,
  run_at TEXT NOT NULL,
  policy TEXT NOT NULL,
  privacy_scope TEXT NOT NULL,
  cutoff TEXT NOT NULL,
  provenance_deleted INTEGER NOT NULL DEFAULT 0,
  units_deleted INTEGER NOT NULL DEFAULT 0,
  links_deleted INTEGER NOT NULL DEFAULT 0,
  oldest_retained TEXT NOT NULL DEFAULT '',
  receipt_hash TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_prov_unit ON evidence_provenance(unit_id);
CREATE INDEX IF NOT EXISTS idx_prov_task ON evidence_provenance(task_id);
CREATE INDEX IF NOT EXISTS idx_prov_session ON evidence_provenance(session_id);
CREATE INDEX IF NOT EXISTS idx_prov_epoch ON evidence_provenance(epoch);
CREATE INDEX IF NOT EXISTS idx_prov_time ON evidence_provenance(timestamp);
CREATE INDEX IF NOT EXISTS idx_links_value ON evidence_links(link_type, link_value);
CREATE INDEX IF NOT EXISTS idx_units_claim ON evidence_units(claim_key);

CREATE VIRTUAL TABLE IF NOT EXISTS evidence_fts USING fts5(text, content='evidence_units', content_rowid='id');
"""


def open_namespace(path: Path) -> sqlite3.Connection:
    connection = connect_namespace(path)
    connection.executescript(SCHEMA)
    connection.execute(
        "INSERT OR REPLACE INTO meta(key,value) VALUES('namespace',?)", (NAMESPACE,)
    )
    connection.execute(
        "INSERT OR REPLACE INTO meta(key,value) VALUES('schema_version',?)", (str(SCHEMA_VERSION),)
    )
    connection.execute(
        "INSERT OR REPLACE INTO meta(key,value) VALUES('isolation',?)",
        ("standalone namespace; never registered in CAS memory/knowledge/code-vector search",),
    )
    connection.commit()
    return connection


# --------------------------------------------------------------------------
# claim registry and correction adjudication
# --------------------------------------------------------------------------


@dataclass
class Claim:
    claim_key: str
    title: str
    patterns: list[re.Pattern[str]]
    corrections: list[dict]

    def matches(self, text: str) -> bool:
        return any(pattern.search(text) for pattern in self.patterns)


def load_claims(path: Path) -> list[Claim]:
    if not Path(path).exists():
        return []
    payload = json.loads(Path(path).read_text())
    claims: list[Claim] = []
    for entry in payload.get("claims", []):
        claims.append(
            Claim(
                claim_key=entry["claim_key"],
                title=entry.get("title", ""),
                patterns=[re.compile(p, re.I | re.S) for p in entry.get("patterns", [])],
                corrections=list(entry.get("corrections", [])),
            )
        )
    return claims


def seed_corrections(connection: sqlite3.Connection, claims: Sequence[Claim]) -> int:
    """Record the registry's authoritative corrections (current-source pointers)."""
    written = 0
    for claim in claims:
        for correction in claim.corrections:
            cursor = connection.execute(
                "INSERT OR IGNORE INTO evidence_corrections"
                "(claim_key,relation,correcting_unit_id,authority,evidence_pointer,recorded_at,note) "
                "VALUES(?,?,NULL,?,?,?,?)",
                (
                    claim.claim_key,
                    correction.get("relation", "withdraws"),
                    correction.get("authority", "registry"),
                    correction["evidence_pointer"],
                    correction.get("recorded_at", ""),
                    correction.get("note", ""),
                ),
            )
            written += cursor.rowcount if cursor.rowcount > 0 else 0
    return written


def classify_claim(text: str, claims: Sequence[Claim]) -> tuple[str | None, bool]:
    """Return (claim_key, is_correction) for a unit's text.

    Following mine_failure_modes.py: the lexical match only nominates a
    candidate.  A unit that matches a claim *and* carries a correction marker is
    the record that retires the claim, not another assertion of it.
    """
    for claim in claims:
        if claim.matches(text):
            return claim.claim_key, any(marker.search(text) for marker in CORRECTION_MARKERS)
    return None, False


def apply_correction_states(connection: sqlite3.Connection) -> dict[str, int]:
    """Mark every unit that asserts a withdrawn claim.

    Correction units themselves keep ``correction_state='correction'`` so they
    are never suppressed; they are exactly what retrieval must surface.
    """
    withdrawn = {
        row[0]
        for row in connection.execute(
            "SELECT DISTINCT claim_key FROM evidence_corrections WHERE relation IN ('withdraws','contradicts','supersedes')"
        )
    }
    counts = collections.Counter()
    for (claim_key,) in connection.execute(
        "SELECT DISTINCT claim_key FROM evidence_units WHERE claim_key IS NOT NULL"
    ).fetchall():
        state = "withdrawn" if claim_key in withdrawn else "current"
        cursor = connection.execute(
            "UPDATE evidence_units SET correction_state=? "
            "WHERE claim_key=? AND correction_state<>'correction'",
            (state, claim_key),
        )
        counts[state] += cursor.rowcount
    counts["correction"] = connection.execute(
        "SELECT COUNT(*) FROM evidence_units WHERE correction_state='correction'"
    ).fetchone()[0]
    return dict(counts)


# --------------------------------------------------------------------------
# ingestion
# --------------------------------------------------------------------------


@dataclass
class Watermark:
    source_key: str
    source_path: str
    cursor_kind: str
    byte_offset: int = 0
    line_offset: int = 0
    row_cursor: int = 0
    size_bytes: int = 0
    inode: str = ""


def read_watermark(connection: sqlite3.Connection, source_key: str, source_path: str, kind: str) -> Watermark:
    row = connection.execute(
        "SELECT byte_offset,line_offset,row_cursor,size_bytes,inode FROM ingest_watermarks WHERE source_key=?",
        (source_key,),
    ).fetchone()
    if not row:
        return Watermark(source_key, source_path, kind)
    return Watermark(source_key, source_path, kind, row[0], row[1], row[2], row[3], row[4])


def write_watermark(connection: sqlite3.Connection, mark: Watermark) -> None:
    connection.execute(
        "INSERT INTO ingest_watermarks(source_key,source_path,cursor_kind,byte_offset,line_offset,row_cursor,size_bytes,inode,updated_at) "
        "VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT(source_key) DO UPDATE SET "
        "source_path=excluded.source_path,cursor_kind=excluded.cursor_kind,byte_offset=excluded.byte_offset,"
        "line_offset=excluded.line_offset,row_cursor=excluded.row_cursor,size_bytes=excluded.size_bytes,"
        "inode=excluded.inode,updated_at=excluded.updated_at",
        (
            mark.source_key,
            mark.source_path,
            mark.cursor_kind,
            mark.byte_offset,
            mark.line_offset,
            mark.row_cursor,
            mark.size_bytes,
            mark.inode,
            iso(datetime.now(timezone.utc)),
        ),
    )


def rotated(mark: Watermark, stat: os.stat_result) -> bool:
    """A shrunk file or a new inode means the previous cursor is meaningless."""
    if mark.inode and mark.inode != str(stat.st_ino):
        return True
    return stat.st_size < mark.byte_offset


class Ingestor:
    def __init__(
        self,
        connection: sqlite3.Connection,
        epochs: Epochs,
        references: ReferenceIndex,
        claims: Sequence[Claim],
        host: str,
        project: str,
        scopes: Sequence[str],
    ):
        self.db = connection
        self.epochs = epochs
        self.references = references
        self.claims = claims
        self.host = host
        self.project = project
        self.scopes = set(scopes)
        self.stats: collections.Counter = collections.Counter()
        self.redactions: dict[str, list[int]] = collections.defaultdict(lambda: [0, 0])

    def add(self, record: EvidenceRecord) -> None:
        if record.privacy_scope not in self.scopes:
            self.stats["skipped_out_of_scope"] += 1
            return
        raw = strip_boilerplate(record.text)
        raw, secrets, emails = privacy_redact(raw)
        for piece in chunks(raw):
            self.stats["candidates"] += 1
            self._store(record, piece, secrets, emails)

    def _store(self, record: EvidenceRecord, piece: str, secrets: int, emails: int) -> None:
        content_hash = hashlib.sha256(normalized(piece).encode()).hexdigest()
        row = self.db.execute(
            "SELECT id,privacy_scope,first_seen_at,last_seen_at FROM evidence_units WHERE content_hash=?",
            (content_hash,),
        ).fetchone()
        claim_key, is_correction = classify_claim(piece, self.claims)
        stamp = record.timestamp or ""
        if row is None:
            cursor = self.db.execute(
                "INSERT INTO evidence_units(content_hash,unit_type,text,occurrence_count,redaction_secrets,"
                "redaction_emails,first_seen_at,last_seen_at,privacy_scope,claim_key,correction_state,"
                "embed_state,ingested_at) VALUES(?,?,?,0,?,?,?,?,?,?,?,'pending',?)",
                (
                    content_hash,
                    record.unit_type,
                    piece,
                    secrets,
                    emails,
                    stamp,
                    stamp,
                    record.privacy_scope,
                    claim_key,
                    "correction" if is_correction else "current",
                    iso(datetime.now(timezone.utc)),
                ),
            )
            unit_id = int(cursor.lastrowid)
            self.db.execute("INSERT INTO evidence_fts(rowid,text) VALUES(?,?)", (unit_id, piece))
            self.stats["unique_units"] += 1
            self.redactions[record.unit_type][0] += secrets
            self.redactions[record.unit_type][1] += emails
        else:
            unit_id = int(row[0])
            scope = row[1] if SCOPE_RANK[row[1]] <= SCOPE_RANK[record.privacy_scope] else record.privacy_scope
            first = min(x for x in (row[2], stamp) if x) if (row[2] or stamp) else ""
            last = max(row[3] or "", stamp)
            self.db.execute(
                "UPDATE evidence_units SET privacy_scope=?, first_seen_at=?, last_seen_at=? WHERE id=?",
                (scope, first, last, unit_id),
            )
            self.stats["duplicates_collapsed"] += 1

        if is_correction and claim_key:
            self.db.execute(
                "UPDATE evidence_units SET correction_state='correction' WHERE id=?", (unit_id,)
            )
            self.db.execute(
                "INSERT OR IGNORE INTO evidence_corrections"
                "(claim_key,relation,correcting_unit_id,authority,evidence_pointer,recorded_at,note) "
                "VALUES(?,?,?,?,?,?,?)",
                (
                    claim_key,
                    "withdraws",
                    unit_id,
                    "corpus",
                    f"{record.source_path}:{record.source_locator}",
                    stamp,
                    "correction discovered in ingested evidence",
                ),
            )

        epoch, epoch_version = self.epochs.at(stamp)
        commits = self.references.resolve_commits(piece)
        files = self.references.resolve_files(piece)
        symbols = self.references.resolve_symbols(piece)
        provenance = self.db.execute(
            "INSERT OR IGNORE INTO evidence_provenance(unit_id,source_key,source_path,source_locator,session_id,"
            "task_id,worker,commit_sha,file_path,symbol,timestamp,epoch,epoch_version,privacy_scope,host,project,team) "
            "VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (
                unit_id,
                record.source_key,
                record.source_path,
                record.source_locator,
                record.session_id,
                record.task_id,
                record.worker,
                commits[0] if commits else "",
                files[0][0] if files else "",
                symbols[0][0] if symbols else "",
                stamp,
                epoch,
                epoch_version,
                record.privacy_scope,
                self.host,
                record.project or self.project,
                record.team,
            ),
        )
        if provenance.rowcount == 0:
            return
        provenance_id = int(provenance.lastrowid)
        self.db.execute(
            "UPDATE evidence_units SET occurrence_count=occurrence_count+1 WHERE id=?", (unit_id,)
        )
        self._link(provenance_id, unit_id, commits, files, symbols, record, epoch)

    def _link(
        self,
        provenance_id: int,
        unit_id: int,
        commits: list[str],
        files: list[tuple[str, float, str]],
        symbols: list[tuple[str, float, str]],
        record: EvidenceRecord,
        epoch: str,
    ) -> None:
        rows: list[tuple[int, int, str, str, float, str]] = []
        if record.task_id:
            rows.append((provenance_id, unit_id, "task", record.task_id, 1.0, "row-key"))
        if record.session_id:
            rows.append((provenance_id, unit_id, "session", record.session_id, 1.0, "row-key"))
        if record.worker:
            rows.append((provenance_id, unit_id, "worker", record.worker, 1.0, "path-parse"))
        rows.append((provenance_id, unit_id, "epoch", epoch, 1.0, "history-epochs"))
        for sha in commits:
            rows.append((provenance_id, unit_id, "commit", sha, 1.0, "reference-join"))
            # A resolved commit carries its own files and symbols; that is the
            # join that makes "which symbol did this evidence touch" answerable
            # without re-parsing prose.
            for path in self.references.commit_files.get(sha.lower(), []):
                rows.append((provenance_id, unit_id, "file", path, 0.9, "commit-file-join"))
            for symbol in self.references.commit_symbols.get(sha.lower(), []):
                rows.append((provenance_id, unit_id, "symbol", symbol, 0.9, "commit-symbol-join"))
        for path, confidence, method in files:
            rows.append((provenance_id, unit_id, "file", path, confidence, method))
        for symbol, confidence, method in symbols:
            rows.append((provenance_id, unit_id, "symbol", symbol, confidence, method))
        self.db.executemany(
            "INSERT OR IGNORE INTO evidence_links(provenance_id,unit_id,link_type,link_value,confidence,method) "
            "VALUES(?,?,?,?,?,?)",
            rows,
        )
        self.stats["links"] += len(rows)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def snapshot_source_db(source: Path, workspace: Path) -> tuple[Path, str]:
    """Copy the coordination DB (and WAL/SHM) into the namespace, then verify.

    The live database is only ever *read*; the copy is what ingestion queries,
    which is what makes concurrent daemon writes safe.
    """
    target_dir = assert_writable(workspace / "snapshot")
    target_dir.mkdir(parents=True, exist_ok=True)
    target = target_dir / "cas.db"
    for suffix in ("", "-wal", "-shm"):
        candidate = Path(str(source) + suffix)
        if candidate.exists():
            shutil.copy2(candidate, assert_writable(Path(str(target) + suffix)))
    connection = connect_readonly(target)
    verdict = connection.execute("PRAGMA integrity_check").fetchone()[0]
    connection.close()
    if verdict != "ok":
        raise RuntimeError(f"snapshot integrity_check failed: {verdict}")
    return target, sha256_file(target)


def discover_files(args: argparse.Namespace) -> list[tuple[str, Path]]:
    """Enumerate append-only sources.  Read-only; nothing is opened for write."""
    found: list[tuple[str, Path]] = []
    if args.log_root and Path(args.log_root).exists():
        for path in sorted(Path(args.log_root).glob("*.log")):
            found.append(("daemon_log", path))
    for root in args.claude_root or []:
        if Path(root).exists():
            for path in sorted(Path(root).rglob("*.jsonl")):
                if args.project_marker in str(path) or args.project_marker == "":
                    found.append(("claude_transcript", path))
    for root in args.codex_root or []:
        if Path(root).exists():
            for path in sorted(Path(root).rglob("*.jsonl")):
                found.append(("codex_transcript", path))
    for root in args.grok_root or []:
        if Path(root).exists():
            for path in sorted(Path(root).rglob("chat_history.jsonl")):
                found.append(("grok_transcript", path))
    return found


def ingest(args: argparse.Namespace) -> dict:
    set_writable_root(args.namespace_root)
    namespace_db = Path(args.namespace_root) / "units.sqlite3"
    db = open_namespace(namespace_db)
    started = datetime.now(timezone.utc)
    until = parse_time(args.until) if args.until else None
    scopes = [scope for scope in args.scopes.split(",") if scope]
    for scope in scopes:
        if scope not in PRIVACY_SCOPES:
            raise SystemExit(f"unknown privacy scope: {scope}")

    claims = load_claims(args.claims)
    seed_corrections(db, claims)

    snapshot_sha = ""
    source_db = Path(args.source_db)
    snapshot_path: Path | None = None
    if source_db.exists():
        snapshot_path, snapshot_sha = snapshot_source_db(source_db, Path(args.namespace_root))

    epochs = Epochs([])
    references = ReferenceIndex()
    if snapshot_path:
        snapshot_connection = connect_readonly(snapshot_path)
        epochs = Epochs.load(snapshot_connection)
        references = ReferenceIndex.load(snapshot_connection)
        snapshot_connection.close()

    ingestor = Ingestor(db, epochs, references, claims, args.host, args.project, scopes)
    sources_scanned = 0
    bytes_read = 0

    if snapshot_path:
        snapshot_connection = connect_readonly(snapshot_path)
        for unit_type in DB_SOURCES:
            if unit_type not in args.tables.split(",") and args.tables != "all":
                continue
            source_key = f"db:{unit_type}"
            mark = read_watermark(db, source_key, str(source_db), "row-cursor")
            remaining = args.max_rows
            while remaining > 0:
                batch = min(args.batch, remaining)
                records, highest = db_records(
                    snapshot_connection, unit_type, str(source_db), mark.row_cursor, batch, until
                )
                if not records and highest == mark.row_cursor:
                    break
                for record in records:
                    ingestor.add(record)
                remaining -= batch
                if highest == mark.row_cursor:
                    break
                mark.row_cursor = highest
            write_watermark(db, mark)
            sources_scanned += 1
        snapshot_connection.close()

    for unit_type, path in discover_files(args):
        if unit_type not in args.tables.split(",") and args.tables != "all":
            continue
        try:
            stat = path.stat()
        except OSError:
            continue
        source_key = f"{unit_type}:{path}"
        mark = read_watermark(db, source_key, str(path), "byte-offset")
        if rotated(mark, stat):
            mark.byte_offset = 0
            mark.line_offset = 0
        if stat.st_size <= mark.byte_offset:
            write_watermark(db, mark)
            continue
        reader = FILE_READERS[unit_type]
        records, new_offset, new_line = reader(path, mark.byte_offset, mark.line_offset, args.max_bytes)
        bytes_read += new_offset - mark.byte_offset
        for record in records:
            ingestor.add(record)
        mark.byte_offset = new_offset
        mark.line_offset = new_line
        mark.size_bytes = stat.st_size
        mark.inode = str(stat.st_ino)
        write_watermark(db, mark)
        sources_scanned += 1

    correction_counts = apply_correction_states(db)
    finished = datetime.now(timezone.utc)
    receipt_source = json.dumps(
        {
            "started": iso(started),
            "sources": sources_scanned,
            "bytes": bytes_read,
            "stats": dict(ingestor.stats),
            "snapshot_sha256": snapshot_sha,
        },
        sort_keys=True,
    )
    receipt_hash = hashlib.sha256(receipt_source.encode()).hexdigest()
    run = db.execute(
        "INSERT INTO ingest_runs(started_at,finished_at,window_until,sources_scanned,bytes_read,candidates,"
        "unique_units,duplicates_collapsed,scopes_ingested,snapshot_sha256,receipt_hash) "
        "VALUES(?,?,?,?,?,?,?,?,?,?,?)",
        (
            iso(started),
            iso(finished),
            args.until or "",
            sources_scanned,
            bytes_read,
            ingestor.stats["candidates"],
            ingestor.stats["unique_units"],
            ingestor.stats["duplicates_collapsed"],
            ",".join(scopes),
            snapshot_sha,
            receipt_hash,
        ),
    )
    run_id = int(run.lastrowid)
    for unit_type, (secrets, emails) in sorted(ingestor.redactions.items()):
        db.execute(
            "INSERT INTO redaction_receipts(run_id,unit_type,secrets_redacted,emails_redacted,recorded_at) "
            "VALUES(?,?,?,?,?)",
            (run_id, unit_type, secrets, emails, iso(finished)),
        )
    db.commit()

    total_candidates = ingestor.stats["candidates"]
    collapsed = ingestor.stats["duplicates_collapsed"]
    summary = {
        "run_id": run_id,
        "namespace": NAMESPACE,
        "namespace_db": str(namespace_db),
        "sources_scanned": sources_scanned,
        "bytes_read": bytes_read,
        "candidates": total_candidates,
        "unique_units": ingestor.stats["unique_units"],
        "duplicates_collapsed": collapsed,
        "structural_reduction_pct": round(100.0 * collapsed / total_candidates, 2) if total_candidates else 0.0,
        "links_written": ingestor.stats["links"],
        "skipped_out_of_scope": ingestor.stats["skipped_out_of_scope"],
        "scopes": scopes,
        "correction_states": correction_counts,
        "snapshot_sha256": snapshot_sha,
        "receipt_hash": receipt_hash,
        "duration_seconds": round((finished - started).total_seconds(), 3),
        "embedded": 0,
        "note": "ingestion never embeds and never writes memories or knowledge pages",
    }
    db.close()
    return summary


# --------------------------------------------------------------------------
# cas-9d92 v1 reproduction (SQL-derived; vectors are not consulted)
# --------------------------------------------------------------------------


def reproduce(args: argparse.Namespace) -> dict:
    """Reproduce the cas-9d92 v1 structured findings from normalized sources.

    Every number here comes from SQL.  The correction registry is applied so a
    claim cas-9d92 later withdrew cannot re-enter the findings as a live result.
    """
    set_writable_root(args.namespace_root)
    namespace_db = Path(args.namespace_root) / "units.sqlite3"
    units = connect_readonly(namespace_db)
    snapshot = connect_readonly(args.snapshot)

    def scalar(sql: str, params: tuple = ()) -> int:
        try:
            row = snapshot.execute(sql, params).fetchone()
        except sqlite3.Error:
            return -1
        return int(row[0]) if row and row[0] is not None else 0

    findings: list[dict] = []

    notices = scalar("SELECT COUNT(*) FROM supervisor_queue WHERE event_type='worker_died'")
    unprocessed = scalar(
        "SELECT COUNT(*) FROM supervisor_queue WHERE event_type='worker_died' AND processed_at IS NULL"
    )
    findings.append(
        {
            "finding": "worker_died supervisor notices unprocessed",
            "measured": {"notices": notices, "unprocessed": unprocessed},
            "ratio_pct": round(100.0 * unprocessed / notices, 2) if notices > 0 else 0.0,
            "method": "sql",
        }
    )

    queue_rows = scalar("SELECT COUNT(*) FROM prompt_queue")
    zero_attempts = scalar("SELECT COUNT(*) FROM prompt_queue WHERE COALESCE(delivery_attempts,0)=0")
    findings.append(
        {
            "finding": "prompt_queue.delivery_attempts = 0",
            "measured": {"rows": queue_rows, "at_zero": zero_attempts},
            "ratio_pct": round(100.0 * zero_attempts / queue_rows, 2) if queue_rows > 0 else 0.0,
            "method": "sql",
        }
    )

    by_day: list[dict] = []
    try:
        for day, total, undelivered in snapshot.execute(
            "SELECT substr(created_at,1,10) AS d, COUNT(*), "
            "SUM(CASE WHEN processed_at IS NULL THEN 1 ELSE 0 END) "
            "FROM prompt_queue GROUP BY d ORDER BY d DESC LIMIT ?",
            (args.days,),
        ):
            by_day.append(
                {
                    "day": day,
                    "rows": int(total),
                    "undelivered": int(undelivered or 0),
                    "pct": round(100.0 * (undelivered or 0) / total, 2) if total else 0.0,
                }
            )
    except sqlite3.Error:
        pass
    findings.append({"finding": "undelivered rate by day", "measured": by_day, "method": "sql"})

    pending_reasons: list[dict] = []
    try:
        for reason, count in snapshot.execute(
            "SELECT COALESCE(last_pending_reason,'(null)'), COUNT(*) FROM prompt_queue "
            "GROUP BY 1 ORDER BY 2 DESC LIMIT 10"
        ):
            pending_reasons.append({"last_pending_reason": reason, "rows": int(count)})
    except sqlite3.Error:
        pass
    findings.append({"finding": "pending reason attribution", "measured": pending_reasons, "method": "sql"})

    epoch_strata: list[dict] = []
    try:
        for epoch, count in units.execute(
            "SELECT epoch, COUNT(*) FROM evidence_provenance GROUP BY epoch ORDER BY COUNT(*) DESC LIMIT 12"
        ):
            epoch_strata.append(
                {"epoch": epoch, "provenance_rows": int(count), "post_fix_eligible": not str(epoch).startswith("mixed:")}
            )
    except sqlite3.Error:
        pass
    findings.append(
        {
            "finding": "deployed-binary epoch stratification of normalized units",
            "measured": epoch_strata,
            "method": "sql",
            "discipline": "mixed epochs are never post-fix evidence (cas-9d92)",
        }
    )

    withdrawn: list[dict] = []
    for claim_key, relation, authority, pointer, note in units.execute(
        "SELECT claim_key, relation, authority, evidence_pointer, note FROM evidence_corrections ORDER BY claim_key, authority"
    ):
        asserting = units.execute(
            "SELECT COUNT(*) FROM evidence_units WHERE claim_key=? AND correction_state='withdrawn'",
            (claim_key,),
        ).fetchone()[0]
        withdrawn.append(
            {
                "claim_key": claim_key,
                "relation": relation,
                "authority": authority,
                "evidence_pointer": pointer,
                "note": note,
                "units_marked_withdrawn": int(asserting),
            }
        )

    live_claims = [
        row[0]
        for row in units.execute(
            "SELECT DISTINCT claim_key FROM evidence_units WHERE claim_key IS NOT NULL AND correction_state='current'"
        )
    ]

    snapshot.close()
    units.close()
    return {
        "generated_at": iso(datetime.now(timezone.utc)),
        "source": "structured metrics derived by SQL only; vectors are not consulted",
        "findings": findings,
        "withdrawn_claims_not_reproduced": withdrawn,
        "claims_still_live": live_claims,
    }


# --------------------------------------------------------------------------
# retention
# --------------------------------------------------------------------------


def parse_retention(value: str) -> dict[str, int]:
    policy = dict(DEFAULT_RETENTION_DAYS)
    for item in value.split(","):
        if not item.strip():
            continue
        scope, _, days = item.partition("=")
        scope = scope.strip()
        if scope not in PRIVACY_SCOPES:
            raise SystemExit(f"unknown privacy scope in retention policy: {scope}")
        policy[scope] = int(days)
    return policy


def retention(args: argparse.Namespace) -> dict:
    set_writable_root(args.namespace_root)
    namespace_db = Path(args.namespace_root) / "units.sqlite3"
    db = open_namespace(namespace_db)
    policy = parse_retention(args.policy)
    now = parse_time(args.now) if args.now else datetime.now(timezone.utc)
    receipts: list[dict] = []

    for scope, days in sorted(policy.items()):
        cutoff = iso(now - timedelta(days=days))
        # Timestamp-less provenance (grok chat_history) is retained: deleting on
        # an unknown age would be a silent, unreceipted loss.
        victims = [
            int(row[0])
            for row in db.execute(
                "SELECT id FROM evidence_provenance WHERE privacy_scope=? AND timestamp<>'' AND timestamp<?",
                (scope, cutoff),
            )
        ]
        links_deleted = 0
        if victims:
            placeholders = ",".join("?" * len(victims))
            links_deleted = db.execute(
                f"DELETE FROM evidence_links WHERE provenance_id IN ({placeholders})", victims
            ).rowcount
            db.execute(f"DELETE FROM evidence_provenance WHERE id IN ({placeholders})", victims)
        orphans = [
            int(row[0])
            for row in db.execute(
                "SELECT u.id FROM evidence_units u LEFT JOIN evidence_provenance p ON p.unit_id=u.id "
                "WHERE p.id IS NULL AND u.correction_state<>'correction'"
            )
        ]
        units_deleted = 0
        if orphans:
            placeholders = ",".join("?" * len(orphans))
            db.execute(f"DELETE FROM evidence_fts WHERE rowid IN ({placeholders})", orphans)
            units_deleted = db.execute(
                f"DELETE FROM evidence_units WHERE id IN ({placeholders})", orphans
            ).rowcount
        oldest = db.execute(
            "SELECT MIN(timestamp) FROM evidence_provenance WHERE privacy_scope=? AND timestamp<>''",
            (scope,),
        ).fetchone()[0] or ""
        payload = {
            "scope": scope,
            "days": days,
            "cutoff": cutoff,
            "provenance_deleted": len(victims),
            "links_deleted": links_deleted,
            "units_deleted": units_deleted,
            "oldest_retained": oldest,
        }
        receipt_hash = hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()
        db.execute(
            "INSERT INTO retention_receipts(run_at,policy,privacy_scope,cutoff,provenance_deleted,units_deleted,"
            "links_deleted,oldest_retained,receipt_hash) VALUES(?,?,?,?,?,?,?,?,?)",
            (
                iso(now),
                args.policy,
                scope,
                cutoff,
                len(victims),
                units_deleted,
                links_deleted,
                oldest,
                receipt_hash,
            ),
        )
        receipts.append({**payload, "receipt_hash": receipt_hash})
    db.commit()
    db.close()
    return {"run_at": iso(now), "policy": policy, "receipts": receipts}


# --------------------------------------------------------------------------
# correction-aware retrieval
# --------------------------------------------------------------------------

WITHDRAWN_PENALTY = 0.15


def search_units(
    connection: sqlite3.Connection, text: str, top: int = 8, scopes: Sequence[str] | None = None
) -> list[dict]:
    """Lexical retrieval that can never return a withdrawn claim bare.

    cas-c505 measured the failure this prevents: repeated historical prose about
    a claim outranked the authoritative correction, and neither lexical nor
    vector ranking surfaced the correction at all.  Here the correction travels
    with the claim by construction, and the withdrawn assertion is down-ranked
    below its own correction.
    """
    terms = list(dict.fromkeys(re.findall(r"[A-Za-z][A-Za-z0-9_-]{2,}", text.lower())))
    if not terms:
        return []
    expression = " OR ".join('"' + term.replace('"', "") + '"' for term in terms[:24])
    rows = connection.execute(
        "SELECT u.id, u.unit_type, u.text, u.occurrence_count, u.privacy_scope, u.claim_key, "
        "u.correction_state, bm25(evidence_fts) FROM evidence_fts "
        "JOIN evidence_units u ON u.id = evidence_fts.rowid "
        "WHERE evidence_fts MATCH ? ORDER BY bm25(evidence_fts) LIMIT 400",
        (expression,),
    ).fetchall()

    allowed = set(scopes) if scopes else set(PRIVACY_SCOPES)
    scored: list[dict] = []
    for unit_id, unit_type, unit_text, occurrences, scope, claim_key, state, bm25 in rows:
        if scope not in allowed:
            continue
        score = -float(bm25)
        if state == "withdrawn":
            score *= WITHDRAWN_PENALTY
        elif state == "correction":
            score *= 1.0 + WITHDRAWN_PENALTY
        scored.append(
            {
                "unit_id": int(unit_id),
                "unit_type": unit_type,
                "score": round(score, 6),
                "occurrence_count": int(occurrences),
                "privacy_scope": scope,
                "claim_key": claim_key,
                "correction_state": state,
                "text": unit_text,
            }
        )
    scored.sort(key=lambda item: item["score"], reverse=True)

    results = scored[:top]
    attached: list[dict] = []
    for item in results:
        if item["claim_key"]:
            item["corrections"] = [
                {
                    "relation": relation,
                    "authority": authority,
                    "evidence_pointer": pointer,
                    "recorded_at": recorded,
                    "note": note,
                }
                for relation, authority, pointer, recorded, note in connection.execute(
                    "SELECT relation,authority,evidence_pointer,recorded_at,note FROM evidence_corrections "
                    "WHERE claim_key=? ORDER BY authority",
                    (item["claim_key"],),
                )
            ]
            if item["correction_state"] == "withdrawn":
                for row in connection.execute(
                    "SELECT id,unit_type,text FROM evidence_units WHERE claim_key=? AND correction_state='correction'",
                    (item["claim_key"],),
                ):
                    if not any(existing["unit_id"] == int(row[0]) for existing in results + attached):
                        attached.append(
                            {
                                "unit_id": int(row[0]),
                                "unit_type": row[1],
                                "score": item["score"],
                                "occurrence_count": 0,
                                "privacy_scope": "project",
                                "claim_key": item["claim_key"],
                                "correction_state": "correction",
                                "text": row[2],
                                "attached_because": "authoritative correction for a withdrawn claim in the result set",
                            }
                        )
        else:
            item["corrections"] = []
    return results + attached


def query(args: argparse.Namespace) -> dict:
    namespace_db = Path(args.namespace_root) / "units.sqlite3"
    connection = connect_readonly(namespace_db)
    scopes = [scope for scope in args.scopes.split(",") if scope]
    results = search_units(connection, args.query, args.top, scopes)
    connection.close()
    return {"query": args.query, "scopes": scopes, "results": results}


def status(args: argparse.Namespace) -> dict:
    namespace_db = Path(args.namespace_root) / "units.sqlite3"
    if not namespace_db.exists():
        return {"namespace_db": str(namespace_db), "exists": False}
    connection = connect_readonly(namespace_db)

    def rows(sql: str) -> list:
        try:
            return connection.execute(sql).fetchall()
        except sqlite3.Error:
            return []

    payload = {
        "namespace": NAMESPACE,
        "namespace_db": str(namespace_db),
        "exists": True,
        "units": rows("SELECT COUNT(*) FROM evidence_units")[0][0],
        "provenance": rows("SELECT COUNT(*) FROM evidence_provenance")[0][0],
        "links": rows("SELECT COUNT(*) FROM evidence_links")[0][0],
        "by_type": [{"unit_type": t, "units": n} for t, n in rows(
            "SELECT unit_type, COUNT(*) FROM evidence_units GROUP BY unit_type ORDER BY 2 DESC"
        )],
        "by_scope": [{"privacy_scope": s, "units": n} for s, n in rows(
            "SELECT privacy_scope, COUNT(*) FROM evidence_units GROUP BY privacy_scope ORDER BY 2 DESC"
        )],
        "correction_states": [{"state": s, "units": n} for s, n in rows(
            "SELECT correction_state, COUNT(*) FROM evidence_units GROUP BY correction_state ORDER BY 2 DESC"
        )],
        "embed_states": [{"state": s, "units": n} for s, n in rows(
            "SELECT embed_state, COUNT(*) FROM evidence_units GROUP BY embed_state ORDER BY 2 DESC"
        )],
        "watermarks": [
            {"source_key": k, "cursor_kind": c, "byte_offset": b, "row_cursor": r, "updated_at": u}
            for k, c, b, r, u in rows(
                "SELECT source_key,cursor_kind,byte_offset,row_cursor,updated_at FROM ingest_watermarks ORDER BY source_key"
            )
        ],
        "runs": [
            {"id": i, "started_at": s, "candidates": c, "unique_units": u, "duplicates_collapsed": d, "receipt_hash": h}
            for i, s, c, u, d, h in rows(
                "SELECT id,started_at,candidates,unique_units,duplicates_collapsed,receipt_hash FROM ingest_runs ORDER BY id DESC LIMIT 10"
            )
        ],
        "retention_receipts": [
            {"run_at": r, "scope": s, "cutoff": c, "provenance_deleted": p, "units_deleted": u, "receipt_hash": h}
            for r, s, c, p, u, h in rows(
                "SELECT run_at,privacy_scope,cutoff,provenance_deleted,units_deleted,receipt_hash FROM retention_receipts ORDER BY id DESC LIMIT 10"
            )
        ],
        "redaction_receipts": [
            {"run_id": r, "unit_type": t, "secrets": s, "emails": e}
            for r, t, s, e in rows(
                "SELECT run_id,unit_type,secrets_redacted,emails_redacted FROM redaction_receipts ORDER BY id DESC LIMIT 20"
            )
        ],
    }
    connection.close()
    return payload


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def add_common(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--namespace-root", type=Path, default=DEFAULT_NAMESPACE_ROOT)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)

    ing = sub.add_parser("ingest", help="incremental read-only ingestion")
    add_common(ing)
    ing.add_argument("--source-db", type=Path, default=DEFAULT_SOURCE_DB)
    ing.add_argument("--log-root", type=Path, default=DEFAULT_LOG_ROOT)
    ing.add_argument("--claude-root", action="append", type=Path, default=[])
    ing.add_argument("--codex-root", action="append", type=Path, default=[])
    ing.add_argument("--grok-root", action="append", type=Path, default=[])
    ing.add_argument("--claims", type=Path, default=DEFAULT_CLAIMS)
    ing.add_argument("--scopes", default="host,project,team")
    ing.add_argument("--tables", default="all", help="comma list of unit types, or 'all'")
    ing.add_argument("--project", default=DEFAULT_PROJECT)
    ing.add_argument("--project-marker", default="cas-src")
    ing.add_argument("--host", default=os.uname().nodename)
    ing.add_argument("--until", default="", help="exclusive RFC3339 upper bound on row timestamps")
    ing.add_argument("--batch", type=int, default=2000)
    ing.add_argument("--max-rows", type=int, default=20000, help="per-table row budget for one run")
    ing.add_argument("--max-bytes", type=int, default=8 << 20, help="per-file byte budget for one run")
    ing.set_defaults(func=ingest)

    rep = sub.add_parser("reproduce", help="reproduce the cas-9d92 v1 findings by SQL")
    add_common(rep)
    rep.add_argument("--snapshot", type=Path, required=True)
    rep.add_argument("--days", type=int, default=7)
    rep.set_defaults(func=reproduce)

    ret = sub.add_parser("retention", help="apply retention policy and emit deletion receipts")
    add_common(ret)
    ret.add_argument("--policy", default="host=30,project=365,team=365")
    ret.add_argument("--now", default="", help="override 'now' for deterministic runs")
    ret.set_defaults(func=retention)

    qry = sub.add_parser("query", help="correction-aware lexical retrieval")
    add_common(qry)
    qry.add_argument("query")
    qry.add_argument("--top", type=int, default=8)
    qry.add_argument("--scopes", default="host,project,team")
    qry.set_defaults(func=query)

    sta = sub.add_parser("status", help="watermarks, receipts, and namespace counts")
    add_common(sta)
    sta.set_defaults(func=status)

    args = parser.parse_args(argv)
    print(json.dumps(args.func(args), indent=2, sort_keys=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
