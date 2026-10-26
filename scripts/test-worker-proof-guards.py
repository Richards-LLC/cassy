#!/usr/bin/env python3
"""Behavior fixtures for changed-string warnings; no compiler or systemd."""
import importlib.util
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('worker_guards', SCRIPTS / 'worker-proof-guards.py')
GUARDS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GUARDS)


class ChangedStrings(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.repo = Path(self.directory.name)
        (self.repo / 'scripts').mkdir()
        for name in ['check-test-shape.py', 'rust_test_source.py']:
            shutil.copy(SCRIPTS / name, self.repo / 'scripts' / name)
        self.git('init', '-q')

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.repo), *args], text=True).strip()

    def commit(self):
        self.git('add', '.')
        self.git('-c', 'user.name=Test', '-c', 'user.email=test@example.com', 'commit', '-qm', 'fixture')
        return self.git('rev-parse', 'HEAD')

    def test_changed_receipt_names_inline_and_integration_assertions(self):
        product = self.repo / 'product.rs'
        product.write_text('fn receipt() -> &\'static str { "old receipt" }\n')
        (self.repo / 'tests').mkdir()
        (self.repo / 'tests/receipt.rs').write_text('#[test]\nfn codex_receipt() { assert!(reply.contains("old receipt")); }\n')
        (self.repo / 'inline.rs').write_text('#[cfg(test)] mod tests { #[test] fn alias_receipt() { assert_eq!(receipt(), r#"old receipt"#); } }\n')
        base = self.commit()
        product.write_text('fn receipt() -> &\'static str { "new receipt" }\n')
        hits = GUARDS.changed_string_matches(self.repo, base)
        self.assertEqual({(path, name) for path, _, name in hits},
                         {('inline.rs', 'alias_receipt'), ('tests/receipt.rs', 'codex_receipt')})

    def test_deleted_escaped_literal_finds_raw_test_reference(self):
        product = self.repo / 'product.rs'
        product.write_text('fn receipt() { println!("old\\nreceipt"); }\n')
        (self.repo / 'inline.rs').write_text('#[test]\nfn multiline_receipt() { assert_eq!(receipt(), r#"old\nreceipt"#); }\n')
        base = self.commit()
        product.unlink()
        hits = GUARDS.changed_string_matches(self.repo, base)
        self.assertEqual([name for _, _, name in hits], ['multiline_receipt'])

    def test_first_token_docstring_uses_test_file_label(self):
        product = self.repo / 'product.rs'
        product.write_text('fn receipt() { "old receipt"; }\n')
        (self.repo / 'test_receipt.py').write_text('"""old receipt"""\n')
        base = self.commit()
        product.write_text('fn receipt() { "new receipt"; }\n')
        self.assertEqual(GUARDS.changed_string_matches(self.repo, base),
                         [('test_receipt.py', 1, '<test file>')])

    def test_unchanged_literals_comments_and_production_functions_do_not_warn(self):
        product = self.repo / 'product.rs'
        product.write_text('fn receipt() { "old receipt"; "keep"; }\n')
        (self.repo / 'inline.rs').write_text('// #[test] fn fake() { "old receipt"; }\nfn production() { "old receipt"; }\n#[test] fn unchanged() { "keep"; }\n')
        base = self.commit()
        product.write_text('fn receipt() { "new receipt"; "keep"; }\n')
        self.assertEqual(GUARDS.changed_string_matches(self.repo, base), [])

    def test_format_receipt_field_names_substring_assertion(self):
        product = self.repo / 'product.rs'
        product.write_text('fn receipt() { format!("Message queued\\nTo: {target}"); }\n')
        (self.repo / 'inline.rs').write_text('#[test] fn supervisor_alias() { assert!(reply.contains("To: cosmic-bear")); }\n')
        base = self.commit()
        product.write_text('fn receipt() { format!("queued -> {target}"); }\n')
        self.assertEqual([name for _, _, name in GUARDS.changed_string_matches(self.repo, base)],
                         ['supervisor_alias'])


if __name__ == '__main__':
    unittest.main()
