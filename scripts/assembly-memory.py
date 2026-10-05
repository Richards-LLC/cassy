#!/usr/bin/env python3
"""Memory guard and stable linker admission for assembly producers.

Linkers share memory-bounded host/user slots, independent of Cargo job counts.
The wrapper preserves the selected native linker and records waited-driver RSS separately from the external link estimate.
"""
import argparse
from contextlib import ExitStack
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import resource
import shlex
import signal
import stat
import subprocess
import sys
import time
import tomllib

spec = importlib.util.spec_from_file_location("assembly_proof", Path(__file__).with_name("assembly-proof.py"))
proof = importlib.util.module_from_spec(spec)
spec.loader.exec_module(proof)


def append(path, event):
    with Path(path).open("a") as stream:
        fcntl.flock(stream, fcntl.LOCK_EX)
        stream.write(json.dumps(event, sort_keys=True) + "\n")


def settings(env):
    return json.loads(env.get("CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLICY", "{}"))


def deadline(env):
    return proof.positive_knob(env, "CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS") or 600


def poll(env):
    return proof.positive_knob(env, "CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLL_SECS") or 1


# Stable across native/archive TMPDIRs, proof roots and source-keyed wrappers.
# Lease files are tiny; build and fixture scratch remains separately configured.
LINK_LEASE_ROOT = Path("/var/tmp")


