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
        # Workflow sources the behavioural cases below read beyond the registry.
        cls.files.update(['.github/actions/setup-rust-linux/action.yml', '.github/workflows/release.yml', '.github/workflows/lane.yml', '.github/actions/run-script-suite-tests/action.yml'])

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
        step = next(s for s in self.ci()['jobs']['fast-validation-ci-tiers']['steps']
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
                # cas-a1b0: the pinned linter comes from the root lockfile.
                (bin_dir / 'npm').write_text('#!/usr/bin/env bash\nprintf "npm %s\\n" "$*" >> "$CALLS"\n')
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
                    self.assertNotIn('npm ci --no-audit --no-fund', calls)
                else:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    count = len(self.entries) + len(json.loads((policy.HERE / 'prose-pins.json').read_text()))
                    self.assertIn(f'parsed policy/prose: {count} passed; 0 failed', result.stdout)
                    if mode == 'present':
                        self.assertEqual(calls, ['import', 'npm ci --no-audit --no-fund', 'make', str(bin_dir / 'python3')])
                        self.assertFalse((runner_temp / 'ci-tiers-venv').exists())
                    else:
                        self.assertEqual(calls, ['import', 'venv',
                            'pip install --disable-pip-version-check --quiet -r scripts/ci_tiers/requirements.txt',
                            'npm ci --no-audit --no-fund',
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

    @staticmethod
    def gh_value(expression, context):
        """Evaluate a GitHub expression over a flat context mapping."""
        import re
        body = expression.strip()
        if body.startswith('${{') and body.endswith('}}'):
            body = body[3:-2]
        elif not any(op in body for op in ('==', '!=', '&&', '||', '(')):
            return body  # a literal such as runs-on: ubuntu-latest
        body = re.sub(r'\b(?:github|needs|steps|vars|inputs|env)\.[\w.-]+', lambda m: repr(context.get(m.group(0), '')), body)
        body = body.replace('always()', 'True').replace('&&', ' and ').replace('||', ' or ')
        body = re.sub(r'!(?!=)', ' not ', body)
        return eval(body, {'__builtins__': {}, 'startsWith': lambda a, b: str(a).startswith(b), 'fromJSON': json.loads})

    @classmethod
    def gh_expr(cls, expression, context):
        """Evaluate a GitHub `if:` expression over a flat context mapping."""
        return bool(cls.gh_value(expression, context))

    def test_slim_factory_lanes_run_only_in_lane_yml_on_opted_in_runners_cas_32e9_cas_3b5e(self):
        """cas-14e0/cas-32e9: factory pushes start in lane.yml. Slim on: only
        the named lane jobs run, locally when CASSY_LANE_SELF_HOSTED is enabled.
        Slim off: the full ci.yml is called. No fork-capable trigger reaches
        the local runners, and ci.yml routes nothing else there."""
        lane = policy.load(self.root / '.github/workflows/lane.yml')
        ci = self.ci()
        local = ['self-hosted', 'Linux', 'X64', 'cas-lane-light']
        self.assertEqual((lane['name'], ci['name']), ('CI', 'CI'), 'ci_completed and the CI watcher select runs by name')
        # cas-3b5e: the light slots take lanes only; the 12-way Rust jobs of the
        # merge queue and release never select them.
        for source in ('ci.yml', 'release.yml'):
            text = (self.root / '.github/workflows' / source).read_text()
            self.assertNotIn('cas-lane-light', text, source)
        self.assertNotIn('cas-ci-32core', json.dumps(lane['jobs']))
        self.assertEqual(lane['on'], {'push': {'branches': ['factory/**']}})
        self.assertNotIn('factory/**', ci['on']['push']['branches'])
        self.assertIn('workflow_call', ci['on'])
        self.assertNotIn('pull_request_target', ci['on'])
        jobs = lane['jobs']
        self.assertEqual(jobs['full']['uses'], './.github/workflows/ci.yml')
        # GitHub validates every callee job, including jobs skipped on a lane.
        authority = {'none':0,'read':1,'write':2}
        ceiling = jobs['full']['permissions']
        for name,job in ci['jobs'].items():
            for permission,value in job.get('permissions',ci['permissions']).items():
                self.assertLessEqual(authority[value],authority[ceiling.get(permission,'none')],
                                     f'{name} requests {permission}:{value} above the reusable caller ceiling')
        # All slim factory diffs execute the rows once, in classification,
        # before either docs or code admission can pass. No worker receipt is trusted.
        fast_steps = [(name, step) for name, job in jobs.items()
                      for step in job.get('steps', [])
                      if './scripts/release-gate.sh --fast-rows' in step.get('run', '')]
        self.assertEqual(len(fast_steps), 1)
        name, fast = fast_steps[0]
        self.assertEqual(name, 'ci-diff')
        self.assertNotIn('if', fast)
        self.assertNotIn('--only scripts/terminal-qa.test.mjs', json.dumps(jobs),
                         'the changed-script row already runs terminal QA')
        self.assertEqual(fast['env']['BASE_SHA'], '${{ steps.classify-diff.outputs.comparison-base }}')
        bin_dir = self.root / 'bin';bin_dir.mkdir()
        calls = self.root / 'fast-rows.args'
        npm = bin_dir / 'npm'
        npm.write_text('#!/bin/sh\nexit 0\n')
        npm.chmod(0o755)
        gate = self.root / 'scripts/release-gate.sh'
        gate.parent.mkdir(parents=True, exist_ok=True)
        gate.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$CALLS"\nexit "${GATE_STATUS:-0}"\n')
        gate.chmod(0o755)
        for base, expected in [('comparison-base', 'comparison-base'),
                               ('', 'origin/main'), ('0' * 40, 'origin/main')]:
            with self.subTest(fast_rows_base=base):
                result = self.run_body(fast['run'], dict(
                    PATH=f'{bin_dir}:{os.environ["PATH"]}', CALLS=str(calls),
                    BASE_SHA=base, ZERO_BASE_REF='origin/main'))
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(calls.read_text().splitlines(), ['--fast-rows', '--base', expected])
        failed = self.run_body(fast['run'], dict(
            PATH=f'{bin_dir}:{os.environ["PATH"]}', CALLS=str(calls),
            BASE_SHA='comparison-base', ZERO_BASE_REF='origin/main', GATE_STATUS='1'))
        self.assertNotEqual(failed.returncode, 0, 'failed rows must fail CI admission')
        slim_jobs = {name: job for name, job in jobs.items() if name != 'full'}
        self.assertEqual(
            {job['name'] for job in slim_jobs.values()},
            {'Classify CI diff', 'Docs Lint', 'Scoped Validation (fast)', 'Scoped Validation (factory/PR)'},
            'merge admission and the CI watchers read these exact check names')
        for slim in ('true', '', 'false'):
            ctx = {'vars.CAS_LANE_SLIM': slim, 'needs.ci-diff.outputs.class': 'rust-touched'}
            self.assertEqual(self.gh_expr(jobs['full']['if'], ctx), slim != 'true', slim)
            for name in ('ci-diff', 'scoped-validation-fast', 'scoped-validation'):
                with self.subTest(job=name, slim=slim):
                    self.assertEqual(self.gh_expr(slim_jobs[name]['if'], ctx), slim == 'true')
        for name, job in slim_jobs.items():
            with self.subTest(job=name):
                runner = lambda enabled: self.gh_value(job['runs-on'], {'vars.CASSY_LANE_SELF_HOSTED': enabled})
                self.assertEqual(runner('enabled'), local)
                for off in ('', 'disabled'):
                    self.assertEqual(runner(off), 'ubuntu-latest')
        for name, job in ci['jobs'].items():
            if name != 'fast-validation-runner-route':
                self.assertNotIn('cas-ci-32core', json.dumps(job.get('runs-on', '')), name)

    def test_macos_check_warms_main_cache_but_stays_off_branch_pushes_cas_861c0(self):
        job = self.ci()['jobs']['macos-check']
        def context(event, ref, run='false', reuse='', diff='rust-touched'):
            return {'github.event_name': event, 'github.ref': ref, 'github.base_ref': 'main',
                    'github.event.repository.default_branch': 'main', 'needs.ci-diff.outputs.class': diff,
                    'needs.fast-validation-main-push-dedupe.outputs.run-fast-validation': run,
                    'needs.fast-validation-main-push-dedupe.outputs.reuse-source': reuse}
        cases = [
            # A deduplicated main push still runs: it saves main's Darwin cache.
            (context('push', 'refs/heads/main'), True),
            (context('merge_group', 'refs/heads/gh-readonly-queue/main/pr-1-abc', run='true'), True),
            (context('push', 'refs/heads/main', diff='docs-only'), False),
            (context('push', 'refs/heads/factory/x', run='true'), False),
        ]
        for ctx, expected in cases:
            with self.subTest(event=ctx['github.event_name'], ref=ctx['github.ref'], diff=ctx['needs.ci-diff.outputs.class']):
                self.assertEqual(self.gh_expr(job['if'], ctx), expected)
        # Mutation control: without the main-push clause main never warms the cache.
        without = job['if'].replace("|| (github.event_name == 'push' && github.ref == 'refs/heads/main')", '')
        self.assertNotEqual(without, job['if'])
        self.assertFalse(self.gh_expr(without, cases[0][0]))
        # Only main saves the cargo cache; queue and tag refs restore main's copy.
        cache = next(step for step in job['steps'] if str(step.get('uses', '')).startswith('Swatinem/rust-cache'))
        save_if = cache['with']['save-if']
        for ref, expected in [('refs/heads/main', True), ('refs/heads/gh-readonly-queue/main/pr-1-abc', False)]:
            with self.subTest(save_ref=ref):
                self.assertEqual(self.gh_expr(save_if, {'github.ref': ref}), expected)

    def test_lane_slim_is_off_by_default_and_only_slims_factory_pushes_cas_eac2(self):
        jobs = self.ci()['jobs']
        def runs(job, name, slim, fast, event='push', ref='refs/heads/factory/w'):
            step = next(s for s in jobs[job]['steps'] if s.get('name') == name)
            context = {
                'needs.ci-diff.outputs.class': 'rust-touched',
                'github.event_name': event, 'github.ref': ref, 'vars.CAS_LANE_SLIM': slim,
                'steps.classify-diff.outputs.fast-admission': fast,
                'steps.classify-diff.outputs.rust-unaffected': 'false'}
            for key, value in jobs[job].get('env', {}).items():
                context[f'env.{key}'] = str(self.gh_value(value, context)).lower()
            return self.gh_expr(step['if'], context)
        rust = 'Run change-scoped tests with impact evidence'
        for job, name, fast in [
            ('docs-lint', 'Run no-build release gate rows', 'true'), ('docs-lint', 'Markdown lint', 'true'),
            ('scoped-validation-fast', 'Run no-build release gate rows', 'true'),
            ('scoped-validation-fast', rust, 'true'),
            ('scoped-validation-fast', 'Test snapshot-pinned CLI output surfaces', 'true'),
            ('scoped-validation', rust, 'false'),
            ('scoped-validation', 'Test snapshot-pinned CLI output surfaces', 'false'),
        ]:
            with self.subTest(job=job, step=name):
                self.assertTrue(runs(job, name, '', fast), 'unset CAS_LANE_SLIM keeps the lane as it was')
                self.assertFalse(runs(job, name, 'true', fast))
        self.assertTrue(runs('scoped-validation', rust, 'true', 'false', event='pull_request', ref='refs/pull/1/merge'))

    def test_release_fixtures_run_only_for_scripts_changes_in_both_lane_paths(self):
        scripts = ['test-release.sh', 'test-release-publish.sh', 'test-install-on-publish.sh', 'test-hub-pin.sh']
        calls = self.root / 'fixture-calls'
        for name in scripts:
            script = self.root / 'scripts' / name
            script.parent.mkdir(parents=True, exist_ok=True)
            script.write_text('#!/usr/bin/env bash\nprintf "%s\\n" "${0##*/}" >> "$CALLS"\n'
                              '[[ "${FAIL_FIXTURE:-}" != "${0##*/}" ]]\n')
            script.chmod(0o755)
        # Pre-push runs this suite with Git's repository-local variables set.
        # Keep both fixture Git commands and workflow bodies in this repository.
        environment = os.environ.copy()
        for key in subprocess.check_output(['git', 'rev-parse', '--local-env-vars'], text=True).splitlines():
            environment.pop(key, None)
        def git(*args):
            return subprocess.check_output(['git', '-C', str(self.root), *args], env=environment, text=True).strip()
        def run(step, base, failing=''):
            return subprocess.run([shutil.which('bash'), '-euo', 'pipefail', '-c', step['run']],
                                  cwd=self.root, env={**environment, 'HOME': str(self.root),
                                  'BASE_SHA': base, 'CALLS': str(calls), 'FAIL_FIXTURE': failing},
                                  text=True, capture_output=True)
        git('init', '-q')
        git('config', 'user.name', 'CI fixture')
        git('config', 'user.email', 'fixture@example.test')
        git('add', '.')
        git('commit', '-qm', 'base')
        for changed, expected in [('scripts/fixture.sh', scripts), ('docs/fixture.md', []),
                                  ('.github/workflows/fixture.yml', [])]:
            base = git('rev-parse', 'HEAD')
            path = self.root / changed
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('fixture\n')
            git('add', changed)
            git('commit', '-qm', changed)
            for workflow in ['ci.yml', 'lane.yml']:
                with self.subTest(workflow=workflow, changed=changed):
                    job = policy.load(self.root / '.github/workflows' / workflow)['jobs']['scoped-validation']
                    steps = [s for s in job['steps'] if './scripts/test-release.sh' in s.get('run', '')]
                    self.assertEqual(len(steps), 1, 'release fixtures must be wired once in the existing scoped job')
                    step = steps[0]
                    signal = {'steps.pr-dedupe.outputs.covered': 'false'}
                    self.assertTrue(self.gh_expr(step['if'], signal))
                    self.assertFalse(self.gh_expr(step['if'], {**signal, 'steps.pr-dedupe.outputs.covered': 'true'}))
                    calls.write_text('')
                    result = run(step, base)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(calls.read_text().splitlines(), expected)
                    if expected:
                        calls.write_text('')
                        result = run(step, base, scripts[1])
                        self.assertNotEqual(result.returncode, 0, 'a red fixture must redden the lane')
                        self.assertEqual(calls.read_text().splitlines(), scripts[:2])

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
        pr = next(s for s in job['steps'] if 'SCOPED' in s.get('env', {}))
        self.assertIn('scoped-validation-fast', job['needs'])
        for state in ['success', 'skipped', 'failure', 'cancelled', '']:
            self.assertEqual(self.run_body(pr['run'], {'CI_DIFF': 'success', 'SCOPED': state}).returncode == 0,
                             state in ['success', 'skipped'])
        step = next(s for s in job['steps'] if 'test "$PREFLIGHT" = success' in s.get('run', ''))
        healthy = dict(PREFLIGHT='success', SUITE='success', DOCS='success',
                       CI_TIERS='success', SLACK_BRIDGE='success', WEB='success', JOURNEYS='success')
        self.assertEqual(self.run_body(step['run'], healthy).returncode, 0)
        for key, value in itertools.product(healthy, ['failure', 'cancelled']):
            with self.subTest(dependency=key, verdict=value):
                self.assertNotEqual(self.run_body(step['run'], {**healthy, key:value}).returncode, 0)
        # Input-gated lanes may be skipped; the always-on lanes may not.
        for key in ['CI_TIERS', 'SLACK_BRIDGE', 'WEB', 'JOURNEYS']:
            with self.subTest(skipped=key):
                self.assertEqual(self.run_body(step['run'], {**healthy, key: 'skipped'}).returncode, 0)
        for key in ['PREFLIGHT', 'SUITE', 'DOCS']:
            with self.subTest(skipped=key):
                self.assertNotEqual(self.run_body(step['run'], {**healthy, key: 'skipped'}).returncode, 0)
        # Every red input-gated lane is reported in the same run, not just the first.
        result = self.run_body(step['run'], {**healthy, 'WEB': 'failure', 'JOURNEYS': 'failure'})
        self.assertIn('WEB lane is failure', result.stdout)
        self.assertIn('JOURNEYS lane is failure', result.stdout)
        shard = next(s for s in self.ci()['jobs']['fast-validation-suite']['steps'] if 'test "$SHARDS" = success' in s.get('run', ''))
        self.assertEqual(self.run_body(shard['run'], {'SHARDS':'success'}).returncode, 0)
        for result in ['failure', 'cancelled']:
            self.assertNotEqual(self.run_body(shard['run'], {'SHARDS':result}).returncode, 0)
        publication = next(s for s in job['steps'] if 'PUBLICATION' in s.get('env', {}))
        self.assertIn('queue-flake-publication',job['needs'])
        for result in ['success','skipped','failure','cancelled','']:
            with self.subTest(publication=result):
                self.assertEqual(self.run_body(publication['run'],{'PUBLICATION':result}).returncode == 0,
                                 result in ['success','skipped'])

    def test_real_script_suite_action_runs_an_input_pr_and_installs_nothing_for_unrelated_pr(self):
        action = '.github/actions/run-script-suite-tests'
        jobs = self.ci()['jobs']
        lane = policy.load(self.root / '.github/workflows/lane.yml')
        for job in [jobs['scoped-validation-fast'], lane['jobs']['scoped-validation-fast'], jobs['fast-validation-ci-tiers']]:
            self.assertEqual(sum(step.get('uses') == f'./{action}' for step in job['steps']), 1)
        admission = jobs['scoped-validation-fast']
        self.assertFalse(any(step.get('uses') == f'./{action}' for step in jobs['ci-diff']['steps']))
        # Full Rust lanes run the Makefile tier, so their classifier must not
        # run the provisioned suites a second time. PRs and web-only queues do.
        for event, ref, unaffected, expected in [
            ('pull_request', 'refs/pull/1/merge', 'false', True),
            ('push', 'refs/heads/factory/example', 'false', True),
            ('merge_group', 'refs/heads/gh-readonly-queue/main/pr-1', 'false', False),
            ('merge_group', 'refs/heads/gh-readonly-queue/main/pr-1', 'true', True),
        ]:
            self.assertEqual(self.gh_expr(admission['if'], {'github.event_name': event, 'github.ref': ref,
                'needs.ci-diff.outputs.class': 'docs-only' if unaffected == 'true' else 'rust-touched',
                'needs.ci-diff.outputs.script-tests-needed': 'true',
                'needs.ci-diff.outputs.rust-unaffected': unaffected}), expected)
        steps = policy.load(self.root / action / 'action.yml')['runs']['steps']
        select = next(s for s in jobs['ci-diff']['steps'] if s.get('id') == 'script-tests')
        run = steps[-1]
        for path in ['scripts/ci-script-tests-for-diff.py', 'cas-cli/Makefile']:
            destination = self.root / path
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(policy.ROOT / path, destination)
        subject = self.root / 'scripts/journey-bundles.py'
        subject.write_text('baseline\n')
        # Fake only the leaf suite, keeping real Git diffs, Makefile parsing,
        # action conditions and its shell bodies on the production path.
        (self.root / 'scripts/test-journey-bundles.py').write_text("print('suite actually ran')\n")
        unrelated = self.root / 'docs/unrelated.md'
        unrelated.write_text('baseline\n')
        def git(*args):
            subprocess.run(['git', '-C', str(self.root), *args], check=True, capture_output=True)
        git('init', '-q')
        git('add', '.')
        git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
            '-c', 'commit.gpgsign=false', 'commit', '-qm', 'PR base')
        for file, expected in [(subject, True), (unrelated, False)]:
            with self.subTest(pr=file.name):
                file.write_text('PR input changed\n')
                git('add', '.')
                git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                    '-c', 'commit.gpgsign=false', 'commit', '-qm', 'PR head')
                output = self.root / 'selection-output'
                output.write_text('')
                env = dict(BASE_SHA='HEAD^', GITHUB_OUTPUT=str(output), FULL_TIER='false')
                result = self.run_body(select['run'], env)
                self.assertEqual(result.returncode, 0, result.stderr)
                signals = dict(line.split('=', 1) for line in output.read_text().splitlines())
                self.assertEqual(signals['needed'], str(expected).lower())
                self.assertEqual({key: val for key, val in signals.items() if key != 'needed'},
                                 dict(web='false', browser='false', pdf='false', lint='false'))
                context = {'inputs.full-tier': 'false', **{
                    f'steps.script-tests.outputs.{key}': val for key, val in signals.items()}}
                routing = {'github.event_name': 'pull_request', 'github.ref': 'refs/pull/1/merge',
                           'needs.ci-diff.outputs.class': 'docs-only',
                           'needs.ci-diff.outputs.script-tests-needed': signals['needed']}
                self.assertEqual(self.gh_expr(admission['if'], routing), expected)
                self.assertFalse(self.gh_expr(admission['env']['LEGACY_SCOPED'], routing))
                # Reusing the job for PR suites must not start its legacy Rust
                # or release checks for events that previously skipped them.
                for step in admission['steps'][1:]:
                    if step.get('uses') != f'./{action}':
                        self.assertFalse(self.gh_expr(step['if'], {**routing, 'env.LEGACY_SCOPED': 'false'}))
                # Unrelated PRs select no setup, install, or suite step.
                for step in steps[1:]:
                    self.assertEqual(self.gh_expr(step['if'], context),
                                     expected and 'uses' not in step)
                if expected:
                    # This no-dependency suite needs no fake npm/pip/browser.
                    provision = self.run_body(steps[-2]['run'], dict(FULL_TIER='false',
                        NEED_LINT='false', NEED_WEB='false', NEED_BROWSER='false', NEED_PDF='false'))
                    self.assertEqual(provision.returncode, 0, provision.stderr)
                    result = self.run_body(run['run'], env)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertIn('suite actually ran', result.stdout)
                    self.assertIn('PASS python3 scripts/test-journey-bundles.py', result.stdout)
                    (self.root / 'scripts/test-journey-bundles.py').write_text('raise SystemExit(17)\n')
                    failed = self.run_body(run['run'], env)
                    self.assertEqual(failed.returncode, 1, failed.stdout + failed.stderr)
                    self.assertIn('FAIL (exit 17)', failed.stdout)
                git('reset', '--hard', 'HEAD^')
                output.unlink()

    def test_real_markdown_step_filters_assets_and_skips_an_asset_only_diff(self):
        source = {'file': '.github/workflows/ci.yml', 'job': 'docs-lint'}
        _, step = policy.role(self.ci()['jobs']['docs-lint'], 'Markdown lint', source)
        bin_dir = self.root / 'bin'
        bin_dir.mkdir()
        (bin_dir / 'git').write_text('#!/usr/bin/env bash\ncase "$1" in merge-base) echo fixture-base;; diff) cat "$CHANGED_PATHS";; *) exit 2;; esac\n')
        (bin_dir / 'python3').write_text('#!/usr/bin/env bash\nprintf \'%s\\n\' "$@" > "$NPX_LOG"\n')
        for child in bin_dir.iterdir(): child.chmod(0o755)
        changed = self.root / 'changed'
        log = self.root / 'npx.args'
        assets = ['docs/round-3/schemes.css', 'docs/round-3/schemes.mjs', 'docs/round-3/thread-a.html', 'docs/round-3/thread-a.png']
        environment = dict(BASE_SHA='base', ZERO_BASE_REF='HEAD', CHANGED_PATHS=str(changed), NPX_LOG=str(log), PATH=f'{bin_dir}:/usr/bin:/bin')
        changed.write_text('\n'.join(assets + ['docs/round-3/visual-qa.md']) + '\n')
        result = self.run_body(step['run'], environment)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(log.read_text().splitlines(), ['scripts/markdownlint.py', '--config', '.markdownlint-cli2.jsonc', 'docs/round-3/visual-qa.md'])
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
