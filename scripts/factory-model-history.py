#!/usr/bin/env python3
"""Extract factory worker/model history from local CAS stores and transcripts.

The extractor is deliberately read-only with respect to its inputs.  SQLite is
opened using ``mode=ro`` and the generated CSV/Markdown files are written to a
separate output directory.  No transcript contents are copied to the outputs;
only aggregate usage and join metadata are emitted.
"""

from __future__ import annotations

import argparse
import csv
import json
import math
import re
import sqlite3
import statistics
import sys
from collections import defaultdict
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable, Optional


UTC = timezone.utc
WORKTREE_RE = re.compile(r"(?:^|/)\.cas/worktrees/([^/]+)(?:/|$)")
PUSH_RE = re.compile(r"\bpushed\b|\bpush(?:ed)?\s+(?:successfully|complete|ready)\b", re.I)
NEGATIVE_PUSH_RE = re.compile(r"\b(?:not|no|without|hold|held)\s+push(?:ed|ing)?\b", re.I)
SEND_BACK_RE = re.compile(r"\brequest_changes\b|\bchanges requested\b", re.I)
URGENT_RE = re.compile(r"\burgent[- _]?stop\b|\bwork halted\b|\bworker halted\b|\burgent redirect\b", re.I)
MERGE_REQUIRED_RE = re.compile(r"\bmerge required\b", re.I)

SESSION_FIELDS = [
    "project",
    "project_path",
    "worker_name",
    "session_id",
    "harness",
    "source_db",
    "source_transcript",
    "source_factory_logs",
    "factory_session",
    "spawn_id",
    "spawn_at",
    "cli",
    "model",
    "effort",
    "transcript_join_key",
    "transcript_joined",
    "session_first_at",
    "session_last_at",
    "task_id",
    "task_title",
    "task_status",
    "task_closed",
    "task_created_at",
    "task_closed_at",
    "lease_acquired_at",
    "lease_released_at",
    "lease_terminal_event",
    "send_back_count",
    "urgent_stop_count",
    "merge_required_count",
    "close_reason",
    "first_push_at",
    "minutes_to_first_push",
    "input_tokens",
    "cached_input_tokens",
    "cache_creation_input_tokens",
    "output_tokens",
    "reasoning_tokens",
    "tool_calls",
    "cost_usd",
]

SCORECARD_FIELDS = [
    "scope",
    "project",
    "model",
    "effort",
    "sessions",
    "tasks_delivered",
    "send_backs",
    "send_back_rate",
    "urgent_stops",
    "median_minutes_to_first_push",
    "median_input_tokens_per_delivered_task",
    "median_cached_input_tokens_per_delivered_task",
    "median_output_tokens_per_delivered_task",
    "median_tool_calls_per_delivered_task",
    "cost_usd",
]

CSV_COLUMN_DESCRIPTIONS = {
    "project": "basename of the project containing the CAS DB",
    "project_path": "absolute project root",
    "worker_name": "factory worker name from the DB or transcript worktree path",
    "session_id": "Codex/Claude session ID, or an explicit DB fallback key",
    "harness": "codex, claude, or db when no transcript matched",
    "source_db": "read-only CAS database path",
    "source_transcript": "Codex/Claude JSONL path, blank when unavailable",
    "source_factory_logs": "sibling .cas/logs directory scanned",
    "factory_session": "CAS factory session identifier",
    "spawn_id": "spawn_queue row ID",
    "spawn_at": "spawn or registered-at timestamp",
    "cli": "spawned CLI",
    "model": "model from rollout turn_context, then spawn metadata",
    "effort": "effort from rollout turn_context, then spawn metadata",
    "transcript_join_key": "bounded project + worker-name join key",
    "transcript_joined": "yes when a transcript matched the DB worker row",
    "session_first_at": "first timestamp observed in the transcript",
    "session_last_at": "last timestamp observed in the transcript",
    "task_id": "DB task ID; blank for transcript-only or taskless rows",
    "task_title": "DB task title",
    "task_status": "DB task status",
    "task_closed": "yes only when task status is closed",
    "task_created_at": "DB task creation timestamp",
    "task_closed_at": "DB task close timestamp",
    "lease_acquired_at": "first claimed lease timestamp",
    "lease_released_at": "last released/revoked/expired lease timestamp",
    "lease_terminal_event": "last lease terminal event",
    "send_back_count": "request_changes / Changes requested occurrences in task notes",
    "urgent_stop_count": "urgent-stop / halt marker occurrences in task notes",
    "merge_required_count": "MERGE REQUIRED occurrences in task notes",
    "close_reason": "DB task close reason",
    "first_push_at": "first positive pushed marker in a matching factory log",
    "minutes_to_first_push": "minutes from transcript first timestamp to first push",
    "input_tokens": "summed explicit input tokens",
    "cached_input_tokens": "summed cached/cache-read input tokens",
    "cache_creation_input_tokens": "summed Claude cache-creation input tokens",
    "output_tokens": "summed explicit output tokens",
    "reasoning_tokens": "summed explicit reasoning/thinking tokens",
    "tool_calls": "Codex function_call/custom_tool_call records or Claude tool_use blocks",
    "cost_usd": "price-file-derived usage cost; blank without a verified price",
}

SCORECARD_COLUMN_DESCRIPTIONS = {
    "scope": "project or overall model/effort aggregation",
    "project": "project name; blank for overall rows",
    "model": "model grouping",
    "effort": "effort grouping",
    "sessions": "distinct worker sessions",
    "tasks_delivered": "distinct closed task IDs",
    "send_backs": "deduplicated send-back occurrences",
    "send_back_rate": "send-backs divided by delivered tasks",
    "urgent_stops": "deduplicated urgent-stop occurrences",
    "median_minutes_to_first_push": "median delivered-task minutes to first push",
    "median_input_tokens_per_delivered_task": "median session input tokens divided by delivered tasks in that session",
    "median_cached_input_tokens_per_delivered_task": "median cached tokens divided by delivered tasks in that session",
    "median_output_tokens_per_delivered_task": "median output tokens divided by delivered tasks in that session",
    "median_tool_calls_per_delivered_task": "median tool calls divided by delivered tasks in that session",
    "cost_usd": "sum of price-file-derived session costs",
}


