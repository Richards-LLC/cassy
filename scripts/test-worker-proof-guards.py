#!/usr/bin/env python3
"""Behavior fixtures for changed-string warnings; no compiler or systemd."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('worker_guards', SCRIPTS / 'worker-proof-guards.py')
GUARDS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GUARDS)
sys.path.insert(0, str(SCRIPTS))
SPEC = importlib.util.spec_from_file_location('worker_stability', SCRIPTS / 'worker-test-stability.py')
STABILITY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(STABILITY)


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

    def test_touched_test_stability_runs_five_concurrent_copies_and_keeps_a_failure(self):
        (self.repo / 'Cargo.toml').write_text('[package]\nname="cas"\nversion="0.1.0"\n')
        (self.repo / 'src').mkdir()
        source = self.repo / 'src/lib.rs'
        source.write_text('#[test] fn touched() { assert!(ok()); }\n#[test] fn untouched() {}\n')
        base = self.commit()
        source.write_text('#[test] fn touched() { assert!(ok()); println!("changed"); }\n#[test] fn untouched() {}\n')
        self.commit()
        fake = self.repo / 'cargo-fixture'
        fake.write_text('''#!/usr/bin/env python3
import fcntl,json,os,sys,time
from pathlib import Path
args=sys.argv[1:]
if args[:2]==['nextest','list']:
 print(json.dumps({'rust-suites': {'fixture': {'testcases': {'touched': {'ignored':False}, 'untouched': {'ignored':False}}}}}))
elif any('test_support::' in arg for arg in args):
 print('     Summary [ 0.01s] 2 tests run: 2 passed, 0 skipped')
elif any('test(=touched)' in arg for arg in args):
 assert '--retries' in args and args[args.index('--retries')+1]=='0',args
 assert not any('untouched' in arg for arg in args),args
 state=Path(os.environ['STABILITY_FIXTURE_STATE'])
 with state.open('a+') as file:
  fcntl.flock(file,fcntl.LOCK_EX)
  file.seek(0); index=len(file.readlines())+1
  file.write(str(index)+'\\n'); file.flush()
 deadline=time.monotonic()+10
 while len(state.read_text().splitlines())<5:
  assert time.monotonic()<deadline,'five concurrent runs never arrived'
  time.sleep(.01)
 failed=os.environ['STABILITY_FIXTURE_FAIL']=='yes' and index==3
 print(f'     Summary [ 0.01s] 1 test run: {0 if failed else 1} passed'+(', 1 failed' if failed else ', 0 skipped'))
 sys.exit(1 if failed else 0)
else:
 print('     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped')
''')
        fake.chmod(0o755)
        for failed in [False, True]:
            with self.subTest(failed=failed):
                state = self.repo / ('failed-runs' if failed else 'passed-runs')
                count = self.repo / 'count'
                result = subprocess.run([
                    'python3', str(SCRIPTS / 'worker-proof-guards.py'), '--base', base,
                    '--verified-script', (SCRIPTS / 'run-verified-tests.sh').read_text(),
                    '--cargo', str(fake), '--count-file', str(count), '--',
                    'nextest', 'run', '-p', 'cas', '--lib', '-E', 'test(touched)',
                ], cwd=self.repo, env={**os.environ, 'STABILITY_FIXTURE_STATE': str(state),
                                     'VERIFIED_TEST_REPO_ROOT': str(self.repo),
                                     'STABILITY_FIXTURE_FAIL': 'yes' if failed else 'no'},
                    capture_output=True, text=True, timeout=25)
                self.assertEqual(state.read_text().splitlines() if state.exists() else [],
                                 ['1', '2', '3', '4', '5'], result.stdout + result.stderr)
                self.assertEqual(result.returncode == 0, not failed, result.stdout + result.stderr)
                receipt = count.with_suffix('.stability.json')
                self.assertEqual(receipt.exists(), not failed)
                if not failed:
                    proof = json.loads(receipt.read_text())
                    self.assertEqual(proof['tests'], ['src/lib.rs::touched'])
                    self.assertEqual(proof['runs'], 5)
                    self.assertEqual(proof['parallelism'], 5)

    def test_selection_covers_every_parameter_case_without_claiming_plain_name_descendants(self):
        (self.repo / 'Cargo.toml').write_text('[package]\nname="cas"\nversion="0.1.0"\n')
        (self.repo / 'src').mkdir()
        source = self.repo / 'src/lib.rs'
        fake = self.repo / 'cargo-fixture'
        cases = [
            ('#[test] fn touched() {}', ['touched', 'touched::unrelated'], 'test(touched)', ['touched']),
            ('#[rstest] fn touched() {}', ['touched::case_1', 'touched::case_2'], 'test(case_1)', ValueError),
            ('#[rstest] fn touched() {}', ['touched::case_1', 'touched::case_2'], 'test(touched)', ['touched::case_1', 'touched::case_2']),
            ('#[test] fn touched() {}', ['touched::unrelated'], 'test(touched)', ValueError),
            ('include!("included.rs");', ['nested::touched'], 'test(touched)', ['nested::touched']),
        ]
        for body, names, expression, expected in cases:
            with self.subTest(body=body, expression=expression, names=names):
                source.write_text(body)
                included = self.repo / 'src/included.rs'
                included.write_text('mod nested { #[test] fn touched() {} }')
                path = 'src/included.rs' if body.startswith('include!') else 'src/lib.rs'
                test = STABILITY.items((self.repo / path).read_text())[0][0]
                touched = {'tests': [{**test, 'file': path, 'id': path+'::'+test['function']}]}
                listing = {'rust-suites': {'fixture': {'testcases': {name: {'ignored': False} for name in names}}}}
                fake.write_text('#!/usr/bin/env python3\nprint('+repr(json.dumps(listing))+')\n')
                fake.chmod(0o755)
                requested = ['nextest', 'run', '-p', 'cas', '--lib', '-E', expression]
                if expected is ValueError:
                    with self.assertRaises(ValueError):
                        STABILITY.selected(self.repo, touched, requested, str(fake))
                else:
                    ids, resolved = STABILITY.selected(self.repo, touched, requested, str(fake))
                    self.assertEqual(ids, [touched['tests'][0]['id']])
                    self.assertEqual(resolved, expected)


if __name__ == '__main__':
    unittest.main()
