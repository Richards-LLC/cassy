#!/usr/bin/env python3
"""Owned scratch contracts with real locks/processes; no Rust builds."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location('scratch', Path(__file__).with_name('release_scratch.py'))
scratch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scratch)


class ScratchTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / 'repo'
        self.repo.mkdir()
        self.git('init', '-q')
        self.git('config', 'user.email', 'scratch@example.invalid')
        self.git('config', 'user.name', 'Scratch fixture')
        (self.repo / 'source').write_text('source')
        self.git('add', '.')
        self.git('commit', '-qm', 'fixture')
        self.parent = self.root / 'scratch'
        self.parent.mkdir()
        self.base = self.parent / 'base'
        self.env = dict(os.environ, TMPDIR=str(self.parent), CAS_RELEASE_SCRATCH_EXTRA_BASES='')
        # Isolate the inventory from unrelated opaque service processes. The
        # entries still point at real /proc handles of this test and its children.
        self.proc = self.root / 'proc'
        self.proc.mkdir()
        (self.proc / str(os.getpid())).symlink_to(Path('/proc') / str(os.getpid()), target_is_directory=True)
        patch = mock.patch.object(scratch, 'PROC_ROOT', self.proc)
        patch.start()
        self.addCleanup(patch.stop)

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.repo), *args], stderr=subprocess.PIPE)

    def old(self, path):
        os.utime(path, (1, 1))

    def wait_until(self, predicate):
        deadline = time.monotonic() + 5
        while not predicate():
            self.assertLess(time.monotonic(), deadline, 'fixture readiness expired')
            time.sleep(.01)

    def spawn(self, command, **kwargs):
        process = subprocess.Popen(command, env=self.env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, **kwargs)
        (self.proc / str(process.pid)).symlink_to(Path('/proc') / str(process.pid), target_is_directory=True)
        def reap():
            if process.poll() is None:
                process.kill()
            process.communicate(timeout=5)
        self.addCleanup(reap)
        return process

    def test_owned_directory_cleans_normal_and_error_paths(self):
        for fail in (False, True):
            try:
                with scratch.OwnedDirectory('base.', self.parent) as directory:
                    (directory / 'extract').mkdir()
                    (directory / 'extract/output').write_bytes(b'x' * 2048)
                    if fail:
                        raise RuntimeError('fixture')
            except RuntimeError:
                pass
            self.assertFalse(list(self.parent.glob('base.*')))

    def test_sigkill_owner_is_swept_and_concurrent_owner_is_preserved(self):
        ready = self.root / 'ready'
        program = "import importlib.util,sys,time;from pathlib import Path;s=importlib.util.spec_from_file_location('s',sys.argv[1]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m);d=m.OwnedDirectory('base.',sys.argv[2]);(d.path/'extract').mkdir();Path(sys.argv[3]).write_text(str(d.path));time.sleep(60)"
        process = self.spawn([sys.executable, '-c', program, str(Path(scratch.__file__).resolve()), str(self.parent), str(ready)])
        self.wait_until(ready.exists)
        abandoned = Path(ready.read_text())
        process.kill()
        process.communicate(timeout=5)
        self.old(abandoned)
        with scratch.OwnedDirectory('base.', self.parent) as live:
            self.old(live)
            result = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
            self.assertFalse(abandoned.exists(), result)
            self.assertTrue(live.exists(), result)
            self.assertGreater(result['reclaimed_bytes'], 0)

    def test_sigkill_parent_leaves_descendant_owner_lease_held(self):
        ready = self.root / 'descendant-ready'
        program = r'''import importlib.util,sys
from pathlib import Path
s=importlib.util.spec_from_file_location('s',sys.argv[1]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
with m.ChildScope() as scope, m.OwnedDirectory('base.',sys.argv[2]) as directory:
    child="import os,time,json;from pathlib import Path;Path("+repr(sys.argv[3])+").write_text(json.dumps({'pid':os.getpid(),'directory':"+repr(str(directory))+"}));time.sleep(60)"
    scope.run([sys.executable,'-c',child])
'''
        parent = self.spawn([sys.executable, '-c', program, str(Path(scratch.__file__).resolve()), str(self.parent), str(ready)])
        child_pid = None
        try:
            self.wait_until(ready.exists)
            info = json.loads(ready.read_text())
            child_pid = info['pid']
            directory = Path(info['directory'])
            parent.kill()
            parent.wait(timeout=5)  # Child still owns inherited stderr and lease.
            self.old(directory)
            result = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
            self.assertTrue(directory.exists(), result)
            os.kill(child_pid, signal.SIGTERM)
            parent.communicate(timeout=5)
            child_pid = None
            self.old(directory)
            result = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
            self.assertFalse(directory.exists(), result)
        finally:
            if child_pid is not None:
                try:
                    os.kill(child_pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass

    def test_legacy_live_cwd_and_registered_worktree_are_preserved(self):
        live = self.parent / 'base.live'
        live.mkdir()
        (live / 'output').write_text('live')
        ready = self.root / 'ready'
        process = self.spawn([sys.executable, '-c', 'import pathlib,sys,time;pathlib.Path(sys.argv[1]).touch();time.sleep(60)', str(ready)], cwd=live)
        self.wait_until(ready.exists)
        self.old(live)
        registered = self.parent / 'base.registered'
        self.git('worktree', 'add', '-q', '-b', 'fixture/registered', str(registered))
        self.old(registered)
        result = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
        self.assertTrue(live.exists(), result)
        self.assertTrue(registered.exists(), result)
        process.terminate()
        process.communicate(timeout=5)
        result = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
        # The first inventory created an owned lease; restore age for the retry.
        self.old(live)
        result = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
        self.assertFalse(live.exists(), result)
        self.assertTrue(registered.exists(), result)

    def test_report_is_read_only_and_symlink_escape_is_untouched(self):
        old = self.parent / 'base.old'
        old.mkdir()
        (old / 'output').write_bytes(b'x' * 1234)
        self.old(old)
        escape = self.root / 'keep'
        escape.mkdir()
        (self.parent / 'base.escape').symlink_to(escape, target_is_directory=True)
        before = sorted(str(path.relative_to(self.root)) for path in self.root.rglob('*'))
        result = scratch.sweep(self.repo, self.base, env=self.env)
        after = sorted(str(path.relative_to(self.root)) for path in self.root.rglob('*'))
        self.assertEqual(before, after)
        self.assertEqual(result['reclaimable_bytes'], 1234)
        scratch.sweep(self.repo, self.base, clean=True, env=self.env)
        self.assertTrue(escape.exists())
        self.assertFalse(old.exists())

    def test_cache_size_and_age_bounds_evict_before_and_after_use(self):
        target = self.parent / 'assembly-target'
        target.mkdir()
        (target / 'output').write_bytes(b'x' * 2048)
        env = dict(self.env, CAS_ASSEMBLY_TARGET_MAX_GIB='0.000001')
        with scratch.BoundedCache(target, env, self.repo):
            self.assertFalse(target.exists())
            target.mkdir()
            (target / 'output').write_bytes(b'x' * 2048)
        self.assertFalse(target.exists())
        target.mkdir()
        (target / 'output').write_text('old')
        self.old(target)
        with scratch.BoundedCache(target, self.env, self.repo):
            self.assertFalse(target.exists())

    def test_cache_preserves_open_output_and_registered_checkout(self):
        target = self.parent / 'assembly-target'
        target.mkdir()
        (target / 'output').write_bytes(b'x' * 2048)
        env = dict(self.env, CAS_ASSEMBLY_TARGET_MAX_GIB='0.000001')
        with (target / 'output').open('rb'), self.assertRaisesRegex(ValueError, 'live users'):
            with scratch.BoundedCache(target, env, self.repo):
                pass
        self.git('worktree', 'add', '-q', '-b', 'fixture/cache', str(target / 'registered'))
        with self.assertRaisesRegex(ValueError, 'registered worktree'):
            with scratch.BoundedCache(target, env, self.repo):
                pass
        self.assertTrue((target / 'registered/source').exists())

    def test_parent_waits_beyond_nested_five_second_teardown(self):
        ready = self.root / 'slow-ready'
        finished = self.root / 'slow-finished'
        program = r'''import importlib.util,sys
s=importlib.util.spec_from_file_location('s',sys.argv[1]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
with m.ChildScope() as scope, m.OwnedDirectory('base.',sys.argv[2]) as directory:
    child="import signal,time,sys;from pathlib import Path;\ndef stop(s,f):\n signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(5.1);Path("+repr(sys.argv[4])+").touch();sys.exit(0)\nsignal.signal(signal.SIGTERM,stop);Path("+repr(sys.argv[3])+").write_text("+repr(str(directory))+");time.sleep(60)"
    scope.run([sys.executable,'-c',child])
'''
        parent = self.spawn([sys.executable, '-c', program, str(Path(scratch.__file__).resolve()), str(self.parent), str(ready), str(finished)])
        self.wait_until(ready.exists)
        directory = Path(ready.read_text())
        parent.send_signal(signal.SIGTERM)
        time.sleep(.1)
        parent.send_signal(signal.SIGTERM)  # Cannot interrupt a finally/reap a second time.
        self.assertTrue(directory.exists(), 'scratch removed before child teardown')
        parent.communicate(timeout=10)
        self.assertTrue(finished.exists(), 'parent killed nested guard before its five-second budget')
        self.assertFalse(directory.exists())

    def test_opaque_process_evidence_fails_closed(self):
        original = os.readlink
        def unreadable(path, *args, **kwargs):
            if Path(path) == self.proc / str(os.getpid()) / 'cwd':
                raise PermissionError('fixture opaque process')
            return original(path, *args, **kwargs)
        with mock.patch.object(scratch.os, 'readlink', side_effect=unreadable):
            self.assertTrue(scratch.process_uses(self.parent / 'nonexistent'))

    def test_guard_signals_reap_before_base_and_extract_cleanup(self):
        for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
            with self.subTest(signal=sig):
                ready = self.root / ('ready-' + str(sig))
                script = self.root / 'fixture.sh'
                script.write_text('set -eu\nbase=$(mktemp -d "$CAS_RELEASE_GATE_HOME_DIR.XXXXXX")\n'
                    'python3 "$HELPER" --owner-dir "$CAS_RELEASE_GATE_SCRATCH_RUN_DIR" --path "$base" register\n'
                    'mkdir "$base/extract"\nprintf "%s" "$base" > "$READY"\nexec python3 -c "import time;time.sleep(60)"\n')
                env = dict(self.env, CAS_RELEASE_GATE_HOME_DIR=str(self.base), HELPER=str(Path(scratch.__file__).resolve()), READY=str(ready))
                process = subprocess.Popen([sys.executable, str(Path(scratch.__file__).resolve()), '--repo', str(self.repo), '--base', str(self.base), 'guard', '--', 'bash', str(script)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
                try:
                    self.wait_until(ready.exists)
                    process.send_signal(sig)
                    process.communicate(timeout=10)
                    self.assertNotEqual(process.returncode, 0)
                    self.assertFalse(Path(ready.read_text()).exists())
                    self.assertFalse(list(self.parent.glob('cas-release-gate.*')))
                finally:
                    if process.poll() is None:
                        process.kill()
                    process.communicate(timeout=5)


if __name__ == '__main__':
    unittest.main()
