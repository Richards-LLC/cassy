#!/usr/bin/env python3
"""Exercise fast admission against real Git trees and real no-build checks."""

import os
import re
import importlib.util
import json
import hashlib
import sys
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
SPEC = importlib.util.spec_from_file_location("lane_fast_rows", ROOT / "scripts/check-lane-fast-rows.py")
LANE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(LANE)


def _pinned_linter():
    """The repository's one markdownlint-cli2 (scripts/markdownlint.py), or None.

    Fixture repositories have no node_modules, so their markdown-lint row runs
    the real root's pinned linter through the MARKDOWNLINT_CLI2 seam instead of
    a PATH or npx-cache lookup that a scrubbed HOME cannot see (cas-a677).
    """
    result = subprocess.run([sys.executable, str(ROOT / "scripts/markdownlint.py"), "--resolve"],
                            capture_output=True, text=True)
    return result.stdout.strip() if result.returncode == 0 and result.stdout.strip() else None


LINTER = _pinned_linter()
if LINTER:
    os.environ["MARKDOWNLINT_CLI2"] = LINTER


EXAMPLE_SKILL = "---\nname: example\ndescription: Use when exercising the fixture contract.\n---\nfixture contract\n"
VIOLET_SKILL = "---\nname: violet\ndescription: Use when posting to Slack.\n---\nShort body.\n"


def catalog(name):
    entries = "".join(
        f'    BuiltinFile {{\n        path: "skills/{skill}/SKILL.md",\n'
        f'        content: include_str!("builtins/skills/{skill}/SKILL.md"),\n    }},\n'
        for skill in ("example", "violet"))
    return f"pub const {name}: &[BuiltinFile] = &[\n{entries}];\n"


