#!/usr/bin/env python3
"""Fixture tests for scripts/factory-model-history.py."""

import importlib.util
import json
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("factory-model-history.py")
SPEC = importlib.util.spec_from_file_location("factory_model_history", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class FactoryModelHistoryFixtures(unittest.TestCase):
    def test_codex_rollout_sums_usage_and_tool_calls(self):
        rows = [
            {
                "timestamp": "2026-09-01T10:00:00.000Z",
                "type": "session_meta",
                "payload": {
                    "session_id": "codex-session-1",
                    "cwd": "/tmp/project/.cas/worktrees/worker-1",
                },
            },
            {
                "timestamp": "2026-09-01T10:01:00.000Z",
                "type": "turn_context",
                "payload": {
                    "model": "gpt-fixture",
                    "effort": "high",
                    "cwd": "/tmp/project/.cas/worktrees/worker-1",
                },
            },
            {
                "timestamp": "2026-09-01T10:02:00.000Z",
                "type": "response_item",
                "payload": {"type": "function_call", "name": "exec"},
            },
            {
                "timestamp": "2026-09-01T10:03:00.000Z",
                "type": "token_usage_record",
                "payload": {
                    "usage": {
                        "input_tokens": 10,
                        "cached_input_tokens": 4,
                        "output_tokens": 5,
                        "reasoning_output_tokens": 2,
                    }
                },
            },
            {
                "timestamp": "2026-09-01T10:04:00.000Z",
                "type": "response_item",
                "payload": {"type": "function_call", "name": "grep"},
            },
            {
                "timestamp": "2026-09-01T10:05:00.000Z",
                "type": "token_usage_record",
                "payload": {
                    "usage": {
                        "input_tokens": 3,
                        "cached_input_tokens": 1,
                        "output_tokens": 7,
                        "reasoning_output_tokens": 1,
                    }
                },
            },
        ]
        with tempfile.NamedTemporaryFile("w", suffix=".jsonl") as stream:
            for row in rows:
                stream.write(json.dumps(row) + "\n")
            stream.flush()
            result = MODULE.parse_codex_rollout(Path(stream.name))
        self.assertEqual(result.session_id, "codex-session-1")
        self.assertEqual(result.worker_name, "worker-1")
        self.assertEqual(result.model, "gpt-fixture")
        self.assertEqual(result.effort, "high")
        self.assertEqual(result.input_tokens, 13)
        self.assertEqual(result.cached_input_tokens, 5)
        self.assertEqual(result.output_tokens, 12)
        self.assertEqual(result.reasoning_tokens, 3)
        self.assertEqual(result.tool_calls, 2)

    def test_claude_transcript_reads_usage_tools_and_metadata(self):
        rows = [
            {
                "type": "assistant",
                "timestamp": "2026-09-02T11:00:00.000Z",
                "cwd": "/tmp/project/.cas/worktrees/worker-2",
                "sessionId": "claude-session-2",
                "effort": "medium",
                "message": {
                    "model": "claude-fixture",
                    "usage": {
                        "input_tokens": 20,
                        "cache_read_input_tokens": 8,
                        "cache_creation_input_tokens": 2,
                        "output_tokens": 6,
                        "output_tokens_details": {"thinking_tokens": 3},
                    },
                    "content": [{"type": "tool_use", "name": "Bash"}],
                },
            },
            {
                "type": "assistant",
                "timestamp": "2026-09-02T11:05:00.000Z",
                "cwd": "/tmp/project/.cas/worktrees/worker-2",
                "sessionId": "claude-session-2",
                "effort": "medium",
                "message": {
                    "model": "claude-fixture",
                    "usage": {
                        "input_tokens": 4,
                        "cache_read_input_tokens": 1,
                        "cache_creation_input_tokens": 0,
                        "output_tokens": 2,
                    },
                    "content": [{"type": "text", "text": "done"}],
                },
            },
        ]
        with tempfile.NamedTemporaryFile("w", suffix=".jsonl") as stream:
            for row in rows:
                stream.write(json.dumps(row) + "\n")
            stream.flush()
            result = MODULE.parse_claude_transcript(Path(stream.name))
        self.assertEqual(result.session_id, "claude-session-2")
        self.assertEqual(result.worker_name, "worker-2")
        self.assertEqual(result.model, "claude-fixture")
        self.assertEqual(result.effort, "medium")
        self.assertEqual(result.input_tokens, 24)
        self.assertEqual(result.cached_input_tokens, 9)
        self.assertEqual(result.output_tokens, 8)
        self.assertEqual(result.reasoning_tokens, 3)
        self.assertEqual(result.tool_calls, 1)

    def test_database_joins_lease_notes_and_factory_log_push(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "project"
            db_path = root / ".cas" / "cas.db"
            db_path.parent.mkdir(parents=True)
            db = sqlite3.connect(db_path)
            db.executescript(
                """
                CREATE TABLE spawn_queue (
                    id INTEGER PRIMARY KEY, action TEXT, worker_names TEXT,
                    worker_spec TEXT, factory_session TEXT, task_id TEXT,
                    spawn_worker TEXT, created_at TEXT
                );
                CREATE TABLE agents (
                    id TEXT, name TEXT, metadata TEXT, factory_session TEXT,
                    registered_at TEXT
                );
                CREATE TABLE tasks (
                    id TEXT, title TEXT, status TEXT, notes TEXT,
                    created_at TEXT, closed_at TEXT, close_reason TEXT
                );
                CREATE TABLE task_lease_history (
                    id INTEGER PRIMARY KEY, task_id TEXT, agent_id TEXT,
                    event_type TEXT, timestamp TEXT, reason TEXT, details TEXT
                );
                CREATE TABLE events (
                    id INTEGER PRIMARY KEY, event_type TEXT, entity_id TEXT,
                    summary TEXT, created_at TEXT, session_id TEXT
                );
                """
            )
            db.execute(
                "INSERT INTO spawn_queue VALUES (1, 'spawn', NULL, ?, 'factory-1', 'task-1', 'worker-1', '2026-09-01T10:00:00+00:00')",
                (json.dumps({"cli": "codex", "model": "gpt-fixture", "effort": "high"}),),
            )
            db.execute(
                "INSERT INTO agents VALUES ('agent-1', 'worker-1', ?, 'factory-1', '2026-09-01T10:00:01+00:00')",
                (json.dumps({"worker_cli": "codex"}),),
            )
            db.execute(
                "INSERT INTO tasks VALUES ('task-1', 'Fixture task', 'closed', ?, '2026-09-01T10:00:00+00:00', '2026-09-01T10:30:00+00:00', 'pushed')",
                ("[2026-09-01] request_changes\nMERGE REQUIRED",),
            )
            db.executemany(
                "INSERT INTO task_lease_history(task_id, agent_id, event_type, timestamp, reason, details) VALUES (?, ?, ?, ?, ?, ?)",
                [
                    ("task-1", "agent-1", "claimed", "2026-09-01T10:01:00+00:00", None, None),
                    ("task-1", "agent-1", "released", "2026-09-01T10:30:00+00:00", "Task closed", None),
                ],
            )
            db.commit()
            db.close()
            logs = root / ".cas" / "logs"
            logs.mkdir()
            (logs / "factory-session.log").write_text(
                json.dumps(
                    {
                        "event": "coordination_message",
                        "agent": "worker-1",
                        "task_id": "task-1",
                        "summary": "task-1 pushed and ready",
                        "timestamp": "2026-09-01T10:20:00+00:00",
                    }
                )
                + "\n"
            )
            rows, source = MODULE.extract_database(db_path)
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["worker_name"], "worker-1")
        self.assertEqual(rows[0]["task_id"], "task-1")
        self.assertEqual(rows[0]["send_back_count"], 1)
        self.assertEqual(rows[0]["merge_required_count"], 1)
        self.assertEqual(rows[0]["first_push_at"], "2026-09-01T10:20:00.000Z")
        self.assertEqual(source["task_notes_storage"], "tasks.notes")

    def test_discovery_skips_worktrees_artifacts_and_epic_directories(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for relative in (
                "project/.cas/cas.db",
                "project/.cas/worktrees/worker/.cas/cas.db",
                "project/.cas/artifacts/run/.cas/cas.db",
                "project/epic-old/.cas/cas.db",
            ):
                path = root / relative
                path.parent.mkdir(parents=True)
                path.touch()
            found = MODULE.discover_database_paths(root)
        self.assertEqual(found, [root / "project" / ".cas" / "cas.db"])


if __name__ == "__main__":
    unittest.main()
