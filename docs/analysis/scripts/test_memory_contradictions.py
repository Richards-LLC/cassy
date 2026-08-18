#!/usr/bin/env python3
"""Tests for the memory-vs-observed-behaviour review queue (M4, cas-2332).

The load-bearing property is negative: **no memory is ever mutated without an
explicit, named approval.** Most of this file exists to try to make that fail.
"""

from __future__ import annotations

import json
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import memory_contradictions as mc  # noqa: E402

SEEDS = [
    {
        "id": "cas-32ee",
        "title": "epic close gate false-positives on squash-then-evolved children",
        "fix_commit": "abcdef1234",
        "fix_built_at": "2026-08-16T09:00:00-04:00",
        "lexical_terms": ["squash-then-evolved"],
    }
]


def verdict(state: str, **overrides) -> dict:
    payload = {
        "id": "cas-32ee",
        "title": SEEDS[0]["title"],
        "state": state,
        "reason": f"reason for {state}",
        "epoch_evidence": {
            "clean_post_from": "2026-08-17T13:12:34Z",
            "fix_started_running": "2026-08-17T13:12:34Z",
        },
        "exposure": {"clean_pre": 900, "mixed": 0, "clean_post": 1453, "threshold": 100},
        "evidence_cards": [],
    }
    payload.update(overrides)
    return {"verdicts": [payload]}


def make_memories_db(path: Path, rows: list[tuple[str, str, str, str]]) -> None:
    """rows: (id, title, content, tags)"""
    conn = sqlite3.connect(path)
    conn.execute(
        """
        CREATE TABLE entries(
          id TEXT PRIMARY KEY, title TEXT, content TEXT NOT NULL, tags TEXT,
          created TEXT NOT NULL DEFAULT '2026-08-01T00:00:00Z', archived INTEGER NOT NULL DEFAULT 0,
          importance REAL NOT NULL DEFAULT 0.5, stability REAL NOT NULL DEFAULT 0.5,
          valid_from TEXT, valid_until TEXT, helpful_count INTEGER NOT NULL DEFAULT 0,
          harmful_count INTEGER NOT NULL DEFAULT 0, memory_tier TEXT NOT NULL DEFAULT 'working')
        """
    )
    conn.executemany("INSERT INTO entries(id, title, content, tags) VALUES (?,?,?,?)", rows)
    conn.commit()
    conn.close()


class QueueTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.db = Path(self.tmp.name) / "cas.db"
        make_memories_db(
            self.db,
            [
                ("m1", "epic close gate", "The epic close gate is broken for cas-32ee children.", ""),
                ("m2", "unrelated", "Coffee machine notes.", ""),
                ("m3", "style", "Always run the scoped suite before handing off (cas-32ee).", ""),
            ],
        )

    def queue_for(self, state: str) -> dict:
        return mc.build_queue(mc.load_memories(self.db), SEEDS, mc.load_verdicts_from(verdict(state)), "test")

    def test_fixed_verdict_proposes_an_end_date_for_a_defect_claim(self) -> None:
        queue = self.queue_for("fixed")
        item = next(i for i in queue["items"] if i["memory"]["id"] == "m1")
        self.assertEqual(item["suggested_action"], "set_valid_until")
        self.assertEqual(item["memory"]["claim_kind"], "defect_assertion")
        self.assertIn("1453", item["rationale"])

    def test_recurred_verdict_reinforces_instead_of_ageing_out(self) -> None:
        item = next(i for i in self.queue_for("recurred")["items"] if i["memory"]["id"] == "m1")
        self.assertEqual(item["suggested_action"], "opinion_reinforce")

    def test_unobserved_verdict_proposes_nothing(self) -> None:
        # The whole point: "we did not look" must never age a memory out.
        queue = self.queue_for("insufficient-post-fix-data")
        self.assertTrue(all(item["suggested_action"] is None for item in queue["items"]))
        self.assertEqual(queue["counts"]["with_suggested_action"], 0)
        self.assertGreater(queue["counts"]["held_for_insufficient_evidence"], 0)

    def test_unlinked_memories_never_enter_the_queue(self) -> None:
        ids = {item["memory"]["id"] for item in self.queue_for("fixed")["items"]}
        self.assertNotIn("m2", ids)

    def test_every_item_names_the_link_that_produced_it(self) -> None:
        for item in self.queue_for("fixed")["items"]:
            self.assertTrue(item["link"], "an item without a link channel is unauditable")
            for channel in item["link"]:
                self.assertIn(channel["channel"], {"task_id", "fix_commit", "lexical"})
                self.assertTrue(channel["matched"])

    def test_a_prescription_is_surfaced_but_not_aged_out_by_a_fix(self) -> None:
        item = next(i for i in self.queue_for("fixed")["items"] if i["memory"]["id"] == "m3")
        self.assertEqual(item["memory"]["claim_kind"], "prescription")
        self.assertIsNone(item["suggested_action"])

    def test_a_passing_mention_of_the_fix_never_carries_an_action(self) -> None:
        # Measured on the live store: a CI-timing note that happens to cite
        # three fix ids in a list is not a claim about any of those defects.
        make_memories_db(
            self.db.with_name("incidental.db"),
            [
                (
                    "m9",
                    "CI timing",
                    "Widening CI to the workspace suite fails the 10-minute budget "
                    "(measured while landing cas-32ee and two others).",
                    "",
                )
            ],
        )
        queue = mc.build_queue(
            mc.load_memories(self.db.with_name("incidental.db")),
            SEEDS,
            mc.load_verdicts_from(verdict("fixed")),
            "test",
        )
        item = queue["items"][0]
        self.assertTrue(item["link"], "the link is real — it is just incidental")
        self.assertTrue(item["link_is_incidental"])
        self.assertIsNone(item["suggested_action"])
        self.assertIn("mentioned in passing", item["rationale"])

    def test_a_claim_about_the_fix_is_not_marked_incidental(self) -> None:
        item = next(i for i in self.queue_for("fixed")["items"] if i["memory"]["id"] == "m1")
        self.assertFalse(item["link_is_incidental"])

    def test_queue_items_start_unapproved(self) -> None:
        for item in self.queue_for("fixed")["items"]:
            self.assertFalse(item["approved"])
            self.assertIsNone(item["approver"])

    def test_memories_database_is_opened_read_only(self) -> None:
        conn = mc.connect_ro(self.db)
        with self.assertRaises(sqlite3.OperationalError):
            conn.execute("UPDATE entries SET content='tampered'")
        conn.close()


class ApplyTest(unittest.TestCase):
    def item(self, **overrides) -> dict:
        base = {
            "memory": {"id": "m1"},
            "fix": {"id": "cas-32ee"},
            "verdict": {"state": "fixed", "source": "test", "clean_post_from": "2026-08-17T13:12:34Z"},
            "suggested_action": "set_valid_until",
            "approved": False,
            "approver": None,
        }
        base.update(overrides)
        return {"items": [base]}

    def runner(self):
        calls: list[tuple[list[str], str]] = []

        def run(command: list[str], payload: str) -> int:
            calls.append((command, payload))
            return 0

        return calls, run

    def approved(self, **overrides):
        return self.item(approved=True, approver="daniel", **overrides)

    def test_unapproved_item_is_never_executed(self) -> None:
        calls, run = self.runner()
        result = mc.apply_queue(self.item(), execute=True, executor=["true"], runner=run)
        self.assertEqual(calls, [])
        self.assertEqual(result["refused"][0]["reason"], "not approved")

    def test_approval_without_a_named_approver_is_refused(self) -> None:
        calls, run = self.runner()
        result = mc.apply_queue(self.item(approved=True), execute=True, executor=["true"], runner=run)
        self.assertEqual(calls, [])
        self.assertEqual(result["refused"][0]["reason"], "approved without a named approver")

    def test_dry_run_is_the_default_and_executes_nothing(self) -> None:
        calls, run = self.runner()
        result = mc.apply_queue(self.approved(), execute=False, executor=["true"], runner=run)
        self.assertEqual(calls, [])
        self.assertEqual(result["mode"], "dry-run")
        self.assertEqual(len(result["planned"]), 1)
        self.assertEqual(result["executed"], [])

    def test_execute_without_an_executor_refuses_rather_than_pretends(self) -> None:
        # The opinion ops live in mcp__cas__memory, not in the `cas` binary.
        # Without a route to them, an approved item must be reported as
        # unexecuted, never counted as applied.
        calls, run = self.runner()
        result = mc.apply_queue(self.approved(), execute=True, runner=run)
        self.assertEqual(calls, [])
        self.assertEqual(result["executed"], [])
        self.assertEqual(result["mode"], "no-executor")
        self.assertIn(mc.MEMORY_TOOL, result["refused"][0]["reason"])

    def test_approved_item_executes_through_the_memory_tool(self) -> None:
        calls, run = self.runner()
        result = mc.apply_queue(self.approved(), execute=True, executor=["cas-memory-op"], runner=run)
        self.assertEqual(len(calls), 1)
        command, payload = calls[0]
        self.assertEqual(command, ["cas-memory-op"])
        self.assertEqual(json.loads(payload)["tool"], mc.MEMORY_TOOL)
        self.assertEqual(json.loads(payload)["arguments"]["action"], "update")
        self.assertEqual(json.loads(payload)["arguments"]["valid_until"], "2026-08-17T13:12:34Z")
        self.assertEqual(result["executed"][0]["approver"], "daniel")

    def test_items_without_a_suggested_action_are_not_executable(self) -> None:
        calls, run = self.runner()
        result = mc.apply_queue(
            self.approved(suggested_action=None), execute=True, executor=["true"], runner=run
        )
        self.assertEqual(calls, [])
        self.assertEqual(result["planned"], [])

    def test_each_opinion_op_maps_to_its_own_memory_action(self) -> None:
        for action in ("opinion_reinforce", "opinion_weaken", "opinion_contradict"):
            operation = mc.operation_for(self.item(suggested_action=action)["items"][0])
            self.assertEqual(operation["tool"], mc.MEMORY_TOOL)
            self.assertEqual(operation["arguments"]["action"], action)
            self.assertIn("content", operation["arguments"])


