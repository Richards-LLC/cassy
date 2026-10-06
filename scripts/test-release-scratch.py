#!/usr/bin/env python3
"""Owned scratch contracts with real locks/processes; no Rust builds."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import shutil
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
        if Path('/proc').is_dir():
            (self.proc / str(os.getpid())).symlink_to(Path('/proc') / str(os.getpid()), target_is_directory=True)
        else:
            self.proc.rmdir()  # Exercise the macOS ps/lsof path on macOS.
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
        kwargs.setdefault("stdout", subprocess.DEVNULL)
        process = subprocess.Popen(command, env=self.env, stderr=subprocess.PIPE, **kwargs)
        if self.proc.is_dir():
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
        # Fresh SIGKILL debris is eligible immediately; no artificial ageing.
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
            result = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
            self.assertTrue(directory.exists(), result)
            os.kill(child_pid, signal.SIGTERM)
            parent.communicate(timeout=5)
            child_pid = None
            result = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
            self.assertFalse(directory.exists(), result)
        finally:
            if child_pid is not None:
                try:
                    os.kill(child_pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass

    def test_normal_parent_exit_retains_escaped_lease_holder_until_child_exit(self):
        ready = self.root / 'escaped-ready'
        program = r'''import importlib.util,subprocess,sys,json
from pathlib import Path
s=importlib.util.spec_from_file_location('s',sys.argv[1]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
with m.ChildScope() as scope, m.OwnedDirectory('base.',sys.argv[2]) as directory:
    child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)'],start_new_session=True,pass_fds=tuple(scope.leases))
    Path(sys.argv[3]).write_text(json.dumps({'pid':child.pid,'directory':str(directory)}))
'''
        parent = self.spawn([sys.executable, '-c', program, scratch.__file__, str(self.parent), str(ready)])
        child_pid = None
        try:
            self.wait_until(ready.exists)
            info = json.loads(ready.read_text())
            child_pid = info['pid']
            parent.wait(timeout=5)
            directory = Path(info['directory'])
            self.assertTrue(directory.exists(), 'live inherited lease path was removed')
            report = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
            self.assertTrue(directory.exists(), report)
            os.kill(child_pid, signal.SIGTERM)
            parent.communicate(timeout=5)
            child_pid = None
            report = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
            self.assertFalse(directory.exists(), report)
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

    def idle_owner(self, path, cache=False):
        path.mkdir(mode=0o700, exist_ok=True)
        owner = scratch.owner_record(path, path.with_name(path.name + ".lock") if cache else path / scratch.LOCK)
        owner.update(pid=0, start='idle')
        (path / scratch.OWNER).write_text(json.dumps(owner))

    def test_size_counts_a_file_removed_mid_scan_as_zero(self):
        # A live run deletes build output while inventory is measuring it.
        root = self.parent / 'churn'
        root.mkdir()
        (root / 'kept').write_bytes(b'x' * 10)
        (root / 'gone').write_bytes(b'y' * 20)
        real_stat = Path.stat
        def flaky_stat(path, *args, **kwargs):
            if path.name == 'gone':
                raise FileNotFoundError(str(path))
            return real_stat(path, *args, **kwargs)
        with unittest.mock.patch.object(Path, 'stat', flaky_stat):
            self.assertEqual(scratch.size(root), 10)

    def test_cache_size_and_age_bounds_evict_before_and_after_use(self):
        target = self.parent / 'assembly-target'
        env = dict(self.env, CAS_ASSEMBLY_TARGET_MAX_GIB='0.000001')
        # Created under the lease protocol, not silently adopted legacy output.
        with scratch.BoundedCache(target, env, self.repo):
            (target / 'output').write_bytes(b'x' * 2048)
        self.assertFalse(target.exists())
        with scratch.BoundedCache(target, self.env, self.repo):
            (target / 'output').write_text('old')
        self.old(target / '.cas-last-used')
        with scratch.BoundedCache(target, self.env, self.repo) as directory:
            self.assertFalse((directory / 'output').exists())

    def test_cache_preserves_open_output_and_registered_checkout(self):
        target = self.parent / 'assembly-target'
        with scratch.BoundedCache(target, self.env, self.repo):
            (target / 'output').write_bytes(b'x' * 2048)
        env = dict(self.env, CAS_ASSEMBLY_TARGET_MAX_GIB='0.000001')
        with (target / 'output').open('rb'):
            report = scratch.cache_report(self.repo, target, clean=True, env=env)
            self.assertIn('live users', report['reason'])
            self.assertTrue((target / 'output').exists())
        self.git('worktree', 'add', '-q', '-b', 'fixture/cache', str(target / 'registered'))
        with self.assertRaisesRegex(ValueError, 'registered worktree'):
            with scratch.BoundedCache(target, env, self.repo):
                pass
        self.assertTrue((target / 'registered/source').exists())

    def test_legacy_cache_requires_explicit_adoption_and_refuses_held_lease(self):
        target = self.parent / 'assembly-target'
        target.mkdir()
        (target / 'output').write_bytes(b'x' * 2048)
        env = dict(self.env, CAS_ASSEMBLY_TARGET_MAX_GIB='0.000001')
        report = scratch.cache_report(self.repo, target, clean=True, env=env)
        self.assertIn('legacy cache retained', report['reason'])
        self.assertEqual(report['retained_bytes'], 2048)
        self.assertFalse((target / scratch.OWNER).exists())
        with scratch.BoundedCache(target, self.env, self.repo):
            report = scratch.cache_report(self.repo, target, clean=True, adopt=True, env=env)
            self.assertIn('protected', report['reason'])
            self.assertTrue(target.exists())
        report = scratch.cache_report(self.repo, target, clean=True, adopt=True, env=env)
        self.assertTrue(report['adopted'])
        self.assertEqual(report['reclaimed_bytes'], 2048)
        self.assertFalse(target.exists())

    def test_registered_remap_keeps_base_but_reclaims_only_known_siblings(self):
        base = self.parent / 'base.retained'
        self.idle_owner(base)
        self.git('worktree', 'add', '-q', '-b', 'fixture/remap', str(base / 'workspace-remap'))
        for name in scratch.REGENERABLE:
            if name == 'suite.tar.zst':
                (base / name).write_bytes(b'x' * 2048)
            else:
                (base / name).mkdir()
                (base / name / 'output').write_bytes(b'x' * 2048)
        (base / 'unknown-evidence').write_text('keep')
        self.old(base)
        protected_before = scratch.worktrees(self.repo)
        report = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
        self.assertTrue(base.exists())
        self.assertEqual(protected_before, scratch.worktrees(self.repo))
        self.assertTrue((base / 'workspace-remap/source').exists())
        self.assertTrue((base / 'unknown-evidence').exists())
        self.assertEqual(report['reclaimed_bytes'], 5 * 2048)
        self.assertGreater(report['retained_bytes'], 0)
        self.assertTrue(report['entries'][0]['retained_base'])
        self.assertFalse(any((base / name).exists() for name in scratch.REGENERABLE))

    def generated_remap(self, dead=True, parent=None):
        program = r'''
import importlib.util,json,pathlib,subprocess,sys
spec=importlib.util.spec_from_file_location('scratch',sys.argv[1])
s=importlib.util.module_from_spec(spec);spec.loader.exec_module(s)
resource=s.OwnedDirectory('base.generated-',sys.argv[2])
base=resource.path; remap=base/'workspace-remap'; repo=pathlib.Path(sys.argv[3])
subprocess.run(['git','-C',str(repo),'worktree','add','--detach',str(remap),'HEAD'],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
s.register_remap(base,repo)
print(json.dumps(str(base)),flush=True)
sys.stdin.read()
'''
        child = self.spawn([sys.executable, '-c', program, str(Path(scratch.__file__)), str(parent or self.parent), str(self.repo)],
                           stdout=subprocess.PIPE, stdin=subprocess.PIPE)
        ready = child.stdout.readline()
        if not ready:
            _, errors = child.communicate(timeout=5)
            self.fail('generated remap registration failed: ' + errors.decode())
        base = Path(json.loads(ready))
        if dead:
            child.kill()
            child.communicate(timeout=5)
        return base, child

    def test_dead_generated_remap_unregisters_exact_admin_and_reclaims_base_cas_638d(self):
        base, _ = self.generated_remap()
        remap = base / 'workspace-remap'
        admin = Path(subprocess.check_output(['git','-C',str(remap),'rev-parse','--absolute-git-dir'], text=True).strip())
        # A different absent parked checkout must keep its metadata: no global prune.
        parked = self.root / 'parked'
        self.git('worktree','add','-q','-b','factory/parked',str(parked))
        parked_admin = Path(subprocess.check_output(['git','-C',str(parked),'rev-parse','--absolute-git-dir'], text=True).strip())
        shutil.rmtree(parked)
        report = scratch.sweep(self.repo,self.base,clean=True,env=self.env)
        self.assertFalse(base.exists(), report)
        self.assertFalse(admin.exists())
        self.assertTrue(parked_admin.exists())
        self.assertNotIn(remap,scratch.worktrees(self.repo))

    def test_remap_live_lease_branch_dirty_and_locked_are_preserved_cas_638d(self):
        live, child = self.generated_remap(dead=False)
        report = scratch.sweep(self.repo,self.base,clean=True,env=self.env)
        self.assertTrue((live/'workspace-remap/source').exists(), report)
        child.kill();child.communicate(timeout=5)
        scratch.sweep(self.repo,self.base,clean=True,env=self.env)
        for mutation in ('branch','dirty','locked'):
            base, _ = self.generated_remap()
            remap = base/'workspace-remap'
            if mutation == 'branch':
                subprocess.run(['git','-C',str(remap),'checkout','-qb','factory/parked-'+base.name], check=True)
            elif mutation == 'dirty':
                (remap/'source').write_text('delivery evidence')
            else:
                self.git('worktree','lock',str(remap))
            before = scratch.worktrees(self.repo)
            report = scratch.sweep(self.repo,self.base,clean=True,env=self.env)
            self.assertTrue((remap/'source').exists(), (mutation, report))
            self.assertEqual(before,scratch.worktrees(self.repo))

    def test_generated_receipt_cannot_unregister_replaced_or_missing_unknown_checkout_cas_638d(self):
        base, _ = self.generated_remap()
        remap = base/'workspace-remap'
        admin = Path(subprocess.check_output(['git','-C',str(remap),'rev-parse','--absolute-git-dir'],text=True).strip())
        self.git('worktree','remove',str(remap))
        self.git('worktree','add','-q','-b','factory/new-delivery',str(remap))
        replacement = scratch.worktrees(self.repo)
        report = scratch.sweep(self.repo,self.base,clean=True,env=self.env)
        self.assertTrue((remap/'source').exists(),report)
        self.assertEqual(replacement,scratch.worktrees(self.repo))

    def test_missing_generated_checkout_prunes_only_receipted_admin_cas_638d(self):
        base, _ = self.generated_remap()
        remap = base/'workspace-remap'
        admin = Path(subprocess.check_output(['git','-C',str(remap),'rev-parse','--absolute-git-dir'],text=True).strip())
        shutil.rmtree(remap)
        report = scratch.sweep(self.repo,self.base,clean=True,env=self.env)
        self.assertFalse(admin.exists(),report)
        self.assertFalse(base.exists(),report)

    def test_generated_report_is_read_only_and_cassy_checkout_is_retained_cas_638d(self):
        base, _ = self.generated_remap()
        before = {str(path):path.read_bytes() for path in base.rglob('*') if path.is_file()}
        protected = scratch.worktrees(self.repo)
        report = scratch.sweep(self.repo,self.base,env=self.env)
        self.assertTrue(report['entries'][0]['reclaimable'],report)
        self.assertEqual(protected,scratch.worktrees(self.repo))
        self.assertEqual(before,{str(path):path.read_bytes() for path in base.rglob('*') if path.is_file()})
        # Even an ignored .cas in a detached checkout is delivery provenance.
        remap = base/'workspace-remap'
        with (self.repo / '.git/info/exclude').open('a') as stream:
            stream.write('\n.cas/\n')
        (remap/'.cas').mkdir()
        (remap/'.cas/parked-task').write_text('preserve')
        self.assertFalse(scratch.git_output(remap, 'status', '--porcelain', '--untracked-files=all'))
        report = scratch.sweep(self.repo,self.base,clean=True,env=self.env)
        self.assertTrue((remap/'.cas/parked-task').exists(),report)

    def test_malformed_receipt_after_interrupted_removal_is_retained_cas_638d(self):
        for field, value in (('head', None), ('admin', None), ('admin_identity', [True, 0])):
            with self.subTest(field=field):
                base, _ = self.generated_remap()
                self.git('worktree', 'remove', str(base / 'workspace-remap'))
                receipt_path = base / scratch.REMAP_RECEIPT
                receipt = json.loads(receipt_path.read_text())
                receipt[field] = value
                receipt_path.write_text(json.dumps(receipt))
                report = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
                self.assertTrue(base.exists(), report)
                self.assertIn('protected:', next(row['reason'] for row in report['entries'] if row['path'] == str(base)))

    def test_symlink_admin_files_are_retained_without_following_cas_638d(self):
        for name in ('HEAD', 'gitdir', 'commondir', 'index'):
            with self.subTest(name=name):
                base, _ = self.generated_remap()
                remap = base / 'workspace-remap'
                admin = Path(subprocess.check_output(['git', '-C', str(remap), 'rev-parse', '--absolute-git-dir'], text=True).strip())
                metadata = admin / name
                outside = self.root / ('outside-' + name)
                outside.write_bytes(metadata.read_bytes())
                metadata.unlink()
                metadata.symlink_to(outside)
                before = outside.read_bytes()
                report = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
                self.assertTrue((remap / 'source').exists(), report)
                self.assertTrue(admin.exists(), report)
                self.assertEqual(outside.read_bytes(), before)

    def test_dangling_receipt_is_unknown_provenance_cas_638d(self):
        base, _ = self.generated_remap()
        self.git('worktree', 'remove', str(base / 'workspace-remap'))
        receipt = base / scratch.REMAP_RECEIPT
        receipt.unlink()
        receipt.symlink_to(self.root / 'missing-receipt')
        report = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
        self.assertTrue(base.exists(), report)

    def test_unknown_owner_or_receipt_preserves_generated_registration_cas_638d(self):
        for mutation in ('owner', 'owner-array', 'receipt'):
            with self.subTest(mutation=mutation):
                base, _ = self.generated_remap()
                if mutation == 'owner':
                    (base / scratch.OWNER).unlink()
                    self.old(base)
                elif mutation == 'owner-array':
                    (base / scratch.OWNER).write_text('[1]')
                    self.old(base)
                else:
                    (base / scratch.REMAP_RECEIPT).unlink()
                before = scratch.worktrees(self.repo)
                report = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
                self.assertTrue((base / 'workspace-remap/source').exists(), report)
                self.assertEqual(before, scratch.worktrees(self.repo))
                self.assertTrue((self.repo / 'source').exists())

    def test_registry_and_admin_symlinks_preserve_all_checkouts_cas_638d(self):
        for component in ('registry', 'admin', 'checkout', 'git-file', 'receipt'):
            with self.subTest(component=component):
                base, _ = self.generated_remap()
                remap = base / 'workspace-remap'
                admin = Path(subprocess.check_output(['git', '-C', str(remap), 'rev-parse', '--absolute-git-dir'], text=True).strip())
                path = {'registry': admin.parent, 'admin': admin, 'checkout': remap,
                        'git-file': remap / '.git', 'receipt': base / scratch.REMAP_RECEIPT}[component]
                outside = self.root / ('moved-' + component)
                path.rename(outside)
                path.symlink_to(outside, target_is_directory=outside.is_dir())
                try:
                    report = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
                    self.assertTrue((remap / 'source').exists(), report)
                    self.assertTrue(admin.exists(), report)
                    self.assertTrue((self.repo / 'source').exists(), report)
                finally:
                    path.unlink()
                    outside.rename(path)

    def test_completed_targeted_removal_can_resume_base_cleanup_cas_638d(self):
        base, _ = self.generated_remap()
        self.git('worktree', 'remove', str(base / 'workspace-remap'))
        report = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
        self.assertFalse(base.exists(), report)
        self.assertTrue((self.repo / 'source').exists())

    def test_generated_remap_removal_never_removes_registered_sibling_cas_638d(self):
        base, _ = self.generated_remap()
        worker = base / 'worker'
        self.git('worktree', 'add', '-q', '-b', 'factory/worker', str(worker))
        worker_admin = scratch.git_output(worker, 'rev-parse', '--absolute-git-dir').decode().strip()
        report = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
        self.assertFalse((base / 'workspace-remap').exists(), report)
        self.assertTrue((worker / 'source').exists(), report)
        self.assertTrue(Path(worker_admin).exists())
        self.assertIn(worker, scratch.worktrees(self.repo))
        self.assertTrue(base.exists(), report)
        self.assertTrue((self.repo / 'source').exists())

    def test_alias_scratch_parent_uses_physical_receipt_identity_cas_638d(self):
        alias = self.root / 'scratch-alias'
        alias.symlink_to(self.parent, target_is_directory=True)
        base, _ = self.generated_remap(parent=alias)
        report = scratch.sweep(self.repo, alias / 'base', clean=True, env=dict(self.env, TMPDIR=str(alias)))
        self.assertFalse(base.exists(), report)

    def test_guard_preserves_unrelated_live_remap_user_before_unregister_cas_638d(self):
        ready, release, reader_ready = (self.root / name for name in ('guard-ready', 'guard-release', 'reader-ready'))
        program = r'''
import importlib.util,os,pathlib,subprocess,sys,tempfile,time
spec=importlib.util.spec_from_file_location('scratch',sys.argv[1])
s=importlib.util.module_from_spec(spec);spec.loader.exec_module(s)
base=pathlib.Path(tempfile.mkdtemp(prefix='base.guard-',dir=sys.argv[2]))
s.register(base,pathlib.Path(os.environ['CAS_RELEASE_GATE_SCRATCH_RUN_DIR']))
repo=pathlib.Path(sys.argv[3]); remap=base/'workspace-remap'
subprocess.run(['git','-C',str(repo),'worktree','add','--detach',str(remap),'HEAD'],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
s.register_remap(base,repo)
pathlib.Path(sys.argv[4]).write_text(str(base))
while not pathlib.Path(sys.argv[5]).exists(): time.sleep(.01)
'''
        env = dict(self.env, CAS_RELEASE_GATE_HOME_DIR=str(self.base))
        guardian = subprocess.Popen([sys.executable, scratch.__file__, '--repo', str(self.repo), '--base', str(self.base),
            'guard', '--', sys.executable, '-c', program, scratch.__file__, str(self.parent), str(self.repo), str(ready), str(release)],
            env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        self.addCleanup(lambda: guardian.poll() is None and guardian.kill())
        self.wait_until(ready.exists)
        base = Path(ready.read_text())
        reader = self.spawn([sys.executable, '-c',
            'import pathlib,sys,time;f=open(sys.argv[1]);pathlib.Path(sys.argv[2]).touch();time.sleep(60)',
            str(base / 'workspace-remap/source'), str(reader_ready)])
        self.wait_until(reader_ready.exists)
        release.touch()
        _, errors = guardian.communicate(timeout=10)
        self.assertEqual(guardian.returncode, 0, errors.decode())
        self.assertTrue((base / 'workspace-remap/source').exists(), errors.decode())
        self.assertIn(base / 'workspace-remap', scratch.worktrees(self.repo))
        reader.kill()
        reader.communicate(timeout=5)
        report = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
        self.assertFalse(base.exists(), report)

    def test_start_time_mismatch_is_dead_but_matching_owner_is_live(self):
        old = self.parent / 'base.reused-pid'
        self.idle_owner(old)
        owner = json.loads((old / scratch.OWNER).read_text())
        owner.update(pid=os.getpid(), start='different start')
        (old / scratch.OWNER).write_text(json.dumps(owner))
        self.old(old)
        self.assertFalse(scratch.owner_live(owner))
        scratch.sweep(self.repo, self.base, clean=True, env=self.env)
        self.assertFalse(old.exists())
        with scratch.OwnedDirectory('base.', self.parent) as live:
            self.old(live)
            self.assertTrue(scratch.owner_live(scratch.read_owner(live)))
            scratch.sweep(self.repo, self.base, clean=True, env=self.env)
            self.assertTrue(live.exists())

    def test_opaque_process_blocks_unknown_but_not_dead_lease_managed_scratch(self):
        managed, unknown = self.parent / 'base.managed', self.parent / 'base.unknown'
        self.idle_owner(managed)
        unknown.mkdir()
        (unknown / 'output').write_text('preserve')
        self.old(managed)
        self.old(unknown)
        if not self.proc.is_dir():
            self.skipTest('Linux permission fixture; macOS opaque lsof fixture is separate')
        original = os.readlink
        def opaque(path, *args, **kwargs):
            if Path(path) == self.proc / str(os.getpid()) / 'cwd':
                raise PermissionError('controlled non-dumpable process')
            return original(path, *args, **kwargs)
        with mock.patch.object(scratch.os, 'readlink', side_effect=opaque):
            report = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
        self.assertFalse(managed.exists(), report)
        self.assertTrue(unknown.exists(), report)
        self.assertGreater(report['reclaimed_bytes'], 0)
        self.assertGreater(report['retained_bytes'], 0)

    def test_registration_refuses_populated_unknown_and_symlink_paths(self):
        with scratch.OwnedDirectory('cas-release-gate.', self.parent) as owner:
            unsafe = self.parent / 'base.not-fresh'
            unsafe.mkdir(mode=0o700)
            (unsafe / 'output').write_text('keep')
            link = self.parent / 'base.link'
            link.symlink_to(unsafe, target_is_directory=True)
            with mock.patch.dict(os.environ, self.env):
                for path in (unsafe, link, self.repo):
                    with self.assertRaises(ValueError):
                        scratch.register(path, owner)
            self.assertFalse((owner / 'paths').exists())
            self.assertTrue((unsafe / 'output').exists())

    def test_macos_lsof_exempts_only_own_exact_lease_fd(self):
        path = self.parent / 'base.mac'
        path.mkdir()
        lock = path / scratch.LOCK
        with lock.open('a+') as stream, mock.patch.object(scratch, 'PROC_ROOT', self.root / 'absent-proc'):
            fields = f'p{os.getpid()}\0f{stream.fileno()}w\0n{lock}\0'.encode()
            with mock.patch.object(scratch.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, fields, b'')):
                self.assertFalse(scratch.process_uses(path, stream.fileno()))
                self.assertTrue(scratch.process_uses(path))
            with mock.patch.object(scratch.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1, b'', b'opaque process')):
                self.assertTrue(scratch.process_uses(path))
                self.assertFalse(scratch.process_uses(path, lease_managed=True))

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

    def test_dead_managed_owner_is_reclaimed_with_the_real_host_process_table(self):
        if not Path('/proc').is_dir():
            self.skipTest('real Linux host fixture; real macOS owner children covered above')
        old = self.parent / 'base.real-host'
        self.idle_owner(old)
        (old / 'extract').mkdir()
        (old / 'extract/output').write_bytes(b'x' * 4096)
        self.old(old)
        with mock.patch.object(scratch, 'PROC_ROOT', Path('/proc')):
            report = scratch.sweep(self.repo, self.base, clean=True, env=self.env)
        self.assertFalse(old.exists(), report)
        self.assertGreaterEqual(report['reclaimed_bytes'], 4096)

    def test_gc_cli_reports_legacy_cache_and_is_read_only(self):
        target = self.parent / 'assembly-target'
        target.mkdir()
        (target / 'output').write_bytes(b'x' * 2345)
        before = sorted(str(path.relative_to(self.root)) for path in self.root.rglob('*'))
        command = [sys.executable, scratch.__file__, '--repo', str(self.repo), '--base', str(self.base), '--cache', str(target), 'report']
        result = subprocess.run(command, env=self.env, text=True, capture_output=True, check=True)
        report = json.loads(result.stdout)
        self.assertEqual(report['caches'][0]['retained_bytes'], 2345)
        self.assertGreaterEqual(report['retained_bytes'], 2345)
        self.assertEqual(before, sorted(str(path.relative_to(self.root)) for path in self.root.rglob('*')))
        self.assertEqual(scratch.select_cache(target), target.with_name('assembly-target-leased-v1'))
        with scratch.BoundedCache(target, self.env, self.repo):
            result = subprocess.run(command[:-1] + ['--adopt-legacy-cache', 'clean'], env=self.env, text=True, capture_output=True, check=True)
            self.assertIn('protected', json.loads(result.stdout)['caches'][0]['reason'])
            self.assertFalse((target / scratch.OWNER).exists())

    def test_gc_cli_reclaims_over_bound_managed_cache_without_self_argv_false_positive(self):
        target = self.parent / 'assembly-target'
        self.idle_owner(target, cache=True)
        (target / 'output').write_bytes(b'x' * 2048)
        env = dict(self.env, CAS_ASSEMBLY_TARGET_MAX_GIB='0.000001')
        result = subprocess.run([sys.executable, scratch.__file__, '--repo', str(self.repo),
                                 '--base', str(self.base), '--cache', str(target), 'clean'],
                                env=env, text=True, capture_output=True, check=True)
        report = json.loads(result.stdout)
        self.assertFalse(target.exists(), report)
        self.assertGreaterEqual(report['reclaimed_bytes'], 2048)

    def test_constructor_signal_unwinds_registered_resource_before_context_entry(self):
        original = scratch.owner_record
        def interrupt(path, lease):
            record = original(path, lease)
            os.kill(os.getpid(), signal.SIGTERM)  # Pending until registration completes.
            return record
        with self.assertRaises(InterruptedError), mock.patch.object(scratch, 'owner_record', side_effect=interrupt):
            with scratch.ChildScope(), scratch.OwnedDirectory('base.', self.parent):
                self.fail('pending signal was not delivered')
        self.assertFalse(list(self.parent.glob('base.*')))

    def test_signal_during_child_spawn_registers_then_reaps(self):
        spawned = []
        original = scratch.subprocess.Popen
        def interrupt(command, **kwargs):
            child = original(command, **kwargs)
            spawned.append(child.pid)
            os.kill(os.getpid(), signal.SIGTERM)
            return child
        with self.assertRaises(InterruptedError), mock.patch.object(scratch.subprocess, 'Popen', side_effect=interrupt):
            with scratch.ChildScope() as scope, scratch.OwnedDirectory('base.', self.parent):
                scope.run([sys.executable, '-c', 'import time;time.sleep(60)'])
        self.assertFalse(list(self.parent.glob('base.*')))
        with self.assertRaises(ProcessLookupError):
            os.kill(spawned[0], 0)

    def test_opaque_process_evidence_fails_closed(self):
        if not self.proc.is_dir():
            self.skipTest('Linux proc permission fixture')
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
