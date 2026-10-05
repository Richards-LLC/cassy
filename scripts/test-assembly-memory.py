#!/usr/bin/env python3
"""Assembly memory guard contracts; real child groups, no Cargo or Rust."""
import importlib.util
import itertools
import json
import os
from pathlib import Path
import subprocess
import signal
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location("assembly_memory", Path(__file__).with_name("assembly-memory.py"))
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


class GuardTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        guard.proof.git(self.root, "init", "-q")
        self.events = self.root / "events.jsonl"
        self.env = {"PATH": os.environ["PATH"], "CARGO_HOME": str(self.root / "cargo-home")}
        self.high = {"total_bytes": 64 * guard.proof.GIB, "available_bytes": 60 * guard.proof.GIB, "source": "fixture"}

    def test_native_flags_and_linker_overrides_survive_wrapping(self):
        config = self.root / ".cargo/config.toml"
        config.parent.mkdir()
        config.write_text('[target.fixture-host]\nlinker="gcc"\nrustflags=["-C", "target-cpu=x86-64", "-C", "link-arg=-fuse-ld=mold"]\n')
        env = dict(self.env, CARGO_BUILD_TARGET="fixture-host")
        linker, flags = guard.native_configuration(self.root, env)
        self.assertEqual(linker, "gcc")
        self.assertIn("link-arg=-fuse-ld=mold", flags)
        self.assertIn("target-cpu=x86-64", flags)
        self.assertEqual(guard.native_configuration(self.root, dict(env, CARGO_TARGET_FIXTURE_HOST_LINKER="clang"))[0], "clang")
        self.assertEqual(guard.native_configuration(self.root, dict(env, RUSTFLAGS="-C linker=custom-linker")),
                         ("custom-linker", ["-C", "linker=custom-linker"]))
        self.assertEqual(guard.native_configuration(self.root, dict(env, CARGO_ENCODED_RUSTFLAGS=""))[1], [])

    def test_stable_wrapper_path_is_independent_of_clone_path(self):
        env = dict(self.env, CAS_RELEASE_GATE_ASSEMBLY_LINK_GUARD_DIR=str(self.root / "shared"))
        first = guard.stable_wrapper(self.root, env)
        second = guard.stable_wrapper(self.root / "random-clone", env)
        self.assertEqual(first, second)
        self.assertTrue(os.access(first, os.X_OK))

    def test_compile_pauses_actual_group_then_resumes(self):
        low = dict(self.high, available_bytes=17 * guard.proof.GIB)
        pidfile = self.root / "pid"
        program = 'import os,pathlib,time; pathlib.Path('+repr(str(pidfile))+').write_text(str(os.getpid())); time.sleep(.35)'
        calls = []
        def snapshot():
            calls.append(len(calls))
            if len(calls) == 4:
                state = subprocess.check_output(["ps", "-o", "state=", "-p", pidfile.read_text()], text=True)
                self.assertTrue(state.strip().startswith("T"), state)
            return low if len(calls) in (3, 4) else self.high
        with mock.patch.dict(os.environ, self.env, clear=True), \
                mock.patch.object(guard.proof, "memory_snapshot", side_effect=snapshot), \
                mock.patch.object(guard, "poll", return_value=.1):
            status = guard.compile_guard([sys.executable, "-c", program], '{}', self.events, self.root)
        self.assertEqual(status, 0)
        actions = [json.loads(line).get("action") for line in self.events.read_text().splitlines()]
        self.assertIn("pause", actions)
        self.assertIn("resume", actions)

    def test_reserve_breach_aborts_and_reaps_actual_child(self):
        low = dict(self.high, available_bytes=15 * guard.proof.GIB)
        with mock.patch.dict(os.environ, self.env, clear=True), \
                mock.patch.object(guard.proof, "memory_snapshot", side_effect=[self.high, low]), \
                self.assertRaisesRegex(ValueError, "breached memory reserve"):
            guard.compile_guard([sys.executable, "-c", "import time;time.sleep(60)"], '{}', self.events, self.root)
        self.assertEqual(json.loads(self.events.read_text().splitlines()[-1])["action"], "reserve-breached-abort")

    def test_compile_pause_has_a_deadline_and_resumes_before_teardown(self):
        low = dict(self.high, available_bytes=17 * guard.proof.GIB)
        with mock.patch.dict(os.environ, self.env, clear=True), \
                mock.patch.object(guard.proof, "memory_snapshot", side_effect=itertools.chain([self.high], itertools.repeat(low))), \
                mock.patch.object(guard.time, "monotonic", side_effect=itertools.count(0, 2)), \
                mock.patch.object(guard, "deadline", return_value=1), \
                self.assertRaisesRegex(ValueError, "recovery deadline expired"):
            guard.compile_guard([sys.executable, "-c", "import time;time.sleep(60)"], '{}', self.events, self.root)
        self.assertEqual(json.loads(self.events.read_text().splitlines()[-1])["action"], "memory-pause-deadline-abort")

    def test_compile_child_inherits_scratch_lease_cas_72f4(self):
        lease = self.root / 'lease.lock'
        result = self.root / 'inherited'
        with lease.open('a+') as stream:
            env = dict(self.env, CAS_RELEASE_GATE_SCRATCH_LEASE_FDS=str(stream.fileno()))
            program = 'import os,pathlib;os.fstat('+str(stream.fileno())+');pathlib.Path('+repr(str(result))+').touch()'
            with mock.patch.dict(os.environ, env, clear=True), \
                    mock.patch.object(guard.proof, 'memory_snapshot', return_value=self.high):
                self.assertEqual(guard.compile_guard([sys.executable, '-c', program], '{}', self.events, self.root), 0)
        self.assertTrue(result.exists())

    def test_signal_during_compile_spawn_reaps_child_cas_72f4(self):
        original = guard.subprocess.Popen
        spawned = []
        def interrupt(command, **kwargs):
            child = original(command, **kwargs)
            spawned.append(child.pid)
            os.kill(os.getpid(), signal.SIGTERM)
            return child
        with mock.patch.dict(os.environ, self.env, clear=True), \
                mock.patch.object(guard.proof, 'memory_snapshot', return_value=self.high), \
                mock.patch.object(guard, 'native_configuration', return_value=('cc', [])), \
                mock.patch.object(guard, 'stable_wrapper', return_value=self.root / 'unused-wrapper'), \
                mock.patch.object(guard.subprocess, 'Popen', side_effect=interrupt), \
                self.assertRaises(InterruptedError):
            guard.compile_guard([sys.executable, '-c', 'import time;time.sleep(60)'], '{}', self.events, self.root)
        with self.assertRaises(ProcessLookupError):
            os.kill(spawned[0], 0)

    def test_sigterm_reaps_the_compile_child(self):
        pidfile = self.root / "pid"
        program = 'import os,pathlib,time; pathlib.Path('+repr(str(pidfile))+').write_text(str(os.getpid())); time.sleep(60)'
        timer = threading.Timer(.2, lambda: os.kill(os.getpid(), signal.SIGTERM))
        try:
            with mock.patch.dict(os.environ, self.env, clear=True), \
                    mock.patch.object(guard.proof, "memory_snapshot", return_value=self.high), \
                    self.assertRaisesRegex(InterruptedError, "SIGTERM"):
                timer.start()
                guard.compile_guard([sys.executable, "-c", program], '{}', self.events, self.root)
            with self.assertRaises(ProcessLookupError):
                os.kill(int(pidfile.read_text()), 0)
        finally:
            timer.cancel()
            timer.join()

    def test_cross_producer_slot_serializes_children_and_records_rss(self):
        program = r'''
import importlib.util, sys, os
from pathlib import Path
spec=importlib.util.spec_from_file_location('memory', sys.argv[1]); m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
m.LINK_LEASE_ROOT=Path(os.environ['TMPDIR'])
m.proof.memory_snapshot=lambda: {'total_bytes':64*m.proof.GIB,'available_bytes':60*m.proof.GIB,'source':'fixture'}
sys.exit(m.link([sys.executable, '-c', "import pathlib,time; p=pathlib.Path("+repr(sys.argv[2])+"); f=p.open('a');f.write(str(time.monotonic())+' start\\n');f.flush();time.sleep(.15);f.write(str(time.monotonic())+' end\\n');f.close() "]))
'''
        env = dict(self.env, TMPDIR=str(self.root), CAS_RELEASE_GATE_ASSEMBLY_LINK_RSS_LOG=str(self.events))
        env["CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLICY"] = json.dumps({"CAS_RELEASE_GATE_ASSEMBLY_LINK_JOBS": "1"})
        log = self.root / "link-times"
        command = [sys.executable, "-c", program, str(Path(guard.__file__).resolve()), str(log)]
        children = [subprocess.Popen(command, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE) for _ in range(2)]
        try:
            for child in children:
                stdout, stderr = child.communicate(timeout=10)
                self.assertEqual(child.returncode, 0, stderr.decode())
        finally:
            for child in children:
                if child.poll() is None:
                    child.kill()
                child.communicate(timeout=10)
        self.assertEqual([line.split()[1] for line in log.read_text().splitlines()], ["start", "end", "start", "end"])
        completed = [item for item in map(json.loads, self.events.read_text().splitlines()) if item["phase"] == "link-complete"]
        self.assertEqual(len(completed), 2)
        self.assertTrue(all(item["peak_waited_driver_rss_bytes"] > 0 and not item["estimate_exceeded"] for item in completed))
        self.assertTrue(all("excludes mold workers" in item["rss_scope"] for item in completed))
        self.assertTrue(all("peak_child_rss_bytes" not in item for item in completed))

    def wait_until(self, predicate):
        until = time.monotonic() + 5
        while time.monotonic() < until:
            if predicate():
                return
            time.sleep(.01)
        self.fail("fixture condition did not arrive")

    def launch_link(self, name, available, maximum=2, wait=5):
        program = r'''import importlib.util, json, os, sys
from pathlib import Path
spec=importlib.util.spec_from_file_location('memory', sys.argv[1]);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
m.LINK_LEASE_ROOT=Path(sys.argv[2]);m.poll=lambda env:.02
m.proof.memory_snapshot=lambda: {'total_bytes':64*m.proof.GIB,'available_bytes':int(Path(sys.argv[3]).read_text()),'source':'fixture'}
child="import os,time;from pathlib import Path;root=Path("+repr(sys.argv[2])+");name="+repr(sys.argv[4])+"; (root/(name+'.started')).write_text(str(os.getpid()));\nwhile not (root/(name+'.release')).exists(): time.sleep(.01)"
sys.exit(m.link([sys.executable,'-c',child]))
'''
        scratch = self.root / (name + "-tmp")
        scratch.mkdir()
        env = dict(self.env, TMPDIR=str(scratch),
                   CAS_RELEASE_GATE_ASSEMBLY_LINK_RSS_LOG=str(self.events),
                   CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLICY=json.dumps({
                       "CAS_RELEASE_GATE_ASSEMBLY_LINK_JOBS": str(maximum),
                       "CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS": str(wait)}))
        process = subprocess.Popen([sys.executable, "-c", program, str(Path(guard.__file__).resolve()),
                                    str(self.root), str(available), name], env=env,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        def cleanup():
            (self.root / (name + ".release")).touch()
            if process.poll() is None:
                process.terminate()
            process.communicate(timeout=5)
            started = self.root / (name + ".started")
            if started.exists():
                try:
                    os.kill(int(started.read_text()), signal.SIGTERM)
                except ProcessLookupError:
                    pass
        self.addCleanup(cleanup)
        return process

    def finish_link(self, process, name):
        (self.root / (name + ".release")).touch()
        _, stderr = process.communicate(timeout=5)
        self.assertEqual(process.returncode, 0, stderr.decode())

    def test_parallel_children_share_cap_and_resample_after_shrink(self):
        available = self.root / "available"
        available.write_text(str(60 * guard.proof.GIB))
        first = self.launch_link("first", available)
        self.wait_until(lambda: (self.root / "first.started").exists())
        second = self.launch_link("second", available)
        self.wait_until(lambda: (self.root / "second.started").exists())
        # Both real children are live. Now capacity shrinks to 1, even though
        # slot 1 (above the new limit) remains owned after slot 0 exits.
        available.write_text(str(16 * guard.proof.GIB + guard.proof.GUARD_HEADROOM_BYTES + guard.proof.LINK_BYTES))
        third = self.launch_link("third", available, maximum=8)
        self.wait_until(lambda: self.events.exists() and 'live link capacity occupied' in self.events.read_text())
        self.finish_link(first, "first")
        time.sleep(.1)
        self.assertFalse((self.root / "third.started").exists())
        self.finish_link(second, "second")
        self.wait_until(lambda: (self.root / "third.started").exists())
        self.finish_link(third, "third")
        admitted = [event for event in map(json.loads, self.events.read_text().splitlines()) if event.get("admitted")]
        self.assertEqual([event["link_slots"] for event in admitted], [2, 2, 1])
        self.assertEqual([event["active_links"] for event in admitted], [0, 1, 0])
        self.assertEqual([event["slot_index"] for event in admitted], [0, 1, 0])

    def test_link_waits_for_memory_and_deadline_records_refusals(self):
        available = self.root / "available"
        available.write_text(str(17 * guard.proof.GIB))
        process = self.launch_link("recovered", available)
        self.wait_until(lambda: self.events.exists() and 'memory reserve/headroom' in self.events.read_text())
        self.assertFalse((self.root / "recovered.started").exists())
        available.write_text(str(60 * guard.proof.GIB))
        self.wait_until(lambda: (self.root / "recovered.started").exists())
        self.finish_link(process, "recovered")
        available.write_text(str(17 * guard.proof.GIB))
        expired = self.launch_link("expired", available, wait=1)
        _, stderr = expired.communicate(timeout=5)
        self.assertNotEqual(expired.returncode, 0)
        self.assertIn("deadline expired", stderr.decode())
        self.assertFalse((self.root / "expired.started").exists())

    def test_lease_survives_abrupt_wrapper_exit_until_linker_exits(self):
        available = self.root / "available"
        available.write_text(str(60 * guard.proof.GIB))
        first = self.launch_link("orphan", available, maximum=1)
        self.wait_until(lambda: (self.root / "orphan.started").exists())
        first.kill()
        first.wait(timeout=5)
        second = self.launch_link("waiting", available, maximum=1)
        self.wait_until(lambda: 'live link capacity occupied' in self.events.read_text())
        self.assertFalse((self.root / "waiting.started").exists())
        (self.root / "orphan.release").touch()
        self.wait_until(lambda: (self.root / "waiting.started").exists())
        self.finish_link(second, "waiting")
        first.communicate(timeout=5)

    def test_symlink_lease_fails_closed(self):
        directory = self.root / "pool"
        directory.mkdir()
        victim = self.root / "victim"
        victim.write_text("unchanged")
        (directory / "slot-0.lock").symlink_to(victim)
        with mock.patch.object(guard.proof, "memory_snapshot", return_value=self.high), self.assertRaises(OSError):
            guard.claim_slot(directory, {})
        self.assertEqual(victim.read_text(), "unchanged")

    def test_live_slot_budget_and_override(self):
        memory = {"budget_bytes": 32 * guard.proof.GIB}
        self.assertEqual(guard.link_capacity({}, memory), 8)
        self.assertEqual(guard.link_capacity({"CAS_RELEASE_GATE_ASSEMBLY_LINK_JOBS": "3"}, memory), 3)
        memory["budget_bytes"] = guard.proof.GUARD_HEADROOM_BYTES + 2 * guard.proof.LINK_BYTES
        self.assertEqual(guard.link_capacity({}, memory), 2)
        memory["budget_bytes"] = 0
        self.assertEqual(guard.link_capacity({}, memory), 1)
        for value in ("", "0", "-1", "auto"):
            with self.assertRaisesRegex(ValueError, "LINK_JOBS"):
                guard.link_capacity({"CAS_RELEASE_GATE_ASSEMBLY_LINK_JOBS": value}, memory)


if __name__ == "__main__":
    unittest.main()
