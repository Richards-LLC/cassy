#!/usr/bin/env python3
"""Host admission tests using Python children and fake JS runner; no browsers/npm."""
import contextlib
import fcntl
import importlib.util
import io
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
BASELINE = '--baseline' in sys.argv
if BASELINE:
    sys.argv.remove('--baseline')


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


host = load('host_memory', ROOT / 'scripts/host_memory.py')
sys.modules['host_memory'] = host
worker = load('worker_memory', ROOT / 'scripts/worker-memory.py')
GIB = 1024**3
HIGH = {'total_bytes': 64*GIB, 'available_bytes': 60*GIB, 'reserve_bytes': 16*GIB, 'budget_bytes': 44*GIB, 'source': 'fixture'}
# How long a counted command may take merely to start. This bounds process
# spawn latency, not admission: a serialized command never starts while its
# peers hold their release barrier, so a generous bound still fails on serial
# admission. Two seconds missed by 7 ms at a load average of 11.9 (cas-e4e3).
SPAWN_DEADLINE_SECS = float(os.environ.get('CAS_TEST_SPAWN_DEADLINE_SECS', '20'))
TRAIN_ENV_KEYS = ('CAS_RELEASE_TRAIN_INVOCATION_KIND', 'CAS_RELEASE_TRAIN_RUN_DIR',
                  'CAS_RELEASE_TRAIN_STAGE')


class AdmissionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.pool = self.root / 'pool'
        self.events = []
        self.env = dict(os.environ)
        self.env.pop(host.LEASE_ENV, None)
        # Never start or contact the host's real compiler-cache server.
        for key in ('RUSTC_WRAPPER', 'CARGO_BUILD_RUSTC_WRAPPER'):
            self.env.pop(key, None)

    def admit(self, role, **kwargs):
        return host.admission(role, self.env, lambda _: HIGH, directory=self.pool,
                              wait_secs=.5, poll_secs=.02, report=self.events.append, **kwargs)

    def start_counted_command(self, executable, name, budget):
        binary = self.root / executable
        if not binary.exists():
            binary.write_text('#!' + sys.executable + '\n'
                              'import pathlib,sys,time\n'
                              'pathlib.Path(sys.argv[1]).write_text("started")\n'
                              'while not pathlib.Path(sys.argv[2]).exists(): time.sleep(.01)\n')
            binary.chmod(0o755)
        marker = self.root / (name + '-started')
        release = self.root / (name + '-release')
        launcher = (
            f"import sys,pathlib;sys.path.insert(0,{str(ROOT/'scripts')!r});"
            f"import importlib.util;spec=importlib.util.spec_from_file_location('worker',{str(ROOT/'scripts/worker-memory.py')!r});"
            "m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);"
            f"m.proof.memory_budget=lambda env:{budget!r};"
            f"sys.exit(m.run({[str(binary), str(marker), str(release)]!r},directory=pathlib.Path({str(self.pool)!r})))"
        )
        env = dict(self.env, CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS='3',
                   CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLL_SECS='1')
        child = subprocess.Popen([sys.executable, '-c', launcher], env=env,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        def cleanup():
            release.touch()
            if child.poll() is None: child.terminate()
            child.communicate(timeout=5)
        self.addCleanup(cleanup)
        return child, marker, release

    def wait_for_markers(self, markers):
        deadline = time.monotonic() + SPAWN_DEADLINE_SECS
        while not all(marker.exists() for marker in markers):
            self.assertLess(time.monotonic(), deadline,
                            f'commands did not run concurrently: {[p.name for p in markers if p.exists()]}')
            time.sleep(.01)

    def test_cas_4cb9_two_browser_suites_and_typecheck_run_concurrently(self):
        commands = [self.start_counted_command('playwright', 'browser-one', HIGH),
                    self.start_counted_command('playwright', 'browser-two', HIGH),
                    self.start_counted_command('tsc', 'typecheck', HIGH)]
        self.wait_for_markers([marker for _, marker, _ in commands])
        # All three native commands are alive, waiting for our release barrier.
        self.assertTrue(all(child.poll() is None for child, _, _ in commands))
        for _, _, release in commands: release.touch()
        for child, _, _ in commands:
            stdout, stderr = child.communicate(timeout=5)
            self.assertEqual(child.returncode, 0, stdout + stderr)

    def test_cas_4cb9_low_budget_serializes_browser_suites(self):
        low = dict(HIGH, budget_bytes=8*GIB)
        first, one, release_one = self.start_counted_command('playwright', 'first', low)
        self.wait_for_markers([one])
        second, two, release_two = self.start_counted_command('playwright', 'second', low)
        deadline = time.monotonic() + SPAWN_DEADLINE_SECS
        while True:
            self.assertFalse(two.exists(), 'low budget admitted two browser estimates')
            self.assertLess(time.monotonic(), deadline)
            readable, _, _ = select.select([second.stdout], [], [], .02)
            if readable and 'waiting for host memory' in second.stdout.readline(): break
        release_one.touch()
        stdout, stderr = first.communicate(timeout=5)
        self.assertEqual(first.returncode, 0, stdout + stderr)
        self.wait_for_markers([two])
        release_two.touch()
        stdout, stderr = second.communicate(timeout=5)
        self.assertEqual(second.returncode, 0, stdout + stderr)

    def test_cas_4cb9_light_command_passes_waiting_browser_at_low_budget(self):
        low = dict(HIGH, budget_bytes=8*GIB)
        first, one, release_one = self.start_counted_command('playwright', 'first', low)
        self.wait_for_markers([one])
        second, two, release_two = self.start_counted_command('playwright', 'second', low)
        light, typecheck, release_light = self.start_counted_command('tsc', 'typecheck', low)
        self.wait_for_markers([one, typecheck])
        self.assertFalse(two.exists(), 'waiting browser consumed light-command capacity')
        release_light.touch()
        stdout, stderr = light.communicate(timeout=5)
        self.assertEqual(light.returncode, 0, stdout + stderr)
        release_one.touch()
        stdout, stderr = first.communicate(timeout=5)
        self.assertEqual(first.returncode, 0, stdout + stderr)
        self.wait_for_markers([two])
        release_two.touch()
        stdout, stderr = second.communicate(timeout=5)
        self.assertEqual(second.returncode, 0, stdout + stderr)

    def test_cas_4cb9_weights_follow_shell_and_actual_package_scripts(self):
        cases = [
            (['tsc', '--noEmit'], 1), (['vite', 'build'], 1),
            (worker.constrained(['npx', 'vitest', 'run']), 2),
            (['sh', '-c', 'tsc --noEmit && tsc --noEmit -p e2e'], 1),
            (['bash', '-c', 'cd hub-web && npm run typecheck > /tmp/log 2>&1'], 1),
            (['bash', '-c', 'cd hub-web && npm run build'], 1),
            (['bash', '-c', 'cd hub-web && npm test'], 2),
            (['bash', '-c', 'vitest run --maxWorkers=2'], 2),
            (['bash', '-c', 'tsc && playwright test'], 4),
            (['bash', '-c', 'tsc & vite build'], 4),
            (['bash', '-c', 'tsc && "$COMMAND"'], 4),
            (['bash', '-c', 'vitest run --maxWorkers=2 --maxWorkers=8'], 4),
            (['vitest', 'run', '--maxWorkers=2', '--browser'], 4),
            (['tsc', '--watch'], 4), (['npm', 'run', 'unknown'], 4),
        ]
        for command, weight in cases:
            with self.subTest(command=command):
                self.assertEqual(worker.estimate(command, ROOT), weight * GIB)

    def test_cas_4cb9_package_entrypoints_have_weighted_admission(self):
        commands = json.loads((ROOT/'hub-web/package.json').read_text())['scripts']
        for name, weight in [('build', 1), ('typecheck', 1), ('visual-qa', 4)]:
            self.assertEqual(worker.estimate(['sh', '-c', commands[name]], ROOT/'hub-web'), weight*GIB)

    def test_cas_4cb9_legacy_proof_and_worker_exclude_new_workers(self):
        host.private_directory(self.pool)
        for role, mode in [('proof', fcntl.LOCK_SH), ('worker', fcntl.LOCK_EX)]:
            with self.subTest(role=role), host.private_file(self.pool/'budget.lock') as budget:
                fcntl.flock(budget, mode)
                for applicant in ('proof', 'worker'):
                    with self.assertRaisesRegex(ValueError, 'deadline expired'):
                        with self.admit(applicant): self.fail('admitted over legacy lease')

    def test_cas_4cb9_legacy_proof_intent_waits_for_new_worker(self):
        with self.admit('worker'), host.private_file(self.pool/'intent.lock') as intent:
            with self.assertRaises(BlockingIOError):
                fcntl.flock(intent, fcntl.LOCK_EX | fcntl.LOCK_NB)

    def test_cas_4cb9_nested_admission_passes_live_slot_descriptors(self):
        with self.admit('worker') as (env, fds):
            self.assertTrue(host.inherited(env, self.pool))
            with host.admission('worker', env, lambda _: self.fail('nested resampled'),
                                directory=self.pool) as (_, nested_fds):
                self.assertEqual(nested_fds, fds)
            record = json.loads(env[host.LEASE_ENV])
            with host.private_file(self.pool/record['slot'], False) as probe:
                with self.assertRaises(BlockingIOError):
                    fcntl.flock(probe, fcntl.LOCK_EX | fcntl.LOCK_NB)

    def test_cas_4cb9_node_style_nested_spawn_can_reuse_live_ancestor(self):
        with self.admit('worker') as (env, _):
            program = f'''import sys,pathlib,json
sys.path.insert(0,{str(ROOT/'scripts')!r})
import host_memory as host
with host.admission('worker',dict(__import__('os').environ),lambda env: (_ for _ in ()).throw(AssertionError('resampled')),directory=pathlib.Path({str(self.pool)!r})) as (env,fds):
    assert fds == (), fds
'''
            child = subprocess.run([sys.executable, '-c', program], env=env, capture_output=True, text=True)
            self.assertEqual(child.returncode, 0, child.stdout + child.stderr)

    def test_cas_7b7b9_assembly_rows_get_no_host_descriptors_and_holder_tracks_them(self):
        # Replaces the 4cb9 contract that merged intent/budget into the rows'
        # inherited leases: rows now get none, the lock stays held, and every
        # row's process group is tracked by the proof's lease holder.
        tracked = []
        def inspect(root, clone, env, *args):
            record = json.loads(env[host.LEASE_ENV])
            self.assertTrue(set(record['fds']).isdisjoint(worker.proof.release_scratch.inherited_leases(env)))
            with host.private_file(self.pool/'intent.lock') as probe:
                with self.assertRaises(BlockingIOError):
                    fcntl.flock(probe, fcntl.LOCK_SH | fcntl.LOCK_NB)
            self.assertEqual(len(worker.proof.release_scratch.CURRENT.spawn_hooks), 1)
            tracked.append(worker.proof.release_scratch.CURRENT.spawn_hooks[0])
            return 'ran'
        with mock.patch.object(worker.proof, 'HOST_MEMORY_DIRECTORY', self.pool), \
             mock.patch.object(worker.proof, '_run_contexts', side_effect=inspect), \
             worker.proof.release_scratch.ChildScope() as scope:
            self.assertEqual(worker.proof.run_contexts(self.root, self.root, self.env, self.root, self.root, {}), 'ran')
            self.assertEqual(scope.spawn_hooks, [])
        self.assertEqual(len(tracked), 1)
        with self.admit('proof'): pass  # released on a normal return

    def daemonizing_program(self, pidfile):
        # Double fork + setsid: the grandchild leaves the command's process
        # group and session, like an sccache server, and outlives the command.
        return f'''import os,pathlib,time
if os.fork() == 0:
    os.setsid()
    if os.fork() == 0:
        pathlib.Path({str(pidfile)!r}).write_text(str(os.getpid()))
        time.sleep(30)
    os._exit(0)
os.wait()
while not pathlib.Path({str(pidfile)!r}).exists(): time.sleep(.01)
'''

    def held_pool_locks(self, pid):
        names = set()
        for descriptor in Path(f'/proc/{pid}/fd').iterdir():
            try:
                target = os.readlink(descriptor)
            except OSError:
                continue
            if target.startswith(str(self.pool)):
                names.add(Path(target).name)
        return names

    def kill_pidfile(self, pidfile):
        if pidfile.exists():
            try:
                os.kill(int(pidfile.read_text()), 9)
            except ProcessLookupError:
                pass

    @unittest.skipUnless(Path('/proc/self/fd').is_dir(), 'needs /proc')
    def test_cas_7b7b9_daemonized_child_does_not_keep_the_lease(self):
        pidfile = self.root / 'daemon-pid'
        self.addCleanup(self.kill_pidfile, pidfile)
        with mock.patch.object(worker.proof, 'memory_budget', return_value=HIGH), \
             mock.patch.object(worker.proof, 'positive_knob', return_value=None):
            status = worker.run([sys.executable, '-c', self.daemonizing_program(pidfile)],
                                env=self.env, directory=self.pool)
        self.assertEqual(status, 0)
        daemon = int(pidfile.read_text())
        os.kill(daemon, 0)  # still running after its command and wrapper finished
        self.assertEqual(self.held_pool_locks(daemon), set(), 'the daemon inherited admission descriptors')
        with self.admit('proof'): pass  # nothing holds the budget any more
        with self.admit('worker'):
            self.assertEqual(self.events[-1]['reserved_bytes'], 0)

    @unittest.skipUnless(Path('/proc/self/fd').is_dir(), 'needs /proc')
    def test_cas_7b7b9_killed_wrapper_holds_until_its_command_group_ends(self):
        marker, release = self.root/'cmd-started', self.root/'cmd-release'
        program = (f'import pathlib,time;pathlib.Path({str(marker)!r}).touch()\n'
                   f'while not pathlib.Path({str(release)!r}).exists(): time.sleep(.01)')
        launcher = (f"import sys,pathlib;sys.path.insert(0,{str(ROOT/'scripts')!r});"
                    f"import importlib.util;spec=importlib.util.spec_from_file_location('worker',{str(ROOT/'scripts/worker-memory.py')!r});"
                    "m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);"
                    f"m.proof.memory_budget=lambda env:{HIGH!r};"
                    f"sys.exit(m.run([sys.executable,'-c',{program!r}],directory=pathlib.Path({str(self.pool)!r})))")
        # stderr is not a pipe: the orphaned command keeps its copy open.
        wrapper = subprocess.Popen([sys.executable, '-c', launcher], env=self.env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.addCleanup(release.touch)
        try:
            self.wait_for_markers([marker])
            wrapper.kill()  # SIGKILL: no teardown, the command keeps running
            wrapper.wait(timeout=3)
            with self.assertRaisesRegex(ValueError, 'deadline expired'):
                with self.admit('proof'): self.fail('proof admitted over a killed wrapper\'s running command')
            release.touch()
            deadline = time.monotonic() + 5
            while True:
                try:
                    with self.admit('proof'): break
                except ValueError:
                    self.assertLess(time.monotonic(), deadline, 'holder outlived its command group')
        finally:
            release.touch()
            if wrapper.poll() is None: wrapper.kill()
            wrapper.wait(timeout=3)

    @unittest.skipUnless(Path('/proc/self/fdinfo').is_dir(), 'needs /proc fdinfo')
    def test_cas_7b7b9_an_escaped_inheritor_is_named_not_blamed_on_a_suite(self):
        # The incident shape: an older wrapper passed its descriptors to the
        # command, the command started a daemon, and the wrapper exited.
        pidfile = self.root / 'escaped-pid'
        self.addCleanup(self.kill_pidfile, pidfile)
        launcher = f'''import os,sys,pathlib,subprocess
sys.path.insert(0,{str(ROOT/'scripts')!r})
import host_memory as host
with host.admission('worker',dict(os.environ),lambda env:{HIGH!r},directory=pathlib.Path({str(self.pool)!r})) as (env,fds):
    subprocess.run([sys.executable,'-c',{self.daemonizing_program(pidfile)!r}],env=env,pass_fds=fds,check=True)
'''
        subprocess.run([sys.executable, '-c', launcher], env=self.env, check=True, timeout=10)
        daemon = int(pidfile.read_text())
        self.assertIn('slot-0.lock', self.held_pool_locks(daemon))
        with self.assertRaises(ValueError) as refused:
            with self.admit('proof'): self.fail('admitted over an escaped holder')
        self.assertIn(f'pid {daemon}', str(refused.exception))
        self.assertIn('not an admitted suite', str(refused.exception))
        self.assertNotIn('worker suite running', str(refused.exception))
        os.kill(daemon, 9)
        deadline = time.monotonic() + 2
        while Path(f'/proc/{daemon}').exists():
            self.assertLess(time.monotonic(), deadline)
            time.sleep(.01)
        with self.admit('proof'): pass

    def test_cas_7b7b9_compiler_cache_server_starts_outside_the_lease(self):
        self.assert_compiler_cache_starts_outside_lease()

    def test_compiler_cache_preserves_release_train_environment(self):
        self.env.update(zip(TRAIN_ENV_KEYS, ('cut', str(self.root / 'train'), 'gate')))
        self.assert_compiler_cache_starts_outside_lease()

    def assert_compiler_cache_starts_outside_lease(self):
        calls = self.root / 'calls'
        fake = self.root / 'bin' / 'sccache'
        fake.parent.mkdir()
        fake.write_text('#!' + sys.executable + '\nimport json,os,sys\n'
                        f'lease_keys = {host.LEASE_ENV_KEYS!r}\n'
                        f'train_keys = {TRAIN_ENV_KEYS!r}\n'
                        f'open({str(calls)!r},"a").write(json.dumps([sys.argv[1:], sorted(k for k in os.environ if k in lease_keys), [os.readlink("/proc/self/fd/"+n) for n in os.listdir("/proc/self/fd") if os.path.exists("/proc/self/fd/"+n)] if os.path.isdir("/proc/self/fd") else [], {{k: os.environ[k] for k in train_keys if k in os.environ}}])+"\\n")\n'
                        'sys.exit(2)\n')
        fake.chmod(0o755)
        lease = os.open(self.root / 'lease', os.O_RDWR | os.O_CREAT, 0o600)
        self.addCleanup(os.close, lease)
        os.set_inheritable(lease, True)  # what an inherited lease looks like
        env = dict(self.env, RUSTC_WRAPPER=str(fake), **{host.LEASE_ENV: '{}',
                   'CAS_RELEASE_GATE_SCRATCH_LEASE_FDS': str(lease)})
        self.assertTrue(host.start_compiler_cache(env))  # "Address in use" (exit 2) is fine
        argv, lease_keys, descriptors, train_env = json.loads(calls.read_text().splitlines()[0])
        self.assertEqual(argv, ['--start-server'])
        self.assertEqual(lease_keys, [])
        self.assertNotIn(str(self.root / 'lease'), descriptors)
        self.assertEqual(train_env, {k: self.env[k] for k in TRAIN_ENV_KEYS if k in self.env})
        self.assertFalse(host.start_compiler_cache(dict(self.env, RUSTC_WRAPPER='/usr/bin/ccache')))
        self.assertFalse(host.start_compiler_cache({k: v for k, v in self.env.items()
                                                    if k not in ('RUSTC_WRAPPER', 'CARGO_BUILD_RUSTC_WRAPPER')}))

    def test_cas_4cb9_slot_symlink_fails_closed(self):
        host.private_directory(self.pool)
        (self.root/'target').write_text('')
        (self.pool/'slot-0.lock').symlink_to(self.root/'target')
        with self.assertRaises(OSError):
            with self.admit('worker'): self.fail('unsafe slot admitted')

    def test_cas_4cb9_proof_window_excludes_another_proof(self):
        with self.admit('proof'):
            with self.assertRaisesRegex(ValueError, 'deadline expired'):
                with self.admit('proof'): self.fail('parallel proof admitted')

    def test_cas_4cb9_orphan_child_keeps_weight_and_proof_exclusion(self):
        marker, release = self.root/'orphan-started', self.root/'orphan-release'
        program = ('import pathlib,time;pathlib.Path(' + repr(str(marker)) + ').touch();'
                   '\nwhile not pathlib.Path(' + repr(str(release)) + ').exists(): time.sleep(.01)')
        launcher = f'''import os,sys,pathlib,subprocess,time
sys.path.insert(0,{str(ROOT/'scripts')!r})
import host_memory as host
with host.admission('worker',dict(os.environ),lambda env:{HIGH!r},directory=pathlib.Path({str(self.pool)!r})) as (env,fds):
    subprocess.Popen([sys.executable,'-c',{program!r}],env=env,pass_fds=fds)
    while not pathlib.Path({str(marker)!r}).exists(): time.sleep(.01)
    os._exit(0)
'''
        wrapper = subprocess.Popen([sys.executable, '-c', launcher], env=self.env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            self.wait_for_markers([marker])
            self.assertEqual(wrapper.wait(timeout=3), 0)
            low = dict(HIGH, budget_bytes=8*GIB)
            with self.assertRaisesRegex(ValueError, 'deadline expired'):
                with host.admission('worker', self.env, lambda _: low, directory=self.pool,
                                    wait_secs=.1, poll_secs=.01, report=self.events.append):
                    self.fail('orphan reservation lost')
            with self.admit('worker', estimate_bytes=GIB):
                self.assertEqual(self.events[-1]['reserved_bytes'], 4*GIB)
            with self.assertRaisesRegex(ValueError, 'deadline expired'):
                with self.admit('proof'): self.fail('proof started over orphan')
        finally:
            release.touch()
            if wrapper.poll() is None: wrapper.kill()
            wrapper.communicate(timeout=3)
        with self.admit('proof'): pass
        with self.admit('worker'):
            self.assertEqual(self.events[-1]['reserved_bytes'], 0)

    def wait_for_proof(self, child, marker):
        # Node/Python startup can exceed a fixed sleep under assembly load.
        # Keep the proof lease until the real admission wait is observable.
        lines = []
        deadline = time.monotonic() + 3
        while True:
            self.assertFalse(marker.exists(), 'worker native suite started while proof held reserve')
            self.assertIsNone(child.poll(), 'worker exited before reporting its admission wait')
            self.assertLess(time.monotonic(), deadline, 'worker admission wait was not reported')
            readable, _, _ = select.select([child.stdout], [], [], .02)
            if readable:
                line = child.stdout.readline()
                lines.append(line)
                if 'waiting for host memory (proof running)' in line:
                    return ''.join(lines)

    def test_frontend_runner_waits_for_proof_and_reports_wait(self):
        # Execute the actual frontend entry point with one fake native runner.
        # Relocate only the lease directory to avoid touching a production proof.
        scripts = self.root / 'scripts'
        scripts.mkdir()
        for name in ['host_memory.py', 'worker-memory.py', 'assembly-proof.py', 'release_scratch.py', 'proof_target.py']:
            shutil.copy(ROOT / 'scripts' / name, scripts / name)
        (scripts / 'host_memory.py').write_text((scripts / 'host_memory.py').read_text().replace(
            "DIRECTORY = Path('/var/tmp') / f'cas-host-memory-{os.getuid()}'", f'DIRECTORY = Path({str(self.pool)!r})'))
        runner = self.root / 'hub/scripts/run-verified-tests.mjs'
        runner.parent.mkdir(parents=True)
        body = subprocess.check_output(['git', 'show', 'aa6418b37:hub-web/scripts/run-verified-tests.mjs'], cwd=ROOT, text=True) if BASELINE else (ROOT / 'hub-web/scripts/run-verified-tests.mjs').read_text()
        runner.write_text(body)
        marker = self.root / 'started'
        fake = self.root / 'hub/node_modules/vitest/vitest.mjs'
        fake.parent.mkdir(parents=True)
        fake.write_text("import {writeFileSync} from 'node:fs'; writeFileSync(" + json.dumps(str(marker)) + ", 'started'); const file=process.argv.find(a=>a.startsWith('--outputFile=')).slice(13); writeFileSync(file, JSON.stringify({numPassedTests:1}));")
        env = dict(self.env, CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS='3')
        with self.admit('proof'):
            child = subprocess.Popen(['node', str(runner), 'vitest'], cwd=self.root/'hub', env=env,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                prefix = self.wait_for_proof(child, marker)
            except BaseException:
                child.kill()
                child.communicate(timeout=3)
                raise
        stdout, stderr = child.communicate(timeout=5)
        stdout = prefix + stdout
        self.assertEqual(child.returncode, 0, stdout + stderr)
        self.assertIn('waiting for host memory (proof running)', stdout)
        self.assertIn('verified-web-tests: PASS (1 vitest tests passed)', stdout)
        self.assertTrue(marker.exists())

    def test_package_build_typecheck_and_visual_qa_wait_at_actual_entrypoints(self):
        scripts = self.root / 'scripts'
        scripts.mkdir()
        for name in ['host_memory.py', 'worker-memory.py', 'assembly-proof.py', 'release_scratch.py', 'proof_target.py']:
            shutil.copy(ROOT / 'scripts' / name, scripts / name)
        (scripts / 'host_memory.py').write_text((scripts / 'host_memory.py').read_text().replace(
            "DIRECTORY = Path('/var/tmp') / f'cas-host-memory-{os.getuid()}'", f'DIRECTORY = Path({str(self.pool)!r})'))
        hub = self.root / 'hub'
        hub.mkdir()
        binaries = self.root / 'bin'
        binaries.mkdir()
        marker = self.root / 'started'
        for name in ['npm', 'vite', 'tsc']:
            stub = binaries / name
            stub.write_text('#!' + sys.executable + "\nimport pathlib;pathlib.Path(" + repr(str(marker)) + ").write_text('started')\n")
            stub.chmod(0o755)
        env = dict(self.env, PATH=str(binaries)+os.pathsep+self.env['PATH'])
        commands = json.loads((ROOT/'hub-web/package.json').read_text())['scripts']
        for name in ['build', 'typecheck', 'visual-qa']:
            with self.subTest(name=name):
                marker.unlink(missing_ok=True)
                with self.admit('proof'):
                    child = subprocess.Popen(['sh', '-c', commands[name]], cwd=hub, env=env,
                                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                    try:
                        prefix = self.wait_for_proof(child, marker)
                    except BaseException:
                        child.kill()
                        child.communicate(timeout=3)
                        raise
                stdout, stderr = child.communicate(timeout=5)
                stdout = prefix + stdout
                self.assertEqual(child.returncode, 0, stdout+stderr)
                self.assertIn('waiting for host memory (proof running)', stdout)
                self.assertTrue(marker.exists())

    def test_worker_wait_has_deadline_without_starting_command(self):
        with self.admit('proof'):
            result = []
            def waiter():
                try:
                    with self.admit('worker'):
                        result.append('started')
                except ValueError as exc:
                    result.append(str(exc))
            thread = threading.Thread(target=waiter)
            thread.start()
            thread.join(timeout=2)
            self.assertFalse(thread.is_alive())
        self.assertRegex(result[0], 'deadline expired.*command was not started')
        self.assertNotIn('started', result)
        self.assertTrue(any(e['reason'] == 'proof running' for e in self.events))

    def test_low_budget_waits_then_admits_after_fresh_sample(self):
        snapshots = iter([dict(HIGH, budget_bytes=3*GIB), HIGH])
        with host.admission('worker', self.env, lambda _: next(snapshots), directory=self.pool,
                            wait_secs=.5, poll_secs=.01, report=self.events.append):
            pass
        self.assertEqual([e['reason'] for e in self.events],
                         ['worker suite estimate + headroom exceeds fresh memory budget', 'admitted'])

    def test_proof_priority_blocks_a_second_worker(self):
        order = []
        with self.admit('worker'):
            def proof_waiter():
                with self.admit('proof'):
                    order.append('proof')
                    time.sleep(.08)
            proof_thread = threading.Thread(target=proof_waiter)
            proof_thread.start()
            deadline = time.monotonic() + 1
            while not self.events or not any(e['role'] == 'proof' for e in self.events):
                self.assertLess(time.monotonic(), deadline)
                time.sleep(.01)
            def worker_waiter():
                with self.admit('worker'):
                    order.append('worker')
            worker_thread = threading.Thread(target=worker_waiter)
            worker_thread.start()
            time.sleep(.05)
        for thread in (proof_thread, worker_thread):
            thread.join(timeout=2)
            self.assertFalse(thread.is_alive())
        self.assertEqual(order, ['proof', 'worker'])

    def test_stale_or_unrelated_inherited_admission_does_not_skip(self):
        with self.admit('proof') as (env, _):
            self.assertTrue(host.inherited(env, self.pool))
            self.assertFalse(host.inherited(dict(env, **{host.LEASE_ENV: 'garbage'}), self.pool))
            unrelated = dict(env)
            with mock.patch.object(host.os, 'getpid', return_value=123456), mock.patch.object(host, 'parent_pid', return_value=1):
                self.assertFalse(host.inherited(unrelated, self.pool))
        self.assertFalse(host.inherited(env, self.pool))

    def test_symlink_and_permissive_lease_files_fail_closed(self):
        target = self.root/'target'
        target.mkdir(mode=0o700)
        self.pool.symlink_to(target)
        with self.assertRaisesRegex(ValueError, 'unsafe.*directory'):
            with self.admit('worker'): pass
        self.pool.unlink()
        host.private_directory(self.pool)
        lock = self.pool/'budget.lock'
        lock.write_text('')
        lock.chmod(0o644)
        with self.assertRaisesRegex(ValueError, 'unsafe.*file'):
            with self.admit('worker'): pass

    def test_wait_is_visible_in_plain_language(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            host.default_report({'role':'worker', 'reason':'proof running', 'elapsed_s':2})
        self.assertIn('waiting for host memory (proof running), 2 s', output.getvalue())

    def test_direct_runner_limits_are_authoritative(self):
        self.assertEqual(worker.constrained(['npx','playwright','test','--workers','20']), ['npx','playwright','test','--workers=4'])
        self.assertEqual(worker.constrained(['npx','playwright','test','--workers=4','--workers=3']), ['npx','playwright','test','--workers=3'])
        self.assertEqual(worker.constrained(['npx','playwright','test']), ['npx','playwright','test','--workers=1'])
        self.assertEqual(worker.constrained(['npx','playwright','test','--workers=50%']), ['npx','playwright','test','--workers=1'])
        self.assertEqual(worker.constrained(['node','/pkg/vitest/vitest.mjs','run','--maxWorkers=50']), ['node','/pkg/vitest/vitest.mjs','run','--maxWorkers=2'])

    def test_signal_teardown_releases_worker_lease_and_reaps_owned_child(self):
        pidfile = self.root / 'child-pid'
        program = "import os,pathlib,time;pathlib.Path(" + repr(str(pidfile)) + ").write_text(str(os.getpid()));time.sleep(30)"
        launcher = "import sys;sys.path.insert(0," + repr(str(ROOT/'scripts')) + ");import importlib.util,pathlib;spec=importlib.util.spec_from_file_location('worker'," + repr(str(ROOT/'scripts/worker-memory.py')) + ");m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);m.proof.memory_budget=lambda env:" + repr(HIGH) + ";sys.exit(m.run([sys.executable,'-c'," + repr(program) + "],directory=pathlib.Path(" + repr(str(self.pool)) + ")))"
        child = subprocess.Popen([sys.executable, '-c', launcher], env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 2
            while not pidfile.exists():
                self.assertLess(time.monotonic(), deadline)
                time.sleep(.01)
            child.terminate()
            child.communicate(timeout=3)
            self.assertNotEqual(child.returncode, 0)
            with self.assertRaises(ProcessLookupError):
                os.kill(int(pidfile.read_text()), 0)
            with self.admit('worker'): pass
        finally:
            if child.poll() is None: child.kill()
            child.communicate(timeout=3)

    def test_memory_pressure_aborts_only_owned_child_group(self):
        snapshots = iter([HIGH, dict(HIGH, budget_bytes=GIB)])
        with mock.patch.object(worker.proof, 'memory_budget', side_effect=lambda _: next(snapshots)), \
             mock.patch.object(worker.proof, 'positive_knob', return_value=None), \
             self.assertRaisesRegex(ValueError, 'memory headroom'):
            worker.run([sys.executable, '-c', 'import time;time.sleep(30)'], env=self.env, directory=self.pool)
        with self.admit('worker'): pass  # aborted command did not strand the lease

    def test_cas_04ebf_background_jobs_are_named_not_cut_down(self):
        # The cas-7c94 command: the shell returned, run() ended its process
        # group, and the npm receipt launcher died under a detached runner.
        refused = [
            'env JOURNEY_OUTPUT=out nohup npm --prefix hub-web run journeys -- a.journey.ts > run.log 2>&1 < /dev/null & sleep 1',
            'tsc & vite build', 'cd hub-web && npm run build &', 'npm test&',
        ]
        allowed = [
            'cd hub-web && npm run typecheck > /tmp/log 2>&1', 'npm run build && npm test',
            "echo 'a & b'", 'npm run journeys |& tee log', 'npm test &>log', 'npm test &>>log',
            'npm test 2>&1 | tee log', 'tsc || true',
        ]
        for command in refused:
            with self.subTest(refused=command):
                self.assertTrue(worker.background_job(command))
        for command in allowed:
            with self.subTest(allowed=command):
                self.assertFalse(worker.background_job(command))
        # End to end: refused before admission, nothing runs, and it says why.
        marker = self.root / 'ran'
        result = subprocess.run(
            [sys.executable, str(ROOT / 'scripts/worker-memory.py'), '--shell-command', f'touch {marker} & true'],
            env=self.env, capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn('background job (&)', result.stderr)
        self.assertIn('foreground of a persistent session', result.stderr)
        time.sleep(.2)
        self.assertFalse(marker.exists(), 'a refused command must not start')


if __name__ == '__main__':
    unittest.main()
