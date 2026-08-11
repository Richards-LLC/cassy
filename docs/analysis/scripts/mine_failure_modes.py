#!/usr/bin/env python3
"""Mine high-precision failure-mode candidates from local agent transcripts.

The script is deliberately lexical and read-only.  It emits candidate evidence;
the report author must adjudicate candidates before calling them incidents.
"""

from __future__ import annotations

import argparse
import csv
import json
import re
import sqlite3
from collections import Counter, defaultdict
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterable, Iterator


FAILURE_PATTERNS: dict[str, tuple[re.Pattern[str], ...]] = {
    "unscoped-test-guard": (
        re.compile(r"factory test scope guard", re.I),
        re.compile(r"unscoped (?:cargo )?test(?: run)? (?:was )?(?:blocked|rejected|denied)", re.I),
    ),
    "workspace-contract-denial": (
        re.compile(r"factory workspace contract", re.I),
        re.compile(r"workspace contract (?:blocked|denied|rejected)", re.I),
    ),
    "wrong-process-or-environment": (
        re.compile(r"(?:killed|stopped|restarted|targeted) the wrong (?:process|daemon|server)", re.I),
        re.compile(r"wrong[- ]process (?:damage|kill|restart|target)", re.I),
        re.compile(r"wrong (?:cas_root|environment|store).{0,80}(?:damage|write|mutat|target)", re.I | re.S),
    ),
    "scope-drift": (
        re.compile(r"scope drift", re.I),
        re.compile(r"(?:change|work|edit|file).{0,80}out of scope", re.I | re.S),
        re.compile(r"out of (?:the )?(?:assigned )?scope.{0,80}(?:remove|revert|reject)", re.I | re.S),
    ),
    "partial-delivery": (
        re.compile(r"partial deliver", re.I),
        re.compile(r"(?:missing|omitted|failed).{0,80}acceptance criter", re.I | re.S),
        re.compile(r"acceptance criter.{0,80}(?:missing|omitted|not (?:met|satisfied))", re.I | re.S),
    ),
    "missed-surface": (
        re.compile(r"missed surface", re.I),
        re.compile(r"(?:mirror|migration pin|generated config).{0,100}(?:missed|omitted|stale|not (?:updated|synced))", re.I | re.S),
        re.compile(r"(?:missed|omitted|stale).{0,100}(?:mirror|migration pin|generated config)", re.I | re.S),
    ),
    "crossed-message-merge-race": (
        re.compile(r"crossed (?:message|messages|merge|amendment)", re.I),
        re.compile(r"predates (?:the )?(?:operator|supervisor).{0,40}amendment", re.I | re.S),
        re.compile(r"(?:message|amendment).{0,80}crossed (?:your )?(?:commit|push|merge|delivery)", re.I | re.S),
    ),
    "premature-done-claim": (
        re.compile(r"premature.{0,40}(?:done|close|complete|completion)", re.I | re.S),
        re.compile(r"claimed (?:it |this )?(?:done|complete).{0,80}(?:but|before|without|missing)", re.I | re.S),
        re.compile(r"(?:was |were )?closed.{0,60}before.{0,40}(?:proof|verif|test|merge)", re.I | re.S),
    ),
    "draft-or-format-violation": (
        re.compile(r"(?:draft|format).{0,60}(?:violation|invalid|rejected|wrong|noncompliant)", re.I | re.S),
        re.compile(r"(?:violation|invalid|rejected|wrong|noncompliant).{0,60}(?:draft|format)", re.I | re.S),
    ),
}

SUPERVISOR_MARKERS = (
    "message from supervisor:",
    "message from director:",
    "decision: changes requested",
    "close rejected:",
    "review rejected:",
)

TASK_TOOL_NAMES = {
    "mcp__cs__task",
    "mcp__cas__task",
    "mcp__cs.task",
    "mcp__cas.task",
}


@dataclass(frozen=True)
class Candidate:
    failure_class: str
    harness: str
    model: str
    session_id: str
    worker: str
    timestamp: str
    pointer: str
    source_kind: str
    excerpt: str


def parse_timestamp(value: object) -> datetime | None:
    if not isinstance(value, str) or not value:
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00")).astimezone(timezone.utc)
    except ValueError:
        return None


def strings(value: object) -> Iterator[str]:
    if isinstance(value, str):
        yield value
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)
    elif isinstance(value, dict):
        for key in ("text", "output", "content"):
            if key in value:
                yield from strings(value[key])