class EvaluateTest(unittest.TestCase):
    def queue(self) -> dict:
        return {
            "items": [
                {"memory": {"id": "m1"}, "fix": {"id": "f1"}, "suggested_action": "set_valid_until"},
                {"memory": {"id": "m2"}, "fix": {"id": "f1"}, "suggested_action": "opinion_reinforce"},
                {"memory": {"id": "m3"}, "fix": {"id": "f1"}, "suggested_action": None},
            ]
        }

    def test_precision_counts_only_flagged_items(self) -> None:
        result = mc.evaluate(self.queue(), {"m1|f1": "correct", "m2|f1": "incorrect"})
        self.assertEqual(result["scored"], 2)
        self.assertEqual(result["precision"], 0.5)
        self.assertEqual(result["mistakes"][0]["key"], "m2|f1")

    def test_unlabelled_items_are_reported_not_assumed_correct(self) -> None:
        result = mc.evaluate(self.queue(), {"m1|f1": "correct"})
        self.assertEqual(result["scored"], 1)
        self.assertEqual(result["unlabelled"], ["m2|f1"])

    def test_precision_is_none_rather_than_one_when_nothing_is_labelled(self) -> None:
        self.assertIsNone(mc.evaluate(self.queue(), {})["precision"])

    def test_held_back_items_are_scored_too(self) -> None:
        # Silence is a decision. A queue that proposes nothing would otherwise
        # score a perfect precision by never being wrong out loud.
        result = mc.evaluate(self.queue(), {"m1|f1": "correct", "m2|f1": "correct", "m3|f1": "incorrect"})
        self.assertEqual(result["precision"], 1.0)
        self.assertEqual(result["decision_scored"], 3)
        self.assertEqual(result["decision_accuracy"], round(2 / 3, 4))
        self.assertEqual(result["decision_mistakes"][0]["key"], "m3|f1")


class ClaimClassificationTest(unittest.TestCase):
    def test_defect_wins_over_prescription_when_both_appear(self) -> None:
        # "never fires" is a defect report, not a rule, even though "never"
        # is also a prescription marker.
        self.assertEqual(mc.classify_claim("The hook never fires after a merge."), "defect_assertion")

    def test_constants_are_recognised_separately(self) -> None:
        self.assertEqual(mc.classify_claim("The limit is 25 per batch."), "constant")

    def test_claim_sentence_extracts_the_asserting_sentence(self) -> None:
        text = "Context line.\nThe epic close gate is broken for squashed children.\nUnrelated tail."
        self.assertEqual(
            mc.claim_sentence(text, mc.DEFECT_RE),
            "The epic close gate is broken for squashed children",
        )


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
