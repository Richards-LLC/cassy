#!/usr/bin/env python3
"""Non-executing Rust fixtures plus real-Git ratchet contracts for rule-026."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('test_env_lint', ROOT / 'scripts/check-test-env.py')
LINT = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = LINT
SPEC.loader.exec_module(LINT)


def analyze(source, path='cas-cli/src/fixture.rs'):
    return LINT.Analyzer({path: source}).run()


def baseline(findings=(), exceptions=False):
    return {'version': 1, 'violations': [] if exceptions else [
        {'id': f['id'], 'reason': 'Retained fixture: exercises a legacy mutation.'} for f in findings],
        'exceptions': [{'id': f['id'], 'reason': 'Ignored child with bounded parent re-exec.'}
                       for f in findings] if exceptions else []}


class SourceContracts(unittest.TestCase):
    def test_retained_incident_and_lifetime_cases(self):
        cases = json.loads((ROOT / 'scripts/fixtures/test-env-lint.json').read_text())['cases']
        for case in cases:
            with self.subTest(case=case['name']):
                rows = analyze(case['source'], case['path'])
                # Comments never grant exemptions. The isolated seed uses an
                # exact reviewed manifest entry, like the real ignored probe.
                allow = baseline()
                if case.get('exception_reason'):
                    self.assertTrue(rows)
                    allow['exceptions'] = [{'id': r['id'], 'reason': case['exception_reason']} for r in rows]
                self.assertEqual(bool(LINT.ratchet(rows, allow)), case['violation'])

    def test_shared_owner_and_helpers(self):
        cases = [
            ('unrelated lock', 'let _g = OTHER_MUTEX.lock(); std::env::set_var("HOME", "x");', True),
            ('guarded raw call', 'let _g = TestEnvGuard::new(); std::env::set_var("HOME", "x");', True),
            ('after drop', 'let g = TestEnvGuard::new(); drop(g); std::env::set_var("HOME", "x");', True),
            ('drop inside block', 'let g = TestEnvGuard::new(); { drop(g); } std::env::remove_var("HOME");', True),
            ('conditional drop', 'let g = TestEnvGuard::new(); if choice { drop(g); } let _other = TestEnvGuard::new();', True),
            ('conditional drop then mutation', 'let g = TestEnvGuard::new(); if choice { drop(g); } std::env::set_var("HOME", "x");', True),
            ('conditional drop then final drop', 'let g = TestEnvGuard::new(); if choice { drop(g); } drop(g); let _next = TestEnvGuard::new();', False),
            ('child unguarded', 'let _g = TestEnvGuard::new(); std::thread::spawn(|| { std::env::set_var("HOME", "x"); });', True),
            ('discarded temporaries overlap', 'consume(TestEnvGuard::new(), TestEnvGuard::new());', True),
            ('temporary statement ends', 'consume(TestEnvGuard::new()); let _next = TestEnvGuard::new();', False),
            ('unused inner helper', 'fn dormant() { std::env::set_var("HOME", "x"); }', True),
        ]
        for name, body, expected in cases:
            with self.subTest(case=name):
                self.assertEqual(bool(analyze('#[test] fn contract() {' + body + '}')), expected)
        good = 'fn helper(g: &mut TestEnvGuard) { g.set("HOME", "x"); } #[test] fn ok() { let mut g = TestEnvGuard::new(); helper(&mut g); }'
        self.assertFalse(analyze(good))
        dropped = 'fn helper(g: TestEnvGuard) { drop(g); std::env::set_var("HOME", "x"); } #[test] fn bad() { helper(TestEnvGuard::new()); }'
        self.assertTrue(analyze(dropped))
        optional = '#[cfg(test)] fn helper(g: Option<TestEnvGuard>) { std::env::set_var("HOME", "x"); } #[test] fn bad() { helper(None); }'
        self.assertTrue(analyze(optional))
        alias = 'use crate::test_support::TestEnvGuard as Guard; fn helper(g: &mut Guard) { g.set("HOME", "x"); } #[test] fn ok() { let mut g = Guard::new(); helper(&mut g); }'
        self.assertFalse(analyze(alias))

    def test_raw_mutations_under_a_guard_need_capture_and_restore(self):
        for mutation in ('set_var("CAS_GH_BIN", "stub")',
                         'remove_var("CAS_GH_BIN")', 'set_current_dir(".")'):
            with self.subTest(mutation=mutation):
                rows = analyze('#[test] fn leaks_on_panic() { let g = TestEnvGuard::new(); '
                               'unsafe { std::env::' + mutation + '; } }')
                self.assertEqual(len(rows), 1)
                self.assertEqual(rows[0]['function'], 'leaks_on_panic')
                self.assertEqual(rows[0]['detail'], 'std::env::' + mutation.split('(')[0])
                self.assertTrue(LINT.ratchet(rows, baseline()))
                # An exact reviewed legacy allowance remains possible; merely
                # holding the guard never grants one automatically.
                self.assertFalse(LINT.ratchet(rows, baseline(rows)))
        safe = '#[test] fn safe() { let mut g = TestEnvGuard::new(); g.set("CAS_GH_BIN", "stub"); g.remove("CAS_GH_BIN"); g.set_current_dir("."); }'
        self.assertFalse(analyze(safe))

    def test_raw_guarded_helpers_and_callbacks_still_report_callers(self):
        helper = 'fn fixture(g: &mut TestEnvGuard) { std::env::set_var("CAS_GH_BIN", "stub"); } '
        rows = analyze(helper + '#[test] fn caller() { let mut g = TestEnvGuard::new(); fixture(&mut g); }')
        self.assertEqual({r['function'] for r in rows}, {'fixture', 'caller'})
        self.assertIn('unguarded-helper-call', {r['kind'] for r in rows})
        callback = '#[test] fn callback() { TestEnvGuard::run_with_temp_home(|_| { std::env::remove_var("CAS_GH_BIN"); }); }'
        self.assertTrue(analyze(callback))

    def test_function_and_module_imports(self):
        for imp, call in [('use std::env::{set_var as change};', 'change'),
                          ('use std::env as state;', 'state::remove_var'),
                          ('use std::{env as state};', 'state::set_current_dir'),
                          ('use std::env::remove_var;', 'remove_var')]:
            with self.subTest(imp=imp):
                self.assertTrue(analyze(imp + '#[test] fn bad() {' + call + '("HOME", "x");}'))
                self.assertTrue(analyze(imp + '#[test] fn guarded() { let g = TestEnvGuard::new(); ' + call + '("HOME", "x");}'))

    def test_absolute_and_generic_mutations(self):
        for call in ('::std::env::set_var', 'std::env::set_var::<&str, &str>'):
            with self.subTest(call=call):
                self.assertTrue(analyze('#[test] fn bad() {' + call + '("HOME", "x");}'))
        source = 'fn fixture<T>() { let _g = TestEnvGuard::new(); } #[test] fn bad() { let _g = TestEnvGuard::new(); fixture::<()>(); }'
        self.assertTrue(analyze(source))

    def test_cfg_test_helpers_without_callers_are_checked(self):
        self.assertTrue(analyze('#[cfg(test)] fn helper() { std::env::set_var("HOME", "x"); }'))
        self.assertTrue(analyze('#[cfg(all(test, unix))] mod helpers { fn helper() { std::env::set_var("HOME", "x"); } }'))
        self.assertFalse(analyze('#[cfg(feature = "test")] fn startup() { std::env::set_var("HOME", "x"); }'))

    def test_local_lookalike_cannot_protect_mutation(self):
        source = 'struct TestEnvGuard; #[test] fn bad() { let _g = TestEnvGuard::new(); std::env::set_var("HOME", "x"); }'
        self.assertTrue(analyze(source))
        self.assertTrue(analyze(source, 'crates/cas-core/src/fixture.rs'))

    def test_local_closure_and_parameter_shadow_unsafe_global_helpers(self):
        sources = {
            'cas-cli/src/cli.rs': 'pub fn run_command() { std::env::set_current_dir("."); }',
            'cas-cli/src/qa.rs': '#[test] fn safe() { let run_command = || { "pure" }; run_command(); }',
        }
        self.assertFalse(LINT.Analyzer(sources).run())
        sources['cas-cli/src/qa.rs'] = '#[test] fn safe(run_command: impl FnOnce()) { run_command(); }'
        self.assertFalse(LINT.Analyzer(sources).run())
        # Scope expiry must restore real helper resolution.
        sources['cas-cli/src/qa.rs'] = '#[test] fn bad() { { let run_command = || {}; run_command(); } run_command(); }'
        self.assertTrue(LINT.Analyzer(sources).run())
        # A shadowing closure cannot hide direct process mutation in its body.
        sources['cas-cli/src/qa.rs'] = '#[test] fn bad() { let run_command = || { std::env::set_var("HOME", "x"); }; run_command(); }'
        self.assertTrue(LINT.Analyzer(sources).run())
        # A declaration does not shadow its own initializer.
        sources['cas-cli/src/qa.rs'] = '#[test] fn bad() { let run_command = run_command(); }'
        self.assertTrue(LINT.Analyzer(sources).run())
        # Conditional patterns must not mask a helper after their branch.
        sources['cas-cli/src/qa.rs'] = '#[test] fn bad() { if let Some(run_command) = value { run_command(); } run_command(); }'
        self.assertTrue(LINT.Analyzer(sources).run())

    def test_shared_guard_nesting_is_checked_in_every_member(self):
        source = 'use test_env_guard::TestEnvGuard; #[test] fn bad() { let _a = TestEnvGuard::new(); let _b = TestEnvGuard::new(); }'
        self.assertTrue(analyze(source, 'crates/cas-core/src/fixture.rs'))

    def test_helper_nesting_reports_new_call_site(self):
        source = 'fn fixture() { let _g = TestEnvGuard::new(); } #[test] fn a() { let _g = TestEnvGuard::new(); fixture(); }'
        rows = analyze(source)
        self.assertIn('nested-helper-call', {r['kind'] for r in rows})
        original = baseline(rows)
        rows = analyze(source + '#[test] fn b() { let _g = TestEnvGuard::new(); fixture(); }')
        self.assertTrue(LINT.ratchet(rows, original))

    def test_named_callbacks_obey_thread_ownership(self):
        helper = 'fn fixture() { let _g = TestEnvGuard::new(); } '
        self.assertTrue(analyze(helper + '#[test] fn bad() { TestEnvGuard::run_with_temp_home(fixture); }'))
        self.assertFalse(analyze(helper + '#[test] fn safe() { let _g = TestEnvGuard::new(); std::thread::spawn(fixture); }'))
        raw = 'fn fixture() { std::env::set_var("HOME", "x"); } '
        self.assertTrue(analyze(raw + '#[test] fn bad() { let _g = TestEnvGuard::new(); std::thread::spawn(fixture); }'))

    def test_legacy_helper_allowance_cannot_hide_new_unsafe_callers(self):
        helper = 'fn fixture() { std::env::set_var("HOME", "x"); } '
        source = helper + '#[test] fn old() { fixture(); }'
        manifest = baseline(analyze(source))
        added = analyze(source + '#[test] fn new() { fixture(); }')
        self.assertTrue(LINT.ratchet(added, manifest))
        self.assertIn('unguarded-helper-call', {r['kind'] for r in added})
        protected = analyze(source + '#[test] fn new() { let _g = TestEnvGuard::new(); fixture(); }')
        self.assertTrue(LINT.ratchet(protected, manifest))
        # A parent guard cannot protect an unguarded child created by a helper.
        child = 'fn fixture() { std::thread::spawn(|| { std::env::set_var("HOME", "x"); }); } '
        child_source = child + '#[test] fn old() { fixture(); }'
        manifest = baseline(analyze(child_source))
        self.assertTrue(LINT.ratchet(analyze(child_source + '#[test] fn new() { let _g = TestEnvGuard::new(); fixture(); }'), manifest))

    def test_returned_guard_keeps_ownership(self):
        source = 'fn fixture() -> TestEnvGuard { TestEnvGuard::new() } #[test] fn bad() { let _g = fixture(); let _second = TestEnvGuard::temp_home(); }'
        self.assertTrue(analyze(source))
        self.assertFalse(analyze(source.replace('let _second', 'drop(_g); let _second')))

    def test_optional_or_result_guard_return_does_not_prove_ownership(self):
        for output, value in [('Option<TestEnvGuard>', 'None'), ('Result<TestEnvGuard, ()>', 'Err(())')]:
            with self.subTest(output=output):
                source = 'fn fixture() -> ' + output + ' {' + value + '} #[test] fn bad() { let _g = fixture(); std::env::set_var("HOME", "x"); }'
                self.assertTrue(analyze(source))

    def test_cross_file_helper_resolution(self):
        sources = {'cas-cli/src/support.rs': 'pub fn fixture() { std::env::set_var("HOME", "x"); }',
                   'cas-cli/src/fixture.rs': '#[test] fn bad() { crate::support::fixture(); }'}
        rows = LINT.Analyzer(sources).run()
        self.assertIn('cas-cli/src/support.rs', {r['path'] for r in rows})
        sources['cas-cli/src/fixture.rs'] = 'use crate::support::*; #[test] fn bad() { fixture(); }'
        self.assertTrue(LINT.Analyzer(sources).run())
        sources['cas-cli/src/fixture.rs'] = '#[test] fn still_unsafe() { let _g = TestEnvGuard::new(); crate::support::fixture(); }'
        self.assertTrue(LINT.Analyzer(sources).run())
        sources['cas-cli/src/support.rs'] = 'pub fn fixture(g: &mut TestEnvGuard) { g.set("HOME", "x"); }'
        sources['cas-cli/src/fixture.rs'] = '#[test] fn safe() { let mut g = TestEnvGuard::new(); crate::support::fixture(&mut g); }'
        self.assertFalse(LINT.Analyzer(sources).run())

    def test_module_glob_does_not_reach_unrelated_production_commands(self):
        for mutation in ('set_var("TOKEN", "x")', 'remove_var("TOKEN")', 'set_current_dir(".")'):
            with self.subTest(mutation=mutation):
                sources = {
                    'cas-cli/src/cli/update.rs': 'pub fn execute() {}',
                    'cas-cli/src/cli/open.rs': 'pub fn execute() { std::env::' + mutation + '; }',
                    'cas-cli/src/cli/integrate/mod.rs': 'pub fn execute() { violet::execute(); }',
                    'cas-cli/src/cli/integrate/violet.rs': 'pub fn execute() { std::env::' + mutation + '; }',
                    'cas-cli/src/cli/update_tests/tests.rs': 'use crate::cli::update::*; #[test] fn post_swap() { let g = TestEnvGuard::new(); execute(); }',
                }
                self.assertFalse(LINT.Analyzer(sources).run())
                # A glob must still resolve the intended module's unsafe
                # function; narrowing resolution cannot hide the real hazard.
                sources['cas-cli/src/cli/update.rs'] = 'pub fn execute() { std::env::' + mutation + '; }'
                rows = LINT.Analyzer(sources).run()
                self.assertEqual({r['path'] for r in rows}, {
                    'cas-cli/src/cli/update.rs', 'cas-cli/src/cli/update_tests/tests.rs'})
                sources['cas-cli/src/cli/update_tests/tests.rs'] = sources['cas-cli/src/cli/update_tests/tests.rs'].replace('crate::cli::update::*', 'crate::unresolved::*')
                self.assertTrue(LINT.Analyzer(sources).run())

    def test_reviewed_production_mutations_are_reported_once_at_the_source(self):
        sources = {
            'cas-cli/src/eval.rs': 'struct NeutralEnv; impl NeutralEnv { fn acquire() { std::env::set_var("HOME", "fixture"); } } pub fn rank() { NeutralEnv::acquire(); }',
            'cas-cli/tests/eval_test.rs': '#[test] fn rank_case() { let g = TestEnvGuard::new(); crate::eval::rank(); }',
        }
        unreviewed = LINT.Analyzer(sources).run()
        self.assertIn('unguarded-helper-call', {r['kind'] for r in unreviewed})
        reviewed = {r['id'] for r in unreviewed if r['kind'] == 'unguarded-mutation'}
        rows = LINT.Analyzer(sources, reviewed_mutations=reviewed).run()
        self.assertEqual({r['id'] for r in rows}, reviewed)
        # A new or changed production mutation must still propagate.
        sources['cas-cli/src/eval.rs'] = sources['cas-cli/src/eval.rs'].replace('"fixture"', '"different"')
        self.assertIn('unguarded-helper-call', {r['kind'] for r in LINT.Analyzer(sources, reviewed_mutations=reviewed).run()})

    def test_reviewed_test_helpers_still_propagate_to_every_caller(self):
        for path, helper in (
            ('cas-cli/tests/support.rs', 'pub fn fixture() { std::env::remove_var("HOME"); }'),
            ('cas-cli/src/support.rs', '#[cfg(test)] pub fn fixture() { std::env::remove_var("HOME"); }'),
        ):
            with self.subTest(path=path):
                sources = {path: helper, 'cas-cli/tests/cases.rs': '#[test] fn old() { let g = TestEnvGuard::new(); crate::support::fixture(); }'}
                original = LINT.Analyzer(sources).run()
                reviewed = {r['id'] for r in original if r['kind'] == 'unguarded-mutation'}
                sources['cas-cli/tests/cases.rs'] += '#[test] fn new() { let g = TestEnvGuard::new(); crate::support::fixture(); }'
                rows = LINT.Analyzer(sources, reviewed_mutations=reviewed).run()
                self.assertEqual({r['function'] for r in rows}, {'fixture', 'old', 'new'})
                self.assertTrue(LINT.ratchet(rows, baseline(original)))

    def test_canonical_exception_is_exact_and_cannot_cover_ordinary_tests(self):
        source = 'impl TestEnvGuard { fn set() { std::env::set_var("HOME", "x"); } fn remove() { std::env::remove_var("HOME"); } fn set_current_dir() { std::env::set_current_dir("."); } }'
        rows = analyze(source, LINT.CANONICAL)
        self.assertEqual(len(rows), 3)
        self.assertTrue(all(r['canonical_impl'] for r in rows))
        self.assertFalse(LINT.ratchet(rows, baseline(rows, exceptions=True)))
        more = analyze(source.replace('"x"', '"different"'), LINT.CANONICAL)
        self.assertTrue(LINT.ratchet(more, baseline(rows, exceptions=True)))
        ordinary = analyze('#[test] fn bad() { std::env::set_var("HOME", "x"); }', LINT.CANONICAL)
        self.assertTrue(LINT.ratchet(ordinary, baseline(ordinary, exceptions=True)))
        guarded = analyze('#[test] fn bad() { let g = TestEnvGuard::new(); std::env::remove_var("HOME"); }', LINT.CANONICAL)
        self.assertTrue(LINT.ratchet(guarded, baseline(guarded, exceptions=True)))

    def test_baseline_schema_and_staleness(self):
        rows = analyze('#[test] fn bad() { std::env::set_var("HOME", "x"); }')
        manifest = baseline(rows)
        self.assertFalse(LINT.ratchet(rows, manifest))
        self.assertTrue(LINT.ratchet([], manifest))
        manifest['violations'][0]['reason'] = ''
        with self.assertRaises(ValueError):
            LINT.ratchet(rows, manifest)
        manifest = baseline(rows)
        manifest['violations'].append(manifest['violations'][0].copy())
        with self.assertRaises(ValueError):
            LINT.ratchet(rows, manifest)

    def test_identity_survives_line_edits_but_not_new_sites(self):
        source = '#[test] fn bad() { std::env::set_var("HOME", "x"); }'
        rows = analyze(source)
        self.assertEqual(rows[0]['id'], analyze('// new line\n' + source)[0]['id'])
        twice = analyze(source.replace('; }', '; std::env::set_var("HOME", "x"); }'))
        self.assertEqual(len(twice), 2)
        self.assertTrue(LINT.ratchet(twice, baseline(rows)))


class GitRatchet(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.write('Cargo.toml', '[workspace]\nmembers = ["cas-cli", "crates/other"]\n')
        self.write('cas-cli/src/fixture.rs', '#[test] fn safe() {}')
        self.write('crates/other/src/lib.rs', '#[test] fn safe() {}')
        self.write(LINT.BASELINE, json.dumps(baseline()))
        if self._testMethodName == 'test_fix_requires_pruning_and_pruning_passes':
            source = '#[test] fn bad() { std::env::set_var("HOME", "x"); }'
            self.write('cas-cli/src/fixture.rs', source)
            self.write(LINT.BASELINE, json.dumps(baseline(analyze(source))))
        if self._testMethodName in ('test_reviewed_production_source_stops_cli_propagation',
                                    'test_reviewed_test_helper_keeps_cli_caller_ratchet'):
            support = 'pub fn fixture() { std::env::set_var("HOME", "fixture"); }'
            is_test_helper = self._testMethodName.endswith('caller_ratchet')
            if is_test_helper:
                support = '#[cfg(test)] ' + support
            caller = '#[test] fn old() { let g = TestEnvGuard::new(); crate::support::fixture(); }'
            self.write('cas-cli/src/support.rs', support)
            self.write('cas-cli/src/fixture.rs', caller)
            rows = LINT.Analyzer({'cas-cli/src/support.rs': support,
                                  'cas-cli/src/fixture.rs': caller}).run()
            if not is_test_helper:
                rows = [r for r in rows if r['kind'] == 'unguarded-mutation']
            self.write(LINT.BASELINE, json.dumps(baseline(rows)))
        if self._testMethodName in ('test_scoped_new_caller_reaches_unchanged_helper',
                                    'test_scoped_helper_change_checks_unchanged_callers'):
            support = ('#[cfg(test)] fn fixture() { std::env::set_var("HOME", "x"); }'
                       if self._testMethodName.endswith('unchanged_helper') else '#[cfg(test)] fn fixture() {}')
            self.write('cas-cli/src/support.rs', support)
            self.write(LINT.BASELINE, json.dumps(baseline(analyze(support, 'cas-cli/src/support.rs'))))
        if self._testMethodName in ('test_scoped_unaffected_baseline_is_not_treated_as_stale',
                                    'test_scoped_deleted_source_requires_pruning'):
            path = ('crates/other/src/lib.rs' if 'unaffected' in self._testMethodName
                    else 'cas-cli/src/fixture.rs')
            source = '#[test] fn bad() { std::env::remove_var("HOME"); }'
            self.write(path, source)
            self.write(LINT.BASELINE, json.dumps(baseline(analyze(source, path))))
        for args in [('init', '-q'), ('config', 'user.name', 'Lint fixture'),
                     ('config', 'user.email', 'fixture@example.invalid')]:
            self.git(*args)
        self.commit()
        self.base = self.git('rev-parse', 'HEAD').strip()

    def write(self, name, body):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body)

    def git(self, *args):
        return LINT.git(self.root, *args)

    def commit(self):
        self.git('add', '.')
        self.git('commit', '-qm', 'fixture')

    def lint(self, base=None, changed_paths=False):
        return subprocess.run([sys.executable, str(ROOT / 'scripts/check-test-env.py'),
                               '--root', str(self.root), '--changed-since', base or self.base]
                              + (['--changed-paths'] if changed_paths else []),
                              text=True, capture_output=True)

    def test_scoped_unaffected_baseline_is_not_treated_as_stale(self):
        self.write('cas-cli/src/fixture.rs', '#[test] fn safe() { let value = 1; }')
        result = self.lint(changed_paths=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('1 Rust paths analyzed (affected crates)', result.stdout)
        self.assertEqual(self.lint().returncode, 0)

    def test_scoped_new_caller_reaches_unchanged_helper(self):
        self.write('cas-cli/tests/new.rs', '#[test] fn caller() { crate::support::fixture(); }')
        result = self.lint(changed_paths=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn('cas-cli/tests/new.rs:1: unguarded-helper-call', result.stderr)

    def test_scoped_helper_change_checks_unchanged_callers(self):
        self.write('cas-cli/src/fixture.rs', '#[test] fn caller() { crate::support::fixture(); }')
        self.commit()
        base = self.git('rev-parse', 'HEAD').strip()
        self.write('cas-cli/src/support.rs', '#[cfg(test)] fn fixture() { std::env::remove_var("HOME"); }')
        result = self.lint(base, changed_paths=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn('cas-cli/src/fixture.rs:1: unguarded-helper-call', result.stderr)

    def test_scoped_deleted_source_requires_pruning(self):
        (self.root / 'cas-cli/src/fixture.rs').unlink()
        result = self.lint(changed_paths=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn('stale baseline entry', result.stderr)
        self.write(LINT.BASELINE, json.dumps(baseline()))
        self.assertEqual(self.lint(changed_paths=True).returncode, 0)

    def test_scoped_policy_change_forces_complete_inventory(self):
        self.write('crates/other/src/lib.rs', '#[test] fn broken() {')
        self.commit()
        base = self.git('rev-parse', 'HEAD').strip()
        self.assertEqual(self.lint(base, changed_paths=True).returncode, 0)
        self.write('scripts/check-test-env.py', '# changed lint policy\n')
        result = self.lint(base, changed_paths=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn('unbalanced Rust delimiters', result.stderr)

    def test_scoped_baseline_growth_still_fails(self):
        source = '#[test] fn bad() { std::env::set_var("HOME", "x"); }'
        self.write('crates/other/src/lib.rs', source)
        self.write(LINT.BASELINE, json.dumps(baseline(analyze(source, 'crates/other/src/lib.rs'))))
        result = self.lint(changed_paths=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn('baseline growth', result.stderr)

    def test_new_violation_any_member_and_untracked_file(self):
        self.assertEqual(self.lint().returncode, 0)
        self.write('crates/other/tests/new.rs', '#[test] fn bad() { std::env::set_current_dir("."); }')
        result = self.lint()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('crates/other/tests/new.rs:1', result.stderr)

    def test_held_guard_does_not_hide_raw_mutations_from_cli(self):
        self.write('crates/other/tests/new.rs', '#[test] fn bad() { let mut g = TestEnvGuard::new(); std::env::set_var("CAS_GH_BIN", "stub"); }')
        result = self.lint()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('std::env::set_var', result.stderr)
        self.write('crates/other/tests/new.rs', '#[test] fn safe() { let mut g = TestEnvGuard::new(); g.set("CAS_GH_BIN", "stub"); g.remove("CAS_GH_BIN"); g.set_current_dir("."); }')
        result = self.lint()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_reviewed_production_source_stops_cli_propagation(self):
        result = self.lint()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.write('cas-cli/tests/new.rs', '#[test] fn new() { let g = TestEnvGuard::new(); crate::support::fixture(); }')
        result = self.lint()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('1 findings', result.stdout)
        # The existing allowance never admits a changed production hazard.
        self.write('cas-cli/src/support.rs', 'pub fn fixture() { std::env::set_var("HOME", "different"); }')
        self.assertEqual(self.lint().returncode, 1)

    def test_reviewed_test_helper_keeps_cli_caller_ratchet(self):
        result = self.lint()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.write('cas-cli/tests/new.rs', '#[test] fn new() { let g = TestEnvGuard::new(); crate::support::fixture(); }')
        result = self.lint()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('unguarded-helper-call', result.stderr)

    def test_committed_and_uncommitted_baseline_growth_fail(self):
        source = '#[test] fn bad() { std::env::set_var("HOME", "x"); }'
        self.write('cas-cli/src/fixture.rs', source)
        self.write(LINT.BASELINE, json.dumps(baseline(analyze(source))))
        result = self.lint()
        self.assertEqual(result.returncode, 1)
        self.assertIn('baseline growth', result.stderr)
        self.commit()
        result = self.lint()
        self.assertEqual(result.returncode, 1)
        self.assertIn('baseline growth since ' + self.base, result.stderr)
        full = subprocess.run([sys.executable, str(ROOT / 'scripts/check-test-env.py'),
                               '--root', str(self.root)], text=True, capture_output=True)
        self.assertEqual(full.returncode, 1)
        self.assertIn('baseline growth since ' + self.base, full.stderr)

    def test_fix_requires_pruning_and_pruning_passes(self):
        source = '#[test] fn bad() { std::env::set_var("HOME", "x"); }'
        self.write('cas-cli/src/fixture.rs', source)
        self.write(LINT.BASELINE, json.dumps(baseline(analyze(source))))
        accepted = self.base
        self.write('cas-cli/src/fixture.rs', '#[test] fn safe() {}')
        self.assertIn('stale baseline entry', self.lint(accepted).stderr)
        self.write(LINT.BASELINE, json.dumps(baseline()))
        self.assertEqual(self.lint(accepted).returncode, 0)
        self.commit()
        self.write('cas-cli/src/fixture.rs', source)
        self.write(LINT.BASELINE, json.dumps(baseline(analyze(source))))
        self.commit()
        full = subprocess.run([sys.executable, str(ROOT / 'scripts/check-test-env.py'),
                               '--root', str(self.root)], text=True, capture_output=True)
        self.assertEqual(full.returncode, 1)
        self.assertIn('baseline growth', full.stderr)

    def test_missing_manifest_invalid_ref_and_syntax_fail_closed(self):
        self.assertEqual(self.lint('missing-reference').returncode, 1)
        self.write('cas-cli/src/fixture.rs', '#[test] fn broken() {')
        self.assertIn('unbalanced Rust delimiters', self.lint().stderr)
        self.write('cas-cli/src/fixture.rs', '#[test] fn safe() {}')
        (self.root / LINT.BASELINE).unlink()
        self.assertEqual(self.lint().returncode, 1)
        # Initial inventory remains usable before the first baseline exists;
        # no production mutation can be treated as reviewed in that case.
        self.write('cas-cli/src/fixture.rs', '#[test] fn bad() { let g = TestEnvGuard::new(); std::env::remove_var("HOME"); }')
        result = subprocess.run([sys.executable, str(ROOT / 'scripts/check-test-env.py'),
                                 '--root', str(self.root), '--inventory'], text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(len(json.loads(result.stdout)), 1)

    def test_nonmembers_and_production_startup_are_outside_scope(self):
        self.write('vendor/dependency/src/lib.rs', '#[test] fn bad() { std::env::remove_var("HOME"); }')
        self.write('crates/other/src/lib.rs', 'fn startup() { std::env::remove_var("HOME"); }')
        self.assertEqual(self.lint().returncode, 0)


if __name__ == '__main__':
    unittest.main()