def compact(text: str, limit: int = 280) -> str:
    value = re.sub(r"\s+", " ", text).strip()
    return value if len(value) <= limit else value[: limit - 1] + "…"


def worker_from_cwd(cwd: str) -> str:
    match = re.search(r"/\.cas/worktrees/([^/]+)", cwd)
    return match.group(1) if match else ""


def classes_for(text: str) -> list[str]:
    return [name for name, patterns in FAILURE_PATTERNS.items() if any(p.search(text) for p in patterns)]


def observed_classes(source_kind: str, text: str) -> list[str]:
    """Apply the v1 evidence hierarchy, not every lexical candidate.

    Semantic failures require an authoritative supervisor/task-note statement.
    Tool output can establish only the two guards with unique runtime banners.
    Assistant analysis is searchable context but is not incident evidence.
    """
    matched = classes_for(text)
    if source_kind in {"supervisor-message", "task-note"}:
        return matched
    if source_kind == "tool-output":
        return [
            failure_class
            for failure_class in matched
            if failure_class in {"unscoped-test-guard", "workspace-contract-denial"}
        ]
    return []


def is_task_dump(text: str) -> bool:
    """Reject task lifecycle payloads that replay descriptions/sibling notes."""
    lowered = text.lower()
    return (
        "📋 sibling task notes" in lowered
        or "📋 task notes" in lowered
        or re.search(r"(?:^|output:\s*)\[?\{?\"?type\"?.{0,80}started task:", lowered, re.S) is not None
        or ("task:" in lowered and "acceptance criteria:" in lowered and "description:" in lowered)
    )


def glob_jsonl(roots: Iterable[Path]) -> Iterator[Path]:
    seen: set[Path] = set()
    for root in roots:
        if not root.exists():
            continue
        for path in root.rglob("*.jsonl"):
            if path not in seen:
                seen.add(path)
                yield path


def mine_claude(path: Path, since: datetime, until: datetime) -> tuple[dict[str, str], list[Candidate]]:
    session_id = path.stem
    model = "unknown"
    cwd = ""
    is_factory = "/.cas/worktrees/" in str(path)
    tool_names: dict[str, str] = {}
    raw_hits: list[tuple[str, str, str, int, str]] = []
    first_ts: datetime | None = None
    last_ts: datetime | None = None

    with path.open(errors="replace") as handle:
        for line_no, line in enumerate(handle, 1):
            if "CAS Factory Worker" in line or "CAS Factory Supervisor" in line or "CAS_FACTORY_MODE" in line:
                is_factory = True
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            ts = parse_timestamp(row.get("timestamp"))
            if ts:
                first_ts = min(first_ts, ts) if first_ts else ts
                last_ts = max(last_ts, ts) if last_ts else ts
            session_id = str(row.get("sessionId") or session_id)
            cwd = str(row.get("cwd") or cwd)
            message = row.get("message") if isinstance(row.get("message"), dict) else {}
            if isinstance(message.get("model"), str):
                model = message["model"]
            content = message.get("content")
            if row.get("type") == "assistant" and isinstance(content, list):
                for block in content:
                    if isinstance(block, dict) and block.get("type") == "tool_use":
                        tool_names[str(block.get("id", ""))] = str(block.get("name", ""))
            if not ts or ts < since or ts >= until:
                continue

            eligible: list[tuple[str, str]] = []
            role = message.get("role")
            if role == "assistant":
                eligible.extend(("assistant", text) for text in strings(content))
            elif role == "user" and isinstance(content, list):
                for block in content:
                    if not isinstance(block, dict):
                        continue
                    if block.get("type") == "tool_result":
                        tool_id = str(block.get("tool_use_id", ""))
                        if tool_names.get(tool_id) not in TASK_TOOL_NAMES:
                            eligible.extend(("tool-output", text) for text in strings(block.get("content")))
                    elif block.get("type") == "text":
                        text = str(block.get("text", ""))
                        if any(marker in text.lower() for marker in SUPERVISOR_MARKERS):
                            eligible.append(("supervisor-message", text))
            elif role == "user" and isinstance(content, str):
                if any(marker in content.lower() for marker in SUPERVISOR_MARKERS):
                    eligible.append(("supervisor-message", content))

            for source_kind, text in eligible:
                if source_kind == "tool-output" and is_task_dump(text):
                    continue
                for failure_class in observed_classes(source_kind, text):
                    raw_hits.append((failure_class, source_kind, ts.isoformat(), line_no, compact(text)))

    metadata = {
        "harness": "claude",
        "model": model,
        "session_id": session_id,
        "worker": worker_from_cwd(cwd),
        "first_timestamp": first_ts.isoformat() if first_ts else "",
        "last_timestamp": last_ts.isoformat() if last_ts else "",
        "factory": "true" if is_factory else "false",
    }
    if not is_factory:
        return metadata, []
    hits = [
        Candidate(fc, "claude", model, session_id, metadata["worker"], ts, f"{path}:{line_no}", kind, excerpt)
        for fc, kind, ts, line_no, excerpt in raw_hits
    ]
    return metadata, hits


