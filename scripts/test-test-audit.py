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
        clean = {
            'case.rs': '''fn production_delay() { thread::sleep(Duration::from_secs(1)); }
#[test] fn waits_for_event() { let tmp = tempfile::tempdir().unwrap(); listener.bind(("127.0.0.1",0)); child.kill().unwrap(); child.wait().unwrap(); assert!(!path.exists()); assert_eq!(error.code(), "scope_denied"); }
#[test] fn bounded_poll() { while !ready() && Instant::now() < deadline { thread::sleep(Duration::from_millis(10)); } assert!(ready()); }''',
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

    def test_deleted_serve_teardown_incident_from_dc43f83d(self):
        source = subprocess.check_output(['git', 'show', 'dc43f83d:cas-cli/tests/hub_clean_home_test.rs'], cwd=REPO, text=True)
        report = self.scan({'tests/hub_clean_home_test.rs': source})
        incident = [row for row in report['findings'] if row['test'] == 'clean_home_tailscale_stop_reports_removal_after_serve_exit_teardown' and row['class'] == 'flake-prone']
        self.assertTrue(incident, report['counts'])
        self.assertTrue(any(row['pattern'] in {'instant-port', 'instant-teardown', 'fixed-sleep'} for row in incident))


if __name__ == '__main__':
    unittest.main()