BUILTINS_RS = "".join(catalog(name) for name in ("BUILTIN_SKILLS", "CODEX_BUILTIN_SKILLS", "GROK_BUILTIN_SKILLS"))
JOURNEY_CATALOG = """# Journeys

## Commander

- **Surface-wide:** `hub-web/src/**`

### HUB-J1 · Open a conversation

- **Entry:** the list
- **Goal:** read a thread
- **Touches:** `hub-web/src/list.ts`
- **Suite:** `hub-web/e2e/journeys/example.journey.ts`
- **Gaps:** none

**Steps**

1. Tap the row — the thread opens

**Expected experience**

- The thread shows at once.

**Edge paths**

- An empty list.
"""
JOURNEY_SPEC = "test('HUB-J1 open a conversation', async () => {\n  await test.step('Tap the row', async () => {});\n});\n"
DOCTOR_RS = """impl CheckGroup {
    fn for_name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "symbol index" => Self::Indexes,
            _ => Self::Store,
        }
    }
}
fn run(recorder: &mut Recorder) {
    recorder.mark("store open", &checks);
}
"""
DOCTOR_SNAPSHOT = """---
source: cas-cli/tests/component_output_test.rs
---
Store         [OK] database
Indexes       [OK] symbol index
"""
MIGRATIONS_RS = "mod m001_first;\n\npub static MIGRATIONS: &[Migration] = &[\n    m001_first::MIGRATION,\n];\n"


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
        for helper in ("release-gate.sh", "release_scratch.py", "release-portable.sh", "release-test-env.sh", "release-integration-gates.py", "cas-test-targets.py",
                       "check-workflow-run-interpolation.py", "check-changed-markdown.py", "markdownlint.py",
                       "check-lane-fast-rows.py", "check-lane-compile.py", "check-builtin-doc-hygiene.py", "check-design-paths.py", "builtin-doc-hygiene.json",
                       "check-builtin-contract-phrases.py", "check-test-env.py", "rust_test_source.py",
                       "journeys-for-diff.py", "check-builtin-skill-limits.py", "check-doctor-snapshot.py",
                       "check-migration-registry.py", "check-communication-slos.py", "ci-script-tests-for-diff.py", "check-call-shapes.py",
                       "mcp-call-surface.json", "fast-rows-pre-push.sh", "fast-rows-exception.py", "release-train-announce.py",
                       "release-diary-thread.py"):
            self.write("scripts/" + helper, (ROOT / "scripts" / helper).read_text())
        for source_dir in ("scripts/fixtures/communication-slos", "crates/cas-store/src/communication_slo"):
            for source in (ROOT / source_dir).glob("*.sql"):
                self.write(source_dir + "/" + source.name, source.read_text())
        (self.repo / "scripts/fast-rows-pre-push.sh").chmod(0o755)
        self.write("scripts/test-env-baseline.json", '{"version":1,"violations":[],"exceptions":[]}\n')
        self.write("scripts/builtin-contract-phrases.json", '{"version":2,"documents":{"skills/example/SKILL.md":{"source":"cas-cli/src/builtins/skills/example/SKILL.md","catalogs":["claude","codex","grok"],"contains":[{"text":"fixture contract","reason":"Named fixture contract."}],"absent":[],"any_of":[]}},"alternatives":[]}\n')
        self.write("cas-cli/src/builtins/skills/example/SKILL.md", EXAMPLE_SKILL)
        self.write("cas-cli/src/builtins/skills/violet/SKILL.md", VIOLET_SKILL)
        self.write("cas-cli/src/builtins.rs", BUILTINS_RS)
        self.write("docs/qa/journeys.md", JOURNEY_CATALOG)
        self.write("docs/design/example.brief.md", "# Example\n\nEvidence: <artifacts>/cas-1234/report.json; isolated project/home/config.\n")
        self.write("hub-web/e2e/journeys/example.journey.ts", JOURNEY_SPEC)
        self.write("cas-cli/src/cli/doctor.rs", DOCTOR_RS)
        self.write("cas-cli/tests/snapshots/component_output_test__doctor_snapshot.snap", DOCTOR_SNAPSHOT)
        self.write("cas-cli/src/migration/migrations/m001_first.rs", "pub const MIGRATION: Migration = Migration {\n    id: 1,\n};\n")
        self.write("cas-cli/src/migration/migrations/mod.rs", MIGRATIONS_RS)
        self.write(".markdownlint-cli2.jsonc", (ROOT / ".markdownlint-cli2.jsonc").read_text())
        for crate in ("cas-cli", "crates/cas-types", "crates/cas-search", "crates/cas-store", "crates/cas-core", "crates/cas-mcp"):
            self.write(crate + "/Cargo.toml", '[package]\nname = "fixture"\nversion = "9.99.7"\nautotests = false\n')
        with (self.repo / "cas-cli/Cargo.toml").open("a") as stream:
            stream.write('[[test]]\nname = "sample"\npath = "tests/sample.rs"\n')
        self.write("cas-cli/tests/sample.rs", "#[test]\nfn sample() {}\n")
        self.write("CHANGELOG.md", (
            "## [Unreleased]\n\n### Added — Fixture\n\n"
            "- **Fixture entry** — Example release note.\n\n"
            "## [9.99.7] - 2026-09-30\n\n- Fixture.\n"
        ))
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
                   CAS_RELEASE_GATE_LOG_DIR=str(self.repo / ".git" / "gate-rows"),
                   CAS_RELEASE_GATE_CACHE_DIR=str(self.repo / ".git" / "gate-cache"))
        result = command(self.repo, "bash", "scripts/release-gate.sh", "--fast-rows",
                         "--base", self.base, env=env)
        self.assertFalse((self.repo / "cargo-called").exists(), result.stdout + result.stderr)
        return result

    def integration_rows(self):
        # This tier uses real make and a real environment-reading script.
        self.write("scripts/check-test-shape.py", "print('fixture test-shape PASS')\n")
        self.write("cas-cli/Makefile", "test-ci-tiers:\n\tcd .. && python3 scripts/env-child.py\n")
        self.write("scripts/env-child.py", "import os\nassert not [k for k in os.environ if k.startswith(('CAS_RELEASE_TRAIN_', 'CAS_RELEASE_GATE_'))]\nassert os.environ['GIT_CONFIG_GLOBAL'] == '/dev/null'\nprint('1 script test passed')\n")
        self.commit()
        output = self.repo / ".git/no-build.json"
        result = command(self.repo, "python3", "scripts/release-integration-gates.py", "--run",
                         str(self.repo), self.base, str(output))
        proof = json.loads(output.read_text())
        self.assertEqual(proof["tip"], command(self.repo, "git", "rev-parse", "HEAD").stdout.strip())
        self.assertFalse((self.repo / "cargo-called").exists(), result.stdout + result.stderr)
        return result, proof

    def test_design_docs_reject_absolute_home_paths(self):
        self.write("docs/design/example.brief.md",
                   "# Example\n\nEvidence: /home/alice/.cas/artifacts/cas-1234/report.json\n"
                   "and /Users/bob/.cas/artifacts/cas-1234/report.json\n"
                   "and /root/.cas/artifacts/cas-1234/report.json\n")
        result = self.fast()
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("FAIL markdown-lint", result.stdout)
        self.assertIn("docs/design/example.brief.md:3", result.stdout)
        self.assertIn("docs/design/example.brief.md:4", result.stdout)
        self.assertIn("docs/design/example.brief.md:5", result.stdout)

    def test_integration_no_build_rows_pass_with_all_train_controls_exported(self):
        result, proof = self.integration_rows()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(len(proof['rows']), 22)
        self.assertEqual(proof['rows']['communication-slos'], 'PASS')
        self.assertTrue(all(status == 'PASS' for status in proof['rows'].values()), proof)

    def test_integration_no_build_names_md022_before_assembly(self):
        self.write('docs/bad.md', '# Bad\n## Missing blank lines\nbody\n')
        result, proof = self.integration_rows()
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(proof['rows']['markdown-lint'], 'FAIL')
        self.assertEqual(proof['rows']['ci-script-tests'], 'PASS')
        self.assertIn('markdown-lint', result.stdout)

    def test_integration_train_env_leak_names_script_row(self):
        # Mutation control: removing only the train prefix from the production
        # scrub must turn the real environment-reading make child red.
        path = self.repo / 'scripts/release-test-env.sh'
        path.write_text(path.read_text().replace('CAS_RELEASE_TRAIN_*|', ''))
        result, proof = self.integration_rows()
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(proof['rows']['ci-script-tests'], 'FAIL')
        self.assertIn('ci-script-tests', result.stdout)

    def test_complete_set_no_cargo_and_under_budget(self):
        start = time.monotonic()
        result = self.fast()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertLess(time.monotonic() - start, LANE.fast_rows_budget())
        for row in ("failure-log", "version-literals", "changelog-and-versions", "release-script",
                    "release-notes-shell-injection", "procedure-guardrails", "test-targets",
                    "markdown-lint", "test-shape", "test-env", "builtin-doc-hygiene",
                    "journey-catalog", "builtin-skill-limits", "doctor-snapshot", "migration-registry",
                    "ci-script-tests-changed", "fixture-paths-src", "builtin-projections-src",
                    "call-shapes", "release-notes-draft"):
            if row == "test-shape":
                self.assertIn("SKIP test-shape", result.stdout)
            else:
                self.assertIn("PASS " + row, result.stdout)
        self.assertNotIn("PASS workspace-tests", result.stdout)
        row_dir = self.repo / ".git" / "gate-rows"
        self.assertIn("test-env", (row_dir / "plan.txt").read_text().splitlines())
        self.assertNotIn("test-shape", (row_dir / "plan.txt").read_text().splitlines())
        self.assertEqual(LANE.unfinished_rows(row_dir), [])

    def test_changelog_row_requires_a_titled_unreleased_entry(self):
        self.write("CHANGELOG.md", (
            "## [Unreleased]\n\n### Fixed\n\n- Generic note.\n\n"
            "## [9.99.7] - 2026-09-30\n\n- Fixture.\n"
        ))
        self.commit()
        missing_title = self.fast()
        self.assertNotEqual(missing_title.returncode, 0, missing_title.stdout + missing_title.stderr)
        self.assertIn("FAIL changelog-and-versions", missing_title.stdout)
        self.assertIn("titled category and area format", missing_title.stdout)

        self.write("CHANGELOG.md", (
            "## [Unreleased]\n\n### Fixed — Fixture\n\n"
            "- **Fixture entry** — Restored a titled release note.\n\n"
            "## [9.99.7] - 2026-09-30\n\n- Fixture.\n"
        ))
        self.commit()
        restored = self.fast()
        self.assertEqual(restored.returncode, 0, restored.stdout + restored.stderr)
        self.assertIn("PASS changelog-and-versions", restored.stdout)

    def receipt(self):
        head = command(self.repo, "git", "rev-parse", "HEAD").stdout.strip()
        return head, self.repo / ".git" / "cas" / "fast-rows" / f"{head}.pass"

    def test_a_clean_pass_writes_the_same_tip_receipt_a_park_requires_cas_12ab(self):
        result = self.fast()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        head, path = self.receipt()
        self.assertTrue(path.exists(), result.stdout)
        self.assertTrue(path.read_text().startswith(f"fast-rows: PASS {head} "), path.read_text())
        self.assertIn(str(path), result.stdout)

    def test_a_failed_or_dirty_run_writes_no_pass_receipt_cas_12ab(self):
        self.write("docs/bad.md", "# Bad\n## Missing blank lines\nbody\n")
        self.commit()
        failed = self.fast()
        self.assertNotEqual(failed.returncode, 0)
        # cas-7bce: the failure receipt names rows and fixes, never PASS.
        head, path = self.receipt()
        self.assertFalse(path.exists() and path.read_text().startswith(f"fast-rows: PASS {head} "), failed.stdout)
        (self.repo / "docs/bad.md").unlink()
        self.commit()
        self.write("CHANGELOG.md", (self.repo / "CHANGELOG.md").read_text() + "\n- Uncommitted.\n")
        dirty = self.fast()
        self.assertEqual(dirty.returncode, 0, dirty.stdout + dirty.stderr)
        self.assertFalse(self.receipt()[1].exists(), dirty.stdout)
        self.assertIn("no receipt", dirty.stdout)

    def test_normal_factory_load_allows_rows_over_old_wall_budget(self):
        command(self.repo, "git", "checkout", "-qb", "factory/lane")
        self.write("scripts/example.py", "# scripts-only lane\n")
        self.commit()
        real_popen = subprocess.Popen

        class PassingGate:
            pid = 999999

            def __enter__(self):
                return self

            def __exit__(self, *args):
                return False

            def wait(self, timeout=None):
                # Deterministically model 45 seconds of passing rows at load 1.5/core.
                if timeout is not None and timeout < 45:
                    raise subprocess.TimeoutExpired("passing fast rows", timeout)
                return 0

        def popen(args, **kwargs):
            return PassingGate() if args[0] == "bash" else real_popen(args, **kwargs)

        with patch.object(LANE.os, "getloadavg", return_value=(27, 27, 27)), \
                patch.object(LANE.os, "cpu_count", return_value=18), \
                patch.object(LANE.subprocess, "Popen", side_effect=popen), \
                patch.object(LANE.os, "killpg"):
            self.assertEqual(LANE.check_merge(self.repo, "target", "factory/lane"), 0)

    def test_fast_test_env_skips_unchanged_crate_paths_and_fixture_suite(self):
        self.write("crates/unrelated/Cargo.toml", '[package]\nname = "unrelated"\nversion = "9.99.7"\n')
        self.write("crates/unrelated/src/lib.rs", '#[test] fn broken() {')
        self.write("scripts/test-check-test-env.py", "raise SystemExit('unchanged fixture suite ran')\n")
        self.commit()
        base = command(self.repo, "git", "rev-parse", "HEAD").stdout.strip()
        self.write("scripts/example.py", "# scripts-only lane\n")
        self.commit()
        result = self.fast_from(base)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("PASS test-env", result.stdout)

    def test_timeout_names_unfinished_rows_and_bounded_retry_preserves_refs(self):
        command(self.repo, "git", "checkout", "-qb", "factory/lane")
        self.write("scripts/release-gate.sh", '''#!/bin/bash
mkdir -p "$CAS_RELEASE_GATE_LOG_DIR"
printf 'version-literals\\ntest-env\\nbuiltin-doc-hygiene\\n' >"$CAS_RELEASE_GATE_LOG_DIR/plan.txt"
printf 'row\\tstatus\\nversion-literals\\t0\\n' >"$CAS_RELEASE_GATE_LOG_DIR/timing.tsv"
sleep 2
''')
        self.commit()
        source = command(self.repo, "git", "rev-parse", "HEAD").stdout.strip()
        before = command(self.repo, "git", "worktree", "list", "--porcelain").stdout
        result = command(self.repo, "python3", "scripts/check-lane-fast-rows.py", ".", "target", "factory/lane",
                         "--timeout-secs", "1")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("unfinished rows: test-env, builtin-doc-hygiene", result.stderr)
        self.assertIn("--timeout-secs 2", result.stderr)
        self.assertNotIn("unfinished rows: version-literals", result.stderr)
        retry = command(self.repo, "python3", "scripts/check-lane-fast-rows.py", ".", "target", "factory/lane",
                        "--timeout-secs", "5")
        self.assertEqual(retry.returncode, 0, retry.stdout + retry.stderr)
        self.assertEqual(command(self.repo, "git", "rev-parse", "target").stdout.strip(), self.base)
        self.assertEqual(command(self.repo, "git", "rev-parse", "HEAD").stdout.strip(), source)
        self.assertEqual(command(self.repo, "git", "worktree", "list", "--porcelain").stdout, before)

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
            # cas-462b: rows that failed only at the October 2026 release cut.
            ("journey-catalog", "docs/qa/journeys.md",
             JOURNEY_CATALOG.replace("Tap the row —", "Tap the renamed row —")),
            ("builtin-skill-limits", "cas-cli/src/builtins/skills/violet/SKILL.md",
             VIOLET_SKILL + "x" * 80 + "\n" + "Padding line.\n" * 1200),
            ("builtin-skill-limits", "cas-cli/src/builtins/skills/example/SKILL.md",
             "---\nname: example\ndescription: " + "y" * 251 + "\n---\nfixture contract\n"),
            ("doctor-snapshot", "cas-cli/src/cli/doctor.rs",
             DOCTOR_RS.replace('recorder.mark("store open", &checks);',
                               'recorder.mark("store open", &checks);\n    recorder.mark("code index copies", &checks);')),
            ("doctor-snapshot", "cas-cli/tests/snapshots/component_output_test__doctor_snapshot.snap",
             DOCTOR_SNAPSHOT.replace("[OK] database", "[OK] database  [OK] symbol index")),
            ("migration-registry", "cas-cli/src/migration/migrations/m002_second.rs",
             "pub const MIGRATION: Migration = Migration {\n    id: 2,\n};\n"),
            ("fixture-paths-src", "cas-cli/src/example_tests.rs",
             '#[test] fn reads() { let _ = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("x"); }\n'),
            # cas-ae01, the merge-queue failure: a crate src guard walking its own
            # src at runtime (absent on archive shards).
            ("fixture-paths-src", "crates/cas-store/src/guard.rs",
             '#[test] fn guard() { for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src")).unwrap() { let _ = entry; } }\n'),
            ("fixture-paths-src", "crates/cas-core/tests/reads.rs",
             '#[test] fn reads() { let dir = env!("CARGO_MANIFEST_DIR"); let _ = std::fs::read_to_string(format!("{dir}/x")); }\n'),
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

    def test_contract_phrase_and_oversize_skill_are_refused_at_merge_with_the_row_named(self):
        # The lane check worktree_merge runs: a detached merge preview, then the
        # fast rows. Each cut-time failure class is refused here with its row.
        cases = [
            ("builtin-doc-hygiene", "cas-cli/src/builtins/skills/example/SKILL.md",
             EXAMPLE_SKILL.replace("fixture contract", "drifted wording")),
            ("builtin-skill-limits", "cas-cli/src/builtins/skills/violet/SKILL.md",
             VIOLET_SKILL + "Padding line.\n" * 1200),
            ("journey-catalog", "docs/qa/journeys.md",
             JOURNEY_CATALOG.replace("Tap the row —", "Tap the renamed row —")),
            ("fixture-paths-src", "cas-cli/src/example_tests.rs",
             '#[test] fn reads() { let _ = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent(); }\n'),
        ]
        for row, path, body in cases:
            with self.subTest(row=row):
                command(self.repo, "git", "checkout", "-q", "target")
                command(self.repo, "git", "branch", "-qD", "factory/lane")
                command(self.repo, "git", "checkout", "-qb", "factory/lane")
                self.write(path, body)
                self.commit()
                result = command(self.repo, "python3", "scripts/check-lane-fast-rows.py", ".", "target",
                                 "factory/lane")
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn("FAIL " + row, result.stdout)
                self.assertIn("merge refused", result.stderr)
                self.assertEqual(command(self.repo, "git", "rev-parse", "target").stdout.strip(), self.base)

    def test_script_test_for_a_changed_script_runs_and_its_failure_is_named(self):
        self.write("cas-cli/Makefile", "test-ci-tiers:\n\tcd .. && python3 scripts/test-example.py\n"
                   "\tcd .. && python3 scripts/test-unrelated.py\n")
        self.write("scripts/test-unrelated.py", "raise SystemExit('unrelated script test ran')\n")
        self.write("scripts/test-example.py", "print('example ok')\n")
        self.commit()
        base = command(self.repo, "git", "rev-parse", "HEAD").stdout.strip()
        self.write("scripts/example.py", "# changed subject\n")
        self.commit()
        passing = self.fast_from(base)
        self.assertEqual(passing.returncode, 0, passing.stdout + passing.stderr)
        self.assertIn("PASS ci-script-tests-changed", passing.stdout)
        self.write("scripts/test-example.py", "raise SystemExit('example broke')\n")
        self.commit()
        failing = self.fast_from(base)
        self.assertEqual(failing.returncode, 1, failing.stdout + failing.stderr)
        self.assertIn("FAIL ci-script-tests-changed", failing.stdout)

    def test_changed_markdown_failure(self):
        if not LINTER:
            self.skipTest("pinned Markdown linter unavailable (npm missing)")
        self.write("docs/bad.md", "# Heading\n###Skipped level\n")
        self.commit()
        result = self.fast()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("FAIL markdown-lint", result.stdout)

    def test_markdown_lint_reports_both_checkers_in_one_run(self):
        self.write("scripts/check-design-paths.py", "print('fixture path finding')\nraise SystemExit(1)\n")
        self.write("scripts/check-changed-markdown.py", "print('fixture markdown finding')\nraise SystemExit(1)\n")
        result = self.fast()
        log = (self.repo / ".git/gate-rows/markdown-lint.log").read_text()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("fixture path finding", log)
        self.assertIn("fixture markdown finding", log)

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
        env_lint = "import sys\nassert sys.argv[1:] == ['--changed-since', '" + self.base + "', '--changed-paths'], sys.argv\n"
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
        self.write('scripts/check-test-env.py', "import sys\nassert sys.argv[1:] == ['--changed-since', '" + self.base + "', '--changed-paths']\nprint('bad process state fixture.rs:1')\nsys.exit(1)\n")
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

    # cas-7bce: the five failures of 2026-10-14 that each cost a 30-90 minute
    # CI or merge-queue cycle. Each is refused by a second-scale fast row at
    # park, with the exact fix named in the output and in the FAIL receipt the
    # park refusal quotes.
    def refused_at_park(self, row, files, fix):
        command(self.repo, "git", "reset", "-q", "--hard", self.base)
        command(self.repo, "git", "clean", "-qfd")
        for path, body in files.items():
            self.write(path, body)
        self.commit()
        result = self.fast()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("FAIL " + row, result.stdout)
        self.assertIn(f"FIX {row}: ", result.stdout)
        self.assertIn(fix, result.stdout)
        head, path = self.receipt()
        receipt = path.read_text()
        self.assertTrue(receipt.startswith(f"fast-rows: FAIL {head} rows={row} "), receipt)
        self.assertIn(f"FIX {row}: ", receipt)
        self.assertIn(fix, receipt)

    def test_todays_late_failures_are_refused_at_park_with_the_fix_cas_7bce(self):
        shape = {"scripts/check-test-shape.py": (ROOT / "scripts/check-test-shape.py").read_text()}
        linter = bool(LINTER)
        cases = [
            # 1. The merge queue rejected the two_machine tests: a compile-time
            # binary path does not exist on the archive shard runner.
            ("test-shape", {**shape, "cas-cli/tests/distributed_factory_test_cases/two_machine.rs":
                '#[test]\nfn peers_cli() {\n    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cas"));\n}\n'},
             "cas::test_paths::cas_binary()"),
            # 2. The gate rejected the coming version in a comment after the bump.
            ("version-literals", {"crates/cas-core/src/lib.rs": "// Ships in 9.100.0 with the new store.\n"},
             'env!("CARGO_PKG_VERSION")'),
            ("version-literals", {"scripts/example.py": "# patched in 9.99.8\n"}, "9.99.x fixture value"),
            # 5. Process-environment mutation without the guard, and a runtime
            # read of the producer checkout: the environment failures.
            ("test-env", {"cas-cli/tests/sample.rs": '#[test] fn sample() { std::env::set_var("HOME", "other"); }\n'},
             "TestEnvGuard"),
            ("fixture-paths-src", {"crates/cas-core/tests/reads.rs":
                '#[test] fn reads() { let dir = env!("CARGO_MANIFEST_DIR"); let _ = std::fs::read_to_string(format!("{dir}/x")); }\n'},
             "include_str!(concat!"),
        ]
        if linter:
            cases += [
                # 3. docs CI failed markdownlint on a report (MD022/MD032/MD036).
                ("markdown-lint", {"docs/reports/2026-10-14-review/communication-review.md":
                    "# Review\n## Verdict\nText.\n- item\n\n**Sources**\n\nMore.\n"}, "--fix"),
                # 4. The merge-queue lint caught MD034 in a release report.
                ("markdown-lint", {"docs/release-reports/v9.99.7.md":
                    "# Release\n\nSee https://github.com/example/cloud/pull/155 for detail.\n"},
                 "MD034: wrap bare URLs in <>"),
            ]
        for row, files, fix in cases:
            with self.subTest(row=row, files=sorted(files)):
                self.refused_at_park(row, files, fix)

    def test_next_version_literal_elsewhere_is_noted_not_refused_cas_7bce(self):
        self.write("crates/cas-core/src/lib.rs", "// SQLite 9.99.8 fixed it.\n")
        self.commit()
        self.base = command(self.repo, "git", "rev-parse", "HEAD").stdout.strip()
        self.write("scripts/example.py", "# unrelated lane\n")
        self.commit()
        result = self.fast()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("PASS version-literals", result.stdout)
        log = (self.repo / ".git/gate-rows/version-literals.log").read_text()
        self.assertIn("NOTE pre-existing next-version literals", log)
        self.assertIn("crates/cas-core/src/lib.rs:1:", log)

    def test_a_call_shape_error_in_a_builtin_skill_is_refused_with_its_line_cas_043b(self):
        # cas-0f05's CI red: a spaced placeholder hid the reminder's message.
        skill = EXAMPLE_SKILL + ("coordination action=remind remind_event=ci_completed \\\n"
                                 "  remind_filter='{\"sha\":\"<full 40-char sha>\"}' \\\n"
                                 '  remind_message="CI finished"\n')
        start = time.monotonic()
        self.refused_at_park("call-shapes", {"cas-cli/src/builtins/skills/example/SKILL.md": skill},
                             "cas-cli/src/builtins/skills/example/SKILL.md:6: coordination action=remind: missing remind_message")
        self.assertLess(time.monotonic() - start, LANE.fast_rows_budget())

    def push(self, **env):
        return command(self.repo, "git", "push", "origin", "HEAD:refs/heads/factory/lane",
                       env=dict(os.environ, CARGO=str(self.repo / "cargo-tripwire"), **env))

    def install_hooks(self, **overrides):
        # The installer is operator-only. Its fixture must not inherit this
        # test runner's factory identity or the hook's Git repository variables.
        env = {key: value for key, value in os.environ.items()
               if not key.startswith(("CAS_", "GIT_"))}
        env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull)
        env.update(overrides)
        return command(self.repo, "bash", "scripts/install-git-hooks.sh", env=env)

    def copy_hook_installer(self):
        self.write("scripts/install-git-hooks.sh", (ROOT / "scripts/install-git-hooks.sh").read_text())
        (self.repo / "scripts/release-gate.sh").chmod((ROOT / "scripts/release-gate.sh").stat().st_mode)
        for source in (ROOT / ".githooks").glob("*"):
            self.write(".githooks/" + source.name, source.read_text())
            (self.repo / ".githooks" / source.name).chmod(source.stat().st_mode)

    def test_pre_push_refuses_a_failing_factory_tip_and_logs_an_override_cas_043b(self):
        remote = Path(self.scratch.name) / "remote.git"
        command(self.repo, "git", "init", "-q", "--bare", str(remote))
        command(self.repo, "git", "remote", "add", "origin", str(remote))
        self.copy_hook_installer()
        self.commit()
        # A renamed checkout may still point at a deleted hooks directory.
        command(self.repo, "git", "config", "core.hooksPath", str(self.repo / "deleted/hooks"))
        command(self.repo, "git", "config", "extensions.worktreeConfig", "true")
        worker = Path(self.scratch.name) / "worker"
        self.assertEqual(command(self.repo, "git", "worktree", "add", "-q", "-b", "factory/w", str(worker)).returncode, 0)
        command(worker, "git", "config", "--worktree", "core.hooksPath", "private-worker-hooks")
        private_config = self.repo / ".git/worktrees/worker/config.worktree"
        private_before = private_config.read_bytes()
        shared_before = (self.repo / ".git/config").read_bytes()
        installed = self.install_hooks()
        self.assertEqual(installed.returncode, 0, installed.stderr)
        self.assertEqual(private_config.read_bytes(), private_before)
        self.assertEqual((self.repo / ".git/config").read_bytes(), shared_before)
        config = (self.repo / ".git/config.worktree").read_bytes()
        self.assertEqual(self.install_hooks().returncode, 0)
        self.assertEqual((self.repo / ".git/config.worktree").read_bytes(), config)
        self.write("cas-cli/tests/sample.rs", "// fixed in 9.99.8\n")
        self.write("cas-cli/Makefile", "test-ci-tiers:\n\tcd .. && python3 scripts/test-example.py\n")
        self.write("scripts/test-example.py", "raise SystemExit('fixture diagnostic: prebuild poll expired')\n")
        self.commit()
        refused = self.push()
        self.assertNotEqual(refused.returncode, 0, refused.stderr)
        self.assertIn("FIX version-literals: ", refused.stderr)
        self.assertIn("CAS_SKIP_FAST_ROWS", refused.stderr)
        self.assertIn("FAIL ci-script-tests-changed", refused.stderr)
        # The real hook must keep the detailed child output after the gate's
        # scratch guardian has exited, and tell the worker where it is.
        retained = re.search(r"pre-push: fast-row logs: (.+)", refused.stderr)
        self.assertIsNotNone(retained, refused.stderr)
        logs = Path(retained.group(1))
        row_log = logs / "ci-script-tests-changed.log"
        original_log = row_log.read_text()
        self.assertIn("fixture diagnostic: prebuild poll expired", original_log)
        # pin: FAIL plus the row id is the gate's public diagnostic protocol.
        self.assertIn("FAIL ci-script-tests-changed", (logs / "gate.log").read_text())
        self.assertIn("ci-script-tests-changed", (logs / "timing.tsv").read_text())
        self.assertEqual(command(self.repo, "git", "ls-remote", "origin").stdout, "")

        skipped = self.push(CAS_SKIP_FAST_ROWS="hotfix for the queue")
        self.assertEqual(skipped.returncode, 0, skipped.stderr)
        head = command(self.repo, "git", "rev-parse", "HEAD").stdout.strip()
        log = (self.repo / ".git/cas/fast-rows/overrides.log").read_text()
        self.assertIn(f"{head} factory/lane skipped fast rows: hotfix for the queue", log)

        # A same-tip PASS receipt makes the next push free: no rows run.
        self.write("scripts/test-example.py", "print('example ok')\n")
        self.write("cas-cli/tests/sample.rs", "#[test]\nfn sample() {}\n")
        self.commit()
        self.assertEqual(self.fast().returncode, 0)
        repeat = self.push()
        self.assertEqual(repeat.returncode, 0, repeat.stderr)
        self.assertNotIn("running fast rows", repeat.stderr)

        # cas-eac2: lane CI no longer repeats these rows, so a missing tool
        # fails the push loudly instead of passing it.
        self.write("docs/note.md", "# Note\n")
        self.commit()
        missing = self.push(MARKDOWNLINT_CLI2=str(self.repo / "missing-markdownlint"))
        self.assertNotEqual(missing.returncode, 0, missing.stderr)
        self.assertIn("FAIL markdown-lint", missing.stderr)
        later = re.search(r"pre-push: fast-row logs: (.+)", missing.stderr)
        self.assertIsNotNone(later, missing.stderr)
        self.assertNotEqual(Path(later.group(1)), logs)
        # pin: retaining evidence means its bytes survive later attempts unchanged.
        self.assertEqual(row_log.read_text(), original_log, "later pushes retain the original failure")

    def test_hook_installer_preserves_existing_authority_and_refuses_workers_cas_edb0(self):
        self.copy_hook_installer()
        self.commit()
        self.write("custom/pre-push", "#!/bin/sh\nexit 42\n")
        (self.repo / "custom/pre-push").chmod(0o755)
        hook_before = (self.repo / "custom/pre-push").read_bytes()
        command(self.repo, "git", "config", "core.hooksPath", "custom")
        config = (self.repo / ".git/config").read_bytes()
        for identity, cause in (({}, "existing hook authority"), ({"CAS_AGENT_NAME": "worker"}, "factory invocation")):
            refused = self.install_hooks(**identity)
            self.assertNotEqual(refused.returncode, 0, refused.stdout)
            self.assertIn(cause, refused.stderr)
            self.assertEqual((self.repo / ".git/config").read_bytes(), config)
        self.assertEqual((self.repo / "custom/pre-push").read_bytes(), hook_before)
        # A linked worktree is refused even without factory environment markers.
        worker = Path(self.scratch.name) / "worker"
        added = command(self.repo, "git", "worktree", "add", "-q", "-b", "factory/w", str(worker))
        self.assertEqual(added.returncode, 0, added.stderr)
        for source in ("scripts", ".githooks"):
            shutil.copytree(self.repo / source, worker / source, dirs_exist_ok=True)
        refused = command(worker, "bash", "scripts/install-git-hooks.sh",
                          env={key: value for key, value in os.environ.items()
                               if not key.startswith(("CAS_", "GIT_"))})
        self.assertNotEqual(refused.returncode, 0, refused.stdout)
        self.assertIn("linked worktree", refused.stderr)
        self.assertEqual((self.repo / ".git/config").read_bytes(), config)

    def test_builtin_agent_projection_drift_is_refused_cas_7bce(self):
        body = "Verify the task.\n"
        frontmatter = "---\nname: task-verifier\ndescription: Internal.\n---\n"
        self.write("cas-cli/src/builtins/agents/task-verifier.body.md", body)
        self.write(".claude/agents/task-verifier.md", frontmatter + body)
        self.commit()
        self.base = command(self.repo, "git", "rev-parse", "HEAD").stdout.strip()
        self.refused_at_park("builtin-projections-src",
                             {"cas-cli/src/builtins/agents/task-verifier.body.md": "Verify the task twice.\n"},
                             "task-verifier.body.md verbatim")
        command(self.repo, "git", "reset", "-q", "--hard", self.base)
        self.write("cas-cli/src/builtins/agents/task-verifier.body.md", "Verify the task twice.\n")
        self.write(".claude/agents/task-verifier.md", frontmatter + "Verify the task twice.\n")
        self.commit()
        result = self.fast()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("PASS builtin-projections-src", result.stdout)

    def test_posted_release_drafts_skip_lint_but_unposted_drafts_do_not(self):
        path = "docs/release-notes/2026-10-14-v9.99.8-slack.md"
        # This real historical body contains the now-forbidden "factory"
        # wording. Only its posting status should change admission.
        draft = (ROOT / "docs/release-notes/2026-10-13-v3.50.1-slack.md").read_text().partition(
            "\n## POSTED\n")[0]
        receipt = "message_id=1790000000.000001"
        permalink = "<https://example.slack.com/archives/C0123456789/p1790000000000001>"
        timestamp_receipt = (
            "- User thread: #cas-internal root 1793326626.677139, reply 1793326628.503689, "
            "attached v3.90.0.pdf (F0CCZ4PE9FH, sha256 7bf0c33b…998c, verified)."
        )
        cases = (
            ("unposted", draft, False),
            ("posted permalink", draft + f"\n## POSTED\n\n{permalink}\n", True),
            ("posted message id", draft + f"\n## POSTED\n\n{receipt}\n", True),
            ("posted v3.90.0 timestamps", draft + f"\n## POSTED\n\n{timestamp_receipt}\n", True),
            ("fenced heading", draft.replace("\nInstall:", f"\n## POSTED\n{receipt}\n\nInstall:", 1), False),
            ("empty heading", draft + "\n## POSTED\n", False),
            ("placeholder receipt", draft + "\n## POSTED\n\nmessage_id={{MESSAGE_ID}}\n", False),
            ("receipt in next section", draft + f"\n## POSTED\n\n## Other links\n\n{permalink}\n", False),
        )
        for name, body, posted in cases:
            with self.subTest(case=name):
                self.write(path, body)
                self.commit()
                result = self.fast()
                self.assertEqual(result.returncode, 0 if posted else 1, result.stdout + result.stderr)
                row_log = (self.repo / ".git/gate-rows/release-notes-draft.log").read_text()
                if posted:
                    self.assertIn(f"release-notes-draft: skipping posted draft {path}", row_log)
                    self.assertNotIn("forbidden user wording", row_log)
                else:
                    self.assertIn("FAIL release-notes-draft", result.stdout)
                    self.assertIn("forbidden user wording: factory", row_log)

    def test_overlong_release_draft_is_refused_at_park_cas_e7d4(self):
        """A cut once stopped at preflight on a draft committed hours earlier."""
        path = "docs/release-notes/2026-10-14-v9.99.8-slack.md"
        # The historical draft predates the User-reply wording check (cas-c5af);
        # drop its one config-key mention and posting receipt so this remains
        # an unposted draft with only the punch length under test.
        draft = (ROOT / "docs/release-notes/2026-10-13-v3.50.1-slack.md").read_text().replace(
            "factory.write_roots", "write_roots").partition("\n## POSTED\n")[0]
        # The Dev top-level punch: the second line of the third fenced body.
        punch = draft.split("```")[5].strip("\n").splitlines()[2]
        self.write(path, draft)
        self.commit()
        self.base = command(self.repo, "git", "rev-parse", "HEAD").stdout.strip()
        self.write("scripts/example.py", "# unrelated lane\n")
        self.commit()
        result = self.fast()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("no release-notes draft change",
                      (self.repo / ".git/gate-rows/release-notes-draft.log").read_text())
        other = "docs/release-notes/another-history-slack.md"
        overlong = draft.replace(punch, punch + " Every operator shell launches again today.")
        self.refused_at_park("release-notes-draft", {path: overlong, other: overlong},
                             "dev-top-level top-level punch exceeds 25 words")
        head = command(self.repo, "git", "rev-parse", "HEAD").stdout.strip()
        receipt = self.repo / f".git/cas/fast-rows/{head}.pass"
        scoped_files = {line.removeprefix("SCOPE release-notes-draft ")
                        for line in receipt.read_text().splitlines() if line.startswith("SCOPE release-notes-draft ")}
        self.assertEqual(scoped_files, {path, other})
        # pin: both receipt readers require the terminal scope-completion protocol.
        self.assertTrue(receipt.read_text().endswith("scope complete count=2\n"), receipt.read_text())
        failed = receipt.read_bytes()
        record = dict(schema=1, task_id="cas-fixture", head=head, check="release-notes-draft",
                      files=sorted([path, other]), approver="registered-supervisor", reason="Preserve historical bodies",
                      failure_sha256=hashlib.sha256(failed).hexdigest())
        approval = receipt.with_name(f"{head}.cas-fixture.exception.json")
        validate = (sys.executable, "scripts/fast-rows-exception.py", str(receipt.parent), head)
        # An issued approval must match every bound field; an env skip and an
        # overrides.log entry alone are already covered by the push regression.
        for key, wrong in (("head", "0" * 40), ("check", "test-env"), ("files", [path]),
                           ("failure_sha256", "0" * 64), ("approver", ""), ("reason", ""), ("task_id", "")):
            with self.subTest(field=key):
                approval.write_text(json.dumps(dict(record, **{key: wrong})))
                self.assertNotEqual(command(self.repo, *validate).returncode, 0)
        approval.write_text(json.dumps(record))
        self.assertEqual(command(self.repo, *validate).returncode, 0)
        for partial in (failed.replace(b"scope complete count=2\n", b""),
                        failed.replace(b"scope complete count=2", b"scope complete count=1"),
                        failed + b"SCOPE release-notes-draft docs/release-notes/later-slack.md\n"):
            receipt.write_bytes(partial)
            record["failure_sha256"] = hashlib.sha256(partial).hexdigest()
            approval.write_text(json.dumps(record))
            self.assertNotEqual(command(self.repo, *validate).returncode, 0)
        record["failure_sha256"] = hashlib.sha256(failed).hexdigest()
        approval.write_text(json.dumps(record))
        receipt.write_bytes(failed.replace(b"rows=release-notes-draft", b"rows=release-notes-draft,test-env"))
        self.assertNotEqual(command(self.repo, *validate).returncode, 0)
        receipt.write_bytes(failed)
        pushed = command(self.repo, "bash", "scripts/fast-rows-pre-push.sh", "origin", "unused",
                         input=f"refs/heads/factory/lane {head} refs/heads/factory/lane {'0' * 40}\n")
        self.assertEqual(pushed.returncode, 0, pushed.stderr)
        self.assertIn("approved fast-row exception", pushed.stderr)
        self.assertEqual(receipt.read_bytes(), failed)

        # A missing checker alongside known wording failures is not a draft
        # exception. Every failed file must have a genuine lint result.
        diary = "docs/release-notes/broken-harness-diary-slack.md"
        self.write(diary, "# Historical diary\n")
        (self.repo / "scripts/release-diary-thread.py").unlink()
        self.commit()
        self.assertNotEqual(self.fast().returncode, 0)
        head = command(self.repo, "git", "rev-parse", "HEAD").stdout.strip()
        receipt = receipt.with_name(f"{head}.pass")
        failed = receipt.read_bytes()
        unscoped = {line.removeprefix("UNSCOPED release-notes-draft ")
                    for line in failed.decode().splitlines() if line.startswith("UNSCOPED release-notes-draft ")}
        self.assertEqual(unscoped, {diary})
        record.update(head=head, failure_sha256=hashlib.sha256(failed).hexdigest())
        receipt.with_name(f"{head}.cas-fixture.exception.json").write_text(json.dumps(record))
        self.assertNotEqual(command(self.repo, *validate[:-1], head).returncode, 0)

    def test_harness_diary_draft_uses_the_thread_rubric_at_park(self):
        path = "docs/release-notes/2026-10-14-harness-diary-sweep-slack.md"
        draft = """# 2026-10-14 — Harness diary sweep — #cas-internal thread

## Parent post

```text
*Dev — Harness diary sweep — 2026-10-14*
Was: the harness notes stopped at older pins → Now: current installed versions and validated pins are distinguished; no Cassy runtime behavior is announced. The task-verifier reviewed this diary.
```

## Grok reply

```text
• *Range reviewed* — Was: the diary stopped at 1.0.0 → Now: 1.0.1–1.0.2 reviewed, with 1.0.2 installed.

• *Verdict* — Was: compatibility unknown → Now: no Cassy change is needed for the reviewed updates.

Source gaps: none.
```

## Claude reply

```text
• *Range reviewed* — Was: the diary stopped at 2.1.0 → Now: 2.1.1–2.1.2 reviewed, with 2.1.2 installed.

• *Verdict* — Was: compatibility unknown → Now: no Cassy change is needed for the reviewed updates.

Source gaps: none.
```

## Codex reply

```text
• *Range reviewed* — Was: the diary stopped at 0.1.0 → Now: 0.1.1–0.1.2 reviewed, with 0.1.2 installed.

• *Verdict* — Was: compatibility unknown → Now: no Cassy change is needed for the reviewed updates.

Source gaps: none.
```
"""
        self.write(path, draft)
        self.commit()
        result = self.fast()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("PASS release-notes-draft", result.stdout)

        self.base = command(self.repo, "git", "rev-parse", "HEAD").stdout.strip()
        self.refused_at_park("release-notes-draft",
                             {path: draft.replace("no Cassy change is needed", "cas-6718 needs review")},
                             "harness-diary drafts need one parent and exactly three replies ordered Grok, "
                             "Claude, Codex, with no ticket IDs")


