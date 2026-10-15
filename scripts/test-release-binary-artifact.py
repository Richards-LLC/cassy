#!/usr/bin/env python3
"""One local release build per release tree (cas-c096).

The gate records its audited release archive; the helper verifies a reuse
only when the tree, version, target, build identity and digests all match.
The local publisher that reused it is gone (cas-f95b): release.yml publishes
the merge-queue build. Cargo, rustc, zig and the ISA auditor are fakes:
the contract under test is the reuse decision, not a 10 minute build.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent

# A git hook (the pre-push fast rows) exports GIT_DIR and friends; a fixture
# `git init` would then reinitialise the real repository (cas-f95b).
for _name in subprocess.run(['git', 'rev-parse', '--local-env-vars'], capture_output=True,
                            text=True, check=False).stdout.split():
    os.environ.pop(_name, None)
HELPER = ROOT / 'scripts/release-binary-artifact.py'
TARGET = 'x86_64-unknown-linux-gnu'


class Fixture(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.base = Path(self.tmp.name)
        self.repo = self.base / 'repo'
        self.bin = self.base / 'bin'
        for directory in (self.repo / 'scripts', self.repo / 'cas-cli', self.repo / '.context/zig', self.bin):
            directory.mkdir(parents=True)
        self.store = self.base / 'store'
        self.cargo_log = self.base / 'cargo.log'
        self.env = dict(os.environ, PATH=f'{self.bin}:{os.environ["PATH"]}',
                        CAS_RELEASE_BINARY_STORE=str(self.store), CAS_POSTHOG_API_KEY='fixture',
                        CAS_SENTRY_DSN='', FIXTURE_CARGO_LOG=str(self.cargo_log),
                        ZIG=str(self.repo / '.context/zig/zig'))
        for name in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_RUSTFLAGS', 'RUSTC'):
            self.env.pop(name, None)
        self.executable(self.bin / 'rustc', '#!/bin/sh\necho "rustc 1.99.0-fixture"\n')
        self.executable(self.bin / 'cargo-zigbuild', '#!/bin/sh\necho "cargo-zigbuild 0.99.0"\n')
        self.executable(self.repo / '.context/zig/zig', '#!/bin/sh\necho "${FIXTURE_ZIG_VERSION:-0.15.2}"\n')
        self.git('init', '-q', '-b', 'main')
        self.git('config', 'user.name', 'Fixture')
        self.git('config', 'user.email', 'fixture@example.test')
        self.git('init', '-q', '--bare', str(self.base / 'origin'))
        self.git('remote', 'add', 'origin', str(self.base / 'origin'))
        (self.repo / 'cas-cli/Cargo.toml').write_text('[package]\nname = "cas"\nversion = "9.99.9"\n')
        (self.repo / 'LICENSE').write_text('fixture license\n')
        (self.repo / '.gitignore').write_text('target/\ndist/\n.cas/\n')
        shutil.copy(HELPER, self.repo / 'scripts/release-binary-artifact.py')
        self.binary = self.base / 'gate-cas'
        self.binary.write_bytes(b'\x7fELF gate-built fixture executable\n')
        self.commit('fixture')

    def executable(self, path, body):
        path.write_text(body)
        path.chmod(0o755)

    def git(self, *args):
        return subprocess.run(['git', *args], cwd=self.repo, env=self.env, text=True,
                              capture_output=True, check=True).stdout.strip()

    def commit(self, message):
        self.git('add', '.')
        self.git('commit', '-qm', message)

    def helper(self, *args, env=None):
        return subprocess.run(['python3', str(HELPER), *args], cwd=self.repo, env=env or self.env,
                              text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)

    def record(self, env=None):
        result = self.helper('record', str(self.repo), '--target', TARGET, '--binary', str(self.binary),
                             '--license', str(self.repo / 'LICENSE'), '--audit', 'packaged-binary-isa',
                             env=env)
        self.assertEqual(result.returncode, 0, result.stdout)
        return result

    def verify(self, *extra, env=None):
        return self.helper('verify', str(self.repo), '--target', TARGET, *extra, env=env)

    def manifest_path(self):
        manifests = list((self.store / TARGET).glob('*/manifest.json'))
        self.assertEqual(len(manifests), 1, manifests)
        return manifests[0]


class RecordAndVerify(Fixture):
    def test_reuse_binds_tree_version_target_and_digests(self):
        self.record()
        manifest = json.loads(self.manifest_path().read_text())
        self.assertEqual(manifest['tree'], self.git('rev-parse', 'HEAD^{tree}'))
        self.assertEqual(manifest['version'], '9.99.9')
        self.assertEqual(manifest['target'], TARGET)
        result = self.verify('--version', '9.99.9')
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn('REUSE release binary', result.stdout)
        archive = Path(next(line for line in result.stdout.splitlines()
                            if line.startswith('archive=')).removeprefix('archive='))
        with tarfile.open(archive) as bundle:
            self.assertEqual(sorted(bundle.getnames()), ['LICENSE', 'cas'])
            self.assertEqual(bundle.extractfile('cas').read(), self.binary.read_bytes())

    def test_new_tree_rebuilds(self):
        self.record()
        (self.repo / 'cas-cli/lib.rs').write_text('// changed source\n')
        self.commit('source change')
        result = self.verify('--version', '9.99.9')
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn('MISS key=tree reason=no_artifact_for_tree', result.stdout)

    def test_toolchain_change_rebuilds_and_round_trip_reuses(self):
        self.record()
        upgraded = dict(self.env, FIXTURE_ZIG_VERSION='0.16.0')
        result = self.verify(env=upgraded)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn('MISS key=build_identity reason=zig_differs', result.stdout)
        rustflags = dict(self.env, RUSTFLAGS='-C target-cpu=native')
        self.assertIn('env:RUSTFLAGS_differs', self.verify(env=rustflags).stdout)
        self.record(env=upgraded)
        self.assertEqual(self.verify(env=upgraded).returncode, 0)
        self.assertEqual(self.verify().returncode, 0, 'reverted toolchain lost its archive')

    def test_modified_checkout_neither_records_nor_reuses(self):
        self.record()
        (self.repo / 'LICENSE').write_text('edited\n')
        self.assertIn('tracked_files_modified', self.verify().stdout)
        result = self.helper('record', str(self.repo), '--target', TARGET, '--binary', str(self.binary),
                             '--license', str(self.repo / 'LICENSE'))
        self.assertEqual(result.returncode, 1, result.stdout)

    def test_tag_worktree_reuses_the_gate_checkouts_archive(self):
        """The gate records from the epic checkout; release.sh verifies from a
        separate tag worktree of the same repository and must find it."""
        env = dict(self.env)
        env.pop('CAS_RELEASE_BINARY_STORE')
        self.record(env=env)
        worktree = self.base / 'tag-worktree'
        self.git('worktree', 'add', '-q', '--detach', str(worktree), 'HEAD')
        result = self.helper('verify', str(worktree), '--target', TARGET, '--version', '9.99.9', env=env)
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn(str(self.repo.resolve() / '.cas/release-binary/artifacts'), result.stdout)
    def test_prune_keeps_the_newest_entries(self):
        for index in range(8):
            (self.repo / 'cas-cli/lib.rs').write_text(f'// change {index}\n')
            self.commit(f'change {index}')
            self.record()
        entries = [path for path in (self.store / TARGET).iterdir() if not path.name.startswith('.')]
        self.assertEqual(len(entries), 6)
        self.assertEqual(self.verify().returncode, 0)


if __name__ == '__main__':
    unittest.main()
