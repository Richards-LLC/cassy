#!/usr/bin/env python3
"""Behavior fixtures for changed-string warnings; no compiler or systemd."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import threading
import tempfile
import unittest
from unittest.mock import patch

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
        shutil.copy(SCRIPTS / 'worker-js-test-source.mjs', self.repo / 'scripts' / 'worker-js-test-source.mjs')
        hub = self.repo / 'hub-web'
        hub.mkdir()
        (hub / 'package.json').write_text('{}')
        (hub / 'node_modules').symlink_to(Path(__file__).parents[1] / 'hub-web/node_modules')
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
 # nextest pads the counter to the width of the total.
 print(f'        {"FAIL" if failed else "PASS"} [   0.{index}00s] (  1/120) cas touched')
 print(f'     Summary [ 0.01s] 1 test run: {0 if failed else 1} passed'+(', 1 failed' if failed else ', 0 skipped'))
 sys.exit(1 if failed else 0)
elif os.environ['STABILITY_FIXTURE_FAIL']=='red':
 print('        FAIL [   0.010s] (  2/120) cas touched')
 print('     Summary [ 0.01s] 1 test run: 0 passed, 1 failed')
 print('        FAIL [   0.010s] cas touched')
 sys.exit(100)
else:
 print('     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped')
''')
        fake.chmod(0o755)
        for mode in ['no', 'yes', 'red']:
            failed = mode != 'no'
            with self.subTest(mode=mode):
                state = self.repo / (mode+'-runs')
                count = self.repo / 'count'
                result = subprocess.run([
                    'python3', str(SCRIPTS / 'worker-proof-guards.py'), '--base', base,
                    '--verified-script', (SCRIPTS / 'run-verified-tests.sh').read_text(),
                    '--cargo', str(fake), '--count-file', str(count), '--',
                    'nextest', 'run', '-p', 'cas', '--lib', '-E', 'test(touched)',
                ], cwd=self.repo, env={**os.environ, 'STABILITY_FIXTURE_STATE': str(state),
                                     'VERIFIED_TEST_REPO_ROOT': str(self.repo),
                                     'STABILITY_FIXTURE_FAIL': mode},
                    capture_output=True, text=True, timeout=25)
                self.assertEqual(state.read_text().splitlines() if state.exists() else [],
                                 [] if mode == 'red' else ['1', '2', '3', '4', '5'], result.stdout + result.stderr)
                self.assertEqual(result.returncode == 0, not failed, result.stdout + result.stderr)
                receipt = count.with_suffix('.stability.json')
                self.assertEqual(receipt.exists(), not failed)
                if not failed:
                    proof = json.loads(receipt.read_text())
                    self.assertEqual(proof['tests'], ['src/lib.rs::touched'])
                    self.assertEqual(proof['runs'], 5)
                    self.assertEqual(proof['parallelism'], 5)
                    self.assertEqual(proof['cases'], {'src/lib.rs::touched': ['touched']})
                    self.assertEqual(sorted(proof['durations']['touched']), [0.1, 0.2, 0.3, 0.4, 0.5])
                # A failing requested run binds its failed touched tests for park's fail-first check.
                red = count.with_suffix('.red.json')
                self.assertEqual(red.exists(), mode == 'red', result.stdout + result.stderr)
                if mode == 'red':
                    entry, = json.loads(red.read_text())['tests']
                    self.assertEqual((entry['id'], entry['failed']), ('src/lib.rs::touched', ['touched']))
                    self.assertTrue(entry['fingerprint'])
                    red.unlink()

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

    def test_plan_selects_only_changed_vitest_playwright_and_pytest_cases(self):
        web = self.repo / 'hub-web/src/worker.test.ts'
        web.parent.mkdir(parents=True)
        web.write_text("import { test } from 'vitest';\ntest('stable', () => { expect(1).toBe(1); });\ntest('changed', () => { expect('before').toBe('before'); });\n")
        browser = self.repo / 'hub-web/e2e/worker.spec.ts'
        browser.parent.mkdir(parents=True)
        browser.write_text("import { test, expect } from '@playwright/test';\ntest('stable', async ({ page }) => { await expect(page).toHaveTitle('stable'); });\ntest('changed', async ({ page }) => { await expect(page).toHaveTitle('before'); });\n")
        python = self.repo / 'scripts/test_worker.py'
        python.parent.mkdir(parents=True, exist_ok=True)
        python.write_text("def test_stable():\n    assert 1 == 1\n\ndef test_changed():\n    assert 'before' == 'before'\n")
        base = self.commit()
        web.write_text(web.read_text().replace("expect('before').toBe('before')", "expect('after').toBe('after')"))
        browser.write_text(browser.read_text().replace("toHaveTitle('before')", "toHaveTitle('after')"))
        python.write_text(python.read_text().replace("assert 'before' == 'before'", "assert 'after' == 'after'"))
        head = self.commit()

        planned = STABILITY.plan(self.repo, base, head)
        self.assertEqual(
            {(test['runner'], test['id']) for test in planned['tests']},
            {
                ('vitest', 'hub-web/src/worker.test.ts::changed'),
                ('playwright', 'hub-web/e2e/worker.spec.ts::changed'),
                ('pytest', 'scripts/test_worker.py::test_changed'),
            },
        )
        by_runner = {runner: [test for test in planned['tests'] if test['runner'] == runner]
                     for runner in ('vitest', 'playwright', 'pytest')}
        self.assertEqual(STABILITY.adapter_args('vitest', by_runner['vitest'])[:2],
                         ['--testNamePattern', "^(?:changed)$"])
        self.assertEqual(len(by_runner['playwright']), 1)
        with self.assertRaisesRegex(ValueError, 'Playwright.*not supported yet'):
            STABILITY.adapter_args('playwright', by_runner['playwright'])
        self.assertEqual(len(by_runner['pytest']), 1)
        with self.assertRaisesRegex(ValueError, 'pytest.*not supported yet'):
            STABILITY.adapter_args('pytest', by_runner['pytest'])

        with self.assertRaisesRegex(ValueError, 'dynamic test title'):
            STABILITY.source_tests('dynamic.test.ts', "test(`case ${value}`, () => {});", 'vitest')

        escaped = r'''import test from 'node:test';
test('it\'s fine', () => assert.match("say \"hi\"", /["']/));
test("say \"hi\"", () => {});
test.each(['first', 'second'])('%s case', () => {});
'''
        self.assertEqual(STABILITY.ecosystem('checkout-ports.test.mjs', escaped), 'node-test')
        parsed = STABILITY.source_tests('checkout-ports.test.mjs', escaped, 'node-test', self.repo)
        self.assertEqual([case['function'] for case in parsed], ["it's fine", 'say "hi"', 'first case', 'second case'])

    def test_web_stability_receipt_is_bound_and_one_failed_copy_blocks_it(self):
        source = self.repo / 'hub-web/src/worker.test.ts'
        source.parent.mkdir(parents=True)
        source.write_text("test('changed', () => { expect('old').toBe('old'); });\n")
        base = self.commit()
        source.write_text("test('changed', () => { expect('new').toBe('new'); });\n")
        head = self.commit()
        cas_root = self.repo / '.cas'
        with patch.dict(os.environ, {
            'CAS_WORKER_STABILITY_BASE': base,
            'CAS_WORKER_STABILITY_HEAD': head,
            'CAS_ROOT': str(cas_root),
        }, clear=False):
            for fail in (False, True):
                barrier = threading.Barrier(5)
                calls = []
                call_lock = threading.Lock()
                def invoke(command, *, cwd, env, stdout, stderr, close_fds):
                    barrier.wait(timeout=5)
                    with call_lock:
                        calls.append(command)
                        call_number = len(calls)
                    Path(env['VERIFIED_TEST_COUNT_FILE']).write_text('1\n')
                    Path(env['VERIFIED_TEST_REPORT_FILE']).write_text(json.dumps({
                        'testResults': [{'name': 'hub-web/src/worker.test.ts', 'assertionResults': [
                            {'fullName': 'changed', 'duration': 0.025},
                        ]}],
                    }))
                    return subprocess.CompletedProcess(command, 1 if fail and call_number == 3 else 0)
                with self.subTest(fail=fail):
                    if fail:
                        with self.assertRaisesRegex(ValueError, 'no retries; park has no receipt'):
                            STABILITY.run_adapter_stability(self.repo, 'vitest', None, invoke=invoke)
                        self.assertEqual(list((cas_root / 'worker-checks').rglob(f'{head}-test-*.json')), [])
                    else:
                        STABILITY.run_adapter_stability(self.repo, 'vitest', None, invoke=invoke)
                        self.assertEqual(len(calls), 5)
                        receipt_dir = cas_root / 'worker-checks'
                        receipt, = receipt_dir.rglob(f'{head}-test-*.json')
                        proof = json.loads(receipt.read_text())['test']['stability']
                        self.assertEqual((proof['base'], proof['head'], proof['runs'], proof['parallelism'], proof['retries']),
                                         (base, head, 5, 5, 0))

    def test_real_vitest_runs_five_copies_and_writes_bound_receipt(self):
        source = self.repo / 'hub-web/src/receipt.test.ts'
        source.parent.mkdir(parents=True)
        (self.repo / 'hub-web/scripts').mkdir(parents=True)
        shutil.copy(Path(__file__).parents[1] / 'hub-web/scripts/run-verified-tests.mjs',
                    self.repo / 'hub-web/scripts/run-verified-tests.mjs')
        source.write_text("import { test, expect } from 'vitest';\ntest('changed receipt case', () => expect('old').toBe('old'));\n")
        base = self.commit()
        source.write_text("import { test, expect } from 'vitest';\ntest('changed receipt case', () => expect('new').toBe('new'));\n")
        head = self.commit()
        cas_root = self.repo / '.cas'
        with patch.dict(os.environ, {
            'CAS_WORKER_STABILITY_BASE': base,
            'CAS_WORKER_STABILITY_HEAD': head,
            'CAS_ROOT': str(cas_root),
        }, clear=False):
            STABILITY.run_adapter_stability(self.repo, 'vitest', None)
        receipts = list((cas_root / 'worker-checks').rglob(f'{head}-test-*.json'))
        receipt, = receipts
        data = json.loads(receipt.read_text())
        self.assertEqual((data['head'], data['test']['harness'], data['test']['stability']['runs']),
                         (head, 'vitest', 5))

    def test_node_test_runner_selects_changed_case_and_writes_bound_receipt(self):
        source = self.repo / 'hub-web/e2e/receipt.test.mjs'
        source.parent.mkdir(parents=True)
        (self.repo / 'hub-web/scripts').mkdir(parents=True, exist_ok=True)
        shutil.copy(Path(__file__).parents[1] / 'hub-web/scripts/run-verified-node-tests.mjs',
                    self.repo / 'hub-web/scripts/run-verified-node-tests.mjs')
        source.write_text("import test from 'node:test';\nimport assert from 'node:assert/strict';\ntest('changed receipt case', () => assert.equal('old', 'old'));\n")
        base = self.commit()
        source.write_text("import test from 'node:test';\nimport assert from 'node:assert/strict';\ntest('changed receipt case', () => assert.equal('new', 'new'));\n")
        head = self.commit()
        cas_root = self.repo / '.cas'
        with patch.dict(os.environ, {
            'CAS_WORKER_STABILITY_BASE': base,
            'CAS_WORKER_STABILITY_HEAD': head,
            'CAS_ROOT': str(cas_root),
        }, clear=False):
            STABILITY.run_adapter_stability(self.repo, 'node-test', None)
        receipt, = (cas_root / 'worker-checks').rglob(f'{head}-test-*.json')
        data = json.loads(receipt.read_text())
        self.assertEqual((data['head'], data['test']['harness'], data['test']['stability']['runs']),
                         (head, 'node-test', 5))

    def test_native_reports_resolve_exact_touched_case_durations(self):
        with tempfile.TemporaryDirectory() as scratch:
            report = Path(scratch) / 'report.json'
            vitest = [{'id': 'src/worker.test.ts::suite::changed', 'file': 'src/worker.test.ts',
                       'function': 'suite::changed'}]
            report.write_text(json.dumps({'testResults': [{'name': '/repo/src/worker.test.ts',
                'assertionResults': [{'ancestorTitles': ['suite'], 'title': 'changed', 'duration': 24}]}]}))
            self.assertEqual(STABILITY.report_durations('vitest', report, vitest),
                             {'src/worker.test.ts::suite::changed': [0.024]})
            node = [{'id': 'e2e/worker.test.mjs::suite::changed', 'file': 'e2e/worker.test.mjs',
                     'function': 'suite::changed'}]
            report.write_text('<testsuites><testcase name="suite changed" time="0.045"/></testsuites>')
            self.assertEqual(STABILITY.report_durations('node-test', report, node),
                             {'e2e/worker.test.mjs::suite::changed': [0.045]})

    def test_pytest_wrapper_fails_closed_as_unsupported(self):
        result = subprocess.run([sys.executable, str(SCRIPTS / 'run-verified-pytest.py')],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn('pytest', result.stderr)
        self.assertIn('not supported yet', result.stderr)


if __name__ == '__main__':
    unittest.main()
