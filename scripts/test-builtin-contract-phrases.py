#!/usr/bin/env python3
"""Behavior fixtures for reasoned text-contract checking."""
import copy
import json
import importlib.util
from pathlib import Path
import tempfile
import unittest

path = Path(__file__).with_name("check-builtin-contract-phrases.py")
spec = importlib.util.spec_from_file_location("contract_phrases", path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def policy():
    return {"version": 2, "alternatives": [], "documents": {"skills/example/SKILL.md": {
        "source": "example.md", "catalogs": ["claude", "codex", "grok"],
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


    def test_shared_schema_fixtures(self):
        fixtures = json.loads(path.with_name("builtin-contract-schema-fixtures.json").read_text())
        for fixture in fixtures:
            with self.subTest(fixture=fixture["name"]):
                if fixture["valid"]:
                    module.validate(fixture["policy"])
                else:
                    with self.assertRaises(ValueError):
                        module.validate(fixture["policy"])

    def test_ascii_case_folding_matches_rust(self):
        doc = policy()
        doc["documents"]["skills/example/SKILL.md"]["contains"][0]["case_sensitive"] = False
        self.assertEqual([], module.check_texts(doc, {"skills/example/SKILL.md": "TASK proof"})[1])
        self.assertEqual("Äk", module.ascii_lower("ÄK"))
        self.assertFalse(module.present("linK", "link", False))
        self.assertTrue(module.present("linK", "link", False, True))

    def test_cross_document_alternative_keeps_either_location_valid(self):
        doc = policy()
        doc["documents"]["skills/reference.md"] = copy.deepcopy(doc["documents"]["skills/example/SKILL.md"])
        doc["alternatives"] = [{"reason": "Either layer can teach the report route.", "choices": [
            {"document": name, "text": ".cas/logs", "absent": False, "case_sensitive": True}
            for name in doc["documents"]
        ]}]
        texts = {name: "task receipt" for name in doc["documents"]}
        self.assertEqual(1, len(module.check_texts(doc, texts)[1]))
        for name in texts:
            changed = dict(texts)
            changed[name] += " .cas/logs"
            self.assertEqual([], module.check_texts(doc, changed)[1])

    def test_actual_credential_and_verification_guards_fail_when_mutated(self):
        registry = json.loads(path.with_name("builtin-contract-phrases.json").read_text())
        root = path.parent.parent
        texts = {name: (root / doc["source"]).read_text() for name, doc in registry["documents"].items()}
        self.assertEqual([], module.check_texts(registry, texts)[1])
        # Independent expectations copied from the pre-migration safety guards,
        # not discovered by iterating whatever the registry happens to contain.
        for name, token, forbidden in [
            ("skills/violet/SKILL.md", "never values", False),
            ("skills/violet/SKILL.md", "external signed or private-provider URLs receive no hub credentials", False),
            ("skills/violet/SKILL.md", "xoxb-", True),
            ("skills/violet/references/registration.md", 'auth = "env:VIOLET_SLACK_TOKEN_<LABEL>"', False),
            ("skills/cas-worker/references/recovery.md", "UPDATE tasks SET", True),
            ("skills/cas-supervisor/references/workflow.md", "Never pipe the test run to `tail`", False),
            ("skills/verify-before-claim/SKILL.md", "### 4. Only then, close", False),
        ]:
            with self.subTest(document=name, token=token):
                changed = dict(texts)
                changed[name] = texts[name] + token if forbidden else texts[name].replace(token, "removed contract")
                failures = module.check_texts(registry, changed)[1]
                self.assertTrue(any(name in failure for failure in failures), failures)
        name = "skills/cas-html-reports/SKILL.md"
        changed = dict(texts)
        changed[name] = texts[name].replace("Markdown is the source of truth", "Begin with editable notes")
        self.assertEqual([], module.check_texts(registry, changed)[1])


if __name__ == "__main__":
    unittest.main()
