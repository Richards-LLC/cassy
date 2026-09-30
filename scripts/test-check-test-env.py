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
            ('guarded raw call', 'let _g = TestEnvGuard::new(); std::env::set_var("HOME", "x");', False),
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
        good = 'fn helper(g: &mut TestEnvGuard) { std::env::set_var("HOME", "x"); } #[test] fn ok() { let mut g = TestEnvGuard::new(); helper(&mut g); }'
        self.assertFalse(analyze(good))
        dropped = 'fn helper(g: TestEnvGuard) { drop(g); std::env::set_var("HOME", "x"); } #[test] fn bad() { helper(TestEnvGuard::new()); }'
        self.assertTrue(analyze(dropped))
        optional = '#[cfg(test)] fn helper(g: Option<TestEnvGuard>) { std::env::set_var("HOME", "x"); } #[test] fn bad() { helper(None); }'
        self.assertTrue(analyze(optional))
        alias = 'use crate::test_support::TestEnvGuard as Guard; fn helper(g: &mut Guard) { std::env::set_var("HOME", "x"); } #[test] fn ok() { let mut g = Guard::new(); helper(&mut g); }'
        self.assertFalse(analyze(alias))

    def test_function_and_module_imports(self):
        for imp, call in [('use std::env::{set_var as change};', 'change'),
                          ('use std::env as state;', 'state::remove_var'),
                          ('use std::{env as state};', 'state::set_current_dir'),
                          ('use std::env::remove_var;', 'remove_var')]:
            with self.subTest(imp=imp):
                self.assertTrue(analyze(imp + '#[test] fn bad() {' + call + '("HOME", "x");}'))

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
        self.assertFalse(LINT.ratchet(protected, manifest))
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
        sources['cas-cli/src/fixture.rs'] = '#[test] fn safe() { let _g = TestEnvGuard::new(); crate::support::fixture(); }'
        self.assertFalse(LINT.Analyzer(sources).run())

    def test_canonical_exception_is_exact_and_cannot_cover_ordinary_tests(self):
        source = 'impl TestEnvGuard { fn set() { std::env::set_var("HOME", "x"); } }'
        rows = analyze(source, LINT.CANONICAL)
        self.assertFalse(LINT.ratchet(rows, baseline(rows, exceptions=True)))
        more = analyze(source.replace('"x"', '"different"'), LINT.CANONICAL)
        self.assertTrue(LINT.ratchet(more, baseline(rows, exceptions=True)))
        ordinary = analyze('#[test] fn bad() { std::env::set_var("HOME", "x"); }', LINT.CANONICAL)
        self.assertTrue(LINT.ratchet(ordinary, baseline(ordinary, exceptions=True)))

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

    def lint(self, base=None):
        return subprocess.run([sys.executable, str(ROOT / 'scripts/check-test-env.py'),
                               '--root', str(self.root), '--changed-since', base or self.base],
                              text=True, capture_output=True)

    def test_new_violation_any_member_and_untracked_file(self):
        self.assertEqual(self.lint().returncode, 0)
        self.write('crates/other/tests/new.rs', '#[test] fn bad() { std::env::set_current_dir("."); }')
        result = self.lint()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('crates/other/tests/new.rs:1', result.stderr)

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

    def test_nonmembers_and_production_startup_are_outside_scope(self):
        self.write('vendor/dependency/src/lib.rs', '#[test] fn bad() { std::env::remove_var("HOME"); }')
        self.write('crates/other/src/lib.rs', 'fn startup() { std::env::remove_var("HOME"); }')
        self.assertEqual(self.lint().returncode, 0)


if __name__ == '__main__':
    unittest.main()
