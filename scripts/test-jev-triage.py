#!/usr/bin/env python3
"""Offline regression for the installed triage recipe; never calls Jev."""
import json
import copy
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SKILL = ROOT / "cas-cli/src/builtins/skills/cas-jev"
HELPER = SKILL / "scripts/triage.py"
spec = importlib.util.spec_from_file_location("jev_triage", HELPER)
triage = importlib.util.module_from_spec(spec)
spec.loader.exec_module(triage)


def state(verdict="VALID"):
    return {"target_sha": "frozen-target", "evidence_complete": True,
            "task": {"id": "cas-report", "description": "repair failed operation",
                     "created_at": "2026-10-02T21:00:00Z"},
            "current_code": [{"sha": "frozen-target", "path": "src/handler.rs",
                              "line": 42, "snippet": "relevant source", "supports": verdict}],
            "candidate_commits": [{"sha": "repair", "target_sha": "frozen-target",
                                   "committed_at": "2026-10-03T00:00:00Z",
                                   "predates_task": True, "integrated_on_target": True,
                                   "paths": ["src/handler.rs"], "patch": "complete repair",
                                   "resolution_noul": .95}]}


def evaluation(verdict="VALID", confidence=.95):
    return {"answers": {"verdict": {"choice": verdict, "confidence": confidence}}}


