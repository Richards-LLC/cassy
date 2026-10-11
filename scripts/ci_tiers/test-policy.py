#!/usr/bin/env python3
"""Positive controls, parsed-data mutations and real routing/failure bodies."""
import copy
import itertools
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import policy


class PolicyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.entries = json.loads((policy.HERE / 'policy.json').read_text())
        prose = json.loads((policy.HERE / 'prose-pins.json').read_text())
        cls.files = set()
        for entry in cls.entries + prose:
            cls.files.update(entry['source'].get('files', [entry['source'].get('file')]))

    def setUp(self):
        self.owner = tempfile.TemporaryDirectory(prefix='ci-policy-')
        self.addCleanup(self.owner.cleanup)
        self.root = Path(self.owner.name)
        for file in self.files:
            destination = self.root / file
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(policy.ROOT / file, destination)

    def write(self, file, data):
        (self.root / file).write_text(policy.yaml.safe_dump(data, sort_keys=False, indent=4, width=1000))
        policy.load.cache_clear()

    def ci(self):
        return copy.deepcopy(policy.load(self.root / '.github/workflows/ci.yml'))

    def test_current_policy_and_all_registered_prose_pass(self):
        self.assertEqual(policy.validate(self.root, verbose=False), [])

    def test_yaml_reindentation_and_non_consumed_step_renames_are_harmless(self):
        for file in self.files:
            if not file.endswith(('.yml', '.yaml')): continue
            data = copy.deepcopy(policy.load(self.root / file))
            for job in data.get('jobs', {}).values():
                for i, step in enumerate(job.get('steps', [])):
                    if 'name' in step: step['name'] = f'Harmless display label {i}'
            self.write(file, data)
        rules = self.root / 'docs/branch-protection/main-ruleset.json'
        rules.write_text(json.dumps(json.loads(rules.read_text()), separators=(',', ':')))
        policy.load.cache_clear()
        self.assertEqual(policy.validate(self.root, verbose=False), [])

    def test_public_required_context_rename_is_rejected(self):
        data = self.ci()
        data['jobs']['fast-validation']['name'] = 'A different public check'
        self.write('.github/workflows/ci.yml', data)
        self.assertTrue(policy.validate(self.root, verbose=False))

    def test_trigger_comment_cannot_replace_the_actual_factory_trigger(self):
        data = self.ci()
        data['on']['push']['branches'].remove('factory/**')
        self.write('.github/workflows/ci.yml', data)
        with (self.root / '.github/workflows/ci.yml').open('a') as stream:
            stream.write('\n# - "factory/**"\n')
        policy.load.cache_clear()
        failed = policy.validate(self.root, verbose=False)
        self.assertIn('factory pushes trigger CI', [entry['consumer'] for entry, _ in failed])

    def test_comment_and_display_name_cannot_replace_an_executable_step(self):
        for body in ['# make -C cas-cli test-ci-tiers\ntrue\n', 'true # make -C cas-cli test-ci-tiers\n']:
            with self.subTest(body=body):
                data = self.ci()
                step = next(s for s in data['jobs']['fast-validation-preflight']['steps'] if 'make -C cas-cli test-ci-tiers' in s.get('run', ''))
                step['name'] = 'make -C cas-cli test-ci-tiers'
                step['run'] = body
                self.write('.github/workflows/ci.yml', data)
                self.assertTrue(policy.validate(self.root, verbose=False))
                shutil.copyfile(policy.ROOT / '.github/workflows/ci.yml', self.root / '.github/workflows/ci.yml')
                policy.load.cache_clear()

    def test_required_context_in_a_note_cannot_replace_a_real_context(self):
        file = self.root / 'docs/branch-protection/main-ruleset.json'
        data = json.loads(file.read_text())
        status = next(r for r in data['rules'] if r['type'] == 'required_status_checks')
        status['parameters']['required_status_checks'] = [r for r in status['parameters']['required_status_checks'] if r['context'] != 'Fast Validation']
        data['note'] = '"context": "Fast Validation"'
        file.write_text(json.dumps(data));policy.load.cache_clear()
        self.assertTrue(policy.validate(self.root, verbose=False))

    def test_parser_dependency_absence_is_a_hard_failure(self):
        result = subprocess.run([sys.executable, '-S', str(policy.HERE / 'policy.py'), 'check'], text=True, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('requires pyyaml==6.0.3', result.stderr)

    def test_real_parser_provisioning_step_reuses_yaml_or_isolates_installation(self):
        step = next(s for s in self.ci()['jobs']['fast-validation-preflight']['steps']
                    if 'make -C cas-cli test-ci-tiers' in s.get('run', ''))
        # Isolate tool orchestration and the pip network boundary. The make
        # fixture runs the actual parsed-policy suite, without recursive make.
        interpreter = shlex.quote(sys.executable)
        parser = shlex.quote(str(policy.HERE / 'policy.py'))
        for mode in ['present', 'absent', 'venv-failure', 'install-failure']:
            with self.subTest(mode=mode):
                fixture = self.root / mode;fixture.mkdir()
                bin_dir = fixture / 'bin';bin_dir.mkdir()
                runner_temp = fixture / 'runner';runner_temp.mkdir()
                log = fixture / 'calls'
                log.write_text('')
                (bin_dir / 'python3').write_text('''#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == -c ]]; then
  echo import >> "$CALLS"
  [[ "$MODE" == present ]]
elif [[ "$1 $2" == '-m venv' ]]; then
  echo venv >> "$CALLS"
  [[ "$MODE" != venv-failure ]] || exit 1
  mkdir -p "$3/bin"
  cp "$FIXTURE/pip" "$3/bin/pip"
  cp "$FIXTURE/venv-python" "$3/bin/python3"
else
  printf 'unexpected python %s\\n' "$*" >> "$CALLS"
  exit 2
fi
''')
                (fixture / 'pip').write_text('''#!/usr/bin/env bash
set -euo pipefail
printf 'pip %s\\n' "$*" >> "$CALLS"
[[ "$MODE" != install-failure ]]
''')
                (fixture / 'venv-python').write_text(f'#!/usr/bin/env bash\nexec {interpreter} "$@"\n')
                (bin_dir / 'make').write_text(f'''#!/usr/bin/env bash
set -euo pipefail
[[ "$*" == '-C cas-cli test-ci-tiers' ]]
echo make >> "$CALLS"
command -v python3 >> "$CALLS"
if [[ "$MODE" == present ]]; then
  {interpreter} {parser} check
else
  python3 {parser} check
fi
''')
                for child in [*bin_dir.iterdir(), fixture / 'pip', fixture / 'venv-python']:
                    child.chmod(0o755)
                result = self.run_body(step['run'], dict(
                    PATH=f'{bin_dir}:/usr/bin:/bin', RUNNER_TEMP=str(runner_temp),
                    CALLS=str(log), MODE=mode, FIXTURE=str(fixture)))
                calls = log.read_text().splitlines()
                if mode in ['venv-failure', 'install-failure']:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertNotIn('make', calls)
                    if mode == 'venv-failure': self.assertEqual(calls, ['import', 'venv'])
                else:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    count = len(self.entries) + len(json.loads((policy.HERE / 'prose-pins.json').read_text()))
                    self.assertIn(f'parsed policy/prose: {count} passed; 0 failed', result.stdout)
                    if mode == 'present':
                        self.assertEqual(calls, ['import', 'make', str(bin_dir / 'python3')])
                        self.assertFalse((runner_temp / 'ci-tiers-venv').exists())
                    else:
                        self.assertEqual(calls, ['import', 'venv',
                            'pip install --disable-pip-version-check --quiet -r scripts/ci_tiers/requirements.txt',
                            'make', str(runner_temp / 'ci-tiers-venv/bin/python3')])

    def test_parsed_archive_fail_open_contract_and_all_four_mutations(self):
        job = self.ci()['jobs']['fast-validation-suite-build']
        source = {'file': '.github/workflows/ci.yml', 'job': 'fast-validation-suite-build'}
        probe_i, _ = policy.role(job, 'Verify private self-hosted sccache', source)
        disable_i, _ = policy.role(job, 'Disable sccache for the self-hosted suite archive', source)
        archive_i, _ = policy.role(job, 'Build full suite archive', source)
        def valid(candidate):
            probe, disable = candidate['steps'][probe_i], candidate['steps'][disable_i]
            return probe.get('continue-on-error') is True and "steps.classify-diff.outputs.rust-unaffected != 'true'" in disable['if'] and "needs.fast-validation-runner-route.outputs.mode == 'self-hosted'" in disable['if'] and "echo 'RUSTC_WRAPPER='" in policy.uncomment(disable['run']) and disable_i < archive_i
        self.assertTrue(valid(job), 'positive control for the actual parsed archive contract')
        changed = copy.deepcopy(job);changed['steps'][probe_i].pop('continue-on-error')
        self.assertFalse(valid(changed), 'legacy removed probe fail-open mutation')
        changed = copy.deepcopy(job);changed['steps'][disable_i]['if'] = changed['steps'][disable_i]['if'].replace('self-hosted', 'hosted')
        self.assertFalse(valid(changed), 'legacy removed self-hosted gate mutation')
        changed = copy.deepcopy(job);changed['steps'][disable_i]['run'] = changed['steps'][disable_i]['run'].replace('RUSTC_WRAPPER=', 'RUSTC_WRAPPER=sccache')
        self.assertFalse(valid(changed), 'legacy restored compiler wrapper mutation')
        # Reordering mutates the real steps list, then resolves actual positions.
        changed = copy.deepcopy(job);changed['steps'][disable_i], changed['steps'][archive_i] = changed['steps'][archive_i], changed['steps'][disable_i]
        actual_disable, _ = policy.role(changed, 'Disable sccache for the self-hosted suite archive', source)
        actual_archive, _ = policy.role(changed, 'Build full suite archive', source)
        self.assertGreater(actual_disable, actual_archive)

    def test_heavy_concurrency_positive_controls_and_all_fifteen_mutations(self):
        jobs = self.ci()['jobs']
        for name in ['clippy', 'test-compile-guard', 'panic-isolation-release', 'panic-isolation-release-fast', 'build-benchmark']:
            with self.subTest(job=name):
                job = jobs[name]
                group = f'heavy-tier-{name}-${{{{ github.event_name }}}}-${{{{ github.ref }}}}'
                def valid(candidate):
                    return candidate.get('concurrency', {}).get('group') == group and candidate.get('concurrency', {}).get('cancel-in-progress') is True and all(ref not in candidate['if'] for ref in ['refs/heads/epic/', 'refs/heads/factory/'])
                self.assertTrue(valid(job))
                changed = copy.deepcopy(job);changed['concurrency'].pop('group')
                self.assertFalse(valid(changed))
                changed = copy.deepcopy(job);changed['concurrency']['cancel-in-progress'] = False
                self.assertFalse(valid(changed))
                changed = copy.deepcopy(job);changed['if'] += " || startsWith(github.ref, 'refs/heads/epic/')"
                self.assertFalse(valid(changed))

    def test_superseded_factory_branch_runs_cancel_and_nothing_else_groups_cas_12ab(self):
        factory_group = "${{ startsWith(github.ref, 'refs/heads/factory/') && format('ci-factory-{0}', github.ref) || format('ci-run-{0}', github.run_id) }}"
        factory_cancel = "${{ startsWith(github.ref, 'refs/heads/factory/') }}"
        def valid(workflow):
            concurrency = workflow.get('concurrency', {})
            return (concurrency.get('group') == factory_group
                    and concurrency.get('cancel-in-progress') == factory_cancel)
        workflow = self.ci()
        self.assertTrue(valid(workflow))
        for mutate in (
            lambda w: w.pop('concurrency'),
            lambda w: w['concurrency'].__setitem__('cancel-in-progress', True),
            lambda w: w['concurrency'].__setitem__('group', 'ci-${{ github.ref }}'),
            lambda w: w['concurrency'].__setitem__('cancel-in-progress', False),
        ):
            changed = copy.deepcopy(workflow)
            mutate(changed)
            self.assertFalse(valid(changed))

    def test_queue_full_gate_reuse_skips_linux_lanes_but_never_macos_cas_4cb8(self):
        jobs = self.ci()['jobs']
        dedupe = jobs['fast-validation-main-push-dedupe']
        self.assertEqual(dedupe['outputs']['reuse-source'], '${{ steps.tree-dedupe.outputs.reuse-source }}')
        reuse = "needs.fast-validation-main-push-dedupe.outputs.reuse-source == 'full-gate'"
        run = "needs.fast-validation-main-push-dedupe.outputs.run-fast-validation == 'true'"
        def macos_runs_on_reuse(job):
            return reuse in job['if'] and run in job['if']
        self.assertTrue(macos_runs_on_reuse(jobs['macos-check']))
        changed = copy.deepcopy(jobs['macos-check']); changed['if'] = changed['if'].replace(reuse, "false")
        self.assertFalse(macos_runs_on_reuse(changed))
        # The suite and doctests are proven by the full gate and skip on reuse.
        for name in ['fast-validation-suite-build', 'fast-validation-suite-shards',
                     'fast-validation-suite', 'fast-validation-docs']:
            with self.subTest(job=name):
                self.assertIn(run, jobs[name]['if'])
                self.assertNotIn('full-gate', jobs[name]['if'])
        # The preflight still runs on reuse: the full gate runs no journeys.
        preflight = jobs['fast-validation-preflight']
        self.assertIn(reuse, preflight['if'])
        by_name = {step.get('name', ''): step for step in preflight['steps']}
        web = by_name['Build and test Commander web assets']
        self.assertEqual(web['env']['FULL_GATE_REUSE'], "${{ " + reuse + " }}")
        self.assertIn('npm run journeys -- --workers=4', web['run'].split('exit 0', 1)[0])
        guards = by_name['Test release publication guards']
        self.assertIn("reuse-source != 'full-gate'", guards['if'])
        for kept in ['Check', 'Test portable x86_64 ISA audit', 'Build (without MCP proxy)']:
            self.assertNotIn('full-gate', by_name[kept].get('if', ''), kept)
        rollup = {step.get('name', ''): step for step in jobs['fast-validation']['steps']}
        report = rollup['Report the full-gate receipt reused by this merge-queue run']
        self.assertIn('test "$PREFLIGHT" = success', report['run'])

    def test_every_cache_summary_call_has_its_executable_skew_guard(self):
        sources = ['.github/actions/setup-rust-linux/action.yml', '.github/workflows/ci.yml', '.github/workflows/release.yml']
        def counts():
            values = [node for source in sources for node in policy.nodes(policy.resolve(self.root, {'file': source})) if isinstance(node, str)]
            bodies = [policy.uncomment(value) for value in values]
            return sum(body.count('./scripts/ci-sccache-summary.sh "') for body in bodies), sum(body.count('if [[ -x ./scripts/ci-sccache-summary.sh ]]; then') for body in bodies)
        invocations, guards = counts()
        self.assertGreater(invocations, 0);self.assertEqual(invocations, guards)
        data = self.ci()
        step = next(s for s in data['jobs']['fast-validation-preflight']['steps'] if 'if [[ -x ./scripts/ci-sccache-summary.sh' in s.get('run', ''))
        step['run'] = step['run'].replace('if [[ -x ./scripts/ci-sccache-summary.sh ]]; then', 'if true; then')
        self.write('.github/workflows/ci.yml', data)
        self.assertNotEqual(*counts())

    def run_body(self, body, values):
        environment = os.environ.copy();environment.update(values)
        environment['HOME'] = str(self.root)
        # Resolve the host-selected Bash before fixture PATH isolation. On
        # Darwin /usr/bin:/bin otherwise replaces Homebrew Bash with Bash 3.2,
        # which cannot execute these Ubuntu workflow bodies (e.g. mapfile).
        bash = shutil.which('bash')
        return subprocess.run([bash, '-euo', 'pipefail', '-c', body], env=environment, cwd=self.root, capture_output=True, text=True)

    def test_release_shared_rustup_wiring_rejects_unsafe_route_mutations(self):
        self.assertEqual(policy.validate(self.root, verbose=False), [])
        for file, job in [('.github/workflows/release.yml', 'verify'),
                          ('.github/workflows/release.yml', 'build'),
                          ('.github/workflows/release-prebuild.yml', 'build')]:
            original = copy.deepcopy(policy.load(self.root / file))
            for mutation in ['remove-helper', 'unguarded-action', 'wrong-helper-route']:
                with self.subTest(job=job, mutation=mutation):
                    data = copy.deepcopy(original)
                    steps = data['jobs'][job]['steps']
                    helper = next(s for s in steps if s.get('run') == './scripts/setup-cassy-actions-rust.sh')
                    action = next(s for s in steps if s.get('uses') == 'dtolnay/rust-toolchain@stable')
                    if mutation == 'remove-helper': steps.remove(helper)
                    elif mutation == 'unguarded-action': action.pop('if')
                    else: helper['if'] = helper['if'].replace("'self-hosted'", "'hosted'")
                    self.write(file, data)
                    failures = policy.validate(self.root, verbose=False)
                    self.assertTrue(any('shared Rust' in e['consumer'] for e, _ in failures))
            self.write(file, original)

    def rustup_fixture(self, state):
        fixture = self.root / state
        fixture.mkdir()
        rustup_home = fixture / 'rustup'; rustup_home.mkdir()
        bin_dir = fixture / 'bin'; bin_dir.mkdir()
        # Exercise the actual shell helper with the host's POSIX file locks,
        # without requiring Linux's flock executable on Darwin.
        flock = bin_dir / 'flock'
        flock.write_text(f'#!{sys.executable}\nimport fcntl, sys\nassert len(sys.argv) == 3\nfcntl.flock(int(sys.argv[2]), fcntl.LOCK_EX if sys.argv[1] == "-x" else fcntl.LOCK_UN)\n')
        fake = fixture / 'rustup-fixture'
        fake.write_text(f'''#!{sys.executable}
import os, sys, time
from pathlib import Path
home = Path(os.environ['RUSTUP_HOME'])
state = os.environ['FIXTURE_STATE']
args = sys.argv[1:]
with (home / 'calls').open('a') as log: log.write(' '.join(args) + '\\n')
if args == ['toolchain', 'list']:
    if state != 'missing' or (home / 'installed').exists(): print('stable-x86_64-unknown-linux-gnu (default)')
elif args == ['toolchain', 'install', 'stable', '--profile', 'minimal']:
    (home / 'mutating').mkdir()
    time.sleep(0.15)
    with (home / 'install-count').open('a') as log: log.write('install\\n')
    (home / 'installed').touch()
    (home / 'mutating').rmdir()
elif args == ['run', 'stable', 'rustc', '-vV']:
    print('rustc 1.88.0 (fixture)\\nhost: x86_64-unknown-linux-gnu')
elif args == ['run', 'stable', 'rustc', '--version']:
    print('rustc 1.88.0 (fixture)')
elif args == ['run', 'stable', 'cargo', '--version']:
    sys.exit(1 if state == 'broken-cargo' else 0)
elif args == ['target', 'list', '--toolchain', 'stable', '--installed']:
    if state != 'missing-registry': print('x86_64-unknown-linux-gnu')
elif args == ['run', 'stable', 'rustc', '--print', 'target-libdir']:
    lib = home / 'lib'; lib.mkdir(exist_ok=True)
    if state != 'missing-files': (lib / 'libstd-fixture.rlib').touch()
    print(lib)
else:
    sys.exit('unexpected rustup fixture call: ' + ' '.join(args))
''')
        for path in [flock, fake]: path.chmod(0o755)
        environment = os.environ.copy()
        environment.update(PATH=f'{bin_dir}:{environment["PATH"]}', RUSTUP=str(fake),
                           RUSTUP_HOME=str(rustup_home), FIXTURE_STATE=state,
                           GITHUB_ENV=str(fixture / 'github-env'))
        environment.pop('CASSY_RUSTUP_LOCK_FILE', None)
        return fixture, environment

    def test_shared_rustup_helper_serializes_install_and_exports_toolchain(self):
        fixture, environment = self.rustup_fixture('missing')
        command = [str(policy.ROOT / 'scripts/setup-cassy-actions-rust.sh')]
        processes = [subprocess.Popen(command, env=environment, stdout=subprocess.PIPE,
                                     stderr=subprocess.PIPE, text=True) for _ in range(2)]
        outputs = [p.communicate(timeout=10) for p in processes]
        for process, output in zip(processes, outputs):
            self.assertEqual(process.returncode, 0, output)
        self.assertEqual((fixture / 'rustup/install-count').read_text(), 'install\n')
        self.assertIn('already installed', ''.join(out for out, _ in outputs))
        self.assertEqual((fixture / 'github-env').read_text().splitlines(), ['RUSTUP_TOOLCHAIN=stable'] * 2)

    def test_shared_rustup_helper_rejects_corrupt_registry_and_files(self):
        for state in ['missing-registry', 'missing-files', 'broken-cargo']:
            with self.subTest(state=state):
                fixture, environment = self.rustup_fixture(state)
                result = subprocess.run([str(policy.ROOT / 'scripts/setup-cassy-actions-rust.sh')],
                                        env=environment, capture_output=True, text=True, timeout=10)
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertIn('shared stable Rust toolchain is incomplete', result.stderr)
                self.assertIn('all runner slots are idle', result.stderr)
                self.assertNotIn('toolchain install', (fixture / 'rustup/calls').read_text())
                self.assertFalse((fixture / 'github-env').exists())

    def test_real_required_rollup_rejects_every_failed_or_cancelled_dependency(self):
        job = self.ci()['jobs']['fast-validation']
        step = next(s for s in job['steps'] if 'test "$PREFLIGHT" = success' in s.get('run', ''))
        healthy = dict(PREFLIGHT='success', SUITE='success', DOCS='success')
        self.assertEqual(self.run_body(step['run'], healthy).returncode, 0)
        for key, value in itertools.product(healthy, ['failure', 'cancelled']):
            with self.subTest(dependency=key, verdict=value):
                self.assertNotEqual(self.run_body(step['run'], {**healthy, key:value}).returncode, 0)
        shard = next(s for s in self.ci()['jobs']['fast-validation-suite']['steps'] if 'test "$SHARDS" = success' in s.get('run', ''))
        self.assertEqual(self.run_body(shard['run'], {'SHARDS':'success'}).returncode, 0)
        for result in ['failure', 'cancelled']:
            self.assertNotEqual(self.run_body(shard['run'], {'SHARDS':result}).returncode, 0)

    def test_real_markdown_step_filters_assets_and_skips_an_asset_only_diff(self):
        source = {'file': '.github/workflows/ci.yml', 'job': 'docs-lint'}
        _, step = policy.role(self.ci()['jobs']['docs-lint'], 'Markdown lint', source)
        bin_dir = self.root / 'bin'
        bin_dir.mkdir()
        (bin_dir / 'git').write_text('#!/usr/bin/env bash\ncase "$1" in merge-base) echo fixture-base;; diff) cat "$CHANGED_PATHS";; *) exit 2;; esac\n')
        (bin_dir / 'npx').write_text('#!/usr/bin/env bash\nprintf \'%s\\n\' "$@" > "$NPX_LOG"\n')
        for child in bin_dir.iterdir(): child.chmod(0o755)
        changed = self.root / 'changed'
        log = self.root / 'npx.args'
        assets = ['docs/round-3/schemes.css', 'docs/round-3/schemes.mjs', 'docs/round-3/thread-a.html', 'docs/round-3/thread-a.png']
        environment = dict(BASE_SHA='base', ZERO_BASE_REF='HEAD', CHANGED_PATHS=str(changed), NPX_LOG=str(log), PATH=f'{bin_dir}:/usr/bin:/bin')
        changed.write_text('\n'.join(assets + ['docs/round-3/visual-qa.md']) + '\n')
        result = self.run_body(step['run'], environment)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(log.read_text().splitlines(), ['--yes', 'markdownlint-cli2@0.18.1', '--config', '.markdownlint-cli2.jsonc', 'docs/round-3/visual-qa.md'])
        log.unlink()
        changed.write_text('\n'.join(assets) + '\n')
        result = self.run_body(step['run'], environment)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('No Markdown files changed', result.stdout)
        self.assertFalse(log.exists())

    def test_real_runner_route_defaults_to_hosted_and_requires_both_opt_ins(self):
        route = next(s for s in self.ci()['jobs']['fast-validation-runner-route']['steps'] if s.get('id') == 'route')['run']
        for event, enabled in itertools.product(['merge_group', 'push', 'pull_request', 'schedule'], ['enabled', '', 'disabled']):
            with self.subTest(event=event, enabled=enabled):
                output = self.root / 'route.output';output.write_text('')
                result = self.run_body(route, dict(EVENT_NAME=event, SELF_HOSTED_ENABLED=enabled, GITHUB_OUTPUT=str(output)))
                self.assertEqual(result.returncode, 0, result.stderr)
                expected = ['self-hosted','Linux','X64','cas-ci-32core'] if event == 'merge_group' and enabled == 'enabled' else ['ubuntu-latest']
                lines = dict(line.split('=', 1) for line in output.read_text().splitlines())
                self.assertEqual(json.loads(lines['runner']), expected)
                self.assertEqual(lines['mode'], 'self-hosted' if len(expected)>1 else 'hosted')


if __name__ == '__main__': unittest.main(verbosity=2)
