#!/usr/bin/env python3
"""Offline regression for the installed triage recipe; never calls Jev."""
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
SKILL = ROOT / "cas-cli/src/builtins/skills/cas-jev"


class TriageRecipe(unittest.TestCase):
    def test_missing_implementation_is_not_valid_evidence(self):
        questions = json.loads((SKILL / "references/triage-questions.json").read_text())
        valid = questions["questions"]["verdict"]["criteria"]["VALID"]
        self.assertIn("positive current evidence", valid)
        self.assertNotIn("No supplied implementation resolves", valid)
        self.assertIn("duplicate_scope", questions["questions"])


if __name__ == "__main__":
    unittest.main()
