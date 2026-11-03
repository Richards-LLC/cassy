#!/usr/bin/env python3
"""Exercise draft lint and the last validation before Violet writes."""

import importlib.util
import json
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
        adapter.message_receipt.side_effect = announce.load_report_adapter().message_receipt
        writes = []
        self.reads = []

        def tool(name, arguments):
            if name == "violet_read":
                self.reads.append(arguments)
                if read_effect:
                    read_effect()
                return {"ok": True}
            writes.append(arguments)
            message_id = str(len(writes))
            message = {"message_id": message_id, "permalink": "https://slack.test/" + message_id}
            if len(writes) in getattr(self, "split_replies", {}):
                message["part_ids"] = self.split_replies[len(writes)]
            if len(writes) in getattr(self, "root_threads", {}):
                message["thread_id"] = self.root_threads[len(writes)]
            return {"message": message}

        client.tool.side_effect = tool
        return adapter, writes

    def post(self, adapter):
        with patch.object(announce, "load_report_adapter", return_value=adapter), \
                patch.object(announce, "announce_token_env", return_value=None), \
                patch.object(announce.time, "sleep"):
            announce.post("9.99.8", str(self.draft), str(self.receipt), str(self.body_dir))


    def test_validate_rejects_named_tokens_in_every_body(self):
        for index, name in enumerate(announce.BODY_NAMES):
            for token in ("{{INTERVENTIONS}}", "{{LINUX_SHA256}}", "{{VERSION}}", "{{UNKNOWN}}", "{{}}"):
                with self.subTest(body=name, token=token):
                    bodies = list(BODIES)
                    bodies[index] += " " + token
                    self.write_draft(bodies)
                    result = self.validate()
                    self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                    self.assertIn(name, result.stderr)
                    self.assertIn(token, result.stderr)
                    self.assertIn("line 2" if index in (0, 2) else "line 1", result.stderr)

    def test_pre_publication_allows_release_tokens(self):
        # release.sh resolves VERSION after draft preparation, including titles
        # and links. The fast row and preflight must leave the source untouched.
        bodies = [body.replace("9.99.8", "{{VERSION}}") for body in BODIES]
        bodies[3] += " <https://example.test/v{{VERSION}}|release>"
        self.write_draft(bodies)
        original = self.draft.read_bytes()
        command = [sys.executable, str(SCRIPT), "--validate", str(self.draft), str(self.body_dir)]
        allowed = subprocess.run(command + ["--pre-publication"], capture_output=True, text=True)
        self.assertEqual(allowed.returncode, 0, allowed.stderr)
        fast_row = subprocess.run([sys.executable, str(SCRIPT), "--lint", str(self.draft)],
                                  capture_output=True, text=True)
        self.assertEqual(fast_row.returncode, 0, fast_row.stderr)
        self.assertEqual(self.draft.read_bytes(), original)
        # pin: exported review text must retain VERSION; release.sh owns substitution.
        self.assertEqual((self.body_dir / "user-top-level.txt").read_text(), bodies[0] + "\n")
        strict = subprocess.run(command, capture_output=True, text=True)
        self.assertEqual(strict.returncode, 1)
        self.assertIn("{{VERSION}}", strict.stderr)
        adapter, writes = self.adapter()
        with self.assertRaisesRegex(ValueError, "VERSION"):
            self.post(adapter)
        self.assertFalse(writes)
        adapter.McpClient.assert_not_called()

        # Receipt-fillable digests and PR remain allowed only before publication;
        # arbitrary narrative placeholders must still fail in that phase.
        bodies = list(BODIES)
        bodies[1] += " Linux `{{LINUX_SHA256}}` macOS `{{MACOS_SHA256}}`."
        bodies[3] += " Linux `{{LINUX_SHA256}}` macOS `{{MACOS_SHA256}}`. PR #{{RELEASE_PR}}."
        self.write_draft(bodies)
        command = [sys.executable, str(SCRIPT), "--validate", str(self.draft), str(self.body_dir)]
        allowed = subprocess.run(command + ["--pre-publication"], capture_output=True, text=True)
        self.assertEqual(allowed.returncode, 0, allowed.stderr)
        strict = subprocess.run(command, capture_output=True, text=True)
        self.assertEqual(strict.returncode, 1)
        self.assertIn("{{LINUX_SHA256}}", strict.stderr)
        # cas-52de: the PR number is filled at post-publication too.
        bodies[1] = bodies[1].replace(" Linux `{{LINUX_SHA256}}` macOS `{{MACOS_SHA256}}`.", "")
        bodies[3] = bodies[3].replace(" Linux `{{LINUX_SHA256}}` macOS `{{MACOS_SHA256}}`.", "")
        self.write_draft(bodies)
        pr_only = subprocess.run(command, capture_output=True, text=True)
        self.assertEqual(pr_only.returncode, 1)
        self.assertIn("{{RELEASE_PR}}", pr_only.stderr)
        self.assertEqual(subprocess.run(command + ["--pre-publication"], capture_output=True,
                                        text=True).returncode, 0)
        bodies[3] += " {{INTERVENTIONS}}"
        self.write_draft(bodies)
        other = subprocess.run(command + ["--pre-publication"], capture_output=True, text=True)
        self.assertEqual(other.returncode, 1)
        self.assertIn("{{INTERVENTIONS}}", other.stderr)

    def test_clean_validation_writes_exact_bodies(self):
        result = self.validate()
        self.assertEqual(result.returncode, 0, result.stderr)
        for name, body in zip(announce.BODY_NAMES, BODIES):
            self.assertEqual((self.body_dir / f"{name}.txt").read_text(), body + "\n")

    def test_validate_checks_forbidden_word_in_user_reply_only(self):
        bodies = list(BODIES)
        bodies[1] += " A worker can use it."
        self.write_draft(bodies)
        result = self.validate()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("body user-reply", result.stderr)
        self.assertIn("forbidden user wording: worker", result.stderr)

        bodies[1] = BODIES[1]
        bodies[3] += " A worker can use it."
        self.write_draft(bodies)
        result = self.validate()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_bullet_span_ends_at_a_section_heading(self):
        two_lines = "• *Checks* — Was: checks took longer.\n→ Now: they finish sooner."
        bodies = list(BODIES)
        bodies[1] = f"*Faster*\n\n{two_lines}\n\n*Clearer*\n\n{BODIES[1]}"
        self.write_draft(bodies)
        result = self.validate()
        self.assertEqual(result.returncode, 0, result.stderr)

        bodies[1] = f"{two_lines}\nThey also retry less."
        self.write_draft(bodies)
        result = self.validate()
        self.assertEqual(result.returncode, 1)
        self.assertIn("bullet spans more than two lines", result.stderr)

    def test_line_limit_matches_the_hub_counting_links_by_label(self):
        bodies = list(BODIES)
        link = "<https://github.com/Richards-LLC/cassy/releases/tag/v9.99.8|the release>"
        bodies[1] = BODIES[1] + "\n\nInstall: " + "x" * 270 + " from " + link + "."
        self.write_draft(bodies)
        result = self.validate()
        self.assertEqual(result.returncode, 0, result.stderr)

        bodies[1] = BODIES[1] + " Later " + "é" * 229 + "."
        self.write_draft(bodies)
        result = self.validate()
        self.assertEqual(result.returncode, 1)
        self.assertIn("body user-reply line 1 is 301 characters; the Violet hub refuses lines over 300", result.stderr)

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
        report = announce.load_report_adapter()
        # The PDF file and upload message are separate public receipt identities.
        for file_block in (None, {}, {"file_id": ""}, {"file_id": "pdf-message"}):
            with self.subTest(file_block=file_block):
                envelope = {
                    "message": {"message_id": "pdf-message", "permalink": "https://slack.test/pdf-message"},
                    "download_url": "https://files.test/report.pdf",
                }
                if file_block is not None:
                    envelope["file"] = file_block
                with self.assertRaisesRegex(report.AdapterError, "file id"):
                    report.file_receipt(envelope)
        for split_replies in (
            {},
            {2: ["2.0", "2"], 4: ["4.0", "4.1", "4"]},
            {1: ["1.0", "1"], 2: ["2.0", "2"], 3: ["3.0", "3"], 4: ["4.0", "4"]},
        ):
            with self.subTest(split_replies=split_replies):
                self.split_replies = split_replies
                self.root_threads = {index: parts[0] for index, parts in split_replies.items() if index in (1, 3)}
                self.write_draft(tuple(f"\n{body}\n" for body in BODIES))
                self.assertEqual(self.validate().returncode, 0)
                adapter, writes = self.adapter()
                self.post(adapter)
                self.assertEqual([item["text"] for item in writes], list(BODIES))
                self.assertEqual([item.get("reply_to") for item in writes], [
                    None, self.root_threads.get(1, "1"), None, self.root_threads.get(3, "3"),
                ])
                values = dict(line.split("=", 1) for line in self.receipt.read_text().splitlines())
                self.assertEqual(values["DEV_REPLY_ID"], "4")
                for number, prefix in enumerate(("USER_TOP_LEVEL", "USER_REPLY", "DEV_TOP_LEVEL", "DEV_REPLY"), 1):
                    self.assertEqual(json.loads(values[f"{prefix}_PART_IDS"]),
                                     split_replies.get(number, [str(number)]))
                # The membership read is lean: no channel directory, no file bytes.
                self.assertEqual(len(self.reads), 1)
                self.assertIs(self.reads[0]["include_channels"], False)
                self.assertIs(self.reads[0]["include_files"], False)


if __name__ == "__main__":
    unittest.main()