def parse_timestamp(value: Any) -> Optional[datetime]:
    """Parse the ISO timestamps used by CAS, Codex, and Claude records."""

    if isinstance(value, (int, float)):
        # Codex timestamps are ISO today, but accepting epoch seconds/millis
        # keeps fixture and future formats explicit rather than guessed later.
        seconds = float(value)
        if seconds > 10_000_000_000:
            seconds /= 1000
        try:
            return datetime.fromtimestamp(seconds, UTC)
        except (OverflowError, OSError, ValueError):
            return None
    if not isinstance(value, str) or not value.strip():
        return None
    text = value.strip()
    if text.endswith("Z"):
        text = text[:-1] + "+00:00"
    try:
        parsed = datetime.fromisoformat(text)
    except ValueError:
        return None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=UTC)
    return parsed.astimezone(UTC)


def timestamp_text(value: Any) -> str:
    parsed = value if isinstance(value, datetime) else parse_timestamp(value)
    if not parsed:
        return ""
    return parsed.astimezone(UTC).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def number(value: Any) -> int:
    if isinstance(value, bool):
        return 0
    if isinstance(value, (int, float)) and math.isfinite(float(value)):
        return int(value)
    return 0


def integer_text(value: Any) -> str:
    return "" if value is None or value == "" else str(int(value))


def project_root_for_db(db_path: Path) -> Path:
    return db_path.parent.parent


def project_name(project_path: Path, root: Path) -> str:
    try:
        relative = project_path.relative_to(root)
    except ValueError:
        relative = project_path
    if str(relative) == ".":
        return project_path.name
    # Project names are used for aggregation, while project_path retains the
    # unambiguous full path in the session CSV.
    return project_path.name or str(relative)


def worker_name_from_cwd(cwd: str) -> str:
    if not cwd:
        return ""
    match = WORKTREE_RE.search(cwd.replace("\\", "/"))
    return match.group(1) if match else ""


def is_factory_cwd(cwd: str) -> bool:
    return bool(worker_name_from_cwd(cwd))


def project_path_from_cwd(cwd: str) -> Optional[Path]:
    if not cwd:
        return None
    path = Path(cwd)
    parts = path.parts
    try:
        cas_index = parts.index(".cas")
    except ValueError:
        return None
    if cas_index == 0:
        return None
    return Path(*parts[:cas_index])


def discover_database_paths(root: Path) -> list[Path]:
    """Find project stores while excluding generated/isolated CAS trees."""

    excluded = {"worktrees", "artifacts"}
    found: list[Path] = []
    try:
        candidates = root.rglob("cas.db")
    except OSError:
        return []
    for path in candidates:
        if path.name != "cas.db" or path.parent.name != ".cas":
            continue
        try:
            relative_parts = path.relative_to(root).parts
        except ValueError:
            relative_parts = path.parts
        if any(part in excluded or part.startswith("epic-") for part in relative_parts):
            continue
        found.append(path)
    return sorted(set(found))


def _usage_values(usage: dict[str, Any]) -> tuple[int, int, int, int, int]:
    cached = number(usage.get("cached_input_tokens"))
    if not cached:
        cached = number(usage.get("cache_read_input_tokens"))
    creation = number(usage.get("cache_creation_input_tokens"))
    output = number(usage.get("output_tokens"))
    reasoning = number(usage.get("reasoning_output_tokens"))
    if not reasoning:
        reasoning = number(usage.get("reasoning_tokens"))
    if not reasoning:
        details = usage.get("output_tokens_details")
        if isinstance(details, dict):
            reasoning = number(details.get("thinking_tokens"))
    input_tokens = number(usage.get("input_tokens"))
    if not input_tokens:
        input_tokens = number(usage.get("prompt_tokens"))
    if not output:
        output = number(usage.get("completion_tokens"))
    return input_tokens, cached, creation, output, reasoning


@dataclass
class Transcript:
    harness: str
    path: Path
    source_home: str = ""
    session_id: str = ""
    worker_name: str = ""
    cwd: str = ""
    model: str = ""
    effort: str = ""
    first_at: Optional[datetime] = None
    last_at: Optional[datetime] = None
    input_tokens: int = 0
    cached_input_tokens: int = 0
    cache_creation_input_tokens: int = 0
    output_tokens: int = 0
    reasoning_tokens: int = 0
    tool_calls: int = 0
    parse_errors: int = 0

    def observe_timestamp(self, value: Any) -> None:
        parsed = parse_timestamp(value)
        if not parsed:
            return
        if self.first_at is None or parsed < self.first_at:
            self.first_at = parsed
        if self.last_at is None or parsed > self.last_at:
            self.last_at = parsed

    def add_usage(self, usage: Any) -> None:
        if not isinstance(usage, dict):
            return
        values = _usage_values(usage)
        self.input_tokens += values[0]
        self.cached_input_tokens += values[1]
        self.cache_creation_input_tokens += values[2]
        self.output_tokens += values[3]
        self.reasoning_tokens += values[4]


def _row_timestamp(row: dict[str, Any], payload: Any = None) -> Any:
    value = row.get("timestamp")
    if value is None and isinstance(payload, dict):
        value = payload.get("timestamp")
    return value


def _turn_context_metadata(payload: dict[str, Any]) -> tuple[str, str]:
    """Read model/effort from rollout turn_context metadata.

    Older and newer Codex rollouts place effort under different names, and a
    few records nest the values in collaboration-mode settings.  This metadata
    is the transcript fallback when the CAS spawn row has no worker_spec.
    """

    settings = payload.get("collaboration_mode")
    if isinstance(settings, dict):
        settings = settings.get("settings")
    if not isinstance(settings, dict):
        settings = {}
    model = payload.get("model") or settings.get("model")
    effort = (
        payload.get("effort")
        or payload.get("reasoning_effort")
        or settings.get("effort")
        or settings.get("reasoning_effort")
    )
    return str(model or ""), str(effort or "")


