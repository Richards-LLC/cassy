#!/usr/bin/env python3
"""Fast target isolation fixtures; --rust-fixture is supervisor-only real Cargo."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('proof_target', REPO / 'scripts/proof_target.py')
targets = importlib.util.module_from_spec(spec)
spec.loader.exec_module(targets)


def git(root, *args):
    return subprocess.check_output(['git', '-C', str(root), *args], text=True, stderr=subprocess.PIPE).strip()


def fixture(root):
    root.mkdir()
    git(root, 'init', '-q', '-b', 'main')
    git(root, 'config', 'user.email', 'fixture@example.test')
    git(root, 'config', 'user.name', 'fixture')
    (root / '.gitignore').write_text('target/\n.cas/\n')
    (root / 'Cargo.toml').write_text('[package]\nname="export-probe"\nversion="0.1.0"\nedition="2024"\n')
    (root / 'src').mkdir()
    (root / 'src/lib.rs').write_text('pub const ONLY_A: u8 = 1;\n')
    (root / 'src/main.rs').write_text('fn main() { let _ = export_probe::ONLY_A; }\n')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'A has the export')
    return git(root, 'rev-parse', 'HEAD')


class TargetTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.a = self.base / 'A'
        self.head = fixture(self.a)

    def b_worktree(self):
        b = self.base / 'B'
        git(self.a, 'worktree', 'add', '-q', '-b', 'B', str(b), self.head)
        (b / 'src/lib.rs').write_text('pub const ONLY_B: u8 = 2;\n')
        git(b, 'add', '.')
        git(b, 'commit', '-qm', 'B lacks the A export')
        os.utime(b / 'src/lib.rs', (1, 1))
        return b

    def run_helper(self, root, command, **env):
        return subprocess.run([sys.executable, str(REPO / 'scripts/proof_target.py'), 'run', str(root), '--', *command],
                              env=dict(os.environ, **env), capture_output=True, text=True)

    def test_two_worktrees_never_share_inherited_freshness(self):
        b = self.b_worktree()
        compiler = self.base / 'fixture-compiler.py'
        compiler.write_text('''import json,os,pathlib,sys
p=pathlib.Path(os.environ['CARGO_TARGET_DIR'])/'debug/.fingerprint/export-probe-fixture/fresh.json'
p.parent.mkdir(parents=True,exist_ok=True)
if p.exists(): value=json.loads(p.read_text())
else:
 value='ONLY_A' in pathlib.Path('src/lib.rs').read_text()
 p.write_text(json.dumps(value))
print('fixture-export-present='+str(value))
sys.exit(0 if value else 1)
''')
        shared = self.base / 'shared'
        for root, expected in [(self.a, 0), (b, 1), (self.a, 0)]:
            result = self.run_helper(root, [sys.executable, str(compiler)], CARGO_TARGET_DIR=str(shared))
            self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
            source = json.loads(result.stdout.splitlines()[0].removeprefix('PROOF_SOURCE: '))
            self.assertEqual(source['worktree'], str(root))
            self.assertEqual(source['head'], git(root, 'rev-parse', 'HEAD'))
            self.assertEqual(source['target'], str(root / 'target'))
        self.assertFalse(shared.exists())

    def test_snapshot_seeds_only_dependencies_and_private_metadata(self):
        cache = self.a / '.cas/build-cache'
        snapshot = cache / 'snapshots/warm'
        for path, text in {'debug/deps/libdependency-ab.rlib': 'artifact',
                           'debug/deps/libexport_probe-ab.rlib': 'wrong workspace artifact',
                           'debug/.fingerprint/dependency-ab/lib-dependency': 'freshness',
                           'debug/.fingerprint/export-probe-ab/lib-export_probe': 'wrong workspace freshness',
                           'debug/.cargo-lock': 'lock',
                           'debug/incremental/cached/mutable': 'incremental'}.items():
            p = snapshot / path;p.parent.mkdir(parents=True, exist_ok=True);p.write_text(text)
        (snapshot / '.cas-build-cache-metadata').write_text('source_commit=' + self.head + '\n')
        (cache / 'current').write_text('warm\n')
        for root in (self.a, self.b_worktree()):
            source = targets.prepare(root)
            self.assertEqual(source['seed_snapshot'], 'warm')
            self.assertEqual((root / 'target/debug/deps/libdependency-ab.rlib').stat().st_ino,
                             (snapshot / 'debug/deps/libdependency-ab.rlib').stat().st_ino)
            fingerprint = root / 'target/debug/.fingerprint/dependency-ab/lib-dependency'
            self.assertEqual(fingerprint.stat().st_nlink, 1)
            fingerprint.write_text('private update')
            self.assertEqual((snapshot / 'debug/.fingerprint/dependency-ab/lib-dependency').read_text(), 'freshness')
            self.assertFalse((root / 'target/debug/.fingerprint/export-probe-ab').exists())
            self.assertFalse((root / 'target/debug/deps/libexport_probe-ab.rlib').exists())
            self.assertFalse((root / 'target/debug/.cargo-lock').exists())
            self.assertFalse((root / 'target/debug/incremental').exists())

    def test_changed_head_invalidates_workspace_and_shared_fingerprints(self):
        source = targets.prepare(self.a)
        target = Path(source['target'])
        own = target / 'custom-fast/.fingerprint/export-probe-ab/lib-export_probe'
        shared = target / 'debug/.fingerprint/old-shared-ab/lib-old_shared'
        private = target / 'debug/.fingerprint/dependency-ab/lib-dependency'
        for p in (own, shared, private):p.parent.mkdir(parents=True,exist_ok=True);p.write_text('fresh')
        os.link(shared, target / 'shared-copy')
        (self.a / 'src/lib.rs').write_text('pub const ONLY_B: u8 = 2;\n')
        git(self.a, 'add', '.');git(self.a, 'commit', '-qm', 'change')
        targets.prepare(self.a)
        self.assertFalse(own.exists());self.assertFalse(shared.exists());self.assertTrue(private.exists())

    def test_foreign_owner_and_symlink_targets_refuse(self):
        b = self.b_worktree()
        targets.prepare(self.a)
        (b / 'target').mkdir()
        shutil.copy2(self.a / 'target' / targets.OWNER, b / 'target' / targets.OWNER)
        with self.assertRaisesRegex(ValueError, 'source root differs'):targets.prepare(b)
        shutil.rmtree(b / 'target');(b / 'target').symlink_to(self.a / 'target', target_is_directory=True)
        with self.assertRaisesRegex(ValueError, 'symlink'):targets.prepare(b)

    def test_symlink_profiles_refuse_and_lib_prefixed_packages_are_workspace(self):
        target = self.a / 'target';target.mkdir()
        outside = self.base / 'outside';outside.mkdir()
        (target / 'debug').symlink_to(outside, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, 'symlink'):targets.prepare(self.a)
        self.assertTrue(targets.workspace_unit('libcustom-123', {'libcustom'}))
        self.assertTrue(targets.workspace_unit('liblibcustom-123.rlib', {'libcustom'}))

    def test_explicit_target_override_refuses_before_command(self):
        r = self.run_helper(self.a, [sys.executable, '-c', 'raise Exception("ran")', '--target-dir=/shared'])
        self.assertNotEqual(r.returncode, 0);self.assertIn('owns --target-dir', r.stderr)
        self.assertNotIn('ran', r.stderr)

    def test_scoped_runner_uses_same_guard_and_records_failed_source(self):
        b = self.b_worktree()
        scripts = b / 'scripts';scripts.mkdir()
        for name in ('run-scoped-tests.sh', 'proof_target.py'):shutil.copy2(REPO / 'scripts' / name, scripts / name)
        compiler = self.base / 'stub-cargo'
        compiler.write_text('#!/bin/sh\n[ "$CARGO_TARGET_DIR" = "$PWD/target" ] || exit 9\nprintf "Summary [ 0.001s] 1 tests run: 0 passed, 1 failed, 0 skipped\\n"\nexit 100\n')
        compiler.chmod(0o755);log = self.base / 'failed.log'
        r = subprocess.run(['bash', str(scripts / 'run-scoped-tests.sh'), '-p', 'export-probe', '--lib'],
                           env=dict(os.environ, CARGO=str(compiler), CARGO_TARGET_DIR=str(self.a / 'target'),
                                    SCOPED_TEST_LOG=str(log)), capture_output=True, text=True)
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertIn('the test run exited 100', r.stderr)
        self.assertIn(str(b), log.read_text());self.assertIn(git(b, 'rev-parse', 'HEAD'), log.read_text())


def rust_fixture():
    """Supervisor runs actual Cargo on A, seeded B, then A; no dependencies."""
    with tempfile.TemporaryDirectory(prefix='cas-5ca6-rust-') as temp:
        base = Path(temp);a = base / 'A';head = fixture(a);b = base / 'B'
        git(a, 'worktree', 'add', '-q', '-b', 'B', str(b), head)
        (b / 'src/lib.rs').write_text('pub const ONLY_B: u8 = 2;\n')
        git(b, 'add', '.');git(b, 'commit', '-qm', 'B lacks A export')
        os.utime(b / 'src/lib.rs', (1, 1))
        for index, (root, expected) in enumerate([(a, 0), (b, 1), (a, 0)]):
            result = subprocess.run([sys.executable, str(REPO / 'scripts/proof_target.py'), 'run', str(root), '--',
                                     'cargo', 'check', '--offline'], env=dict(os.environ, CARGO_TARGET_DIR=str(base / 'shared')),
                                    capture_output=True, text=True)
            print(result.stdout, end='');print(result.stderr, end='')
            if (result.returncode == 0) != (expected == 0):raise AssertionError('unexpected Rust result')
            if expected and 'ONLY_A' not in result.stderr:raise AssertionError('B failed for another reason')
            if index == 0:
                cache = a / '.cas/build-cache';snapshot = cache / 'snapshots/warm'
                snapshot.parent.mkdir(parents=True)
                shutil.copytree(a / 'target', snapshot)
                (snapshot / '.cas-build-cache-metadata').write_text('source_commit=' + head + '\n')
                (cache / 'current').write_text('warm\n')
        print('PASS real Rust two-worktree export fixture: A PASS, seeded B refuses missing ONLY_A, A PASS')


if __name__ == '__main__':
    if sys.argv[1:] == ['--rust-fixture']:
        rust_fixture()
    else:
        unittest.main()
