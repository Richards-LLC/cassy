#!/usr/bin/env python3
"""Execute the real web gate row and workflow classifier on isolated Git trees."""
import importlib.util
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location('classify_web_inputs', ROOT / 'scripts/classify-web-inputs.py')
WEB = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(WEB)


class WebGate(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.repo = Path(self.scratch.name) / 'repo'
        self.repo.mkdir()
        for name in ('release-gate.sh', 'release_scratch.py', 'release-portable.sh', 'release-test-env.sh', 'release-integration-gates.py', 'classify-ci-diff.sh', 'classify-fast-admission.sh', 'classify-web-inputs.py', 'resolve-ci-base.sh'):
            self.write('scripts/' + name, (ROOT / 'scripts' / name).read_text())
            (self.repo / 'scripts' / name).chmod(0o755)
        self.write('cas-cli/Cargo.toml', '[package]\nversion = "9.99.7"\n')
        self.write('hub-web/package.json', '{"name":"gate-fixture","private":true}\n')
        self.write('scripts/visual-qa.mjs', 'expected visual QA contract\n')
        self.write('.gitignore', '/npm-calls\n')
        self.write('npm-stub', '''#!/usr/bin/env python3
import os
from pathlib import Path
import sys
command = ' '.join(sys.argv[1:])
with Path('../npm-calls').open('a') as log:
    log.write(command + '\\n')
assert Path.cwd().name == 'hub-web'
if command == os.environ.get('FAIL_WEB_COMMAND'):
    sys.exit(1)
if command == 'test' and Path('../scripts/visual-qa.mjs').read_text() != 'expected visual QA contract\\n':
    print('visual-qa assertion drift', file=sys.stderr)
    sys.exit(1)
''')
        (self.repo / 'npm-stub').chmod(0o755)
        (self.repo / 'bin').mkdir()
        shutil.copy2(self.repo / 'npm-stub', self.repo / 'bin/npm')
        for args in (('init', '-q'), ('config', 'user.name', 'Fixture'), ('config', 'user.email', 'fixture@example.invalid')):
            self.git(*args)
        self.commit()
        self.base = self.git('rev-parse', 'HEAD').stdout.strip()

    def write(self, name, body):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body)

    def git(self, *args):
        result = subprocess.run(['git', *args], cwd=self.repo, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result

    def commit(self):
        self.git('add', '.')
        self.git('commit', '-qm', 'fixture')

    def gate(self, **env):
        return subprocess.run(['bash', 'scripts/release-gate.sh', '9.99.7', '--only', 'hub-web-tests'],
                              cwd=self.repo, env=dict(os.environ, NPM=str(self.repo / 'npm-stub'), **env),
                              text=True, capture_output=True)

    def classify(self):
        action = yaml.safe_load((ROOT / '.github/actions/classify-required-diff/action.yml').read_text())
        body = action['runs']['steps'][0]['run']
        output = Path(self.scratch.name) / 'outputs'
        # These fixtures classify an explicit base/head delta, not the outer
        # factory push. They have no origin/default/epic refs: inheriting that
        # event correctly makes resolve-ci-base fail closed and masks every
        # path assertion as web-needed. Real factory ancestry has its own suite.
        result = subprocess.run(['bash', '-euo', 'pipefail', '-c', body], cwd=self.repo,
                                env=dict(os.environ, BASE_SHA=self.base, ZERO_BASE_REF='', GITHUB_OUTPUT=str(output),
                                         GITHUB_EVENT_NAME='pull_request', GITHUB_REF='refs/pull/1/merge',
                                         FULL_VALIDATION='false'),
                                text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return dict(line.split('=', 1) for line in output.read_text().splitlines())

    def test_gate_installs_typechecks_and_tests_in_order(self):
        result = self.gate()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('PASS hub-web-tests', result.stdout)
        self.assertEqual((self.repo / 'npm-calls').read_text().splitlines(),
                         ['ci --no-audit --no-fund', 'run typecheck', 'test'])

    def test_ci_browser_install_uses_the_locked_local_cli(self):
        # Execute the actual workflow bodies, refusing any npm package download.
        # npm ci has already installed hub-web's locked CLI before these steps.
        self.write('bin/npm', '''#!/usr/bin/env python3
import os,sys
assert '--package' not in ' '.join(sys.argv), 'browser install must use the locked local CLI'
assert sys.argv[1:] in (['exec', '--', 'playwright', 'install', '--no-shell', 'chromium'],
                        ['exec', '--', 'playwright', 'install', '--with-deps', '--no-shell', 'chromium'],
                        ['ci'], ['run', 'visual-qa'],
                        ['run', 'journeys', '--', '--workers=2', '--shard=1/6']), sys.argv
if sys.argv[1:] == ['run', 'visual-qa']:
    assert os.environ.get('CHROME_PATH') == '/locked/chromium', 'visual QA must launch the installed full browser'
''')
        self.write('bin/npx', '''#!/usr/bin/env python3
import sys
assert sys.argv[1:] in (['playwright', 'install', '--no-shell', 'chromium'],
                        ['playwright', 'install', '--with-deps', '--no-shell', 'chromium']), sys.argv
''')
        self.write('bin/node', '''#!/usr/bin/env python3
import sys
assert sys.argv[1:] == ['-p', 'require("@playwright/test").chromium.executablePath()'], sys.argv
print('/locked/chromium')
''')
        self.write('bin/ldd', '''#!/usr/bin/env python3
import sys
assert sys.argv[1:] == ['/locked/chromium'], sys.argv
print('libfixture.so => /lib/libfixture.so')
''')
        # The journey step hands the shard to the real queue-flake runner,
        # which needs a clean tracked tree; outside the queue it runs the
        # journeys once through the same locked npm.
        self.write('scripts/ci-queue-flakes.py', (ROOT / 'scripts/ci-queue-flakes.py').read_text())
        workflow = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
        bodies = [step['run'] for job in workflow['jobs'].values()
                  for step in job.get('steps', []) if 'playwright install' in step.get('run', '')]
        self.assertTrue(bodies, 'no browser install steps found')
        for name in ('npm', 'npx', 'node', 'ldd'):
            (self.repo / 'bin' / name).chmod(0o755)
        self.commit()
        for body in bodies:
            with self.subTest(body=body):
                result = subprocess.run(['bash', '-eu', '-c', body], cwd=self.repo / 'hub-web',
                                        env=dict(os.environ, PATH=str(self.repo / 'bin') + os.pathsep + os.environ['PATH'], SHARD='1',
                                                 GITHUB_WORKSPACE=str(self.repo), GITHUB_EVENT_NAME='pull_request'),
                                        text=True, capture_output=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_visual_qa_drift_fails_named_row(self):
        self.write('scripts/visual-qa.mjs', 'drift\n')
        result = self.gate()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('FAIL hub-web-tests', result.stdout)

    def test_dependency_and_typecheck_failures_stop_before_tests(self):
        for command in ('ci --no-audit --no-fund', 'run typecheck'):
            with self.subTest(command=command):
                (self.repo / 'npm-calls').unlink(missing_ok=True)
                result = self.gate(FAIL_WEB_COMMAND=command)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn('FAIL hub-web-tests', result.stdout)
                self.assertNotIn('test', (self.repo / 'npm-calls').read_text().splitlines())

    def test_older_tree_without_web_package_has_visible_inapplicable_row(self):
        (self.repo / 'hub-web/package.json').unlink()
        result = self.gate()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('PASS hub-web-tests', result.stdout)
        self.assertFalse((self.repo / 'npm-calls').exists())

    def test_visual_qa_only_diff_publishes_web_signal(self):
        self.write('scripts/visual-qa.mjs', 'changed\n')
        self.commit()
        self.assertEqual(self.classify()['web-check-needed'], 'true')

    def test_document_fixture_read_by_web_tests_runs_web_tier(self):
        self.write('hub-web/fixtures/assertion.md', 'test input\n')
        self.commit()
        result = self.classify()
        self.assertEqual(result['web-check-needed'], 'true')
        self.assertNotEqual(result['class'], 'docs-only')

    def test_unrelated_rust_or_release_prose_skips_web_tests(self):
        for path in ('cas-cli/src/example.rs', 'docs/release-notes/example.md'):
            with self.subTest(path=path):
                self.write(path, '// unrelated\n')
                self.commit()
                self.assertEqual(self.classify()['web-check-needed'], 'false')
                self.base = self.git('rev-parse', 'HEAD').stdout.strip()

    def test_uncertain_diff_runs_web_tests(self):
        self.base = 'missing-base'
        self.assertEqual(self.classify()['web-check-needed'], 'true')

    def test_deleted_external_script_and_mixed_diff_keep_web_coverage(self):
        (self.repo / 'scripts/visual-qa.mjs').unlink()
        self.write('cas-cli/src/example.rs', '// Rust and deleted web input\n')
        self.commit()
        self.assertEqual(self.classify()['web-check-needed'], 'true')

    # cas-7822: the ~19 min Commander step runs only for its real inputs.
    def test_unrelated_script_or_workflow_skips_web_tests(self):
        # cas-a1b0: release plumbing (the former worker-memory wrapper closure) too.
        for path in ('scripts/test-jev-triage.py', '.github/workflows/release.yml', 'scripts/release-gate.sh',
                     'scripts/worker-memory.py', 'scripts/host_memory.py', 'scripts/assembly-proof.py',
                     'scripts/proof_target.py', 'scripts/release_scratch.py', 'scripts/release_env.py'):
            with self.subTest(path=path):
                self.write(path, '# unrelated to hub-web\n')
                self.commit()
                self.assertEqual(self.classify()['web-check-needed'], 'false')
                self.base = self.git('rev-parse', 'HEAD').stdout.strip()

    def test_journey_catalog_specs_and_repo_level_inputs_run_web_tests(self):
        for path in ('hub-web/src/main.ts', 'hub-web/dist/app.js', 'hub-web/e2e/journeys/reply.journey.ts',
                     'docs/qa/journeys.md', 'docs/design/design-tokens.json', 'scripts/classify-web-inputs.py'):
            with self.subTest(path=path):
                self.write(path, ('# ' if path.endswith('.py') else '') + 'changed web input\n')
                self.commit()
                result = self.classify()
                self.assertEqual(result['web-check-needed'], 'true')
                self.assertNotEqual(result['class'], 'docs-only')
                self.base = self.git('rev-parse', 'HEAD').stdout.strip()

    # cas-c846: PR #1198 changed only ci.yml outside hub-web and spent ~13 min
    # in the journeys. ci.yml is not an input; the wiring tests below pin it.
    def test_ci_workflow_only_change_skips_web_tests(self):
        self.write('.github/workflows/ci.yml', 'on: push\n')
        self.commit()
        self.assertEqual(self.classify()['web-check-needed'], 'false')

    def test_queue_bundle_ownership_cas_0acc(self):
        # Execute the classifier action and the actual queue build body. The
        # npm boundary models the production build, including a failing build.
        self.write('hub-web/src/main.ts', 'old source\n')
        self.write('hub-web/dist/app.js', 'old source\n' + (self.repo / 'cas-cli/Cargo.toml').read_text())
        self.write('bin/npm', '''#!/usr/bin/env python3
from pathlib import Path
import os, sys
assert sys.argv[1:] == ['run', 'build'], sys.argv
if os.environ.get('FAIL_BUILD') == 'true':
    sys.exit(7)
Path('dist/app.js').write_text(Path('src/main.ts').read_text() + Path('../cas-cli/Cargo.toml').read_text())
if os.environ.get('EXTRA_OUTPUT') == 'true':
    Path('dist/new-chunk.js').write_text('new chunk')
''')
        (self.repo / 'bin/npm').chmod(0o755)
        # The build body runs through the real queue step; this models a
        # non-queue event, where that step runs the build once.
        self.write('scripts/ci-queue-flakes.py', (ROOT / 'scripts/ci-queue-flakes.py').read_text())
        self.commit()
        seed = self.git('rev-parse', 'HEAD').stdout.strip()
        workflow = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
        build = next(step for step in workflow['jobs']['fast-validation-web']['steps']
                     if step.get('id') == 'build')
        action = yaml.safe_load((ROOT / '.github/actions/classify-required-diff/action.yml').read_text())
        self.assertEqual(action['outputs']['web-dist-check-needed']['value'],
                         '${{ steps.classify.outputs.web-dist-check-needed }}')
        self.assertEqual(workflow['jobs']['ci-diff']['outputs']['web-dist-check-needed'],
                         '${{ steps.classify-diff.outputs.web-dist-check-needed }}')
        # Same source-only delivery, then each reason it must retain equality.
        cases = [
            ('source-only', 'source', False, False, 'false', 0),
            ('version-only stale release', 'version', False, False, 'true', 1),
            ('matching release', 'matched-version', False, False, 'true', 0),
            ('changed integration dist', 'dist', False, False, 'true', 1),
            ('unknown base', 'source', True, False, 'true', 1),
            ('unreadable version', 'invalid-version', False, False, 'true', 1),
            ('failed source build', 'source', False, True, 'false', 7),
            ('new release output', 'new-output', False, False, 'true', 1),
        ]
        for label, change, unknown, fail_build, equality, exit_code in cases:
            with self.subTest(label=label):
                self.git('reset', '--hard', seed)
                self.base = 'missing-base' if unknown else seed
                if change in ('version', 'matched-version', 'new-output'):
                    self.write('cas-cli/Cargo.toml', '[package]\nversion = "9.99.8"\n')
                    if change in ('matched-version', 'new-output'):
                        self.write('hub-web/dist/app.js', 'old source\n' + (self.repo / 'cas-cli/Cargo.toml').read_text())
                elif change == 'dist':
                    self.write('hub-web/dist/app.js', 'wrong integration bundle\n')
                elif change == 'invalid-version':
                    self.write('cas-cli/Cargo.toml', '[package]\nversion.workspace = true\n')
                else:
                    self.write('hub-web/src/main.ts', 'new source\n')
                self.commit()
                decision = self.classify()
                self.assertEqual(decision['web-check-needed'], 'true')
                result = subprocess.run(['bash', '-e', '-o', 'pipefail', '-c', build['run']],
                                        cwd=self.repo / 'hub-web', text=True, capture_output=True,
                                        env=dict(os.environ, PATH=str(self.repo / 'bin') + os.pathsep + os.environ['PATH'],
                                                 CHECK_COMMITTED_DIST=decision.get('web-dist-check-needed', ''),
                                                 GITHUB_EVENT_NAME='pull_request', GITHUB_REF='refs/pull/1/merge',
                                                 EXTRA_OUTPUT=str(change == 'new-output').lower(),
                                                 FAIL_BUILD=str(fail_build).lower()))
                self.assertEqual(result.returncode, exit_code, result.stdout + result.stderr)
                self.assertEqual(decision['web-dist-check-needed'], equality)
                self.assertEqual(build['env']['CHECK_COMMITTED_DIST'],
                                 '${{ needs.ci-diff.outputs.web-dist-check-needed }}')

    def test_classifier_cli_ci_workflow_false_and_web_inputs_true(self):
        script = str(ROOT / 'scripts/classify-web-inputs.py')  # the fixture copy may be overwritten below
        cases = [('.github/workflows/ci.yml', 'false')]
        cases += [(entry + 'src/main.ts' if entry.endswith('/') else entry, 'true') for entry in WEB.WEB_INPUTS]
        for path, expected in cases:
            with self.subTest(path=path):
                self.write(path, ('# ' if path.endswith(('.py', '.sh', '.yml')) else '') + 'changed\n')
                self.commit()
                result = subprocess.run(['python3', script, self.base, 'HEAD'], cwd=self.repo,
                                        text=True, capture_output=True)
                self.assertEqual((result.returncode, result.stdout), (0, expected + '\n'), result.stderr)
                self.base = self.git('rev-parse', 'HEAD').stdout.strip()

    def test_failed_web_classifier_runs_web_tests(self):
        self.write('scripts/classify-web-inputs.py', 'raise SystemExit(1)\n')
        self.write('scripts/unrelated.py', '# unrelated\n')
        self.commit()
        self.base = self.git('rev-parse', 'HEAD').stdout.strip()
        self.write('scripts/unrelated.py', '# still unrelated\n')
        self.commit()
        self.assertEqual(self.classify()['web-check-needed'], 'true')

    def test_every_repo_level_file_hub_web_reads_is_a_web_input(self):
        # A new `../scripts/x` or `../docs/x` read in hub-web must join WEB_INPUTS.
        reference = re.compile(r'((?:\.\./)+(?:scripts|docs)/[\w./-]+\.\w+)')
        sources = [ROOT / 'hub-web/package.json', *ROOT.glob('hub-web/*.ts')]
        for folder in ('src', 'scripts', 'e2e', 'fixtures'):
            sources += [p for p in (ROOT / 'hub-web' / folder).rglob('*')
                        if p.suffix in ('.ts', '.mjs', '.js', '.json') and 'node_modules' not in p.parts]
        found = set()
        for source in sources:
            for relative in reference.findall(source.read_text(errors='replace')):
                # Module URLs resolve from the file; npm scripts and test reads from hub-web.
                for base in (source.parent, ROOT / 'hub-web'):
                    target = (base / relative).resolve()
                    if target.exists() and target.is_relative_to(ROOT) and not target.is_relative_to(ROOT / 'hub-web'):
                        found.add(str(target.relative_to(ROOT)))
        workflow = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
        for job in workflow['jobs'].values():
            default = job.get('defaults', {}).get('run', {}).get('working-directory')
            for step in job.get('steps', []):
                if step.get('working-directory', default) == 'hub-web':
                    found.update(str((ROOT / 'hub-web' / r).resolve().relative_to(ROOT))
                                 for r in reference.findall(step.get('run', '')))
        self.assertIn('scripts/visual-qa.mjs', found)
        self.assertNotIn('scripts/worker-memory.py', found)
        self.assertIn('docs/design/design-tokens.json', found)
        self.assertIn('scripts/test-ci-journey-evidence-upload.sh', found)
        missing = sorted(path for path in found if not WEB.covered(path))
        self.assertEqual(missing, [], 'add these hub-web inputs to scripts/classify-web-inputs.py WEB_INPUTS')

    def test_pr_1191_merge_queue_entry_skips_web_tests(self):
        # Real history: PR #1191 touched scripts/test-jev-triage.py and no web input.
        base, head = '1c201ff0692dbf182b6fa8eb948abcc284166a84', 'e8f0c4628d4c8f96b806ed9cbf3c6209020f2371'
        if subprocess.run(['git', 'cat-file', '-e', head], cwd=ROOT, capture_output=True).returncode:
            self.skipTest('shallow checkout lacks PR #1191')
        result = subprocess.run([str(ROOT / 'scripts/classify-web-inputs.py'), base, head], cwd=ROOT,
                                text=True, capture_output=True)
        self.assertEqual((result.returncode, result.stdout), (0, 'false\n'), result.stderr)
        result = subprocess.run([str(ROOT / 'scripts/classify-web-inputs.py'), '7c233bef^', '7c233bef'], cwd=ROOT,
                                text=True, capture_output=True)
        self.assertEqual((result.returncode, result.stdout), (0, 'true\n'), result.stderr)

if __name__ == '__main__':
    unittest.main()