class Budget(unittest.TestCase):
    def test_scales_at_one_to_one_and_a_half_load_per_core_and_caps_extremes(self):
        with patch.object(LANE.os, "cpu_count", return_value=18):
            for load, expected in ((0, 60), (18, 120), (27, 150), (1000, 600)):
                with self.subTest(load=load), patch.object(LANE.os, "getloadavg", return_value=(load, 0, 0)):
                    self.assertEqual(LANE.fast_rows_budget(), expected)

    def test_missing_or_invalid_host_metrics_have_a_finite_budget(self):
        with patch.object(LANE.os, "cpu_count", return_value=None), \
                patch.object(LANE.os, "getloadavg", side_effect=OSError):
            self.assertEqual(LANE.fast_rows_budget(), 60)
        with patch.object(LANE.os, "getloadavg", return_value=(float("nan"), 0, 0)):
            self.assertEqual(LANE.fast_rows_budget(), 60)

    def test_explicit_retry_is_bounded(self):
        for invalid in (0, -1, 1801, True, 1.5):
            with self.subTest(invalid=invalid), self.assertRaisesRegex(ValueError, "1 and 1800"):
                LANE.fast_rows_budget(invalid)
        self.assertEqual(LANE.fast_rows_budget(1800), 1800)


if __name__ == "__main__":
    unittest.main()
