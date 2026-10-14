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
            'const OTHER: &str = env!("CARGO_MANIFEST_DIR");',
        ]:
            with self.subTest(code=code):
                self.assertFalse(self.hits(code, 'cas-cli/tests/probe.rs'))
        self.assertFalse(self.hits('const BIN: &str = env!("CARGO_BIN_EXE_cas");', 'cas-cli/src/lib.rs'))

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


if __name__ == '__main__':
    unittest.main()
