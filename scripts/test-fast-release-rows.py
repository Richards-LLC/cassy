#!/usr/bin/env python3
"""Exercise fast admission against real Git trees and real no-build checks."""

import os
import json
import sys
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))


def command(repo, *args, **kwargs):
    if (repo / "cargo-tripwire").exists() and "env" not in kwargs:
        kwargs["env"] = dict(os.environ, CARGO=str(repo / "cargo-tripwire"))
    return subprocess.run(args, cwd=repo, capture_output=True, text=True, **kwargs)


class FastRows(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.repo = Path(self.scratch.name) / "repo"
        self.repo.mkdir()
        self.write(".gitignore", "/cargo-called\n")
        for helper in ("release-gate.sh", "release-portable.sh", "cas-test-targets.py",
                       "check-workflow-run-interpolation.py", "check-changed-markdown.py",
                       "check-lane-fast-rows.py", "check-lane-compile.py", "check-builtin-doc-hygiene.py", "builtin-doc-hygiene.json",
                       "check-builtin-contract-phrases.py", "check-test-env.py", "rust_test_source.py"):
            self.write("scripts/" + helper, (ROOT / "scripts" / helper).read_text())
        self.write("scripts/test-env-baseline.json", '{"version":1,"violations":[],"exceptions":[]}\n')
        self.write("scripts/builtin-contract-phrases.json", '{"version":2,"documents":{"skills/example/SKILL.md":{"source":"cas-cli/src/builtins/skills/example/SKILL.md","catalogs":["claude","codex","grok"],"contains":[{"text":"fixture contract","reason":"Named fixture contract."}],"absent":[],"any_of":[]}},"alternatives":[]}\n')
        self.write("cas-cli/src/builtins/skills/example/SKILL.md", "fixture contract\n")
        self.write(".markdownlint-cli2.jsonc", (ROOT / ".markdownlint-cli2.jsonc").read_text())
        for crate in ("cas-cli", "crates/cas-types", "crates/cas-search", "crates/cas-store", "crates/cas-core", "crates/cas-mcp"):
            self.write(crate + "/Cargo.toml", '[package]\nname = "fixture"\nversion = "9.99.7"\nautotests = false\n')
        with (self.repo / "cas-cli/Cargo.toml").open("a") as stream:
            stream.write('[[test]]\nname = "sample"\npath = "tests/sample.rs"\n')
        self.write("cas-cli/tests/sample.rs", "#[test]\nfn sample() {}\n")
        self.write("CHANGELOG.md", "## [Unreleased]\n\n## [9.99.7] - 2026-09-30\n\n- Fixture.\n")
        skill = "cas-cli/src/builtins/skills/cas-cut-release/"
        self.write(skill + "SKILL.md", (ROOT / (skill + "SKILL.md")).read_text())
        self.write(skill + "references/failure-log.md", "- **version-literals** — fixture\n")
        self.write("scripts/release.sh", (ROOT / "scripts/release.sh").read_text())
        self.write(".github/workflows/fixture.yml", "jobs:\n  fixture:\n    steps:\n      - run: echo safe\n")
        self.write("cargo-tripwire", '#!/bin/sh\necho called >> cargo-called\nexit 99\n')
        (self.repo / "cargo-tripwire").chmod(0o755)
        for args in (("init", "-q", "-b", "target"), ("config", "user.email", "test@example.invalid"),
                     ("config", "user.name", "Test")):
            self.assertEqual(command(self.repo, "git", *args).returncode, 0)
        self.commit()
        self.base = command(self.repo, "git", "rev-parse", "HEAD").stdout.strip()

    def write(self, path, body):
        target = self.repo / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(body)

    def commit(self):
        self.assertEqual(command(self.repo, "git", "add", ".").returncode, 0)
        result = command(self.repo, "git", "commit", "--allow-empty", "-qm", "fixture")
        self.assertEqual(result.returncode, 0, result.stderr)

    def fast(self):
        env = dict(os.environ, CARGO=str(self.repo / "cargo-tripwire"),
                   CAS_RELEASE_GATE_CACHE_DIR=str(self.repo / ".git" / "gate-cache"))
        result = command(self.repo, "bash", "scripts/release-gate.sh", "--fast-rows",
                         "--base", self.base, env=env)
        self.assertFalse((self.repo / "cargo-called").exists(), result.stdout + result.stderr)
        return result

    def test_complete_set_no_cargo_and_under_budget(self):
        start = time.monotonic()
        result = self.fast()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertLess(time.monotonic() - start, 30)
        for row in ("failure-log", "version-literals", "changelog-and-versions", "release-script",
                    "release-notes-shell-injection", "procedure-guardrails", "test-targets",
                    "markdown-lint", "test-shape", "test-env", "builtin-doc-hygiene"):
            if row == "test-shape":
                self.assertIn("SKIP test-shape", result.stdout)
            else:
                self.assertIn("PASS " + row, result.stdout)
        self.assertNotIn("PASS workspace-tests", result.stdout)

    def test_real_defects_name_the_row(self):
        cases = [
            ("test-env", "cas-cli/tests/sample.rs", '#[test] fn sample() { std::env::set_var("HOME", "other"); }\n'),
            ("test-env", "crates/cas-core/src/lib.rs", '#[test] fn sample() { std::env::set_current_dir("."); }\n'),
            ("test-env", "cas-cli/tests/sample.rs", '#[test] fn sample() { let _a = TestEnvGuard::new(); let _b = TestEnvGuard::new(); }\n'),
            ("release-notes-shell-injection", ".github/workflows/fixture.yml", "jobs:\n  fixture:\n    steps:\n      - run: echo '${{ github.event.head_commit.message }}'\n"),
            ("test-targets", "cas-cli/tests/unwired/main.rs", "#[test]\nfn unwired() {}\n"),
            ("version-literals", "cas-cli/tests/sample.rs", '// current version 9.99.7\n'),
            ("failure-log", "cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md", "- **imaginary-row**\n"),
            ("changelog-and-versions", "crates/cas-core/Cargo.toml", '[package]\nversion = "9.99.8"\n'),
            ("procedure-guardrails", "cas-cli/src/builtins/skills/cas-cut-release/SKILL.md", "missing procedure\n"),
            ("builtin-doc-hygiene", "cas-cli/src/builtins/skills/example/SKILL.md", "Repository Richards-LLC/private-project\n"),
            ("builtin-doc-hygiene", "cas-cli/src/builtins/skills/example/SKILL.md", "removed contract\n"),
        ]
        for row, path, body in cases:
            with self.subTest(row=row):
                command(self.repo, "git", "reset", "--hard", self.base)
                command(self.repo, "git", "clean", "-fd")
                self.write(path, body)
                self.commit()
                result = self.fast()
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn("FAIL " + row, result.stdout)

    def test_changed_markdown_failure(self):
        if not shutil.which("npx") and not shutil.which("markdownlint-cli2"):
            self.skipTest("pinned Markdown linter unavailable")
        self.write("docs/bad.md", "# Heading\n###Skipped level\n")
        self.commit()
        result = self.fast()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("FAIL markdown-lint", result.stdout)

    def test_preview_refuses_failure_without_moving_refs_or_leaking_worktree(self):
        command(self.repo, "git", "checkout", "-qb", "factory/lane")
        self.write("cas-cli/tests/unwired/main.rs", "#[test]\nfn unwired() {}\n")
        self.commit()
        source = command(self.repo, "git", "rev-parse", "HEAD").stdout.strip()
        before = command(self.repo, "git", "worktree", "list", "--porcelain").stdout
        result = command(self.repo, "python3", "scripts/check-lane-fast-rows.py", ".", "target", "factory/lane")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("FAIL test-targets", result.stdout)
        self.assertEqual(command(self.repo, "git", "rev-parse", "target").stdout.strip(), self.base)
        self.assertEqual(command(self.repo, "git", "rev-parse", "HEAD").stdout.strip(), source)
        self.assertEqual(command(self.repo, "git", "worktree", "list", "--porcelain").stdout, before)

    def test_preview_accepts_combined_tree_and_cleans_up(self):
        command(self.repo, "git", "checkout", "-qb", "factory/lane")
        self.write("scripts/example.py", "# valid scripts-only lane\n")
        self.commit()
        result = command(self.repo, "python3", "scripts/check-lane-fast-rows.py", ".", "target", "factory/lane")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("PASS lane fast rows", result.stdout)
        self.assertEqual(command(self.repo, "git", "rev-parse", "target").stdout.strip(), self.base)
        self.assertNotIn("cas-fast-rows-", command(self.repo, "git", "worktree", "list", "--porcelain").stdout)

    def test_preview_checks_the_composition_with_a_new_target_lane(self):
        command(self.repo, "git", "checkout", "-qb", "factory/lane")
        self.write("cas-cli/src/example.rs", "// valid source lane\n")
        self.commit()
        command(self.repo, "git", "checkout", "target")
        self.write("cas-cli/tests/unwired/main.rs", "#[test]\nfn unwired() {}\n")
        self.commit()
        target = command(self.repo, "git", "rev-parse", "target").stdout.strip()
        result = command(self.repo, "python3", "scripts/check-lane-fast-rows.py", ".", "target", "factory/lane")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("FAIL test-targets", result.stdout)
        self.assertEqual(command(self.repo, "git", "rev-parse", "target").stdout.strip(), target)
        self.assertFalse((self.repo / "cargo-called").exists())

    def test_preview_requires_merged_tree_compile_receipt_for_rust(self):
        command(self.repo, "git", "checkout", "-qb", "factory/lane")
        self.write("cas-cli/src/example.rs", "// valid Rust lane\n")
        self.commit()
        result = command(self.repo, "python3", "scripts/check-lane-fast-rows.py", ".", "target", "factory/lane")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("LANE COMPILE REQUIRED", result.stderr)
        self.assertIn("--prove", result.stderr)
        self.assertEqual(command(self.repo, "git", "rev-parse", "target").stdout.strip(), self.base)
        self.assertFalse((self.repo / "cargo-called").exists())

    def fast_from(self, base, **extra_env):
        env = dict(os.environ, CARGO=str(self.repo / "cargo-tripwire"),
                   CAS_RELEASE_GATE_CACHE_DIR=str(self.repo / ".git" / "gate-cache"), **extra_env)
        result = command(self.repo, "bash", "scripts/release-gate.sh", "--fast-rows", "--base", base, env=env)
        self.assertFalse((self.repo / "cargo-called").exists(), result.stdout + result.stderr)
        return result

    def test_force_pushed_vanished_before_sha_falls_back_to_default_branch_merge_base(self):
        # A push event after a force-push names the replaced tip as `before`;
        # rebuild that: the old tip is rewritten away and pruned from the clone.
        command(self.repo, "git", "checkout", "-qb", "factory/lane")
        self.write("scripts/example.py", "# replaced tip\n")
        self.commit()
        vanished = command(self.repo, "git", "rev-parse", "HEAD").stdout.strip()
        command(self.repo, "git", "reset", "-q", "--hard", self.base)
        self.write("scripts/example.py", "# rebased tip\n")
        self.commit()
        for args in (("reflog", "expire", "--expire=now", "--all"), ("gc", "-q", "--prune=now")):
            self.assertEqual(command(self.repo, "git", *args).returncode, 0)
        self.assertNotEqual(command(self.repo, "git", "cat-file", "-e", vanished + "^{commit}").returncode, 0)
        env_lint = "import sys\nassert sys.argv[1:] == ['--changed-since', '" + self.base + "'], sys.argv\n"
        self.write("scripts/check-test-env.py", env_lint)
        self.commit()
        result = self.fast_from(vanished, ZERO_BASE_REF="target")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f"using merge-base with target ({self.base})", result.stdout)
        for row in ("markdown-lint", "test-env"):
            self.assertIn("PASS " + row, result.stdout)
        self.assertNotIn("FAIL ", result.stdout)

    def test_unavailable_base_and_fallback_compare_against_head_parent_without_failing(self):
        self.write("scripts/example.py", "# lane\n")
        self.commit()
        result = self.fast_from("0" * 39 + "1", ZERO_BASE_REF="origin/no-such-branch")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("are unavailable; comparing against HEAD^", result.stdout)
        self.assertNotIn("FAIL ", result.stdout)

    def test_env_baseline_growth_and_staleness_fail_admission(self):
        import importlib.util
        spec = importlib.util.spec_from_file_location('fast_env_lint', ROOT / 'scripts/check-test-env.py')
        lint = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = lint
        spec.loader.exec_module(lint)
        source = '#[test] fn sample() { std::env::set_var("HOME", "x"); }'
        findings = lint.Analyzer({'cas-cli/tests/sample.rs': source}).run()
        manifest = {'version': 1, 'violations': [{'id': r['id'], 'reason': 'Explicit legacy fixture.'} for r in findings], 'exceptions': []}
        self.write('cas-cli/tests/sample.rs', source)
        self.write('scripts/test-env-baseline.json', json.dumps(manifest))
        self.commit()
        grown = self.fast()
        self.assertEqual(grown.returncode, 1, grown.stdout + grown.stderr)
        self.assertIn('FAIL test-env', grown.stdout)
        self.assertIn('baseline growth', grown.stdout)
        self.write('cas-cli/tests/sample.rs', '#[test] fn sample() {}')
        self.commit()
        stale = self.fast()
        self.assertEqual(stale.returncode, 1, stale.stdout + stale.stderr)
        self.assertIn('stale baseline entry', stale.stdout)

    def test_env_checker_is_mandatory_and_receives_base(self):
        self.write('scripts/check-test-env.py', "import sys\nassert sys.argv[1:] == ['--changed-since', '" + self.base + "']\nprint('bad process state fixture.rs:1')\nsys.exit(1)\n")
        self.commit()
        result = self.fast()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('FAIL test-env', result.stdout)
        (self.repo / 'scripts/check-test-env.py').unlink()
        self.commit()
        result = self.fast()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('FAIL test-env', result.stdout)
        self.assertIn('scripts/check-test-env.py is missing', result.stdout)

    def test_optional_shape_checker_receives_the_lane_base_and_failure_is_named(self):
        self.write("scripts/check-test-shape.py", "import sys\nassert sys.argv[1:] == ['--changed-since', '" + self.base + "']\nprint('bad shape fixture.rs:1')\nsys.exit(1)\n")
        self.commit()
        result = self.fast()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("FAIL test-shape", result.stdout)


if __name__ == "__main__":
    unittest.main()
