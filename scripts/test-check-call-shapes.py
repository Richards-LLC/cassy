#!/usr/bin/env python3
"""Behaviour of the call-shape lint against the committed MCP surface."""

import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("call_shapes", ROOT / "scripts/check-call-shapes.py")
LINT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(LINT)
SURFACE = json.loads((ROOT / LINT.SURFACE).read_text())["tools"]


def defects(text, origin="fixture"):
    return [defect for _, defect, _ in LINT.lint(text, origin, SURFACE)]


class CallShapes(unittest.TestCase):
    def test_known_bad_shapes_are_flagged(self):
        for text in [
            '`mcp__cas__coordination action=message target=supervisor message="x"`',
            'use: `{prefix}coordination action=message target={respond_to} message=\\"...\\"`',
            '`mcp__cs__coordination action=remind remind_delay_secs=60`',
            '{sup_ver} action=add task_id={} status=approved summary=\\"...\\"',
            '`cas__verification action=add task_id=x dispatch_id=d files_reviewed=3 status=approved`',
            '`mcp__cas__task action=create title="x" \\\n  priority=2`',
            '`system action=status`',
            'Report via `factory action=message target=x summary="s" message="m"`.',
            '`mcp__cas__coordination action=spawn_workers count=1`',
            'coordination action=remind remind_event=ci_completed \\\n  remind_filter=\'{"sha":"<full 40-char sha>","workflow":"CI"}\' \\\n  cross_session=true remind_message="CI finished on <sha>"',
        ]:
            with self.subTest(text=text):
                self.assertTrue(defects(text))

    def test_valid_and_non_call_text_is_clean(self):
        for text in [
            '`mcp__cas__coordination action=message target=supervisor summary="s" message="m"`',
            'task action=create title="e" task_type=epic',
            "use coordination action=message after the worker registers",
            "the subtask action=create flow",
            "`mcp__cs__factory action=worktree_merge id=factory/x task_id=cas-1 cleanup=true`",
            '`search action=code_search query="x"`',
            "the code_search action=grep fallback",
            "`coordination action=<a>`",
        ]:
            with self.subTest(text=text):
                self.assertEqual(defects(text), [])

    def test_the_capability_bound_verifier_needs_no_dispatch_id(self):
        text = "`verification action=add task_id=x status=approved`"
        for origin in [
            "cas-cli/src/builtins/agents/task-verifier.body.md",
            "cas-cli/src/builtins/codex/agents/task-verifier.toml",
        ]:
            with self.subTest(origin=origin):
                self.assertEqual(defects(text, origin), [])
        self.assertTrue(defects(text, "user/agents/task-verifier.toml"))

    def test_runtime_text_skips_test_modules_and_comments_and_keeps_lines(self):
        source = ('// coordination action=spawn_workers\nfn a() {}\n#[cfg(test)]\nmod tests {\n'
                  '    const X: &str = "system action=status";\n}\nconst Y: &str = "system action=status";\n')
        hits = LINT.lint(LINT.runtime_text(source), "x.rs", SURFACE)
        self.assertEqual([line for line, _, _ in hits], [7])


if __name__ == "__main__":
    unittest.main()