class TriageRecipe(unittest.TestCase):
    def test_missing_implementation_is_not_valid_evidence(self):
        questions = json.loads((SKILL / "references/triage-questions.json").read_text())
        valid = questions["questions"]["verdict"]["criteria"]["VALID"]
        self.assertIn("positive current evidence", valid)
        self.assertNotIn("No supplied implementation resolves", valid)
        self.assertIn("duplicate_scope", questions["questions"])

    def test_reported_false_suggestions_abstain(self):
        fixture = json.loads((ROOT / "docs/research/jev-triage-routing-fixture.json").read_text())
        self.assertEqual(12, len(fixture["cases"]))
        for case in fixture["cases"]:
            with self.subTest(task=case["state"]["task"]["id"]):
                self.assertEqual(case["expected"], triage.route(case["state"], case["evaluation"])["verdict"])

    def test_positive_current_evidence_and_threshold_boundaries(self):
        self.assertEqual("VALID", triage.route(state(), evaluation(confidence=.9))["verdict"])
        for confidence in (.899, -1, 1.01, True, float("nan"), None):
            self.assertEqual("UNCLEAR", triage.route(state(), evaluation(confidence=confidence))["verdict"])
        for change in ({"current_code": []}, {"evidence_complete": False},
                       {"observations": [{"verified": True}], "current_code": []}):
            case = state(); case.update(change)
            self.assertEqual("UNCLEAR", triage.route(case, evaluation())["verdict"])
        self.assertEqual("UNCLEAR", triage.route(state(), {"status": "unavailable"})["verdict"])

    def test_fixed_requires_post_report_integrated_complete_candidate(self):
        source = state("FIXED")
        self.assertEqual("FIXED", triage.route(source, evaluation("FIXED"))["verdict"])
        self.assertTrue(source["candidate_commits"][0]["predates_task"], "Input must not mutate")
        for change in ({"committed_at": "2026-10-01T00:00:00Z", "predates_task": False},
                       {"committed_at": None}, {"committed_at": "2026-10-03T00:00:00"},
                       {"committed_at": "garbage"}, {"target_sha": "another-revision"},
                       {"integrated_on_target": False}, {"resolution_noul": .899},
                       {"patch": ""}, {"truncated": True}, {"paths": ["hub-web/dist/app.js"]},
                       {"paths": []}):
            with self.subTest(change=change):
                case = copy.deepcopy(source); case["candidate_commits"][0].update(change)
                self.assertEqual("UNCLEAR", triage.route(case, evaluation("FIXED"))["verdict"])
        case = state("FIXED"); case["task"]["created_at"] = "missing"
        self.assertEqual("UNCLEAR", triage.route(case, evaluation("FIXED"))["verdict"])

    def test_fixed_and_obsolete_require_current_source_citations(self):
        for verdict in ("FIXED", "OBSOLETE"):
            source = state(verdict); source["retired_premise"] = "workflow removed"
            if verdict == "OBSOLETE":
                source["candidate_commits"] = []
            self.assertEqual(verdict, triage.route(source, evaluation(verdict))["verdict"])
            for change in ({"sha": "stale"}, {"line": 0}, {"snippet": ""},
                           {"path": "../escape.rs"}, {"path": "/absolute.rs"},
                           {"path": "hub-web/dist/app.js"}, {"truncated": True},
                           {"supports": "another-verdict"}):
                case = copy.deepcopy(source); case["current_code"][0].update(change)
                self.assertEqual("UNCLEAR", triage.route(case, evaluation(verdict))["verdict"])
        self.assertEqual("UNCLEAR", triage.route(state("OBSOLETE"), evaluation("OBSOLETE"))["verdict"])

    def test_separate_scope_signal_finds_umbrella_and_refuses_topic_only(self):
        case = state(); case["task"]["id"] = "cas-3e8d"
        case["similar_tasks"] = [{"id": "cas-8b34", "description": "repair failed operation and adjacent requirement"}]
        case["duplicate_candidate"] = {"id": "cas-8b34", "task_scope": case["task"]["description"],
                                       "other_scope": case["similar_tasks"][0]["description"], "scope_noul": .95}
        result = triage.route(case, evaluation("VALID", .98))
        self.assertEqual(("DUPLICATE", "cas-8b34"), (result["verdict"], result["duplicate_of"]))
        case["current_code"] = []
        for change in ({"scope_noul": .899}, {"id": "unknown-task"},
                       {"task_scope": "unrelated"}, {"other_scope": "shared subsystem words"},
                       {"id": "cas-3e8d"}):
            wrong = copy.deepcopy(case); wrong["duplicate_candidate"].update(change)
            self.assertEqual("UNCLEAR", triage.route(wrong, evaluation("DUPLICATE"))["verdict"])

    def test_byte_caps_generated_sources_and_dates(self):
        case = state("FIXED")
        case["candidate_commits"][0]["patch"] = "é" * triage.MAX_HUNK_BYTES
        prepared = triage.prepare_state(case)
        self.assertLessEqual(len(prepared["candidate_commits"][0]["patch"].encode()), triage.MAX_HUNK_BYTES)
        self.assertFalse(prepared["evidence_complete"])
        case = state(); case["current_code"][0]["snippet"] = "é" * triage.MAX_HUNK_BYTES
        self.assertEqual("UNCLEAR", triage.route(case, evaluation())["verdict"])
        case = state(); case["observations"] = ["x" * triage.MAX_STATE_BYTES]
        self.assertEqual("UNCLEAR", triage.route(case, evaluation())["verdict"])
        case = state(); case["candidate_commit"] = case["candidate_commits"].pop()
        self.assertFalse(triage.prepare_state(case)["candidate_commit"]["predates_task"])

    def test_actual_helper_cli_splits_stitches_and_keeps_excluded_rows(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); records = []
            for index in range(51):
                records.append({"record_id": f"record-{index}", "state": state()})
            records.append({"record_id": "oversize", "state": {**state(), "observations": ["x" * triage.MAX_STATE_BYTES]}})
            records.append({"record_id": "incomplete", "state": {**state(), "evidence_complete": False}})
            source = root / "input.jsonl"; directory = root / "run"
            source.write_text("\n".join(json.dumps(row) for row in records))
            subprocess.run([sys.executable, str(HELPER), "prepare", "--input", str(source),
                            "--directory", str(directory), "--tag", "frozen-run"], check=True)
            manifest = json.loads((directory / "manifest.json").read_text())
            self.assertEqual([50, 1], [len(b["indices"]) for b in manifest["batches"]])
            for batch in manifest["batches"]:
                input_rows = (directory / (batch["name"] + ".jsonl")).read_text().splitlines()
                self.assertEqual(len(batch["indices"]), len(input_rows))
                self.assertTrue(all(len(json.loads(row)["state"]["current_code"]) == 1 for row in input_rows))
                (directory / (batch["name"] + ".answers.json")).write_text(json.dumps([evaluation()] * len(input_rows)))
            output = subprocess.run([sys.executable, str(HELPER), "stitch", "--directory", str(directory)],
                                    check=True, text=True, capture_output=True)
            joined = json.loads(output.stdout)
            self.assertEqual([r["record_id"] for r in records], [r["record_id"] for r in joined])
            self.assertTrue(all(r["run_id"] == "frozen-run" and r["request_id"] is None for r in joined))
            self.assertEqual(["VALID"] * 51 + ["UNCLEAR"] * 2, [r["verdict"] for r in joined])
            (directory / "batch-0000.answers.json").write_text("[]")
            with self.assertRaisesRegex(ValueError, "count"):
                triage.stitch(directory)
            with self.assertRaises(FileExistsError):
                triage.prepare(source, directory, "mixing-runs", "classification")

    def test_all_three_harness_catalogs_install_the_helper(self):
        text = (ROOT / "cas-cli/src/builtins.rs").read_text()
        self.assertEqual(3, text.count('path: "skills/cas-jev/scripts/triage.py"'))
        self.assertEqual(3, text.count('include_str!("builtins/skills/cas-jev/scripts/triage.py")'))


if __name__ == "__main__":
    unittest.main()
