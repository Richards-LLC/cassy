#!/usr/bin/env python3
"""Two lanes that both change builtins merge without a reference-history conflict.

python3 scripts/test-regenerate-generated-artifacts.py

cas-fb11: cas-cli/src/builtins/reference-history.json is derived from git
history, so two lanes that each regenerate it always conflicted. The repository
.gitattributes marks it `merge=cas-generated` (keep the target's copy), and
scripts/regenerate-generated-artifacts.sh reruns the real generator on the
merged history. The fixture repository carries this checkout's scripts and
.gitattributes, so the test exercises exactly what ships.
"""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
LEDGER = "cas-cli/src/builtins/reference-history.json"
SCRIPTS = ("gen-builtin-reference-history.sh", "regenerate-generated-artifacts.sh", "cas-merge-drivers.py")


class ReferenceHistoryMergeTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self.tmp.name)
        self.env = {**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.org",
                    "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.org"}
        self.git("init", "-q", "-b", "main")
        (self.repo / "scripts").mkdir()
        for name in SCRIPTS:
            shutil.copy2(ROOT / "scripts" / name, self.repo / "scripts" / name)
        shutil.copy2(ROOT / ".gitattributes", self.repo / ".gitattributes")
        self.write("cas-cli/src/builtins/agents/task-verifier.body.md", "Verify the task.\n")
        self.write("cas-cli/src/builtins/skills/demo/SKILL.md", "---\nname: demo\n---\n# Demo\n")
        # Count generator runs: a wrapper stands in for the real script.
        generator = self.repo / "scripts" / "gen-builtin-reference-history.sh"
        generator.rename(self.repo / "scripts" / "gen-real.sh")
        generator.write_text('#!/usr/bin/env bash\necho run >> "$GEN_RUNS"\nexec bash scripts/gen-real.sh "$@"\n')
        generator.chmod(0o755)
        self.runs = self.repo / ".git" / "gen-runs"
        self.env["GEN_RUNS"] = str(self.runs)
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "baseline")
        self.run_script("gen-builtin-reference-history.sh")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "baseline ledger")
        subprocess.run(["python3", "scripts/cas-merge-drivers.py", "install"], cwd=self.repo, check=True,
                       capture_output=True)

    def tearDown(self):
        self.tmp.cleanup()

    def write(self, relative, text):
        path = self.repo / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def git(self, *args, check=True):
        return subprocess.run(["git", *args], cwd=self.repo, env=self.env, check=check,
                              capture_output=True, text=True)

    def run_script(self, name, *args, ok=True):
        result = subprocess.run(["bash", f"scripts/{name}", *args], cwd=self.repo, env=self.env,
                                capture_output=True, text=True)
        if ok:
            self.assertEqual(result.returncode, 0, f"{name} failed:\n{result.stdout}{result.stderr}")
        return result

    def generator_runs(self):
        runs = self.runs.read_text().count("run") if self.runs.exists() else 0
        self.runs.unlink(missing_ok=True)
        return runs

    def assert_ledger_fresh(self):
        self.assertEqual(self.git("status", "--porcelain").stdout, "", "the regenerated ledger is committed")
        self.run_script("gen-builtin-reference-history.sh")
        fresh = self.git("diff", "--exit-code", "--", LEDGER, check=False)
        self.assertEqual(fresh.returncode, 0, "the ledger equals a fresh generator run:\n" + fresh.stdout)

    def lane(self, branch, reference):
        """A lane adds a builtin reference and commits its own regenerated ledger."""
        self.git("checkout", "-q", "-b", branch, "main")
        self.write(f"cas-cli/src/builtins/skills/demo/references/{reference}.md", f"# {reference}\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", f"{branch}: add {reference}")
        self.run_script("gen-builtin-reference-history.sh")
        self.git("commit", "-q", "-am", f"{branch}: regenerate the ledger")
        self.git("checkout", "-q", "main")

    def prepare_hub_web(self, build):
        self.write(".gitignore", "hub-web/node_modules\n")
        self.write("hub-web/package.json", json.dumps({
            "name": "hub-regeneration-fixture", "version": "1.0.0",
            "scripts": {"build": build},
            "dependencies": {"regeneration-test-dependency": "file:fixture-dependency"},
        }))
        self.write("hub-web/fixture-dependency/package.json", json.dumps({
            "name": "regeneration-test-dependency", "version": "1.0.0", "main": "index.cjs",
        }))
        self.write("hub-web/fixture-dependency/index.cjs", "module.exports = 'locked dependency';\n")
        subprocess.run(["npm", "install", "--package-lock-only", "--ignore-scripts", "--offline",
                        "--no-audit", "--no-fund"], cwd=self.repo / "hub-web", env=self.env,
                       check=True, capture_output=True, text=True)
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "hub-web fixture with a local locked dependency")

    def test_hub_web_regeneration_installs_locked_dependencies_before_building(self):
        self.prepare_hub_web('node -e "const fs = require(\'node:fs\'); '
                             'fs.mkdirSync(\'dist\', {recursive: true}); '
                             'fs.writeFileSync(\'dist/app.txt\', require(\'regeneration-test-dependency\'))"')
        modules = self.repo / "hub-web/node_modules"
        shared = self.repo / ".git/shared-node-modules"
        shared.mkdir()
        (shared / "keep.txt").write_text("another checkout's installation")
        for state in ("absent", "stale", "shared-symlink"):
            with self.subTest(node_modules=state):
                if modules.exists():
                    shutil.rmtree(modules)
                if state == "stale":
                    modules.mkdir()
                elif state == "shared-symlink":
                    modules.symlink_to(shared, target_is_directory=True)
                before = self.git("rev-parse", "HEAD").stdout.strip()
                self.write("hub-web/src/app.ts", f"// {state}\n")
                self.git("add", "-A")
                self.git("commit", "-q", "-m", f"merge hub sources with {state} modules")
                after = self.git("rev-parse", "HEAD").stdout.strip()
                self.run_script("regenerate-generated-artifacts.sh", before, after)
                # pin: fixture dependency exports these bytes; the build must consume the installed package.
                self.assertEqual((self.repo / "hub-web/dist/app.txt").read_text(), "locked dependency")
                self.assertEqual(self.git("status", "--porcelain").stdout, "")
                self.assertFalse(modules.is_symlink(), "regeneration owns its checkout's dependencies")
                self.assertEqual([p.name for p in shared.iterdir()], ["keep.txt"],
                                 "another checkout's node_modules must stay untouched")

    def test_two_lanes_adding_builtin_history_merge_cleanly_and_the_ledger_equals_a_fresh_run(self):
        self.lane("factory/one", "first")
        self.lane("factory/two", "second")
        self.git("merge", "--no-ff", "-q", "factory/one", "-m", "merge one")
        before = self.git("rev-parse", "HEAD").stdout.strip()
        merged = self.git("merge", "--no-ff", "-q", "factory/two", "-m", "merge two", check=False)
        self.assertEqual(merged.returncode, 0, "the ledger must not conflict:\n" + merged.stdout + merged.stderr)
        after = self.git("rev-parse", "HEAD").stdout.strip()
        self.generator_runs()
        self.run_script("regenerate-generated-artifacts.sh", before, after)
        # The ledger commit adds no builtin content, so no second pass is paid.
        self.assertEqual(self.generator_runs(), 1)
        self.assert_ledger_fresh()
        ledger = json.loads((self.repo / LEDGER).read_text())["files"]
        self.assertTrue({"skills/demo/references/first.md", "skills/demo/references/second.md"} <= set(ledger))

    def test_merged_edit_then_revert_regenerates_once_and_ledger_equals_fresh_run(self):
        before = self.git("rev-parse", "HEAD").stdout.strip()
        self.git("checkout", "-q", "-b", "factory/reverted-builtin")
        builtin = "cas-cli/src/builtins/skills/demo/SKILL.md"
        original = (self.repo / builtin).read_text()
        self.write(builtin, original + "An intermediate shipped revision.\n")
        self.git("commit", "-q", "-am", "edit hashed builtin")
        self.write(builtin, original)
        self.git("commit", "-q", "-am", "revert hashed builtin")
        self.git("checkout", "-q", "main")
        self.git("merge", "--no-ff", "-q", "factory/reverted-builtin", "-m", "merge edit and revert")
        after = self.git("rev-parse", "HEAD").stdout.strip()
        self.assertEqual(self.git("diff", before, after, "--", builtin).stdout, "",
                         "the endpoint trees conceal the intermediate revision")
        self.generator_runs()
        self.run_script("regenerate-generated-artifacts.sh", before, after)
        self.assertEqual(self.generator_runs(), 1, "committed builtin history needs one regeneration")
        self.assert_ledger_fresh()

    def test_a_failed_hub_web_build_still_regenerates_the_ledger_and_is_reported(self):
        self.prepare_hub_web("echo vite exploded >&2; exit 3")
        (self.repo / "hub-web/node_modules").mkdir(parents=True)
        before = self.git("rev-parse", "HEAD").stdout.strip()
        self.write("hub-web/src/app.ts", "export {};\n")
        self.write("cas-cli/src/builtins/skills/demo/references/third.md", "# third\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "change hub-web and a builtin")
        after = self.git("rev-parse", "HEAD").stdout.strip()
        result = self.run_script("regenerate-generated-artifacts.sh", before, after, ok=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertRegex(result.stderr.splitlines()[0], r"hub-web/dist.*failed")
        # The merge receipt keeps only line one, so its log must survive exit.
        match = re.search(r"log: ([^)]+)", result.stderr.splitlines()[0])
        self.assertIsNotNone(match, result.stderr)
        log = Path(match[1])
        try:
            # pin: the fixture emits this diagnostic; the retained log must contain the actual build error.
            self.assertIn("vite exploded", log.read_text())
        finally:
            shutil.rmtree(log.parent)
        self.assert_ledger_fresh()
        ledger = json.loads((self.repo / LEDGER).read_text())["files"]
        self.assertIn("skills/demo/references/third.md", ledger)


if __name__ == "__main__":
    unittest.main()
