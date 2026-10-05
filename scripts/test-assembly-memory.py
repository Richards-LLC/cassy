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
import importlib.util, sys
spec=importlib.util.spec_from_file_location('memory', sys.argv[1]); m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
m.proof.memory_snapshot=lambda: {'total_bytes':64*m.proof.GIB,'available_bytes':60*m.proof.GIB,'source':'fixture'}
sys.exit(m.link([sys.executable, '-c', "import pathlib,time; p=pathlib.Path("+repr(sys.argv[2])+"); f=p.open('a');f.write(str(time.monotonic())+' start\\n');f.flush();time.sleep(.15);f.write(str(time.monotonic())+' end\\n');f.close() "]))
'''
        env = dict(self.env, TMPDIR=str(self.root), CAS_RELEASE_GATE_ASSEMBLY_LINK_RSS_LOG=str(self.events))
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
        self.assertTrue(all(item["peak_child_rss_bytes"] > 0 and not item["estimate_exceeded"] for item in completed))


if __name__ == "__main__":
    unittest.main()
