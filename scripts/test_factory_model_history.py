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

    def test_codex_legacy_token_count_uses_last_turn_usage(self):
        rows = [
            {
                "timestamp": "2026-09-01T10:00:00.000Z",
                "type": "session_meta",
                "payload": {
                    "session_id": "codex-legacy",
                    "cwd": "/tmp/project/.cas/worktrees/worker-legacy",
                },
            },
            {
                "timestamp": "2026-09-01T10:01:00.000Z",
                "type": "turn_context",
                "payload": {"model": "gpt-legacy", "effort": "xhigh"},
            },
            {
                "timestamp": "2026-09-01T10:02:00.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": 99,
                            "cached_input_tokens": 90,
                            "output_tokens": 99,
                        },
                        "last_token_usage": {
                            "input_tokens": 11,
                            "cached_input_tokens": 9,
                            "output_tokens": 7,
                            "reasoning_output_tokens": 3,
                        },
                    },
                },
            },
        ]
        with tempfile.NamedTemporaryFile("w", suffix=".jsonl") as stream:
            for row in rows:
                stream.write(json.dumps(row) + "\n")
            stream.flush()
            result = MODULE.parse_codex_rollout(Path(stream.name))
        self.assertEqual(result.input_tokens, 11)
        self.assertEqual(result.cached_input_tokens, 9)
        self.assertEqual(result.output_tokens, 7)
        self.assertEqual(result.reasoning_tokens, 3)

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
                "INSERT INTO spawn_queue VALUES (2, 'spawn', NULL, ?, 'factory-1', NULL, NULL, '2026-09-01T10:00:02+00:00')",
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
        self.assertEqual(source["unassigned_spawn_rows"], 1)

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

    def test_default_harness_roots_include_every_matching_home(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            expected_codex = []
            expected_claude = []
            for home in (".codex", ".codex-support@example", ".codex-extra"):
                path = root / home / "sessions"
                path.mkdir(parents=True)
                expected_codex.append(path)
            for home in (".claude", ".claude-alt", ".claude-user@example"):
                path = root / home / "projects"
                path.mkdir(parents=True)
                expected_claude.append(path)
            codex, claude = MODULE.default_harness_roots(root)
        self.assertEqual(codex, sorted(expected_codex))
        self.assertEqual(claude, sorted(expected_claude))

    def test_transcript_inventory_counts_all_files_and_factory_joins(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            codex = root / ".codex" / "sessions"
            codex.mkdir(parents=True)
            (codex / "interactive.jsonl").write_text(json.dumps({"type": "session_meta", "payload": {"cwd": "/tmp/project"}}) + "\n")
            (codex / "worker.jsonl").write_text(
                json.dumps(
                    {
                        "type": "session_meta",
                        "payload": {
                            "session_id": "codex-factory",
                            "cwd": "/tmp/project/.cas/worktrees/worker-inventory",
                        },
                    }
                )
                + "\n"
            )
            transcripts, inventory = MODULE.discover_transcripts([codex], [])
        self.assertEqual(len(transcripts), 1)
        self.assertEqual(inventory[0]["files"], 2)
        self.assertEqual(inventory[0]["factory_files"], 1)

    def test_cost_and_scorecard_are_blank_or_calculated_only_from_prices(self):
        rows = [
            {
                field: ""
                for field in MODULE.SESSION_FIELDS
            }
            for _ in range(1)
        ]
        rows[0].update(
            {
                "project": "project",
                "session_id": "session-1",
                "model": "gpt-fixture",
                "effort": "high",
                "task_id": "task-1",
                "task_status": "closed",
                "input_tokens": 1000,
                "cached_input_tokens": 500,
                "output_tokens": 250,
                "reasoning_tokens": 100,
            }
        )
        MODULE.apply_costs(
            rows,
            {
                "models": {
                    "gpt-fixture": {
                        "input_per_million": 1,
                        "cached_input_per_million": 0.5,
                        "output_per_million": 2,
                        "reasoning_per_million": 3,
                    }
                }
            },
        )
        self.assertEqual(rows[0]["cost_usd"], "0.002050")
        summary = MODULE.scorecard(rows)
        self.assertEqual(summary[0]["tasks_delivered"], 1)
        self.assertEqual(summary[0]["cost_usd"], "0.002050")

        blank_rows = [dict(rows[0], model="unknown-model", cost_usd="")]
        MODULE.apply_costs(blank_rows, {})
        self.assertEqual(blank_rows[0]["cost_usd"], "")




class ApplyCostsSemantics(unittest.TestCase):
    PRICES = {"models": {"m": {"input_per_million": 1.0, "cached_input_per_million": 0.1, "cache_creation_per_million": 2.0, "output_per_million": 10.0}}}

    def test_codex_cached_tokens_are_a_subset_of_input(self):
        row = {"harness": "codex", "model": "m", "input_tokens": 1_000_000, "cached_input_tokens": 900_000,
               "cache_creation_input_tokens": 0, "output_tokens": 100_000, "reasoning_tokens": 0}
        MODULE.apply_costs([row], self.PRICES)
        # 100K uncached @ $1 + 900K cached @ $0.10 + 100K output @ $10 = 0.10 + 0.09 + 1.00
        self.assertEqual(row["cost_usd"], f"{1.19:.6f}")

    def test_claude_input_tokens_are_already_net_of_cache(self):
        row = {"harness": "claude", "model": "m", "input_tokens": 100_000, "cached_input_tokens": 900_000,
               "cache_creation_input_tokens": 50_000, "output_tokens": 100_000, "reasoning_tokens": 0}
        MODULE.apply_costs([row], self.PRICES)
        # 100K @ $1 + 900K @ $0.10 + 50K @ $2 + 100K @ $10 = 0.10 + 0.09 + 0.10 + 1.00
        self.assertEqual(row["cost_usd"], f"{1.29:.6f}")

    def test_unpriced_model_stays_blank(self):
        row = {"harness": "codex", "model": "nope", "input_tokens": 10, "cached_input_tokens": 0,
               "cache_creation_input_tokens": 0, "output_tokens": 10, "reasoning_tokens": 0, "cost_usd": ""}
        MODULE.apply_costs([row], self.PRICES)
        self.assertEqual(row["cost_usd"], "")

if __name__ == "__main__":
    unittest.main()