def link_capacity(env, memory):
    maximum = proof.positive_knob(env, "CAS_RELEASE_GATE_ASSEMBLY_LINK_JOBS") or 8
    return min(maximum, max(1, (memory["budget_bytes"] - proof.GUARD_HEADROOM_BYTES)
                            // proof.LINK_BYTES))


def lease_file(path):
    # Never follow an injected link or share another user's admission state.
    fd = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    stream = os.fdopen(fd, "a+")
    metadata = os.fstat(fd)
    if metadata.st_uid != os.getuid() or not stat.S_ISREG(metadata.st_mode):
        stream.close()
        raise ValueError("assembly link lease is not an owned regular file")
    return stream


def claim_slot(directory, env):
    """Sample and count every live lease under one atomic admission lock.

    Count high-numbered slots too when capacity shrinks or another proof uses a
    different cap. Free leases stay locked until selection is complete, so an
    unstarted linker is charged its full estimate, not its current zero RSS.
    """
    with lease_file(directory / "admission.lock") as admission:
        fcntl.flock(admission, fcntl.LOCK_EX | fcntl.LOCK_NB)
        memory = proof.memory_budget(env)
        capacity = link_capacity(env, memory)
        event = dict(memory, link_slots=capacity,
                     configured_link_jobs=proof.positive_knob(env, "CAS_RELEASE_GATE_ASSEMBLY_LINK_JOBS") or 8,
                     active_links=0, admitted=False)
        with ExitStack() as probes:
            free = []
            indices = set()
            for path in directory.glob("slot-*.lock"):
                match = re.fullmatch(r"slot-([0-9]+)\.lock", path.name)
                if not match:
                    continue
                index = int(match[1])
                indices.add(index)
                candidate = probes.enter_context(lease_file(path))
                try:
                    fcntl.flock(candidate, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    free.append((index, candidate))
                except BlockingIOError:
                    event["active_links"] += 1
            fits = memory["budget_bytes"] >= proof.LINK_BYTES + proof.GUARD_HEADROOM_BYTES
            if fits and event["active_links"] < capacity:
                if free:
                    index, selected = min(free, key=lambda item: item[0])
                else:
                    index = 0
                    while index in indices:
                        index += 1
                    selected = probes.enter_context(lease_file(directory / f"slot-{index}.lock"))
                    fcntl.flock(selected, fcntl.LOCK_EX | fcntl.LOCK_NB)
                # Duplicate the same open description before closing probes.
                # It keeps the lease locked through the child and its descendants.
                lease = os.fdopen(os.dup(selected.fileno()), "a+")
                event.update(admitted=True, slot_index=index)
                return lease, event
            event["reason"] = "memory reserve/headroom" if not fits else "live link capacity occupied"
            return None, event


def link(command):
    env = settings(os.environ)
    directory = LINK_LEASE_ROOT / f"cas-assembly-links-{os.getuid()}"
    if directory.is_symlink():
        raise ValueError("unsafe assembly link lease directory")
    directory.mkdir(mode=0o700, exist_ok=True)
    metadata = directory.stat()
    if metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
        raise ValueError("assembly link directory must be private and owned")
    # Validate before waiting, including under a busy admission lock.
    proof.positive_knob(env, "CAS_RELEASE_GATE_ASSEMBLY_LINK_JOBS")
    started = time.monotonic()
    receipt = os.environ["CAS_RELEASE_GATE_ASSEMBLY_LINK_RSS_LOG"]
    while True:
        elapsed = time.monotonic() - started
        try:
            lease, event = claim_slot(directory, env)
        except BlockingIOError:
            lease, event = None, {"admitted": False, "reason": "another link admission is in progress"}
        event.update(phase="link", elapsed_s=round(elapsed, 3))
        append(receipt, event)
        if lease is not None:
            break
        if elapsed >= deadline(env):
            raise ValueError("assembly linker memory/slot deadline expired")
        time.sleep(min(poll(env), deadline(env) - elapsed))
    with lease:
        child = subprocess.run(command, pass_fds=tuple({lease.fileno()} | proof.release_scratch.inherited_leases()))
        peak = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
        peak_bytes = int(peak if platform.system() == "Darwin" else peak * 1024)
        append(receipt, {"phase": "link-complete", "status": child.returncode,
                         "slot_index": event["slot_index"], "link_slots": event["link_slots"],
                         "peak_waited_driver_rss_bytes": peak_bytes, "estimate_bytes": proof.LINK_BYTES,
                         "rss_scope": "waited driver RSS; excludes mold workers",
                         "estimate_exceeded": peak_bytes > proof.LINK_BYTES,
                         "wall_s": round(time.monotonic() - started, 3),
                         "measurement_source": "waited driver ru_maxrss (Linux KiB, macOS bytes); excludes unwaited mold workers; not whole-link peak"})
        if peak_bytes > proof.LINK_BYTES:
            print("assembly linker driver exceeded memory estimate; recalibration required", file=sys.stderr)
            return 1
        return child.returncode


def native_configuration(root, env):
    triple = env.get("CARGO_BUILD_TARGET") or (
        ("aarch64" if platform.machine() == "arm64" else platform.machine()) +
        ("-apple-darwin" if platform.system() == "Darwin" else "-unknown-linux-gnu"))
    linker = "cc"
    target_flags, build_flags = [], []
    def flag_list(value):
        return shlex.split(value) if isinstance(value, str) else list(value)
    cargo_home = Path(env.get("CARGO_HOME", str(Path.home() / ".cargo")))
    # Cargo merges from global to deepest checkout config.
    directories = [cargo_home] + [path / ".cargo" for path in reversed((root, *root.parents))]
    for directory in directories:
        for name in ("config", "config.toml"):
            path = directory / name
            if path.is_file():
                config = tomllib.loads(path.read_text())
                targets = config.get("target", {})
                if "CARGO_ENCODED_RUSTFLAGS" not in env and "RUSTFLAGS" not in env and any(
                        key.startswith("cfg(") and value.get("rustflags") for key, value in targets.items()):
                    raise ValueError("assembly linker guard needs explicit native rustflags for cfg target configuration")
                target = targets.get(triple, {})
                value = target.get("linker")
                if value:
                    linker = str((directory.parent / value).resolve()) if "/" in value and not Path(value).is_absolute() else value
                target_flags.extend(flag_list(target.get("rustflags", [])))
                build_flags.extend(flag_list(config.get("build", {}).get("rustflags", [])))
                break
    key = "CARGO_TARGET_" + triple.upper().replace("-", "_")
    linker = env.get(key + "_LINKER", linker)
    target_flags.extend(shlex.split(env.get(key + "_RUSTFLAGS", "")))
    if "CARGO_ENCODED_RUSTFLAGS" in env:
        flags = env["CARGO_ENCODED_RUSTFLAGS"].split("\x1f") if env["CARGO_ENCODED_RUSTFLAGS"] else []
    elif "RUSTFLAGS" in env:
        flags = shlex.split(env["RUSTFLAGS"])
    else:
        flags = target_flags or (build_flags + shlex.split(env.get("CARGO_BUILD_RUSTFLAGS", "")))
    for index, flag in enumerate(flags):
        if flag.startswith("linker=") and index and flags[index - 1] == "-C":
            linker = flag.split("=", 1)[1]
        elif flag.startswith("-Clinker="):
            linker = flag.split("=", 1)[1]
    return linker, flags


def stable_wrapper(root, env):
    # Random archive-clone paths in rustflags would invalidate every cached
    # dependency. Use immutable source-keyed helpers in the shared sweep root.
    parent = Path(env.get("CAS_RELEASE_GATE_ASSEMBLY_LINK_GUARD_DIR") or
                  str(Path(proof.common_dir(root)).parent / ".cas/merge-sweeps/linker-guards"))
    files = {name: Path(__file__).with_name(name).read_bytes()
             for name in ("assembly-memory.py", "assembly-proof.py")}
    scratch_helper = Path(__file__).with_name("release_scratch.py")
    if scratch_helper.is_file():
        files[scratch_helper.name] = scratch_helper.read_bytes()
    key = hashlib.sha256(b"".join(files.values())).hexdigest()
    directory = parent / key
    directory.mkdir(parents=True, exist_ok=True)
    for name, content in files.items():
        path = directory / name
        if path.exists():
            if path.is_symlink() or path.read_bytes() != content:
                raise ValueError("immutable assembly linker helper changed")
        else:
            temporary = path.with_name(name + "." + str(os.getpid()) + ".tmp")
            temporary.write_bytes(content)
            temporary.chmod(0o755)
            temporary.replace(path)
    return directory / "assembly-memory.py"


def compile_guard(command, policy, events, root):
    env = dict(os.environ, CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLICY=policy,
               CAS_RELEASE_GATE_ASSEMBLY_LINK_RSS_LOG=str(events.with_name("link-rss.jsonl")))
    linker, flags = native_configuration(root, env)
    env["CAS_RELEASE_GATE_ASSEMBLY_REAL_LINKER"] = linker
    env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(flags + ["-C", "linker=" + str(stable_wrapper(root, env))])
    policy = json.loads(policy)
    admission = {"phases": []}
    try:
        jobs = proof.admit_phase(policy, admission, "producer-compile", True)
    finally:
        for event in admission["phases"]:
            append(events, event)
    env["CARGO_BUILD_JOBS"] = str(min(int(env.get("CARGO_BUILD_JOBS", jobs)), int(jobs)))
    child = None
    paused = False
    paused_at = None
    handlers = {}
    interrupted = []
    def forward(sig, frame):
        for watched in handlers:
            signal.signal(watched, signal.SIG_IGN)
        interrupted.append(signal.Signals(sig).name)
        if child is not None:
            raise InterruptedError("assembly compile interrupted by " + interrupted[0])
    try:
        for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
            handlers[sig] = signal.signal(sig, forward)
        child = subprocess.Popen(command, env=env, start_new_session=True,
                                 pass_fds=tuple(proof.release_scratch.inherited_leases(env)))
        if interrupted:
            raise InterruptedError("assembly compile interrupted during child creation by " + interrupted[0])
        while child.poll() is None:
            memory = proof.memory_budget(policy)
            event = dict(memory, phase="compile", paused=paused, action="sample")
            if memory["available_bytes"] < memory["reserve_bytes"]:
                event["action"] = "reserve-breached-abort"
                append(events, event)
                raise ValueError("assembly compile breached memory reserve; no PASS can be published")
            if memory["budget_bytes"] < proof.GUARD_HEADROOM_BYTES:
                if not paused:
                    os.killpg(child.pid, signal.SIGSTOP)
                    paused, paused_at, event["action"] = True, time.monotonic(), "pause"
            elif paused and memory["budget_bytes"] >= 2 * proof.GUARD_HEADROOM_BYTES:
                os.killpg(child.pid, signal.SIGCONT)
                paused, event["action"] = False, "resume"
            if paused and time.monotonic() - paused_at >= deadline(policy):
                event["action"] = "memory-pause-deadline-abort"
                append(events, event)
                raise ValueError("assembly compile memory recovery deadline expired")
            append(events, event)
            time.sleep(poll(policy))
        return child.returncode
    finally:
        # Resume stopped descendants before termination; otherwise TERM would
        # stay pending indefinitely and scratch teardown could race builders.
        for watched in handlers:
            signal.signal(watched, signal.SIG_IGN)
        if child is not None:
            for sig in (signal.SIGCONT, signal.SIGTERM):
                try:
                    os.killpg(child.pid, sig)
                except ProcessLookupError:
                    pass
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()
        for sig, handler in handlers.items():
            signal.signal(sig, handler)


def main():
    # rustc invokes its linker with positional arguments, without a CLI verb.
    if "CAS_RELEASE_GATE_ASSEMBLY_REAL_LINKER" in os.environ:
        return link([os.environ["CAS_RELEASE_GATE_ASSEMBLY_REAL_LINKER"], *sys.argv[1:]])
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--policy", required=True)
    parser.add_argument("--events", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    return compile_guard(args.command[1:], args.policy, args.events, args.root)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.CalledProcessError) as exc:
        print("assembly memory guard: " + str(exc), file=sys.stderr)
        sys.exit(1)