def parse_codex_rollout(path: Path) -> Transcript:
    result = Transcript("codex", path)
    modern_usages: list[dict[str, Any]] = []
    legacy_usages: list[dict[str, Any]] = []
    try:
        stream = path.open(errors="replace")
    except OSError:
        result.parse_errors = 1
        return result
    with stream:
        for line in stream:
            try:
                row = json.loads(line)
            except (TypeError, json.JSONDecodeError):
                result.parse_errors += 1
                continue
            if not isinstance(row, dict):
                continue
            payload = row.get("payload")
            if not isinstance(payload, dict):
                payload = {}
            result.observe_timestamp(_row_timestamp(row, payload))
            result.session_id = str(payload.get("session_id") or row.get("session_id") or result.session_id)
            if row.get("type") == "session_meta":
                result.cwd = str(payload.get("cwd") or result.cwd)
            elif row.get("type") == "turn_context":
                result.cwd = str(payload.get("cwd") or result.cwd)
                model, effort = _turn_context_metadata(payload)
                result.model = model or result.model
                result.effort = effort or result.effort
            if row.get("type") == "response_item":
                if payload.get("type") in {"function_call", "custom_tool_call"}:
                    result.tool_calls += 1
            if row.get("type") == "token_usage_record":
                usage = payload.get("usage")
                if isinstance(usage, dict):
                    modern_usages.append(usage)
            elif row.get("type") == "event_msg" and payload.get("type") == "token_count":
                info = payload.get("info")
                if isinstance(info, dict) and isinstance(info.get("last_token_usage"), dict):
                    legacy_usages.append(info["last_token_usage"])
    for usage in modern_usages or legacy_usages:
        result.add_usage(usage)
    result.worker_name = worker_name_from_cwd(result.cwd)
    return result


def _content_blocks(message: dict[str, Any]) -> Iterable[dict[str, Any]]:
    content = message.get("content")
    if isinstance(content, list):
        return (block for block in content if isinstance(block, dict))
    return ()


def parse_claude_transcript(path: Path) -> Transcript:
    result = Transcript("claude", path)
    try:
        stream = path.open(errors="replace")
    except OSError:
        result.parse_errors = 1
        return result
    with stream:
        for line in stream:
            try:
                row = json.loads(line)
            except (TypeError, json.JSONDecodeError):
                result.parse_errors += 1
                continue
            if not isinstance(row, dict):
                continue
            result.observe_timestamp(row.get("timestamp"))
            result.session_id = str(row.get("sessionId") or row.get("session_id") or result.session_id)
            result.cwd = str(row.get("cwd") or result.cwd)
            message = row.get("message")
            if not isinstance(message, dict):
                continue
            result.model = str(message.get("model") or row.get("model") or result.model)
            result.effort = str(row.get("effort") or message.get("effort") or result.effort)
            if row.get("type") == "assistant" or message.get("role") == "assistant":
                result.add_usage(message.get("usage"))
                result.tool_calls += sum(1 for block in _content_blocks(message) if block.get("type") == "tool_use")
    result.worker_name = worker_name_from_cwd(result.cwd)
    return result


def discover_transcripts(codex_roots: Iterable[Path], claude_roots: Iterable[Path]) -> tuple[list[Transcript], list[dict[str, Any]]]:
    transcripts: list[Transcript] = []
    home_inventory: list[dict[str, Any]] = []
    for roots, parser in (((Path(root) for root in codex_roots), parse_codex_rollout), ((Path(root) for root in claude_roots), parse_claude_transcript)):
        for root in roots:
            if not root.exists():
                home_inventory.append({"home": str(root), "harness": "codex" if parser is parse_codex_rollout else "claude", "files": 0, "factory_files": 0})
                continue
            try:
                paths = sorted(root.rglob("*.jsonl"))
            except OSError:
                home_inventory.append({"home": str(root), "harness": "codex" if parser is parse_codex_rollout else "claude", "files": 0, "factory_files": 0})
                continue
            inventory = {
                "home": str(root),
                "harness": "codex" if parser is parse_codex_rollout else "claude",
                "files": len(paths),
                "factory_files": 0,
            }
            for path in paths:
                transcript = parser(path)
                transcript.source_home = str(root)
                # Global transcript directories include ordinary interactive
                # sessions.  Only worktree sessions are factory sessions.
                if transcript.worker_name:
                    transcripts.append(transcript)
                    inventory["factory_files"] += 1
            home_inventory.append(inventory)
    return transcripts, home_inventory


def default_harness_roots(root: Path) -> tuple[list[Path], list[Path]]:
    """Return every configured Codex/Claude home under the host root."""

    codex = sorted(path for path in root.glob(".codex*/sessions") if path.is_dir())
    claude = sorted(path for path in root.glob(".claude*/projects") if path.is_dir())
    return codex, claude


def _table_columns(connection: sqlite3.Connection, table: str) -> set[str]:
    try:
        return {row[1] for row in connection.execute(f'PRAGMA table_info("{table}")')}
    except sqlite3.DatabaseError:
        return set()


def _rows(connection: sqlite3.Connection, table: str, columns: list[str], where: str = "", params: tuple[Any, ...] = ()) -> list[dict[str, Any]]:
    available = _table_columns(connection, table)
    selected = [column for column in columns if column in available]
    if not selected:
        return []
    query = f'SELECT {", ".join(selected)} FROM "{table}"{where}'
    try:
        cursor = connection.execute(query, params)
    except sqlite3.DatabaseError:
        return []
    return [dict(zip(selected, row)) for row in cursor.fetchall()]


def _json_object(value: Any) -> dict[str, Any]:
    if not isinstance(value, str):
        return value if isinstance(value, dict) else {}
    try:
        parsed = json.loads(value)
    except (TypeError, json.JSONDecodeError):
        return {}
    return parsed if isinstance(parsed, dict) else {}


def _split_worker_names(value: Any) -> list[str]:
    if not isinstance(value, str):
        return []
    text = value.strip().strip("[]")
    names = []
    for part in text.split(","):
        part = part.strip().strip("'\"")
        if part:
            names.append(part)
    return names


def _agent_maps(agent_rows: list[dict[str, Any]]) -> tuple[dict[str, dict[str, Any]], dict[str, dict[str, Any]]]:
    by_key: dict[str, dict[str, Any]] = {}
    by_name: dict[str, dict[str, Any]] = {}
    for row in agent_rows:
        name = str(row.get("name") or "")
        if name:
            by_name[name] = row
            by_key[name] = row
        identifier = str(row.get("id") or "")
        if identifier:
            by_key[identifier] = row
        cc_session_id = str(row.get("cc_session_id") or "")
        if cc_session_id:
            by_key[cc_session_id] = row
    return by_key, by_name


def _note_counts(text: str) -> tuple[int, int, int]:
    return (
        len(SEND_BACK_RE.findall(text or "")),
        len(URGENT_RE.findall(text or "")),
        len(MERGE_REQUIRED_RE.findall(text or "")),
    )