def mine_codex(path: Path, since: datetime, until: datetime) -> tuple[dict[str, str], list[Candidate]]:
    session_id = path.stem
    model = "unknown"
    cwd = ""
    is_factory = False
    tool_names: dict[str, str] = {}
    raw_hits: list[tuple[str, str, str, int, str]] = []
    first_ts: datetime | None = None
    last_ts: datetime | None = None

    with path.open(errors="replace") as handle:
        for line_no, line in enumerate(handle, 1):
            if "CAS Factory Worker" in line or "CAS Factory Supervisor" in line or "CAS_FACTORY_MODE" in line:
                is_factory = True
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            ts = parse_timestamp(row.get("timestamp"))
            if ts:
                first_ts = min(first_ts, ts) if first_ts else ts
                last_ts = max(last_ts, ts) if last_ts else ts
            payload = row.get("payload") if isinstance(row.get("payload"), dict) else {}
            if row.get("type") == "session_meta":
                session_id = str(payload.get("session_id") or payload.get("id") or session_id)
                cwd = str(payload.get("cwd") or cwd)
                if "/.cas/worktrees/" in cwd:
                    is_factory = True
            elif row.get("type") == "turn_context":
                if isinstance(payload.get("model"), str):
                    model = payload["model"]
                cwd = str(payload.get("cwd") or cwd)
            elif row.get("type") == "response_item" and payload.get("type") == "function_call":
                tool_names[str(payload.get("call_id", ""))] = str(payload.get("name", ""))

            if not ts or ts < since or ts >= until or row.get("type") != "response_item":
                continue
            eligible: list[tuple[str, str]] = []
            item_type = payload.get("type")
            if item_type == "message" and payload.get("role") == "assistant":
                eligible.extend(("assistant", text) for text in strings(payload.get("content")))
            elif item_type == "message" and payload.get("role") == "user":
                for text in strings(payload.get("content")):
                    if any(marker in text.lower() for marker in SUPERVISOR_MARKERS):
                        eligible.append(("supervisor-message", text))
            elif item_type == "function_call_output":
                call_id = str(payload.get("call_id", ""))
                if tool_names.get(call_id) not in TASK_TOOL_NAMES:
                    eligible.extend(("tool-output", text) for text in strings(payload.get("output")))
            for source_kind, text in eligible:
                if source_kind == "tool-output" and is_task_dump(text):
                    continue
                for failure_class in observed_classes(source_kind, text):
                    raw_hits.append((failure_class, source_kind, ts.isoformat(), line_no, compact(text)))

    metadata = {
        "harness": "codex",
        "model": model,
        "session_id": session_id,
        "worker": worker_from_cwd(cwd),
        "first_timestamp": first_ts.isoformat() if first_ts else "",
        "last_timestamp": last_ts.isoformat() if last_ts else "",
        "factory": "true" if is_factory else "false",
    }
    if not is_factory:
        return metadata, []
    hits = [
        Candidate(fc, "codex", model, session_id, metadata["worker"], ts, f"{path}:{line_no}", kind, excerpt)
        for fc, kind, ts, line_no, excerpt in raw_hits
    ]
    return metadata, hits


