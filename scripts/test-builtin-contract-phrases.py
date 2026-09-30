#!/usr/bin/env python3
"""Behavior fixtures for reasoned text-contract checking."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

path = Path(__file__).with_name("check-builtin-contract-phrases.py")
spec = importlib.util.spec_from_file_location("contract_phrases", path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def policy():
    return {"version": 1, "documents": {"skills/example/SKILL.md": {
        "source": "example.md",
        "contains": [{"text": "task", "reason": "Named lifecycle entry point."}],
        "absent": [{"text": "unsafe", "reason": "Forbidden instruction."}],
        "any_of": [{"texts": ["receipt", "proof"], "reason": "Supported evidence formats."}],
    }}}


class ContractFixtures(unittest.TestCase):
    def check_text(self, text):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "example.md").write_text(text)
            return module.check(root, policy())[1]

    def test_editorial_rewording_does_not_change_the_contract(self):
        self.assertEqual([], self.check_text("Use task, then capture a receipt."))
        self.assertEqual([], self.check_text("New headings and explanation: proof for task."))

    def test_missing_tool_name_refuses(self):
        self.assertEqual(1, len(self.check_text("receipt only")))

    def test_forbidden_instruction_refuses(self):
        self.assertEqual(1, len(self.check_text("unsafe task receipt")))

    def test_alternative_requires_at_least_one_supported_format(self):
        self.assertEqual(1, len(self.check_text("task evidence")))

    def test_missing_reason_refuses(self):
        doc = policy()
        del doc["documents"]["skills/example/SKILL.md"]["contains"][0]["reason"]
        with self.assertRaisesRegex(ValueError, "reason"):
            list(module.contracts(doc))

    def test_empty_registry_and_contract_refuse(self):
        doc = policy()
        doc["documents"] = {}
        with self.assertRaises(ValueError):
            list(module.contracts(doc))
        doc = policy()
        for field in ("contains", "absent", "any_of"):
            doc["documents"]["skills/example/SKILL.md"][field] = []
        with self.assertRaises(ValueError):
            list(module.contracts(doc))

    def test_path_escape_refuses(self):
        doc = policy()
        doc["documents"]["skills/example/SKILL.md"]["source"] = "../foreign.md"
        with self.assertRaisesRegex(ValueError, "relative"):
            list(module.contracts(doc))

    def test_missing_document_refuses(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(FileNotFoundError):
                module.check(Path(directory), policy())


if __name__ == "__main__":
    unittest.main()