def _log_pushes(log_root: Path) -> tuple[dict[tuple[str, str], datetime], dict[str, datetime], dict[str, Any]]:
    by_worker_task: dict[tuple[str, str], datetime] = {}
    by_worker: dict[str, datetime] = {}
    stats: dict[str, Any] = {"files": 0, "lines": 0, "json_lines": 0, "parse_errors": 0}
    if not log_root.exists():
        return by_worker_task, by_worker, stats
    try:
        paths = sorted(log_root.glob("*.log"))
    except OSError:
        paths = []
    for path in paths:
        stats["files"] += 1
        try:
            stream = path.open(errors="replace")
        except OSError:
            stats["parse_errors"] += 1
            continue
        with stream:
            for line in stream:
                stats["lines"] += 1
                try:
                    record = json.loads(line)
                except (TypeError, json.JSONDecodeError):
                    continue
                stats["json_lines"] += 1
                if not isinstance(record, dict):
                    continue
                stamp = parse_timestamp(record.get("timestamp"))
                if not stamp:
                    continue
                text_parts = [
                    str(record.get(key) or "")
                    for key in ("event", "summary", "reason", "message", "detail")
                ]
                body = " ".join(text_parts)
                if not PUSH_RE.search(body) or NEGATIVE_PUSH_RE.search(body):
                    continue
                workers = []
                for key in ("worker", "worker_name", "agent", "actor", "source"):
                    value = record.get(key)
                    if isinstance(value, str) and value and value not in workers and value not in {"unknown", "supervisor"}:
                        workers.append(value)
                task_id = str(record.get("task_id") or "")
                for worker in workers:
                    by_worker[worker] = min(by_worker.get(worker, stamp), stamp)
                    if task_id:
                        key = (worker, task_id)
                        by_worker_task[key] = min(by_worker_task.get(key, stamp), stamp)
    return by_worker_task, by_worker, stats


def _open_read_only(db_path: Path) -> sqlite3.Connection:
    uri = f"file:{db_path}?mode=ro"
    connection = sqlite3.connect(uri, uri=True)
    connection.execute("PRAGMA query_only=ON")
    return connection


def extract_database(db_path: Path) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    """Extract one DB and its sibling factory logs without mutating either."""

    project_path = project_root_for_db(db_path)
    project = project_name(project_path, project_path.parent)
    source: dict[str, Any] = {
        "source": "database",
        "database": str(db_path),
        "project": project,
        "project_path": str(project_path),
        "spawn_rows": 0,
        "lease_rows": 0,
        "session_rows": 0,
        "task_rows": 0,
        "task_notes_storage": "unavailable",
        "unavailable": [],
    }
    try:
        connection = _open_read_only(db_path)
    except (OSError, sqlite3.DatabaseError) as error:
        source["error"] = f"{type(error).__name__}: {error}"
        return [], source
    try:
        agent_rows = _rows(
            connection,
            "agents",
            ["id", "name", "cc_session_id", "registered_at", "metadata", "factory_session", "branch", "worktree_id"],
        )
        by_agent_key, by_agent_name = _agent_maps(agent_rows)
        spawn_rows = _rows(
            connection,
            "spawn_queue",
            ["id", "action", "worker_names", "worker_spec", "factory_session", "task_id", "spawn_worker", "created_at"],
            " WHERE action = 'spawn' ORDER BY id",
        )
        source["spawn_rows"] = len(spawn_rows)
        task_rows = _rows(
            connection,
            "tasks",
            ["id", "title", "status", "notes", "created_at", "closed_at", "close_reason"],
        )
        source["task_rows"] = len(task_rows)
        tasks = {str(row.get("id")): row for row in task_rows if row.get("id") is not None}
        if _table_columns(connection, "tasks") and "notes" in _table_columns(connection, "tasks"):
            source["task_notes_storage"] = "tasks.notes"
        else:
            source["unavailable"].append("full task notes (tasks.notes column absent)")
        # Only note events are needed for the fallback.  Avoid materializing
        # millions of unrelated lifecycle/indexing events from large stores.
        events = _rows(
            connection,
            "events",
            ["event_type", "entity_id", "summary", "created_at"],
            " WHERE event_type = 'task_note_added'",
        )
        event_notes: dict[str, list[str]] = defaultdict(list)
        for event in events:
            if event.get("event_type") == "task_note_added" and event.get("entity_id"):
                event_notes[str(event["entity_id"])].append(str(event.get("summary") or ""))
        lease_rows = _rows(
            connection,
            "task_lease_history",
            ["task_id", "agent_id", "event_type", "timestamp", "reason", "details"],
        )
        source["lease_rows"] = len(lease_rows)
        logs_by_worker_task, logs_by_worker, log_stats = _log_pushes(project_path / ".cas" / "logs")
        source["factory_logs"] = log_stats
        if not log_stats["files"]:
            source["unavailable"].append("exact first-push timestamps (no .cas/logs/*.log files)")

        leases_by_worker: dict[str, list[dict[str, Any]]] = defaultdict(list)
        for lease in lease_rows:
            key = str(lease.get("agent_id") or "")
            agent = by_agent_key.get(key)
            worker = str((agent or {}).get("name") or key)
            leases_by_worker[worker].append(lease)

        spawn_records: list[dict[str, Any]] = []
        for spawn in spawn_rows:
            spec = _json_object(spawn.get("worker_spec"))
            workers = []
            for candidate in (
                str(spawn.get("spawn_worker") or ""),
                str(spec.get("name") or ""),
            ):
                if candidate and candidate not in workers:
                    workers.append(candidate)
            if not workers:
                workers.extend(_split_worker_names(spawn.get("worker_names")))
            if not workers:
                source["unassigned_spawn_rows"] = source.get("unassigned_spawn_rows", 0) + 1
                continue
            for worker in workers:
                agent = by_agent_name.get(worker, {})
                metadata = _json_object(agent.get("metadata"))
                spawn_records.append(
                    {
                        "id": spawn.get("id", ""),
                        "worker_name": worker,
                        "factory_session": spawn.get("factory_session") or agent.get("factory_session") or "",
                        "task_id": str(spawn.get("task_id") or ""),
                        "spawn_at": timestamp_text(spawn.get("created_at")),
                        "spawn_dt": parse_timestamp(spawn.get("created_at")),
                        "cli": str(spec.get("cli") or metadata.get("worker_cli") or ""),
                        "model": str(spec.get("model") or metadata.get("worker_model") or ""),
                        "effort": str(spec.get("effort") or metadata.get("worker_effort") or ""),
                    }
                )

        known_workers = {record["worker_name"] for record in spawn_records}
        for worker, leases in leases_by_worker.items():
            if worker in known_workers:
                continue
            agent = by_agent_name.get(worker, {})
            metadata = _json_object(agent.get("metadata"))
            spawn_records.append(
                {
                    "id": "",
                    "worker_name": worker,
                    "factory_session": agent.get("factory_session") or "",
                    "task_id": "",
                    "spawn_at": timestamp_text(agent.get("registered_at")),
                    "spawn_dt": parse_timestamp(agent.get("registered_at")),
                    "cli": str(metadata.get("worker_cli") or ""),
                    "model": str(metadata.get("worker_model") or ""),
                    "effort": str(metadata.get("worker_effort") or ""),
                }
            )
        source["session_rows"] = len(spawn_records)

        output: list[dict[str, Any]] = []
        for spawn in spawn_records:
            worker = spawn["worker_name"]
            worker_leases = leases_by_worker.get(worker, [])
            task_ids = {str(lease.get("task_id")) for lease in worker_leases if lease.get("task_id")}
            if spawn["task_id"]:
                task_ids.add(spawn["task_id"])
            if not task_ids:
                task_ids = {""}
            for task_id in sorted(task_ids):
                task = tasks.get(task_id, {})
                task_leases = [lease for lease in worker_leases if str(lease.get("task_id") or "") == task_id]
                claims = [parse_timestamp(lease.get("timestamp")) for lease in task_leases if lease.get("event_type") == "claimed"]
                terminals = [
                    lease
                    for lease in task_leases
                    if lease.get("event_type") in {"released", "revoked", "expired"}
                ]
                terminal_dates = [parse_timestamp(lease.get("timestamp")) for lease in terminals]
                note_text = str(task.get("notes") or "") or "\n".join(event_notes.get(task_id, []))
                send_backs, urgent_stops, merge_required = _note_counts(note_text)
                first_push = logs_by_worker_task.get((worker, task_id)) or logs_by_worker.get(worker)
                output.append(
                    {
                        "project": project,
                        "project_path": str(project_path),
                        "worker_name": worker,
                        "session_id": "",
                        "harness": "",
                        "source_db": str(db_path),
                        "source_transcript": "",
                        "source_factory_logs": str(project_path / ".cas" / "logs"),
                        "factory_session": spawn["factory_session"],
                        "spawn_id": integer_text(spawn["id"]),
                        "spawn_at": spawn["spawn_at"],
                        "cli": spawn["cli"],
                        "model": spawn["model"],
                        "effort": spawn["effort"],
                        "transcript_join_key": worker,
                        "transcript_joined": "no",
                        "session_first_at": "",
                        "session_last_at": "",
                        "task_id": task_id,
                        "task_title": str(task.get("title") or ""),
                        "task_status": str(task.get("status") or ""),
                        "task_closed": "yes" if task.get("status") == "closed" else "no",
                        "task_created_at": timestamp_text(task.get("created_at")),
                        "task_closed_at": timestamp_text(task.get("closed_at")),
                        "lease_acquired_at": timestamp_text(min(claims)) if claims else "",
                        "lease_released_at": timestamp_text(max(terminal_dates)) if terminal_dates else "",
                        "lease_terminal_event": str(terminals[-1].get("event_type") or "") if terminals else "",
                        "send_back_count": send_backs,
                        "urgent_stop_count": urgent_stops,
                        "merge_required_count": merge_required,
                        "close_reason": str(task.get("close_reason") or ""),
                        "first_push_at": timestamp_text(first_push),
                        "minutes_to_first_push": "",
                        "input_tokens": 0,
                        "cached_input_tokens": 0,
                        "cache_creation_input_tokens": 0,
                        "output_tokens": 0,
                        "reasoning_tokens": 0,
                        "tool_calls": 0,
                        "cost_usd": "",
                    }
                )
        return output, source
    finally:
        connection.close()


