#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('shape', Path(__file__).with_name('check-test-shape.py'))
shape = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shape)


class Shapes(unittest.TestCase):
    def hits(self, code, path='src/lib.rs'):
        return shape.lint(code, path)

    def test_flag_both_constant_orders(self):
        self.assertEqual(len(self.hits('#[test] fn t() { assert_eq!(LIMIT, 3); assert_eq!("x", ENV); }')), 2)

    def test_pinned_pass(self):
        self.assertFalse(self.hits('#[test] fn t() {\n// pin: external compatibility window\nassert_eq!(LIMIT, 3); }'))

    def test_empty_pin_does_not_pass(self):
        self.assertTrue(self.hits('#[test] fn t() {\n// pin: \nassert_eq!(LIMIT, 3); }'))

    def test_pin_is_not_reused_by_later_assertion(self):
        self.assertEqual(len(self.hits('#[test] fn t() {\n// pin: contract\nassert_eq!(LIMIT, 3); assert_eq!(OTHER, 9); }')), 1)

    def test_clean_behavior_and_production(self):
        self.assertFalse(self.hits('#[test] fn t() { assert_eq!(run(input), 3); } fn real() { assert_eq!(LIMIT, 3); }'))

    def test_literals_are_not_code(self):
        self.assertFalse(self.hits('#[test] fn t() { let s = r###"assert_eq!(LIMIT, 3); include_str!(\"lib.rs\") {"###; /* /*nested*/ assert_eq!(LIMIT, 3); */ }'))

    def test_byte_char_negative_bool_and_qualified_constants(self):
        self.assertEqual(len(self.hits("#[test] fn t() { assert_eq!(other::BYTE, b'x'); assert_eq!(-3, MIN); assert_eq!(FLAG, true); }")), 3)

    def test_include_raw_path_and_multiline_assert(self):
        self.assertEqual(len(self.hits('#[test] fn t() { let src = include_str!(r#"lib.rs"#); assert_eq!(\n LIMIT,\n 3, "message"); }')), 2)

    def test_source_read_and_position_check(self):
        self.assertTrue(self.hits('#[test] fn t() { let p = root.join("lib.rs"); let s = fs::read_to_string(&p).unwrap(); assert!(s.find("a") < s.find("b")); }'))

    def test_test_only_helper_and_integration_helper(self):
        code = 'fn helper() { let s = include_str!("lib.rs"); }'
        self.assertTrue(self.hits(code, 'tests/contract.rs'))
        self.assertTrue(self.hits('#[cfg(test)] mod tests {' + code + '}'))
        self.assertFalse(self.hits(code))

    def test_function_pin_and_comment_text_false_positive(self):
        self.assertFalse(self.hits('#[test]\n// pin: cross-file registry completeness\nfn t() { let s = include_str!("lib.rs"); assert_eq!(LIMIT, 3); }'))
        self.assertTrue(self.hits('#[test] fn t() { let s="// pin: untrusted literal"; assert_eq!(LIMIT, 3); }'))

    def test_test_only_global_source_and_duration_pin(self):
        self.assertTrue(self.hits('const SOURCE: &str = include_str!("lib.rs");', 'tests/probe.rs'))
        self.assertFalse(self.hits('// pin: registration completeness\nconst SOURCES: [(&str, &str); 1] = [("name", include_str!("lib.rs"))];', 'tests/probe.rs'))
        self.assertTrue(self.hits('#[cfg(test)] mod tests { const SOURCE: &str = include_str!("lib.rs"); }'))
        self.assertTrue(self.hits('#[test] fn t() { assert_eq!(WINDOW, Duration::from_secs(60)); }'))
        self.assertFalse(self.hits('#[tokio::test]\n// pin: compatibility\nasync fn t() { assert_eq!(LIMIT, 3); }'))

    def test_non_rust_text_is_clean(self):
        self.assertFalse(self.hits('#[test] fn t() { let s=include_str!("contract.md"); assert!(s.contains("word")); }'))

    def test_exact_prose_pin_needs_a_reasoned_escape(self):
        # // pin: these fixtures exercise the exact prose pin and its intentional exception.
        unpinned = '''def test_prompt_contract():
    assert prompt == "When a test fails, name the responsible boundary and preserve its first result."
def test_workflow_command():
    assert "uses: actions/checkout@v4" in workflow_yaml
'''
        pinned = '''def test_prompt_contract():
    # // pin: this exact wording is a third-party compatibility contract.
    assert prompt == "When a test fails, name the responsible boundary and preserve its first result."
'''
        self.assertEqual(
            shape.lint_text_pins(unpinned, 'tests/test_prompt.py'),
            [(2, 'exact-text pin'), (4, 'exact-text pin')],
        )
        self.assertFalse(shape.lint_text_pins(pinned, 'tests/test_prompt.py'))

    def test_archive_binary_paths_are_rejected_without_pin_escape(self):
        for code in [
            'fn helper() { Command::new(env!("CARGO_BIN_EXE_cas")); }',
            'const BIN: &str = env!(r#"CARGO_BIN_EXE_cas"#);',
            'fn helper() { Command::new(std::env! /* gap */ (\n"CARGO_BIN_EXE_cas",\n)); }',
            '// pin: archive contract\nconst BIN: &str = env! { "CARGO_BIN_EXE_cas" };',
        ]:
            with self.subTest(code=code):
                self.assertEqual(
                    [kind for _, kind in self.hits(code, 'cas-cli/tests/nested/probe.rs')],
                    ['archive-unsafe Cargo binary path'],
                )
        for code in [
            '// env!("CARGO_BIN_EXE_cas")\nfn helper() {}',
            'const DOC: &str = r#"env!("CARGO_BIN_EXE_cas")"#;',
            'fn helper() { Command::new(cas::test_paths::cas_binary()); }',
            'fn helper() { cas::test_paths::binary("cas", option_env!("CARGO_BIN_EXE_cas").map(Into::into)); }',
            'const OTHER: &str = env!("CARGO_MANIFEST_DIR");',
        ]:
            with self.subTest(code=code):
                self.assertFalse(self.hits(code, 'cas-cli/tests/probe.rs'))
        self.assertFalse(self.hits('const BIN: &str = env!("CARGO_BIN_EXE_cas");', 'cas-cli/src/lib.rs'))

    def test_archive_binary_rule_covers_every_crate_and_binary_cas_7bce(self):
        # The workspace archive carries every crate's integration tests.
        self.assertEqual([kind for _, kind in self.hits(
            'fn bin() -> &\'static str { env!("CARGO_BIN_EXE_test_tui") }', 'crates/cas-tui-test/tests/e2e.rs')],
            ['archive-unsafe Cargo binary path (test_tui)'])
        self.assertEqual([kind for _, kind in self.hits(
            'const BIN: &str = env!("CARGO_BIN_EXE_cas-hub");', 'cas-cli/tests/hub.rs')],
            ['archive-unsafe Cargo binary path (cas-hub)'])
        for code in ['const KEYS: [&str; 1] = ["CARGO_BIN_EXE_test_tui"];',
                     'fn bin() { let _ = option_env!("CARGO_BIN_EXE_test_tui"); }']:
            with self.subTest(code=code):
                self.assertFalse(self.hits(code, 'crates/cas-tui-test/tests/e2e.rs'))
        self.assertFalse(self.hits('const BIN: &str = env!("CARGO_BIN_EXE_x");', 'crates/cas-core/src/lib.rs'))

    def test_archive_binary_cli_rejects_reintroduction_in_changed_files(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            def git(*args):
                return subprocess.check_output(['git', '-C', str(root), *args], text=True).strip()
            git('init', '-q', '-b', 'main')
            git('config', 'user.email', 'fixture@example.test')
            git('config', 'user.name', 'fixture')
            path = root/'cas-cli/tests/nested/probe.rs'
            path.parent.mkdir(parents=True)
            path.write_text('fn helper() { Command::new(cas::test_paths::cas_binary()); }')
            git('add', '.')
            git('commit', '-qm', 'portable base')
            base = git('rev-parse', 'HEAD')
            path.write_text('fn helper() { Command::new(env!("CARGO_BIN_EXE_cas")); }')
            command = ['python3', str(Path(shape.__file__).resolve())]
            for args in [[], ['--changed-since', base]]:
                result = subprocess.run(command+args, cwd=root, capture_output=True, text=True)
                self.assertEqual(result.returncode, 1, result.stdout)
                self.assertIn('cas-cli/tests/nested/probe.rs:1: archive-unsafe Cargo binary path', result.stdout)
                self.assertIn('cas::test_paths::cas_binary()', result.stdout)
                self.assertNotIn('add // pin:', result.stdout)

    def test_cli_changed_merge_base_and_invalid_ref(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            def git(*args):
                return subprocess.check_output(['git', '-C', str(root), *args], text=True).strip()
            git('init', '-q', '-b', 'main'); git('config', 'user.email', 'fixture@example.test'); git('config', 'user.name', 'fixture')
            (root/'lib.rs').write_text('#[test] fn t() { assert_eq!(LIMIT, 3); }')
            git('add', '.'); git('commit', '-qm', 'base')
            base=git('rev-parse', 'HEAD')
            cmd=['python3', str(Path(shape.__file__).resolve())]
            bad=subprocess.run(cmd, cwd=root, capture_output=True, text=True)
            self.assertEqual(bad.returncode, 1); self.assertIn('lib.rs:1:', bad.stdout)
            clean=subprocess.run(cmd+['--changed-since', base], cwd=root, capture_output=True, text=True)
            self.assertEqual(clean.returncode, 0)
            (root/'lib.rs').write_text('#[test] fn t() { assert_eq!(run(), 3); }')
            self.assertEqual(subprocess.run(cmd+['--changed-since', base], cwd=root, capture_output=True).returncode, 0)
            self.assertEqual(subprocess.run(cmd+['--changed-since', 'missing-ref'], cwd=root, capture_output=True).returncode, 1)

    def test_cli_changed_since_scans_non_rust_test_files(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            def git(*args):
                return subprocess.check_output(['git', '-C', str(root), *args], text=True).strip()
            git('init', '-q', '-b', 'main')
            git('config', 'user.email', 'fixture@example.test')
            git('config', 'user.name', 'fixture')
            path = root/'tests/test_prompt.py'
            path.parent.mkdir(parents=True)
            path.write_text('def test_prompt():\n    assert prompt == "behavior"\n')
            git('add', '.')
            git('commit', '-qm', 'behavior baseline')
            base = git('rev-parse', 'HEAD')
            path.write_text('def test_prompt():\n    assert prompt == "The release gate should identify which component caused this failure."\n')
            command = ['python3', str(Path(shape.__file__).resolve()), '--changed-since', base]
            result = subprocess.run(command, cwd=root, capture_output=True, text=True)
            self.assertEqual(result.returncode, 1, result.stdout)
            self.assertIn('tests/test_prompt.py:2: exact-text pin', result.stdout)
            path.write_text('def test_prompt():\n    # // pin: this phrase is an external compatibility contract.\n    assert prompt == "The release gate should identify which component caused this failure."\n')
            clean = subprocess.run(command, cwd=root, capture_output=True, text=True)
            self.assertEqual(clean.returncode, 0, clean.stdout)

    def test_cli_changed_since_checks_only_added_or_modified_pin_lines(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            def git(*args):
                return subprocess.check_output(['git', '-C', str(root), *args], text=True).strip()
            git('init', '-q', '-b', 'main')
            git('config', 'user.email', 'fixture@example.test')
            git('config', 'user.name', 'fixture')
            path = root/'tests/test_prompt.rs'
            path.parent.mkdir(parents=True)
            old_pin = '''#[test]
fn old_contract() {
    assert_eq!(prompt, "When a test fails, name the responsible boundary and preserve its first result.");
}
'''
            path.write_text(old_pin)
            git('add', '.')
            git('commit', '-qm', 'legacy prose pin')
            base = git('rev-parse', 'HEAD')
            command = ['python3', str(Path(shape.__file__).resolve()), '--changed-since', base]

            # An unchanged legacy pin must not fail just because this file has
            # a newly added, reasoned pin.
            reasoned_pin = '''
#[test]
fn added_reasoned_contract() {
    // pin: this wording is part of the source compatibility contract.
    assert_eq!(prompt, "When a new test is added, name the behavior it verifies with useful context.");
}
'''
            path.write_text(old_pin + reasoned_pin)
            clean = subprocess.run(command, cwd=root, capture_output=True, text=True)
            self.assertEqual(clean.returncode, 0, clean.stdout)

            # A changed legacy line is in scope, including when only one part
            # of the string literal's text changes.
            modified = old_pin.replace('first result', 'original result') + reasoned_pin
            path.write_text(modified)
            changed = subprocess.run(command, cwd=root, capture_output=True, text=True)
            self.assertEqual(changed.returncode, 1, changed.stdout)
            self.assertIn('tests/test_prompt.rs:3: exact-text pin', changed.stdout)

            # The pin rule still rejects a new unreasoned exact-text pin.
            new_unreasoned = '''
#[test]
fn added_unreasoned_contract() {
    assert_eq!(prompt, "When workers complete a task, record the exact evidence and the branch tip they delivered.");
}
'''
            path.write_text(modified + new_unreasoned)
            added = subprocess.run(command, cwd=root, capture_output=True, text=True)
            self.assertEqual(added.returncode, 1, added.stdout)
            self.assertIn('tests/test_prompt.rs:14: exact-text pin', added.stdout)

            # Without --changed-since, changed test files retain full-file
            # scanning, so the unchanged legacy pin remains visible.
            full = subprocess.run(['python3', str(Path(shape.__file__).resolve())],
                                 cwd=root, capture_output=True, text=True)
            self.assertEqual(full.returncode, 1, full.stdout)
            self.assertIn('tests/test_prompt.rs:3: exact-text pin', full.stdout)


class SyncMergeScope(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.git('init', '-q', '-b', 'main')
        self.git('config', 'user.email', 'fixture@example.test')
        self.git('config', 'user.name', 'fixture')
        (self.root / 'lib.rs').write_text('fn production() {}\n')
        self.base = self.commit()
        self.git('branch', 'worker')

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.root), *args], text=True).strip()

    def commit(self):
        self.git('add', '.')
        self.git('commit', '-qm', 'fixture')
        return self.git('rev-parse', 'HEAD')

    def sync_upstream(self):
        self.git('switch', '-q', 'main')
        (self.root / 'upstream.rs').write_text('#[test] fn upstream() { assert_eq!(UPSTREAM, 3); }\n')
        upstream = self.commit()
        self.git('update-ref', 'refs/remotes/origin/main', upstream)
        self.git('symbolic-ref', 'refs/remotes/origin/HEAD', 'refs/remotes/origin/main')
        self.git('switch', '-q', 'worker')
        self.git('merge', '-q', '--no-ff', 'origin/main', '-m', 'sync upstream')

    def run_guard(self):
        return subprocess.run(['python3', str(Path(shape.__file__).resolve()),
                               '--changed-since', self.base], cwd=self.root,
                              capture_output=True, text=True)

    def test_sync_merge_does_not_lint_upstream_only_files(self):
        self.sync_upstream()
        result = self.run_guard()
        self.assertEqual(result.returncode, 0, result.stdout)

    def test_sync_merge_keeps_task_changes_before_and_after_sync_in_scope(self):
        self.git('switch', '-q', 'worker')
        (self.root / 'own.rs').write_text('#[test] fn own() { assert_eq!(OWN, 3); }\n')
        self.commit()
        self.sync_upstream()
        # A tracked working edit remains in scope alongside pre-sync commits.
        (self.root / 'lib.rs').write_text('#[test] fn pending() { assert_eq!(PENDING, 3); }\n')
        result = self.run_guard()
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn('own.rs:1:', result.stdout)
        self.assertIn('lib.rs:1:', result.stdout)
        self.assertNotIn('upstream.rs:', result.stdout)
        # Once the lane is released, trunk CI still checks its full diff.
        self.git('update-ref', 'refs/remotes/origin/main', 'HEAD')
        released = self.run_guard()
        self.assertEqual(released.returncode, 1, released.stdout)
        self.assertIn('own.rs:1:', released.stdout)
        self.assertIn('upstream.rs:1:', released.stdout)


if __name__ == '__main__':
    unittest.main()