def mine_task_notes(db_path: Path, since: datetime, until: datetime) -> list[Candidate]:
    uri = f"file:{db_path}?mode=ro"
    connection = sqlite3.connect(uri, uri=True)
    rows = connection.execute(
        """
        SELECT e.entity_id, e.summary, e.created_at, e.session_id,
               a.name,
               COALESCE(
                   (SELECT json_extract(sq.worker_spec, '$.cli')
                    FROM spawn_queue sq
                    WHERE sq.task_id = e.entity_id AND sq.created_at <= e.created_at
                    ORDER BY sq.id DESC LIMIT 1),
                   json_extract(a.metadata, '$.worker_cli'),
                   'unknown'
               ) AS cli,
               COALESCE(
                   (SELECT json_extract(sq.worker_spec, '$.model')
                    FROM spawn_queue sq
                    WHERE sq.task_id = e.entity_id AND sq.created_at <= e.created_at
                    ORDER BY sq.id DESC LIMIT 1),
                   'unknown'
               ) AS model,
               COALESCE(
                   a.name,
                   (SELECT sq.spawn_worker
                    FROM spawn_queue sq
                    WHERE sq.task_id = e.entity_id AND sq.created_at <= e.created_at
                    ORDER BY sq.id DESC LIMIT 1),
                   ''
               ) AS resolved_worker
        FROM events e
        LEFT JOIN agents a ON a.id = e.session_id
        WHERE e.event_type = 'task_note_added' AND e.created_at >= ? AND e.created_at < ?
        ORDER BY e.id
        """,
        (since.isoformat(), until.isoformat()),
    )
    candidates: list[Candidate] = []
    for task_id, summary, timestamp, session_id, _agent_worker, cli, model, worker in rows:
        for failure_class in observed_classes("task-note", summary):
            candidates.append(
                Candidate(
                    failure_class,
                    f"factory-note/{cli}",
                    model,
                    session_id or "",
                    worker or "",
                    timestamp,
                    f"task:{task_id}@{timestamp}",
                    "task-note",
                    compact(summary),
                )
            )
    connection.close()
    return candidates


def dedupe(candidates: Iterable[Candidate]) -> list[Candidate]:
    """Keep one pointer per session/source and failure class."""
    result: dict[tuple[str, str, str], Candidate] = {}
    for candidate in candidates:
        key = (candidate.harness, candidate.session_id or candidate.pointer, candidate.failure_class)
        result.setdefault(key, candidate)
    return sorted(result.values(), key=lambda c: (c.harness, c.model, c.session_id, c.failure_class))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--since", required=True, help="inclusive RFC3339 timestamp")
    parser.add_argument("--until", required=True, help="exclusive RFC3339 timestamp")
    parser.add_argument("--claude-root", action="append", type=Path, default=[])
    parser.add_argument("--codex-root", action="append", type=Path, default=[])
    parser.add_argument("--task-db", type=Path)
    parser.add_argument("--output", required=True, type=Path, help="candidate CSV")
    parser.add_argument("--summary", type=Path, help="optional JSON corpus summary")
    args = parser.parse_args()

    since = parse_timestamp(args.since)
    until = parse_timestamp(args.until)
    if since is None or until is None or since >= until:
        parser.error("--since and --until must be ordered RFC3339 timestamps")

    sessions: list[dict[str, str]] = []
    candidates: list[Candidate] = []
    for path in glob_jsonl(args.claude_root):
        # A file not modified since the window opened cannot contain an in-window
        # append. Resumed older sessions remain eligible because their mtime moves.
        if path.stat().st_mtime < since.timestamp():
            continue
        metadata, hits = mine_claude(path, since, until)
        if metadata["factory"] == "true" and metadata["last_timestamp"] and parse_timestamp(metadata["last_timestamp"]) >= since:
            sessions.append(metadata)
            candidates.extend(hits)
    for path in glob_jsonl(args.codex_root):
        if path.stat().st_mtime < since.timestamp():
            continue
        metadata, hits = mine_codex(path, since, until)
        if metadata["factory"] == "true" and metadata["last_timestamp"] and parse_timestamp(metadata["last_timestamp"]) >= since:
            sessions.append(metadata)
            candidates.extend(hits)
    if args.task_db:
        candidates.extend(mine_task_notes(args.task_db, since, until))

    rows = dedupe(candidates)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("w", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=list(Candidate.__dataclass_fields__))
        writer.writeheader()
        writer.writerows(candidate.__dict__ for candidate in rows)

    if args.summary:
        corpus = Counter((s["harness"], s["model"]) for s in sessions)
        failures = Counter((c.harness, c.model, c.failure_class) for c in rows)
        payload = {
            "window": {"since": since.isoformat(), "until": until.isoformat()},
            "factory_sessions": [
                {"harness": harness, "model": model, "sessions": count}
                for (harness, model), count in sorted(corpus.items())
            ],
            "candidate_evidence_units": [
                {"harness": harness, "model": model, "failure_class": failure_class, "count": count}
                for (harness, model, failure_class), count in sorted(failures.items())
            ],
            "method": "high-precision lexical candidates; human adjudication required",
        }
        args.summary.parent.mkdir(parents=True, exist_ok=True)
        args.summary.write_text(json.dumps(payload, indent=2) + "\n")


if __name__ == "__main__":
    main()
