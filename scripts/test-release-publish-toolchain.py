#!/usr/bin/env python3
"""Exercise the train preflight's real zigbuild call seam (no build)."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent

# A git hook (the pre-push fast rows) exports GIT_DIR and friends; a fixture
# `git init` would then reinitialise the real repository (cas-f95b).
for _name in subprocess.run(['git', 'rev-parse', '--local-env-vars'], capture_output=True,
                            text=True, check=False).stdout.split():
    os.environ.pop(_name, None)


class PublishToolchain(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.base = Path(self.tmp.name)
        self.repo = self.base / 'repo'
        self.repo.mkdir()
        self.bin = self.base / 'bin'
        self.bin.mkdir()
        self.env = dict(os.environ, PATH=f'{self.bin}:{os.environ["PATH"]}',
                        CAS_POSTHOG_API_KEY='fixture', CAS_SENTRY_DSN='')
        self.command('git', 'init', '-q', '-b', 'main')
        self.command('git', 'config', 'user.name', 'Fixture')
        self.command('git', 'config', 'user.email', 'fixture@example.test')
        self.command('git', 'init', '-q', '--bare', str(self.base / 'origin'))
        self.command('git', 'remote', 'add', 'origin', str(self.base / 'origin'))
        for directory in ['scripts', 'cas-cli', '.cargo', '.context/zig']:
            (self.repo / directory).mkdir(parents=True)
        (self.repo / 'cas-cli/Cargo.toml').write_text('[package]\nversion = "9.99.9"\n')
        (self.repo / 'LICENSE').write_text('fixture')
        shutil.copy(ROOT / 'scripts/release-zig.sh', self.repo / 'scripts/release-zig.sh')
        for name in ['check-release-host.sh', 'check-release-migration-snapshots.sh',
                     'check-release-preflight.sh', 'check-portable-x86_64-isa.sh',
                     'check-blake3-no-avx512-build.sh', 'test-check-portable-x86_64-isa.sh',
                     'find-release-prebuild.sh']:
            self.executable(self.repo / 'scripts' / name, '#!/bin/sh\nexit 0\n')
        self.executable(self.repo / '.context/zig/zig', '#!/bin/sh\necho 0.15.1\n')
        self.executable(self.bin / 'rustup', '#!/bin/sh\necho x86_64-unknown-linux-gnu\n')
        self.executable(self.bin / 'cargo-zigbuild', '''#!/usr/bin/env python3
import os, shutil, subprocess, sys, tomllib
from pathlib import Path
config = tomllib.loads(Path('.cargo/config.toml').read_text())
if config['build']['jobs'] == 'default':
    print('cargo-zigbuild: invalid build.jobs type', file=sys.stderr)
    sys.exit(1)
if os.environ.get('FAKE_ZIG_SKIP_DELEGATE'):
    sys.exit(0)
if not shutil.which('zig'):
    print('zigbuild: zig missing from PATH', file=sys.stderr)
    sys.exit(1)
args = ['build', '--target', 'x86_64-unknown-linux-gnu', '--locked', '--release', '--package', 'cas']
if os.environ.get('FAKE_ZIG_UNEXPECTED'):
    args = ['check']
sys.exit(subprocess.call([os.environ['CARGO'], *args]))
''')
        self.executable(self.bin / 'cargo', '''#!/bin/sh
if [ "$1" = clean ]; then exit 0; fi
if [ "${FIXTURE_BUILD_FAIL:-1}" = 1 ]; then exit 47; fi
mkdir -p target/x86_64-unknown-linux-gnu/release/build/ghostty_vt_sys-fixture/out/zig-out/lib
printf fixture > target/x86_64-unknown-linux-gnu/release/cas
printf fixture > target/x86_64-unknown-linux-gnu/release/build/ghostty_vt_sys-fixture/out/zig-out/lib/libghostty_vt.a
''')
        (self.repo / '.gitignore').write_text('target/\ndist/\n')
        self.command('git', 'add', '.')
        self.command('git', 'commit', '-qm', 'fixture')
        self.command('git', 'push', '-q', 'origin', 'main')

    def executable(self, path, body):
        path.write_text(body)
        path.chmod(0o755)

    def command(self, *args):
        return subprocess.run(args, cwd=self.repo, env=self.env, text=True,
                              stdout=subprocess.PIPE, stderr=subprocess.STDOUT)

    def preflight(self, jobs):
        (self.repo / '.cargo/config.toml').write_text(f'[build]\njobs = {jobs}\n')
        return self.command('bash', '-c', '''
script_dir="$1/scripts"
source "$script_dir/release-train.d/preflight.sh"
worktree="$2"
run_dir="$2/run"
mkdir -p "$run_dir"
cut_stage_file() { printf '%s/stage.%s.done' "$run_dir" "$1"; }
cut_preflight_check_publish_toolchain
''', 'fixture', str(ROOT), str(self.repo))

    def test_default_jobs_blocks_before_pipeline(self):
        result = self.preflight('"default"')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('BLOCKER publish-toolchain', result.stdout)
        self.assertIn('zigbuild', result.stdout)
        self.assertFalse((self.repo / 'target').exists())

    def test_negative_jobs_pass_without_build(self):
        result = self.preflight('-1')
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertFalse((self.repo / 'target').exists())

    def test_explicit_zig_outside_path_passes(self):
        external = self.base / 'external/zig'
        external.parent.mkdir()
        shutil.move(self.repo / '.context/zig/zig', external)
        self.env['ZIG'] = str(external)
        result = self.preflight('-1')
        self.assertEqual(result.returncode, 0, result.stdout)

    def test_success_without_delegate_fails_closed(self):
        self.env['FAKE_ZIG_SKIP_DELEGATE'] = '1'
        result = self.preflight('-1')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('delegate', result.stdout)

    def test_unexpected_delegate_command_refused(self):
        self.env['FAKE_ZIG_UNEXPECTED'] = '1'
        result = self.preflight('-1')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('unexpected', result.stdout)


if __name__ == '__main__':
    unittest.main()
