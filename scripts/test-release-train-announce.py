#!/usr/bin/env python3
"""Exercise draft lint and the last validation before Violet writes."""

import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch


SCRIPT = Path(__file__).with_name("release-train-announce.py")
spec = importlib.util.spec_from_file_location("announce", SCRIPT)
announce = importlib.util.module_from_spec(spec)
spec.loader.exec_module(announce)

BODIES = (
    "*Live on production — User — Cassy v9.99.8*\n"
    "Was: changes took longer. → Now: checks finish sooner.",
    "• *Checks* — Was: checks took longer. → Now: they finish sooner.",
    "*Live on production — Dev — Cassy v9.99.8*\n"
    "Was: checks repeated. → Now: each result is reused.",
    "• *Checks* — Was: checks repeated. → Now: each result is reused.",
)


class Announce(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.draft = self.root / "draft.md"
        self.body_dir = self.root / "bodies"
        self.receipt = self.root / "announce.receipt"
        self.write_draft(BODIES)

    def write_draft(self, bodies):
        self.draft.write_text("\n\n".join(f"```text\n{body}\n```" for body in bodies))

    def validate(self):
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--validate", str(self.draft), str(self.body_dir)],
            capture_output=True, text=True,
        )

    def adapter(self, read_effect=None):
        adapter = Mock()
        client = adapter.McpClient.return_value
        client.call.return_value = {"tools": [{"name": "violet_read"}, {"name": "violet_post"}]}
        adapter.message_receipt.side_effect = lambda envelope, name: (
            envelope["message_id"], "https://slack.test/" + envelope["message_id"]
        )
        writes = []

        def tool(name, arguments):
            if name == "violet_read":
                if read_effect:
                    read_effect()
                return {"ok": True}
            writes.append(arguments)
            return {"message_id": str(len(writes))}

        client.tool.side_effect = tool
        return adapter, writes

    def post(self, adapter):
        with patch.object(announce, "load_report_adapter", return_value=adapter), \
                patch.object(announce, "announce_token_env", return_value=None), \
                patch.object(announce.time, "sleep"):
            announce.post("9.99.8", str(self.draft), str(self.receipt), str(self.body_dir))

    def test_validate_rejects_named_tokens_in_every_body(self):
        for index, name in enumerate(announce.BODY_NAMES):
            for token in ("{{INTERVENTIONS}}", "{{LINUX_SHA256}}", "{{UNKNOWN}}", "{{}}"):
                with self.subTest(body=name, token=token):
                    bodies = list(BODIES)
                    bodies[index] += " " + token
                    self.write_draft(bodies)
                    result = self.validate()
                    self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                    self.assertIn(name, result.stderr)
                    self.assertIn(token, result.stderr)
                    self.assertIn("line 2" if index in (0, 2) else "line 1", result.stderr)

    def test_clean_validation_writes_exact_bodies(self):
        result = self.validate()
        self.assertEqual(result.returncode, 0, result.stderr)
        for name, body in zip(announce.BODY_NAMES, BODIES):
            self.assertEqual((self.body_dir / f"{name}.txt").read_text(), body + "\n")

    def test_validate_rejects_a_multiline_token(self):
        bodies = list(BODIES)
        bodies[3] += "\n\n{{UNKNOWN\nVALUE}}"
        self.write_draft(bodies)
        result = self.validate()
        self.assertEqual(result.returncode, 1)
        self.assertIn("line 3", result.stderr)
        self.assertIn("{{UNKNOWN\nVALUE}}", result.stderr)

    def test_non_message_template_notes_do_not_block_validation(self):
        with self.draft.open("a") as draft:
            draft.write("\nNotes: {{NOT_POSTED}}\n")
        self.assertEqual(self.validate().returncode, 0)

    def test_post_rejects_a_token_inserted_after_validation_without_writes(self):
        self.assertEqual(self.validate().returncode, 0)
        bodies = list(BODIES)
        bodies[3] += " {{GREEN_TO_PUBLISHED}}"
        self.write_draft(bodies)
        adapter, writes = self.adapter()
        with self.assertRaisesRegex(ValueError, "GREEN_TO_PUBLISHED"):
            self.post(adapter)
        self.assertFalse(writes)
        adapter.McpClient.assert_not_called()
        self.assertFalse(self.receipt.exists())

    def test_post_revalidates_all_files_after_read_before_any_write(self):
        for index, name in enumerate(announce.BODY_NAMES):
            with self.subTest(body=name):
                self.assertEqual(self.validate().returncode, 0)

                def change_body():
                    (self.body_dir / f"{name}.txt").write_text(BODIES[index] + " {{LATE_TOKEN}}\n")

                adapter, writes = self.adapter(change_body)
                with self.assertRaisesRegex(ValueError, "LATE_TOKEN"):
                    self.post(adapter)
                self.assertFalse(writes)
                self.assertFalse(self.receipt.exists())

    def test_post_refuses_clean_body_changes_after_read(self):
        self.assertEqual(self.validate().returncode, 0)

        def change_body():
            (self.body_dir / "dev-reply.txt").write_text(BODIES[3] + " Extra wording.\n")

        adapter, writes = self.adapter(change_body)
        with self.assertRaisesRegex(ValueError, "validated body.*changed"):
            self.post(adapter)
        self.assertFalse(writes)
        self.assertFalse(self.receipt.exists())

    def test_post_sends_validated_bodies_and_preserves_threads_and_receipts(self):
        self.assertEqual(self.validate().returncode, 0)
        adapter, writes = self.adapter()
        self.post(adapter)
        self.assertEqual([item["text"] for item in writes], list(BODIES))
        self.assertEqual([item.get("reply_to") for item in writes], [None, "1", None, "3"])
        values = dict(line.split("=", 1) for line in self.receipt.read_text().splitlines())
        self.assertEqual(values["DEV_REPLY_ID"], "4")


if __name__ == "__main__":
    unittest.main()