def extract_databases(db_paths: Iterable[Path]) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    rows: list[dict[str, Any]] = []
    sources: list[dict[str, Any]] = []
    for db_path in db_paths:
        extracted, source = extract_database(db_path)
        rows.extend(extracted)
        sources.append(source)
    return rows, sources


def _transcript_project(transcript: Transcript, root: Path) -> str:
    path = project_path_from_cwd(transcript.cwd)
    if path is None:
        return ""
    return project_name(path, root)


def attach_transcripts(rows: list[dict[str, Any]], transcripts: list[Transcript], root: Path, home_inventory: Optional[list[dict[str, Any]]] = None) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    grouped_rows: dict[tuple[str, str], list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        grouped_rows[(row["project"], row["worker_name"])].append(row)
    grouped_transcripts: dict[tuple[str, str], list[Transcript]] = defaultdict(list)
    for transcript in transcripts:
        grouped_transcripts[(_transcript_project(transcript, root), transcript.worker_name)].append(transcript)
    matched: set[str] = set()
    join = {
        "transcript_files": len(transcripts),
        "transcript_jsonl_files": sum(int(home.get("files", 0)) for home in (home_inventory or [])),
        "transcript_joined": 0,
        "transcript_only": 0,
        "db_rows_without_transcript": 0,
        "join_key": "project + worker_name from transcript cwd `.cas/worktrees/<worker>`",
        "transcript_homes": [],
    }
    home_stats: dict[str, dict[str, Any]] = defaultdict(lambda: {"files": 0, "factory_files": 0, "joined": 0, "transcript_only": 0})
    for inventory in home_inventory or []:
        home_stats[inventory["home"]].update(
            {
                "harness": inventory.get("harness", "unknown"),
                "files": int(inventory.get("files", 0)),
                "factory_files": int(inventory.get("factory_files", 0)),
            }
        )
    for transcript in transcripts:
        home_stats[transcript.source_home].setdefault("factory_files", 0)
    for key, transcript_group in grouped_transcripts.items():
        target_rows = grouped_rows.get(key, [])
        for transcript in transcript_group:
            if target_rows:
                spawn_times = [parse_timestamp(row.get("spawn_at")) for row in target_rows]
                spawn_times = [stamp for stamp in spawn_times if stamp]
                if spawn_times and transcript.first_at:
                    # A worker name is normally unique.  If a historical store
                    # reused it, nearest spawn time is the only bounded join.
                    _ = min(spawn_times, key=lambda stamp: abs((stamp - transcript.first_at).total_seconds()))
                for row in target_rows:
                    row["session_id"] = transcript.session_id or f"{transcript.harness}:{transcript.path.stem}"
                    row["harness"] = transcript.harness
                    row["source_transcript"] = str(transcript.path)
                    row["transcript_joined"] = "yes"
                    row["session_first_at"] = timestamp_text(transcript.first_at)
                    row["session_last_at"] = timestamp_text(transcript.last_at)
                    row["model"] = transcript.model or row["model"]
                    row["effort"] = transcript.effort or row["effort"]
                    row["input_tokens"] = transcript.input_tokens
                    row["cached_input_tokens"] = transcript.cached_input_tokens
                    row["cache_creation_input_tokens"] = transcript.cache_creation_input_tokens
                    row["output_tokens"] = transcript.output_tokens
                    row["reasoning_tokens"] = transcript.reasoning_tokens
                    row["tool_calls"] = transcript.tool_calls
                matched.add(str(transcript.path))
                join["transcript_joined"] += 1
                home_stats[transcript.source_home]["joined"] += 1
            else:
                project_path = project_path_from_cwd(transcript.cwd)
                row = {field: "" for field in SESSION_FIELDS}
                row.update(
                    {
                        "project": key[0],
                        "project_path": str(project_path or ""),
                        "worker_name": transcript.worker_name,
                        "session_id": transcript.session_id or f"{transcript.harness}:{transcript.path.stem}",
                        "harness": transcript.harness,
                        "source_transcript": str(transcript.path),
                        "transcript_join_key": transcript.worker_name,
                        "transcript_joined": "no",
                        "session_first_at": timestamp_text(transcript.first_at),
                        "session_last_at": timestamp_text(transcript.last_at),
                        "model": transcript.model,
                        "effort": transcript.effort,
                        "input_tokens": transcript.input_tokens,
                        "cached_input_tokens": transcript.cached_input_tokens,
                        "cache_creation_input_tokens": transcript.cache_creation_input_tokens,
                        "output_tokens": transcript.output_tokens,
                        "reasoning_tokens": transcript.reasoning_tokens,
                        "tool_calls": transcript.tool_calls,
                    }
                )
                rows.append(row)
                matched.add(str(transcript.path))
                join["transcript_only"] += 1
                home_stats[transcript.source_home]["transcript_only"] += 1
    for row in rows:
        if not row["session_id"]:
            row["session_id"] = f"db:{row['factory_session'] or 'unknown'}:{row['worker_name']}"
            row["harness"] = row["cli"] or "db"
            row["session_first_at"] = row["spawn_at"]
            join["db_rows_without_transcript"] += 1
    join["transcript_join_miss_rate"] = round(join["transcript_only"] / max(1, len(transcripts)) * 100, 2)
    join["db_row_join_miss_rate"] = round(join["db_rows_without_transcript"] / max(1, len(rows)) * 100, 2)
    join["transcript_homes"] = [
        {
            "home": home,
            **stats,
            "miss_rate": round(stats["transcript_only"] / max(1, stats.get("factory_files", 0)) * 100, 2),
        }
        for home, stats in sorted(home_stats.items())
    ]
    return rows, join


def _load_price_entry(prices: dict[str, Any], model: str) -> dict[str, Any]:
    models = prices.get("models") if isinstance(prices.get("models"), dict) else prices
    entry = models.get(model, {}) if isinstance(models, dict) else {}
    return entry if isinstance(entry, dict) else {}


def _price(entry: dict[str, Any], *keys: str) -> Optional[float]:
    for key in keys:
        value = entry.get(key)
        if isinstance(value, (int, float)) and math.isfinite(float(value)):
            return float(value)
    return None


def load_prices(path: Path) -> tuple[dict[str, Any], str]:
    if not path.exists():
        return {}, "missing"
    try:
        parsed = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        return {}, f"error: {type(error).__name__}: {error}"
    return (parsed if isinstance(parsed, dict) else {}), "loaded"


def apply_costs(rows: list[dict[str, Any]], prices: dict[str, Any]) -> None:
    for row in rows:
        entry = _load_price_entry(prices, str(row.get("model") or ""))
        input_rate = _price(entry, "input_per_million", "input", "input_usd_per_million")
        cached_rate = _price(entry, "cached_input_per_million", "cache_read_per_million", "cached", "cache_read")
        creation_rate = _price(entry, "cache_creation_per_million", "cache_write_per_million", "cache_creation")
        output_rate = _price(entry, "output_per_million", "output", "output_usd_per_million")
        reasoning_rate = _price(entry, "reasoning_per_million", "reasoning")
        components = []
        # Codex rollouts report cached_input_tokens as a subset of input_tokens
        # (token_count: total_tokens == input_tokens + output_tokens); Claude
        # transcripts report input_tokens net of cache reads. Bill the uncached
        # remainder at the input rate so cached tokens are not charged twice.
        billable_input = row["input_tokens"]
        if str(row.get("harness") or "") == "codex":
            billable_input = max(0, row["input_tokens"] - row["cached_input_tokens"])
        if input_rate is not None:
            components.append(billable_input * input_rate)
        if cached_rate is not None:
            components.append(row["cached_input_tokens"] * cached_rate)
        if creation_rate is not None:
            components.append(row["cache_creation_input_tokens"] * creation_rate)
        if output_rate is not None:
            components.append(row["output_tokens"] * output_rate)
        if reasoning_rate is not None:
            components.append(row["reasoning_tokens"] * reasoning_rate)
        if components:
            row["cost_usd"] = f"{sum(components) / 1_000_000:.6f}"


def compute_push_minutes(rows: list[dict[str, Any]]) -> None:
    for row in rows:
        first = parse_timestamp(row.get("session_first_at"))
        pushed = parse_timestamp(row.get("first_push_at"))
        if first and pushed:
            minutes = (pushed - first).total_seconds() / 60
            if minutes >= 0:
                row["minutes_to_first_push"] = f"{minutes:.2f}"


def _float(value: Any) -> Optional[float]:
    try:
        parsed = float(value)
    except (TypeError, ValueError):
        return None
    return parsed if math.isfinite(parsed) else None


def _median(values: Iterable[float]) -> str:
    values = list(values)
    return f"{statistics.median(values):.2f}" if values else ""


def _unique_session_task_rows(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    unique: dict[tuple[str, str], dict[str, Any]] = {}
    for row in rows:
        key = (str(row.get("session_id") or ""), str(row.get("task_id") or ""))
        unique.setdefault(key, row)
    return list(unique.values())


def scorecard(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    unique_rows = _unique_session_task_rows(rows)
    groups: dict[tuple[str, str, str], list[dict[str, Any]]] = defaultdict(list)
    for row in unique_rows:
        groups[(str(row.get("project") or ""), str(row.get("model") or "unknown"), str(row.get("effort") or "unknown"))].append(row)
    grouped: list[tuple[str, str, str, list[dict[str, Any]]]] = [
        (project, model, effort, members) for (project, model, effort), members in sorted(groups.items())
    ]
    grouped.extend(
        ("", model, effort, [row for row in unique_rows if str(row.get("model") or "unknown") == model and str(row.get("effort") or "unknown") == effort])
        for model, effort in sorted({(str(row.get("model") or "unknown"), str(row.get("effort") or "unknown")) for row in unique_rows})
    )
    output: list[dict[str, Any]] = []
    for project, model, effort, members in grouped:
        delivered = [row for row in members if row.get("task_id") and row.get("task_status") == "closed"]
        delivered_by_session = defaultdict(int)
        for row in delivered:
            delivered_by_session[row["session_id"]] += 1
        task_counts: dict[str, tuple[int, int]] = {}
        for row in members:
            task_id = str(row.get("task_id") or "")
            if task_id:
                current = task_counts.get(task_id, (0, 0))
                task_counts[task_id] = (
                    max(current[0], number(row.get("send_back_count"))),
                    max(current[1], number(row.get("urgent_stop_count"))),
                )
        send_backs = sum(pair[0] for pair in task_counts.values())
        urgent_stops = sum(pair[1] for pair in task_counts.values())
        minutes = [_float(row.get("minutes_to_first_push")) for row in delivered]
        minutes = [value for value in minutes if value is not None]
        input_per_task = [number(row.get("input_tokens")) / delivered_by_session[row["session_id"]] for row in delivered if delivered_by_session[row["session_id"]]]
        cached_per_task = [number(row.get("cached_input_tokens")) / delivered_by_session[row["session_id"]] for row in delivered if delivered_by_session[row["session_id"]]]
        output_per_task = [number(row.get("output_tokens")) / delivered_by_session[row["session_id"]] for row in delivered if delivered_by_session[row["session_id"]]]
        tools_per_task = [number(row.get("tool_calls")) / delivered_by_session[row["session_id"]] for row in delivered if delivered_by_session[row["session_id"]]]
        session_costs: dict[str, float] = {}
        for row in members:
            cost = _float(row.get("cost_usd"))
            if cost is not None:
                session_costs.setdefault(row["session_id"], cost)
        output.append(
            {
                "scope": "overall" if not project else "project",
                "project": project,
                "model": model,
                "effort": effort,
                "sessions": len({row["session_id"] for row in members}),
                "tasks_delivered": len({row["task_id"] for row in delivered}),
                "send_backs": send_backs,
                "send_back_rate": f"{send_backs / max(1, len({row['task_id'] for row in delivered})):.4f}",
                "urgent_stops": urgent_stops,
                "median_minutes_to_first_push": _median(minutes),
                "median_input_tokens_per_delivered_task": _median(input_per_task),
                "median_cached_input_tokens_per_delivered_task": _median(cached_per_task),
                "median_output_tokens_per_delivered_task": _median(output_per_task),
                "median_tool_calls_per_delivered_task": _median(tools_per_task),
                "cost_usd": f"{sum(session_costs.values()):.6f}" if session_costs else "",
            }
        )
    return output


def _source_summary(sources: list[dict[str, Any]], join: dict[str, Any], prices_status: str) -> list[str]:
    lines = [
        "## Source coverage and join evidence",
        "",
        "- Database → transcript key: `project + worker_name`; worker name comes from `spawn_queue.spawn_worker` / `agents.name` and transcript `cwd` segment `.cas/worktrees/<worker>`. This is a bounded name join, not an inferred task join.",
        f"- Transcript JSONL files scanned: {join.get('transcript_jsonl_files', 0)}; factory transcript files parsed: {join.get('transcript_files', 0)}; joined to DB worker rows: {join.get('transcript_joined', 0)}; transcript-only sessions: {join.get('transcript_only', 0)}; transcript→DB miss rate: {join.get('transcript_join_miss_rate', 0):.2f}%.",
        f"- DB session rows without a transcript match: {join.get('db_rows_without_transcript', 0)} ({join.get('db_row_join_miss_rate', 0):.2f}% of emitted rows).",
        f"- Prices JSON: `{prices_status}`. Blank `cost_usd` means the model has no supplied price entry; no price was inferred.",
        "",
    ]
    lines.append("### Transcript homes")
    lines.append("")
    for home in join.get("transcript_homes", []):
        lines.append(
            f"- `{home['home']}` ({home.get('harness', 'unknown')}): {home['files']} JSONL files, {home.get('factory_files', 0)} factory files, {home['joined']} DB joins, {home['transcript_only']} transcript-only; home miss rate {home['miss_rate']:.2f}% of factory files.")
    if not join.get("transcript_homes"):
        lines.append("- No configured Codex/Claude transcript homes were present or readable.")
    lines.append("")
    for source in sources:
        unavailable = list(source.get("unavailable") or [])
        if source.get("error"):
            unavailable.append(source["error"])
        logs = source.get("factory_logs") or {}
        lines.append(
            f"- `{source.get('project')}` DB `{source.get('database')}`: {source.get('spawn_rows', 0)} spawn rows ({source.get('unassigned_spawn_rows', 0)} without a resolvable worker name skipped), {source.get('lease_rows', 0)} lease events, {source.get('task_rows', 0)} tasks; task notes source `{source.get('task_notes_storage', 'unavailable')}`; factory logs `{logs.get('files', 0)}` files/{logs.get('json_lines', 0)} JSON lines."
        )
        if unavailable:
            lines.append(f"  Unavailable: {'; '.join(unavailable)}.")
    lines.extend(
        [
            "",
            "### Field policy",
            "",
            "- `task_notes` is not a table in the observed stores. Counts use the full `tasks.notes` field when present, falling back to `events.summary` only for `task_note_added` events; no note text is reconstructed beyond those stores.",
            "- Exact first-push time is populated only when a JSON factory log record names the worker (and task when available) and contains a positive `pushed` marker. Missing markers remain blank; commit time is not substituted.",
            "- Codex usage sums modern `token_usage_record.payload.usage` records or legacy `event_msg.payload.info.last_token_usage` records (modern records win if both exist); Claude usage sums assistant `message.usage`. Cache-read and cache-creation are retained separately, and reasoning comes only from an explicit usage field.",
            "- Transcript task IDs are unavailable in the transcript sources, so task attribution comes only from DB spawn/lease rows. Transcript-only rows intentionally have a blank task ID.",
            "",
            "### CSV columns",
            "",
            "Session CSV (`factory-model-history-YYYY-MM-DD.csv`):",
            "",
        ]
    )
    lines.extend(f"- `{field}`: {CSV_COLUMN_DESCRIPTIONS[field]}." for field in SESSION_FIELDS)
    lines.extend(["", "Scorecard CSV (`factory-model-scorecard-YYYY-MM-DD.csv`):", ""])
    lines.extend(f"- `{field}`: {SCORECARD_COLUMN_DESCRIPTIONS[field]}." for field in SCORECARD_FIELDS)
    return lines


def write_csv(path: Path, rows: list[dict[str, Any]], fields: list[str]) -> None:
    def clean(value: Any) -> Any:
        if not isinstance(value, str):
            return value
        return "\n".join(line.rstrip() for line in value.strip().splitlines())

    with path.open("w", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=fields, extrasaction="ignore", lineterminator="\n")
        writer.writeheader()
        for row in rows:
            cleaned = {key: clean(value) for key, value in row.items()}
            writer.writerow(cleaned)


def write_scorecard_markdown(path: Path, score_rows: list[dict[str, Any]], source_lines: list[str], date_text: str) -> None:
    lines = [
        f"# Factory model scorecard — {date_text}",
        "",
        "Generated by `scripts/factory-model-history.py`. Values are local historical observations; blank values are unavailable, not zero.",
        "",
        "## Scorecard",
        "",
        "| " + " | ".join(SCORECARD_FIELDS) + " |",
        "| " + " | ".join("---" for _ in SCORECARD_FIELDS) + " |",
    ]
    for row in score_rows:
        lines.append("| " + " | ".join(str(row.get(field, "")).replace("|", "\\|") for field in SCORECARD_FIELDS) + " |")
    lines.extend(["", *source_lines, ""])
    path.write_text("\n".join(lines))


def run(args: argparse.Namespace) -> tuple[list[dict[str, Any]], list[dict[str, Any]], dict[str, Any]]:
    root = Path(args.root).expanduser().resolve()
    output_dir = Path(args.output_dir).expanduser().resolve()
    db_paths = discover_database_paths(root)
    rows, sources = extract_databases(db_paths)
    default_codex, default_claude = default_harness_roots(root)
    codex_roots = [Path(path).expanduser().resolve() for path in (args.codex_roots or default_codex)]
    claude_roots = [Path(path).expanduser().resolve() for path in (args.claude_roots or default_claude)]
    transcripts, home_inventory = discover_transcripts(codex_roots, claude_roots)
    rows, join = attach_transcripts(rows, transcripts, root, home_inventory)
    prices, prices_status = load_prices(Path(args.prices).expanduser().resolve())
    apply_costs(rows, prices)
    compute_push_minutes(rows)
    score_rows = scorecard(rows)
    output_dir.mkdir(parents=True, exist_ok=True)
    date_text = args.date or datetime.now(UTC).date().isoformat()
    session_path = output_dir / f"factory-model-history-{date_text}.csv"
    score_path = output_dir / f"factory-model-scorecard-{date_text}.csv"
    markdown_path = output_dir / f"factory-model-scorecard-{date_text}.md"
    sources_path = output_dir / f"factory-model-history-{date_text}-sources.md"
    write_csv(session_path, rows, SESSION_FIELDS)
    write_csv(score_path, score_rows, SCORECARD_FIELDS)
    source_lines = _source_summary(sources, join, prices_status)
    write_scorecard_markdown(markdown_path, score_rows, source_lines, date_text)
    sources_path.write_text("\n".join([f"# Factory model history sources — {date_text}", "", *source_lines, ""]))
    summary = {
        "session_csv": str(session_path),
        "scorecard_csv": str(score_path),
        "scorecard_markdown": str(markdown_path),
        "sources_markdown": str(sources_path),
        "databases": len(db_paths),
        "transcripts": len(transcripts),
        "session_rows": len(rows),
        "scorecard_rows": len(score_rows),
        "join": join,
        "prices": prices_status,
        "codex_roots": [str(path) for path in codex_roots],
        "claude_roots": [str(path) for path in claude_roots],
    }
    return rows, score_rows, summary


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", default="/home/pippenz", help="Root to walk for project .cas/cas.db stores")
    parser.add_argument("--codex-root", dest="codex_roots", action="append", help="One Codex sessions root; repeat for multiple harness homes (default: ~/.codex*/sessions)")
    parser.add_argument("--claude-root", dest="claude_roots", action="append", help="One Claude projects root; repeat for multiple harness homes (default: ~/.claude*/projects)")
    # Retain the first implementation's explicit names as aliases so existing
    # fixture/research invocations remain reproducible.
    parser.add_argument("--codex-sessions", dest="codex_roots", action="append", help=argparse.SUPPRESS)
    parser.add_argument("--claude-projects", dest="claude_roots", action="append", help=argparse.SUPPRESS)
    parser.add_argument("--output-dir", default=str(Path(__file__).resolve().parents[1] / "docs/factory/data"), help="Generated output directory")
    parser.add_argument("--prices", default=str(Path(__file__).resolve().parents[1] / "docs/factory/data/model-prices.json"), help="Model prices JSON")
    parser.add_argument("--date", default="", help="Output date (UTC), default today")
    return parser


def main(argv: Optional[list[str]] = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        _, _, summary = run(args)
    except (OSError, sqlite3.DatabaseError) as error:
        print(f"factory model history failed: {type(error).__name__}: {error}", file=sys.stderr)
        return 1
    print(json.dumps(summary, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
