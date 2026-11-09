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
    def test_human_message_assertions_use_behavior_or_machine_ids(self):
        cases = [
            ('test_example.py', 'with self.assertRaisesRegex(ValueError, "please try later"): run()', True),
            ('test_example.py', r'with self.assertRaisesRegex(ValueError, r"^\S+$"): run()', False),
            ('test_example.py', 'self.assertIn("permission was denied", result.stderr)', True),
            ('test_example.py', 'self.assertIn("titled category and area format", result.stdout)', True),
            ('src/example.rs', '#[test] fn check() { assert!(result.stdout.contains("please try later")); }', True),
            ('test_example.py', 'self.assertEqual("permission was denied", error.message)', True),
            ('test_example.py', 'assert "permission was denied" in str(exc)', True),
            ('test_example.py', 'assert str(error) == "permission was denied"', True),
            ('test_example.py', 'assert f"please retry {name} later" in str(exc)', True),
            ('test_example.py', 'self.assertIn(f"please retry {name} later", result.stderr)', True),
            ('test_example.py', 'assert error.code == "scope_denied"', False),
            ('test_example.py', 'assert error.code == "forbidden"', False),
            ('test_example.py', 'self.assertEqual(error.code, "forbidden")', False),
            ('src/example.rs', '#[test] fn check() { assert_eq!(error.code(), "forbidden"); }', False),
            ('src/example.rs', '#[test] fn check() { assert!(reason.contains("2026-05-01T00:00:00Z")); }', False),
            ('src/example.rs', '#[test] fn check() { assert!(reason.contains("entry:e1")); }', False),
            ('src/example.rs', '#[test] fn check() { assert!(reason.contains("database.md:42")); }', False),
            ('src/example.rs', '#[test] fn check() { assert!(reason.contains("protected=4")); }', False),
            ('src/example.rs', '#[test] fn check() { assert!(reason.contains("project=acme")); }', False),
            ('src/example.rs', '#[test] fn check() { assert!(reason.contains("HTTP 503")); }', False),
            ('src/example.rs', '#[test] fn check() { assert!(reason.contains(".claude-alt")); }', False),
            ('src/example.rs', '#[test] fn check() { assert!(error.message.contains("SKILL.md")); }', False),
            ('src/example.rs', '#[test] fn check() { assert!(reason.contains("$UNKNOWN_ROOT")); }', False),
            ('src/example.rs', '#[test] fn check() { assert!(error.message.contains("4dd898a71")); }', False),
            ('example.test.ts', 'expect(error.code).toBe("forbidden");', False),
            ('test_example.py', 'self.assertIn("[rule: scope_denied]", result.stderr)', False),
            ('test_example.py', 'self.assertIn("FAIL test-shape", result.stdout)', False),
            ('test_example.py', 'self.assertEqual(saved_payload, "hello from the operator")', False),
            ('src/example.rs', '#[test] fn check() { assert!(error.contains("please try later")); }', True),
            ('src/example.rs', 'fn validate(error: &str) { assert!(error.contains("required input phrase")); }', False),
            ('src/example.rs', '#[cfg(test)] mod tests { fn helper(error: &str) { assert!(error.contains("please try later")); } }', True),
            ('src/example.rs', '#[test] fn check() {\n// pin: an old copy preference\nassert!(error.contains("please try later")); }', True),
            ('src/example.rs', '#[test] fn check() { assert_eq!(error.code(), "scope_denied"); }', False),
            ('src/example.rs', r"#[test] fn check() { assert!(!error.contains('\x1b')); }", False),
            ('src/example.rs', r"#[test] fn check() { assert!(!error.contains(b'\n')); }", False),
            ('src/example.rs', '#[test] fn check() { assert_eq!(output.stdout, b"caller payload bytes"); }', False),
            ('src/example.rs', '#[test] fn check() { assert_eq!(receipt, Receipt { warning: Some("caller-provided diagnostic".into()) }); }', False),
            ('src/example.rs', '#[test] fn check() { let fixture = "caller diagnostic"; assert_eq!(error.message, fixture); }', False),
            ('src/example.rs', r'#[test] fn check() { assert!(stderr.contains(&format!("{FIXTURE}\n{SECOND}\n"))); }', False),
            ('src/example.rs', '#[test] fn check() { assert_eq!("please try later", error.message); }', True),
            ('src/example.rs', '#[test] fn check() { assert!(result.unwrap_err().to_string().contains("please try later")); }', True),
            ('src/example.rs', '#[test] fn check() { assert_eq!(get_error().message, "please try later"); }', True),
            ('src/example.rs', '#[test] fn check() { assert!(result.is_err(), "expected an authorization refusal"); }', False),
            ('example.test.ts', 'expect(run()).toThrow("please try later");', True),
            ('example.test.ts', 'expect(run()).toThrow(new Error("please try later"));', True),
            ('example.test.ts', 'expect(error).toMatchObject({\nmessage: "please try later"\n});', True),
            ('example.test.ts', 'expect(error.message).toBe("forbidden");', True),
            ('example.journey.ts', 'expect(error.message).toBe("forbidden");', True),
            ('src/example_tests.rs', 'fn helper(error: &str) { assert!(error.contains("please try later")); }', True),
            ('example.test.ts', 'expect(error.message).toMatch(/permission was denied/);', True),
            ('example.test.ts', 'expect(error.message).toMatch(/\\S/);', False),
            ('example.test.mjs', 'assert.equal(error.message, "please try later");', True),
            ('example.test.mjs', 'assert.throws(run, /please try later/);', True),
            ('example.test.ts', 'expect(error.code).toBe("scope_denied");', False),
            ('src/example.rs', '#[test] fn check() { for phrase in ["please try later", "run it again"] { assert!(error.contains(phrase)); } }', True),
            ('test_example.py', 'def test_x():\n    for phrase in ("please try later",):\n        self.assertIn(phrase, result.stderr)', True),
            ('example.test.ts', 'for (const phrase of ["please try later"]) { expect(error.message).toContain(phrase); }', True),
        ]
        for path, source, rejected in cases:
            with self.subTest(path=path, source=source):
                self.assertEqual(bool(shape.message_assertions(source, path)), rejected)

    def test_cli_parser_payload_round_trip_cas_5442(self):
        source = '''#[test]
fn cli_round_trip() {
    let parsed = crate::cli::Cli::try_parse_from([
        "cas", "artifact", "revoke", "art-fixture", "--reason", "Harmless fixture cleanup",
    ]).unwrap();
    let Some(crate::cli::Commands::Artifact(ArtifactCommands::Revoke(args))) = parsed.command else {
        panic!("unrouted command");
    };
    ASSERTION
}
'''
        for assertion, rejected in [
            ('assert_eq!(args.reason, "Harmless fixture cleanup");', False),
            ('assert_eq!("Harmless fixture cleanup", args.reason);', False),
            ('assert_eq!(args.reason, "Generated refusal wording");', True),
            ('assert_eq!(generated.reason, "Harmless fixture cleanup");', True),
            ('assert_eq!(args.warning, "Harmless fixture cleanup");', True),
            ('assert_ne!(args.reason, "Harmless fixture cleanup");', True),
            ('assert!(args.reason.contains("Harmless fixture cleanup"));', True),
        ]:
            code = source.replace('ASSERTION', assertion)
            line = next(i for i, text in enumerate(code.splitlines(), 1) if text.strip() == assertion)
            for changed in [None, {line}]:
                with self.subTest(assertion=assertion, changed=changed):
                    self.assertEqual(bool(shape.message_assertions(code, 'src/cli.rs', changed)), rejected)

        # Calling generated code or a same-named binding in another block
        # must not make generated prose look like the parser's returned field.
        assertion = 'assert_eq!(args.reason, "Harmless fixture cleanup");'
        parser_bindings = source[source.index('    let parsed'):source.index('    ASSERTION')]
        for extra in ['let args = parsed.warning;', 'let args = render(parsed.command);',
                      'let args = render(parsed.command); {' + parser_bindings + '}']:
            code = source.replace('ASSERTION', extra + '\n    ' + assertion)
            self.assertTrue(shape.message_assertions(code, 'src/cli.rs'))

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
        self.assertFalse(shape.lint_text_pins('expect(skill).toContain("references/attachments.md");', 'example.test.ts'))
        self.assertFalse(shape.lint_text_pins('self.assertIn("WITHIN_BUDGET=true", draft.read_text())', 'test_timing.py'))
        self.assertFalse(shape.lint_text_pins('expect(heard.filter(entry => entry.source === "banner"), "the outage is announced once").toHaveLength(1);', 'reconnect.journey.ts'))
        self.assertFalse(shape.lint_text_pins('expect(heard.filter(entry => entry.source === "banner")).toEqual([expect.stringContaining("Atlas · Linux")]);', 'reconnect.journey.ts'))
        self.assertFalse(shape.lint_text_pins('expect.soft(source, "The original source must preserve every caller value and its exact encoded boundaries.").toEqual(input);', 'payload.test.ts'))
        self.assertFalse(self.hits('#[test] fn t() { let s=include_str!("contract.md"); assert!(s.contains("word")); }'))

    def test_exact_prose_pin_cannot_be_waived_by_comments(self):
        # These fixtures exercise rejection of exact prose, including pin comments.
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
        self.assertEqual(shape.lint_text_pins(pinned, 'tests/test_prompt.py'), [(3, 'exact-text pin')])

    def test_rust_assert_diagnostics_are_not_text_pins_cas_cae5(self):
        message = '"preserve original content and routing instructions"'
        cases = [
            (f'assert_eq!(&row.prompt, body, {message});', False),
            (f'assert_ne!(prompt, render((left, right), [a, b]), {message}, prompt);', False),
            (f'debug_assert!(\n source.contains("marker"),\n {message}\n);', False),
            ('assert!(valid, r#"The prompt must preserve the original content, routing, and useful context."#);', False),
            (f'assert_eq!(prompt, {message}, "unexpected value");', True),
            (f'assert_eq!({message}, prompt, "unexpected value");', True),
            (f'assert_eq!(prompt::<A, B>(), {message}, "unexpected value");', True),
            (f'assert_eq!(prompt::<A, B>(), body, {message});', False),
        ]
        for assertion, expected_pin in cases:
            with self.subTest(assertion=assertion):
                hits = self.hits('#[test] fn preserve_payload() {\n' + assertion + '\n}')
                self.assertEqual(bool(hits), expected_pin, hits)

    def test_git_inputs_and_expect_diagnostics_cas_26f3(self):
        for rejected, code in [
            (False, '#[test] fn t() { assert_eq!(git(&["rev-parse", &format!("refs/heads/{source}")]), source_head); }'),
            (False, '#[test] fn t() { let completed = rows.iter().find(|row| row.prompt.contains(&dependency.id) && row.prompt.contains(&terminal.to_string())).expect("completion names the dependency and its terminal state"); }'),
            (True, '#[test] fn t() { assert_eq!(git(&["show", &format!("refs/heads/{source}")]), "When a test fails, name the responsible boundary and preserve its first result."); }'),
            (True, '#[test] fn t() { assert!(source.contains("uses: actions/checkout@v4")); }'),
        ]:
            with self.subTest(code=code):
                self.assertEqual(bool(self.hits(code, 'tests/probe.rs')), rejected)

    def test_python_file_read_inputs_leave_output_pins_in_scope_cas_c49b(self):
        cases = [
            ("self.assertEqual(json.loads((self.repo/'receipts/receipt.json').read_text())['outcome'], 'FLAKY')", 0),
            ("""label = 'é'; self.assertEqual(json.loads((self.repo / 'receipts/receipt.json').read_text())['outcome'],
    'FLAKY')""", 0),
            ("self.assertEqual((self.repo/'receipts/receipt.json').read_text(), 'When a test fails, name the responsible boundary and preserve its first result.')", 1),
            ("self.assertIn('uses: actions/checkout@v4', (self.repo/'.github/workflows/ci.yml').read_text())", 1),
        ]
        for code, expected in cases:
            with self.subTest(code=code):
                self.assertEqual(len(self.hits(code, 'tests/test_receipt.py')), expected)

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
                self.assertIn('cas-cli/tests/nested/probe.rs:1:', result.stdout)
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
            self.assertIn('tests/test_prompt.py:2:', result.stdout)
            path.write_text('def test_prompt():\n    # // pin: this phrase is an external compatibility contract.\n    assert prompt == "The release gate should identify which component caused this failure."\n')
            clean = subprocess.run(command, cwd=root, capture_output=True, text=True)
            self.assertEqual(clean.returncode, 1, clean.stdout)

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

            # Only the added prose pin fails; a pin comment cannot waive it.
            reasoned_pin = '''
#[test]
fn added_reasoned_contract() {
    // pin: this wording is part of the source compatibility contract.
    assert_eq!(prompt, "When a new test is added, name the behavior it verifies with useful context.");
}
'''
            path.write_text(old_pin + reasoned_pin)
            clean = subprocess.run(command, cwd=root, capture_output=True, text=True)
            self.assertEqual(clean.returncode, 1, clean.stdout)
            self.assertNotIn('tests/test_prompt.rs:3:', clean.stdout)
            self.assertIn('tests/test_prompt.rs:9:', clean.stdout)

            # A changed legacy line is in scope, including when only one part
            # of the string literal's text changes.
            modified = old_pin.replace('first result', 'original result') + reasoned_pin
            path.write_text(modified)
            changed = subprocess.run(command, cwd=root, capture_output=True, text=True)
            self.assertEqual(changed.returncode, 1, changed.stdout)
            self.assertIn('tests/test_prompt.rs:3:', changed.stdout)

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
            self.assertIn('tests/test_prompt.rs:14:', added.stdout)

            # Without --changed-since, changed test files retain full-file
            # scanning, so the unchanged legacy pin remains visible.
            full = subprocess.run(['python3', str(Path(shape.__file__).resolve())],
                                 cwd=root, capture_output=True, text=True)
            self.assertEqual(full.returncode, 1, full.stdout)
            self.assertIn('tests/test_prompt.rs:3:', full.stdout)


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
