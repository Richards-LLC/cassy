#!/usr/bin/env python3
"""Exercise draft lint and the last validation before Violet writes."""

import importlib.util
import json
import os
import runpy
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

# Compatibility fixtures use the reviewed manifest, preserving installed-host aliases.
LEGACY_TOKEN_PREFIX = announce.load_report_adapter().LEGACY_TOKEN_PREFIX
LEGACY_ENV_PREFIX = LEGACY_TOKEN_PREFIX.split("_", 1)[0] + "_"
TOKEN_A = LEGACY_TOKEN_PREFIX + "_A"
TOKEN_B = LEGACY_TOKEN_PREFIX + "_B"
TOKEN_MISSING = "VIOLET_SLACK_TOKEN_MISSING"
TOKEN_OTHER_HOST = LEGACY_TOKEN_PREFIX + "_OTHER_HOST"

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

    def report_stage_parents(self, announcement_receipt):
        """Run the real report stage with only local publication and post stubs."""
        fixture = Path(tempfile.mkdtemp(dir=self.root))
        worktree = fixture / "release"
        worktree.mkdir()
        hooks = fixture / "empty-hooks"
        hooks.mkdir()
        git = ["git", "-C", str(worktree), "-c", "core.hooksPath=" + str(hooks),
               "-c", "user.name=Receipt Fixture", "-c", "user.email=fixture@example.invalid",
               "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"]
        subprocess.run(git + ["init", "-q"], check=True, capture_output=True)
        subprocess.run(git + ["commit", "--allow-empty", "-qm", "fixture"], check=True, capture_output=True)
        subprocess.run(git + ["tag", "-a", "v9.99.8", "-m", "fixture"], check=True, capture_output=True)
        head = subprocess.check_output(git + ["rev-parse", "HEAD"], text=True).strip()
        artifacts = fixture / "artifacts"
        run = artifacts / "v9.99.8-release"
        run.mkdir(parents=True)
        (run / "announce.receipt").write_text(announcement_receipt)
        (run / "landed-main.sha").write_text(head)
        (run / "release-workflow.json").write_text(json.dumps({
            "headBranch": "main", "headSha": head, "event": "push",
            "status": "completed", "conclusion": "success",
        }))
        published = "TAG=v9.99.8\nPUBLISHED_AT=2099-01-02T00:00:00Z\n"
        (run / "release-published.receipt").write_text(
            published + "LINUX_SHA256=" + "a" * 64 + "\nMACOS_SHA256=" + "b" * 64 + "\n")
        (run / "release-latency.receipt").write_text(published + "PUBLISH_LATENCY_SECONDS=1\n")
        reports = worktree / "docs/release-reports"
        reports.mkdir(parents=True)
        for extension in ("md", "html", "pdf"):
            (reports / ("v9.99.8." + extension)).write_text("fixture: no rendering or upload")
        capture = fixture / "post-arguments.txt"
        stub = fixture / "post-stub.sh"
        # Deliberately stop before status can accept a fake report receipt.
        stub.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$CAS_TEST_REPORT_ARGS"\nexit 23\n')
        stub.chmod(0o755)
        env = {name: value for name, value in os.environ.items()
               if not name.startswith(("VIOLET_", LEGACY_ENV_PREFIX, "CAS_RELEASE_"))}
        env.update(CAS_RELEASE_ARTIFACTS_ROOT=str(artifacts), CAS_RELEASE_ENV_FILE=str(fixture / "absent.env"),
                   CAS_RELEASE_TRAIN_REPORT_POST_CMD=str(stub), CAS_TEST_REPORT_ARGS=str(capture))
        result = subprocess.run(
            ["bash", str(SCRIPT.with_name("release-train.sh")), "9.99.8", str(worktree), "--report"],
            env=env, capture_output=True, text=True, timeout=30,
        )
        self.assertEqual(result.returncode, 23, result.stdout + result.stderr)
        return capture.read_text().splitlines()[-2:]

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
                if 1 in split_replies:
                    self.assertEqual(self.report_stage_parents(self.receipt.read_text()), ["1.0", "3.0"])
                    legacy = "\n".join(line for line in self.receipt.read_text().splitlines()
                                       if "_PART_IDS=" not in line) + "\n"
                    self.assertEqual(self.report_stage_parents(legacy), ["1", "3"])
                # Render the real saved-draft receipt, including every transport part.
                result = subprocess.run(
                    ["bash", "-c", 'source "$1"; run_dir="$2"; worktree="$2"; '
                     'release_train_announce_append_posted "$3"', "receipt-test",
                     str(SCRIPT.parent / "release-train.d/announce.sh"), str(self.root), str(self.draft)],
                    capture_output=True, text=True,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                (self.root / "release-report.receipt").write_text(
                    "PDF_MESSAGE_ID=pdf-upload\nPDF_FILE_ID=F-report\n"
                    "PDF_MESSAGE_PERMALINK=https://slack.test/pdf-upload\n")
                # Refresh twice: a later report must enter POSTED without duplicating it.
                for _ in range(2):
                    subprocess.run(result.args, check=True, capture_output=True, text=True)
                posted = self.draft.read_text().split("## POSTED", 1)[1]
                self.assertEqual(posted.count("message_id=pdf-upload"), 1)
                self.assertIn("file_id=F-report", posted)
                self.assertIn("https://slack.test/pdf-upload", posted)
                for number, prefix in enumerate(("USER_TOP_LEVEL", "USER_REPLY", "DEV_TOP_LEVEL", "DEV_REPLY"), 1):
                    expected = split_replies.get(number, [str(number)])
                    self.assertEqual(json.loads(values[f"{prefix}_PART_IDS"]), expected)
                    self.assertIn("part_ids=" + values[f"{prefix}_PART_IDS"], posted)
                # The membership read is lean: no channel directory, no file bytes.
                self.assertEqual(len(self.reads), 1)
                self.assertIs(self.reads[0]["include_channels"], False)
                self.assertIs(self.reads[0]["include_files"], False)
                # An old singular-only receipt cannot prove a complete reply.
                self.receipt.write_text("\n".join(
                    line for line in self.receipt.read_text().splitlines()
                    if not line.startswith("USER_REPLY_PART_IDS=")
                ) + "\n")
                incomplete = subprocess.run(
                    ["bash", "-c", 'source "$1"; run_dir="$2"; worktree="$2"; '
                     'version=9.99.8; CAS_RELEASE_TRAIN_DRAFT="$3"; release_train_announce',
                     "partial-receipt-test", str(SCRIPT.parent / "release-train.d/announce.sh"),
                     str(self.root), str(self.draft)], capture_output=True, text=True,
                )
                self.assertNotEqual(incomplete.returncode, 0)
                self.assertIn("partial receipt", incomplete.stderr)
                self.assertIn("refusing to retry writes", incomplete.stderr)


class TokenPreflight(unittest.TestCase):
    """Exercise the real --cut dispatcher without publishing or using host secrets."""

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.worktree = self.root / "release"
        self.worktree.mkdir()
        self.run = self.root / "artifacts/v9.99.8-release"
        self.env = {name: value for name, value in os.environ.items()
                    if not name.startswith(("VIOLET_", LEGACY_ENV_PREFIX, "CAS_RELEASE_"))}
        self.env.update(
            CLAUDE_CONFIG_DIR=str(self.root),
            CAS_CREDENTIALS_FILE=str(self.root / "credentials.env"),
            CAS_RELEASE_ARTIFACTS_ROOT=str(self.root / "artifacts"),
            CAS_RELEASE_TRAIN_RUN_DIR=str(self.run),
            CAS_RELEASE_TRAIN_DATE="2099-01-02",
            CAS_RELEASE_TRAIN_CAS="/usr/bin/false",
            CAS_RELEASE_TRAIN_PREFLIGHT_SKIP_COMPETING="1",
            CAS_RELEASE_TRAIN_PREFLIGHT_SKIP_TOOLCHAIN="1",
            CAS_RELEASE_TRAIN_ASSEMBLE_CMD='printf "assemble reached\\n"; exit 1',
            CAS_RELEASE_ENV_FILE=str(self.worktree / "release.env"),
            CAS_RELEASE_GATE_HOME_DIR=str(self.root / "scratch"),
        )
        self.env.update({TOKEN_A: "sentinel-secret-alpha",
                         TOKEN_B: "sentinel-secret-beta"})
        self.git("init", "-q", "-b", "release/9.99.8")
        self.git("config", "user.name", "Token Preflight Test")
        self.git("config", "user.email", "test@example.invalid")
        self.git("config", "core.hooksPath", "/dev/null")
        (self.worktree / "release.env").write_text("")
        (self.worktree / "CHANGELOG.md").write_text("# Changelog\n\n## [9.99.8] - 2099-01-02\n\n- fixture\n")
        draft = self.worktree / "docs/release-notes/2099-01-02-v9.99.8-slack.md"
        draft.parent.mkdir(parents=True)
        draft.write_text("\n\n".join(f"```text\n{body}\n```" for body in BODIES))
        zig = self.worktree / ".context/zig/zig"
        zig.parent.mkdir(parents=True)
        zig.write_text("#!/bin/sh\nexit 0\n")
        zig.chmod(0o755)
        self.env["ZIG"] = str(zig)
        self.git("add", ".")
        self.git("-c", "commit.gpgsign=false", "commit", "-qm", "seed")
        head = self.git("rev-parse", "HEAD").strip()
        self.git("update-ref", "refs/remotes/origin/main", head)
        integration = self.worktree / ".cas/merge-sweeps/integration.json"
        integration.parent.mkdir(parents=True)
        integration.write_text(json.dumps({"status": "PASSED", "base": head, "tip": head, "epics": [],
            "no_build": {"tip": head, "rows": {row: "PASS" for row in
                runpy.run_path(str(Path(__file__).with_name("release-integration-gates.py")))["REQUIRED_ROWS"]}}}))
        self.git("add", ".")
        self.git("-c", "commit.gpgsign=false", "commit", "-qm", "receipt")

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.worktree), *args],
                                       stderr=subprocess.PIPE, text=True)

    def cut(self):
        result = subprocess.run(
            ["bash", str(SCRIPT.with_name("release-train.sh")), "9.99.8", str(self.worktree), "--cut"],
            env=self.env, capture_output=True, text=True, timeout=30,
        )
        output = result.stdout + result.stderr
        for value in ("sentinel-secret-alpha", "sentinel-secret-beta"):
            self.assertNotIn(value, output)
            for path in self.run.rglob("*"):
                if path.is_file():
                    self.assertNotIn(value, path.read_text())
        self.assertNotIn("stage publish: start", output)
        return result, output

    def test_ambiguous_tokens_block_cut_before_assemble(self):
        result, output = self.cut()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("BLOCKER announce-token:", output)
        self.assertIn(TOKEN_A, output)
        self.assertIn(TOKEN_B, output)
        self.assertIn("set VIOLET_SLACK_TOKEN_ENV", output)
        self.assertNotIn("assemble reached", output)
        self.assertFalse((self.run / "stage.preflight.done").exists())

    def assert_preflight_passes(self):
        _, output = self.cut()
        self.assertIn("assemble reached", output)
        self.assertNotIn("BLOCKER announce-token:", output)
        self.assertTrue((self.run / "stage.preflight.done").is_file())

    def test_explicit_selector_unblocks_cut(self):
        self.env["VIOLET_SLACK_TOKEN_ENV"] = TOKEN_A
        self.assert_preflight_passes()

    def test_missing_explicit_token_blocks_cut(self):
        self.env["VIOLET_SLACK_TOKEN_ENV"] = TOKEN_MISSING
        _, output = self.cut()
        self.assertIn("BLOCKER announce-token:", output)
        self.assertIn(f"credential variable {TOKEN_MISSING} is unset or empty", output)
        self.assertNotIn("assemble reached", output)

    def test_credential_file_ambiguity_blocks_cut(self):
        del self.env[TOKEN_A]
        del self.env[TOKEN_B]
        Path(self.env["CAS_CREDENTIALS_FILE"]).write_text(
            f'export {TOKEN_A}="sentinel-secret-alpha"\n'
            f'{TOKEN_B}=sentinel-secret-beta\n')
        _, output = self.cut()
        self.assertIn("BLOCKER announce-token:", output)
        self.assertIn("multiple Violet token variables found", output)
        self.assertNotIn("assemble reached", output)

    def test_default_proxy_selects_canonical_alias_from_credentials(self):
        proxy = self.worktree / ".cas/proxy.toml"
        proxy.write_text(f'auth = "env:{TOKEN_A}"\n')
        self.git("add", ".")
        self.git("-c", "commit.gpgsign=false", "commit", "-qm", "proxy")
        del self.env[TOKEN_A]
        Path(self.env["CAS_CREDENTIALS_FILE"]).write_text(
            'VIOLET_SLACK_TOKEN_A=sentinel-secret-alpha\n')
        self.assert_preflight_passes()

    def test_unavailable_proxy_token_falls_back_to_machine_registration(self):
        proxy = self.root / "other-host-proxy.toml"
        proxy.write_text(f'auth = "env:{TOKEN_OTHER_HOST}"\n')
        self.env["CAS_RELEASE_TRAIN_PROXY_TOML"] = str(proxy)
        (self.root / ".claude.json").write_text(json.dumps({
            "mcpServers": {"violet": {"headers": {
                "Authorization": f"Bearer ${{{TOKEN_A}}}"}}}}))
        self.assert_preflight_passes()

    def test_absent_tokens_block_cut(self):
        del self.env[TOKEN_A]
        del self.env[TOKEN_B]
        _, output = self.cut()
        self.assertIn("BLOCKER announce-token:", output)
        self.assertIn("no Violet token found", output)
        self.assertNotIn("assemble reached", output)

    def test_external_announcer_and_embargo_keep_their_auth_contract(self):
        for name in ("CAS_RELEASE_TRAIN_ANNOUNCE_CMD", "CAS_RELEASE_TRAIN_ANNOUNCE_POST_CMD",
                     "CAS_RELEASE_TRAIN_ANNOUNCEMENT_EMBARGO"):
            with self.subTest(name=name):
                self.env[name] = "fixture"
                self.assert_preflight_passes()
                del self.env[name]
                (self.run / "stage.preflight.done").unlink()

    def test_check_token_never_creates_a_network_client(self):
        adapter = announce.load_report_adapter()
        self.env["VIOLET_SLACK_TOKEN_ENV"] = TOKEN_A
        with patch.dict(os.environ, self.env, clear=True), \
                patch.object(announce, "load_report_adapter", return_value=adapter), \
                patch.object(adapter, "McpClient") as client:
            self.assertEqual(announce.main([str(SCRIPT), "--check-token"]), 0)
            client.assert_not_called()


if __name__ == "__main__":
    unittest.main()
