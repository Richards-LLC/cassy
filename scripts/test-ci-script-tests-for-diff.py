#!/usr/bin/env python3
"""Selection of script-tier tests, including real git diffs and selector calls."""
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    "script_tests", Path(__file__).resolve().parent / "ci-script-tests-for-diff.py")
SELECTOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SELECTOR)
TIER = ["python3 scripts/test-worker-memory.py", "python3 scripts/test-example.py",
        "python3 scripts/test-release-portable.py"]


class Selection(unittest.TestCase):
    def test_shared_walk_kit_selects_its_suite_once(self):
        commands = SELECTOR.tier_commands(Path(__file__).resolve().parents[1])
        kit = "cas-cli/src/builtins/skills/cas-qa-craft/scripts/walk-kit.mjs"
        suite = "scripts/test-walk-kit.mjs"
        for changed in [[kit], [suite], [kit, suite], ["docs/unrelated.md"]]:
            selected = SELECTOR.select(commands, changed)
            selected = [(test, command) for test, command in selected if test == suite]
            self.assertEqual(len(selected), 0 if changed == ["docs/unrelated.md"] else 1)
            if selected:
                self.assertEqual(selected[0][1], "node --test scripts/test-walk-kit.mjs")

    def test_provisioned_suites_follow_real_pr_inputs_and_ignore_unrelated_pr(self):
        # Independent consumer inputs, not a copy of the selector's registry.
        inputs = {
            'test-cas-image-generate.sh': 'cas-cli/src/builtins/skills/cas-image-generate/scripts/generate-image.sh',
            'test-check-changelog-lint.sh': '.markdownlint-cli2.jsonc',
            'test-codex-cargo-admission.py': '.codex/hooks.json',
            'test-factory-client-terminal.py': 'cas-cli/src/ui/factory/client.rs',
            'test-journey-bundles.py': 'scripts/journey-bundles.py',
            'test-opencode-slack-policy.mjs': 'crates/cas-mux/src/opencode.rs',
            'test-release-budget-breach.py': 'scripts/release-metrics.py',
            'test-release-report-post.py': 'scripts/release-report-post.py',
            'test-release-report.py': 'cas-cli/src/deliverable/assets/check-pdf.py',
            'test-verified-web-runners.mjs': 'hub-web/scripts/run-verified-tests.mjs',
            'test_factory_model_history.py': 'scripts/factory-model-history.py',
            **{f'visual-qa{suffix}.test.mjs': 'scripts/visual-qa.mjs' for suffix in
               ['', '-enter-transition', '-entrypoint', '-readiness', '-secrets']},
        }
        with tempfile.TemporaryDirectory(prefix='script-pr-selection-') as directory:
            repo = Path(directory)
            (repo / 'cas-cli').mkdir()
            shutil.copyfile(Path(__file__).resolve().parents[1] / 'cas-cli/Makefile', repo / 'cas-cli/Makefile')
            transition = 'scripts/visual-qa-fixtures/enter-transition.html'
            clean = 'scripts/visual-qa-fixtures/clean.html'
            allowlist = 'hub-web/visual-qa-allowlist.json'
            paths = {*inputs.values(), *(f'scripts/{name}' for name in inputs),
                     'docs/unrelated.md', transition, clean, allowlist}
            for path in paths:
                file = repo / path
                file.parent.mkdir(parents=True, exist_ok=True)
                file.write_text('baseline\n')
            def git(*args):
                subprocess.run(['git', '-C', str(repo), *args], check=True, capture_output=True)
            git('init', '-q')
            git('add', '.')
            git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                '-c', 'commit.gpgsign=false', 'commit', '-qm', 'PR base')
            def selected(changed, comparison=None):
                for path in changed:
                    (repo / path).write_text('PR head\n')
                git('add', '.')
                git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                    '-c', 'commit.gpgsign=false', 'commit', '-qm', 'PR head')
                result = subprocess.run([sys.executable, SELECTOR.__file__, '--repo', str(repo),
                    *(comparison or ['--base', 'HEAD^', '--provisioned']), '--dry-run'],
                    capture_output=True, text=True)
                git('reset', '--hard', 'HEAD^')
                self.assertEqual(result.returncode, 0, result.stderr)
                return result.stdout
            for name, subject in inputs.items():
                for path in [f'scripts/{name}', subject]:
                    with self.subTest(suite=name, changed=path):
                        output = selected([path])
                        expected = ({f'scripts/{key}' for key, value in inputs.items() if value == subject}
                                    if path == subject else {f'scripts/{name}'})
                        actual = {SELECTOR.test_script(line.split('would run ', 1)[1])
                                  for line in output.splitlines() if 'would run ' in line}
                        self.assertEqual(actual, expected, output)
            self.assertIn('no script test covers', selected(['docs/unrelated.md']))
            output = selected([transition])
            actual = {SELECTOR.test_script(line.split('would run ', 1)[1])
                      for line in output.splitlines() if 'would run ' in line}
            self.assertEqual(actual, {'scripts/visual-qa.test.mjs', 'scripts/visual-qa-enter-transition.test.mjs'})
            # The integration tier keeps its other machinery checks, while
            # visual-QA tool fixtures follow their own inputs. An unknown base
            # must retain all fixture coverage, like exhaustive assembly.
            for changed, base, expected in [
                (['docs/unrelated.md'], 'HEAD^', set()),
                (['scripts/visual-qa.mjs'], 'HEAD^',
                 {f'scripts/{name}' for name in inputs if name.startswith('visual-qa')}),
                ([transition], 'HEAD^',
                 {'scripts/visual-qa.test.mjs', 'scripts/visual-qa-enter-transition.test.mjs'}),
                ([clean], 'HEAD^',
                 {'scripts/visual-qa.test.mjs', 'scripts/visual-qa-entrypoint.test.mjs',
                  'scripts/visual-qa-secrets.test.mjs'}),
                ([allowlist], 'HEAD^', {'scripts/visual-qa.test.mjs'}),
                (['docs/unrelated.md'], 'missing-base',
                 {f'scripts/{name}' for name in inputs if name.startswith('visual-qa')}),
            ]:
                with self.subTest(integration_inputs=changed, base=base):
                    output = selected(changed, ['--all', '--scope-base', base])
                    actual = {SELECTOR.test_script(line.split('would run ', 1)[1])
                              for line in output.splitlines() if 'would run ' in line
                              and 'visual-qa' in line}
                    self.assertEqual(actual, expected, output)
                    self.assertIn('would run python3 scripts/test-proof-target.py', output)

    def test_terminal_qa_inputs_select_the_real_tier_command_once(self):
        terminal = "scripts/terminal-qa.test.mjs"
        builtin = "cas-cli/src/builtins/skills/cas-cli-craft/scripts/terminal-qa.mjs"
        runner = "scripts/terminal-qa.mjs"
        unrelated = "scripts/unrelated.txt"
        other_test = "scripts/test-proof-target.py"
        paths = [runner, terminal, builtin, unrelated, other_test]
        with tempfile.TemporaryDirectory(prefix="terminal-qa-selection-") as directory:
            repo = Path(directory)
            (repo / "cas-cli").mkdir()
            shutil.copyfile(Path(__file__).resolve().parents[1] / "cas-cli/Makefile",
                            repo / "cas-cli/Makefile")
            for path in paths:
                file = repo / path
                file.parent.mkdir(parents=True, exist_ok=True)
                file.write_text("baseline\n")
            def git(*args):
                subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True)
            git("init", "-q")
            git("add", ".")
            git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                "-c", "commit.gpgsign=false", "commit", "-qm", "base")
            for changed, expected in [
                ([runner], True), ([terminal], True), ([builtin], True),
                ([runner, builtin, other_test], True), ([unrelated, other_test], False),
            ]:
                with self.subTest(changed=changed):
                    for path in changed:
                        (repo / path).write_text("changed\n")
                    output = ("ci-script-tests-changed: would run node --test " + terminal + "\n"
                              if expected else "ci-script-tests-changed: no script test covers this lane's changes\n")
                    modes = [[], ["--only", terminal]] if other_test not in changed else [["--only", terminal]]
                    for mode in modes:
                        result = subprocess.run(
                            [sys.executable, SELECTOR.__file__, "--repo", str(repo),
                             "--base", "HEAD", "--dry-run", *mode],
                            capture_output=True, text=True,
                        )
                        self.assertEqual(result.returncode, 0, result.stderr)
                        self.assertEqual(result.stdout, output)
                    git("restore", ".")

    def test_package_scripts_select_the_named_weight_assertion_not_the_suite_cas_eac2(self):
        # cas-2077 changed hub-web/package.json; the lane selected no script test
        # and the merge queue found 4 GiB != 2 GiB.
        selected = SELECTOR.select(TIER, ["hub-web/package.json"])
        self.assertEqual(selected, [("hub-web/package.json", SELECTOR.NAMED["hub-web/package.json"])])
        self.assertIn("AdmissionTests.test_cas_4cb9_weights_follow_shell_and_actual_package_scripts",
                      selected[0][1])
        self.assertEqual(SELECTOR.select(TIER, ["hub-web/src/app.ts"]), [])

    def test_suites_inside_a_train_step_ignore_the_outer_job_cas_f235(self):
        # Train run 42351777395: suites inherited FULL_SET and the train event,
        # so fixture repositories classified as the outer full set or re-wrapped
        # their selector in a queue step that the fixture does not contain.
        with tempfile.TemporaryDirectory(prefix='script-suite-env-') as directory:
            repo = Path(directory)
            (repo / 'cas-cli').mkdir()
            (repo / 'scripts').mkdir()
            (repo / 'cas-cli/Makefile').write_text('test-ci-tiers:\n\tcd .. && python3 scripts/test-probe.py\n')
            (repo / 'scripts/test-probe.py').write_text(
                "import os\nassert not {'FULL_SET', 'BASE_SHA'} & set(os.environ), os.environ\n")
            env = dict(os.environ, GITHUB_EVENT_NAME='push', GITHUB_REF='refs/heads/epic/release-train-x',
                       CAS_QUEUE_STEP='scripts/test-fast-release-rows.py', FULL_SET='true', BASE_SHA='abc')
            result = subprocess.run([sys.executable, SPEC.origin, '--repo', str(repo), '--all',
                                     '--only', 'scripts/test-probe.py'],
                                    env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn('PASS python3 scripts/test-probe.py', result.stdout)


if __name__ == "__main__":
    unittest.main()
