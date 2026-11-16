#!/usr/bin/env python3
"""Exercise the shipped audit through its CLI against executable source fixtures."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[1]
SCRIPT = REPO / 'cas-cli/src/builtins/skills/cas-test-audit/scripts/audit.py'


class Audit(unittest.TestCase):
    def scan(self, files):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, source in files.items():
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(source)
            result = subprocess.run(['python3', str(SCRIPT), '--root', str(root)], text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(result.stdout)

    def test_rust_js_flake_patterns_and_clean_controls(self):
        cases = {
            'fixed-sleep': ('thread::sleep(Duration::from_secs(1));', 'await page.waitForTimeout(200);'),
            'instant-port': ('assert!(TcpStream::connect(addr).is_err());', 'await expect(connect(port)).rejects.toThrow();'),
            'instant-teardown': ('child.kill().unwrap(); assert!(!path.exists());', 'process.kill(pid); expect(fs.existsSync(path)).toBe(false);'),
            'tight-deadline': ('assert!(start.elapsed() < Duration::from_millis(200));', 'await expect(ready).toPass({timeout: 200});'),
            'shared-port': ('TcpListener::bind(("127.0.0.1", 4173)).unwrap();', 'server.listen(4173);'),
            'shared-path': ('fs::write("/tmp/shared-test", b"x").unwrap();', 'fs.writeFileSync("/tmp/shared-test", "x");'),
            'shared-state': ('std::env::set_var("GLOBAL", "x");', 'globalThis.sharedCounter++;'),
            'unseeded-random': ('let n = rand::random::<u32>(); assert!(n > 0);', 'expect(Math.random()).toBeGreaterThan(0);'),
        }
        for pattern, (rust, js) in cases.items():
            for path, source in [('case.rs', '#[test] fn incident() { '+rust+' }'), ('case.test.ts', 'test("incident", async () => { '+js+' });')]:
                with self.subTest(pattern=pattern, path=path):
                    findings = self.scan({path: source})['findings']
                    matches = [row for row in findings if row['pattern'] == pattern]
                    self.assertTrue(matches, findings)
                    self.assertTrue(all(row['line'] >= 1 and row['fix'] for row in matches))
        for path, source in {
            'tests/order.rs': 'static mut ORDER: u32 = 0; #[test] fn one() { unsafe { ORDER += 1; } assert!(true); }',
            'order.test.ts': 'let order = 0; test("one", () => { order++; expect(run()).toBe(1); }); test("two", () => { order++; expect(run()).toBe(2); });',
        }.items():
            with self.subTest(pattern='order-dependence', path=path):
                self.assertTrue(any(row['pattern'] == 'order-dependence' for row in self.scan({path: source})['findings']))
        clean = {
            'case.rs': '''fn production_delay() { thread::sleep(Duration::from_secs(1)); }
#[test] fn waits_for_event() { let tmp = tempfile::tempdir().unwrap(); listener.bind(("127.0.0.1",0)); child.kill().unwrap(); child.wait().unwrap(); assert!(!path.exists()); assert_eq!(error.code(), "scope_denied"); }
#[test] fn diagnostics() { assert!(reason.is_empty(), "This explains a failure"); assert_eq!(stdout, b"caller payload bytes"); }
#[test] fn bounded_poll() { while !ready() && Instant::now() < deadline { thread::sleep(Duration::from_millis(10)); } assert!(ready()); }''',
            'production.ts': 'export function production(value) { return /ready/.test(value); }',
            'destructured.test.ts': 'test("one", async ({page}) => { await page.click("#one"); expect(run()).toBe(1); }); test("two", async ({page}) => { await page.click("#two"); expect(run()).toBe(2); });',
            'case.test.ts': '''test("virtual time", async () => { vi.useFakeTimers(); const note = "await page.waitForTimeout(100)"; setTimeout(done, 200); vi.advanceTimersByTime(200); expect(error.code).toBe("scope_denied"); });
// page.waitForTimeout(99)
''',
        }
        self.assertEqual(self.scan(clean)['findings'], [])

    def test_other_classes_share_the_scan(self):
        report = self.scan({'case.test.ts': '''import {run} from "./app";
test.skip("retired", () => { run(); });
test("first", () => { expect(run()).toBe(3); });
test("second", () => { expect(run()).toBe(3); });
test("prose", () => { expect(error.message).toBe("Permission was denied"); });
test("style", () => { expect(css.includes("--brand-ink")).toBe(true); });
test("slow", async () => { await page.waitForTimeout(2000); });''',
            'no_assertions.rs': '#[test] fn nothing() { let x = 3; }'})
        classes = {row['class'] for row in report['findings']}
        self.assertTrue({'dead', 'duplicate', 'wording-pinned', 'style-pinned', 'slow', 'restates-implementation'} <= classes)
        self.assertEqual(sum(report['counts'].values()), len(report['findings']))

    def test_history_distinguishes_same_source_from_other_shas_and_missing_proof(self):
        import importlib.util
        spec = importlib.util.spec_from_file_location('audit_history', SCRIPT.with_name('history.py'))
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        runs = [
            {'head_sha': 'a'*40, 'workflow_id': 1, 'event': 'merge_group', 'conclusion': 'failure', 'run_attempt': 1, 'html_url': 'run/1'},
            {'head_sha': 'a'*40, 'workflow_id': 1, 'event': 'merge_group', 'conclusion': 'success', 'run_attempt': 2, 'html_url': 'run/2'},
            {'head_sha': 'b'*40, 'workflow_id': 1, 'event': 'push', 'conclusion': 'failure', 'html_url': 'run/3'},
            {'head_sha': 'c'*40, 'workflow_id': 1, 'event': 'push', 'conclusion': 'success', 'html_url': 'run/4'},
        ]
        issue = {'number': 1, 'title': 'flaky native', 'url': 'issue/1', 'state': 'OPEN', 'body': '<!-- cas-queue-flake: {"test":{"name":"native"}} -->'}
        rows = module.analyze(runs, [issue, issue])
        self.assertEqual(sum(row['pattern']=='same-source-workflow-flip' for row in rows), 1)
        self.assertEqual(sum(row['pattern']=='historical-issue' for row in rows), 1)
        self.assertEqual(sum(row['pattern']=='historical-rerun' for row in rows), 1)
        self.assertTrue(all(row['confidence'] != 'high' for row in rows))
        same_tree = module.analyze(runs, [], lambda sha: 'same-tree' if sha in {'b'*40,'c'*40} else sha)
        self.assertEqual(sum(row['pattern']=='same-source-workflow-flip' for row in same_tree), 2)

    def test_deleted_serve_teardown_incident_from_dc43f83d(self):
        source = (REPO / 'scripts/fixtures/test-audit/serve-teardown.rs.txt').read_text()
        report = self.scan({'tests/hub_clean_home_test.rs': source})
        incident = [row for row in report['findings'] if row['test'] == 'clean_home_tailscale_stop_reports_removal_after_serve_exit_teardown' and row['class'] == 'flake-prone']
        self.assertTrue(incident, report['counts'])
        self.assertTrue(any(row['pattern'] in {'instant-port', 'instant-teardown', 'fixed-sleep'} for row in incident))


if __name__ == '__main__':
    unittest.main()
