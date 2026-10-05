#!/usr/bin/env python3
"""Owned release scratch, signal teardown and bounded assembly build cache.

The CLI also supplies the read-only gc_report and explicit gc_cleanup seam.
Only recognized direct children of the configured scratch parents are swept.
"""
import argparse
import contextlib
import fcntl
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time

OWNER = ".cas-scratch-owner.json"
LOCK = ".cas-scratch-owner.lock"
CURRENT = None
PROC_ROOT = Path("/proc")
TEARDOWN_WAIT_SECS = 20


def positive(env, key, default):
    value = float(env.get(key, default))
    if not 0 < value < float("inf"):
        raise ValueError(key + " must be finite and positive")
    return value


def alive(pid):
    try:
        os.kill(int(pid), 0)
        return True
    except ProcessLookupError:
        return False
    except (PermissionError, ValueError, TypeError):
        return True  # Unknown identity fails closed.


def size(path):
    return sum(item.stat(follow_symlinks=False).st_size
               for parent, dirs, files in os.walk(path, followlinks=False)
               for item in (Path(parent) / name for name in files)
               if not item.is_symlink())


def worktrees(repo):
    # Refuse deletion when Git cannot establish the protected set.
    raw = subprocess.check_output(["git", "-C", str(repo), "worktree", "list", "--porcelain", "-z"])
    return [Path(field[9:].decode()).resolve() for field in raw.split(b"\0")
            if field.startswith(b"worktree ")]


def contains(path, child):
    return path == child or path in child.parents


def process_uses(path, own_lock_fd=None):
    if PROC_ROOT.is_dir():
        for process in PROC_ROOT.iterdir():
            if not process.name.isdigit():
                continue
            try:
                if process.stat().st_uid != os.getuid():
                    continue  # Owned 0700 scratch is not accessible to other users.
                state = (process / "stat").read_text().rsplit(") ", 1)[1].split()[0]
                if state in ("Z", "X"):
                    continue  # Exited tasks have no live cwd, mappings or file handles.
                probes = [process / "cwd", process / "exe"] + list((process / "fd").iterdir())
                for probe in probes:
                    if (int(process.name) == os.getpid() and probe.parent.name == "fd"
                            and probe.name == str(own_lock_fd)):
                        continue  # Exempt this exact eviction descriptor, not other own outputs.
                    try:
                        if contains(path, Path(os.readlink(probe))):
                            return True
                    except FileNotFoundError:
                        pass
                for argument in (process / "cmdline").read_bytes().split(b"\0"):
                    value = os.fsdecode(argument).split("=", 1)[-1]
                    if value.startswith("/") and contains(path, Path(value)):
                        return True
                for line in (process / "maps").read_text().splitlines():
                    fields = line.split(maxsplit=5)
                    if len(fields) == 6 and fields[5].startswith("/") and contains(path, Path(fields[5])):
                        return True
            except FileNotFoundError:
                if process.exists():
                    return True  # Missing evidence of a still-present process is unknown.
            except PermissionError:
                return True
        return False
    try:
        result = subprocess.run(["lsof", "-t", "+D", str(path)], capture_output=True)
        return result.returncode != 1 or bool(result.stdout) or bool(result.stderr)
    except OSError:
        return True


class OwnedDirectory:
    def __init__(self, prefix, parent):
        self.path = Path(tempfile.mkdtemp(prefix=prefix, dir=parent))
        self.lock = (self.path / LOCK).open("a+")
        fcntl.flock(self.lock, fcntl.LOCK_EX)
        (self.path / OWNER).write_text(json.dumps({"pid": os.getpid(), "created": time.time()}))
        if CURRENT is not None:
            with CURRENT.lock:
                CURRENT.leases.add(self.lock.fileno())

    def __enter__(self):
        return self.path

    def __exit__(self, *exc):
        try:
            if CURRENT is not None:
                CURRENT.stop()  # Reap before clone/base finally-cleanup unwinds.
            shutil.rmtree(self.path)
        finally:
            if CURRENT is not None:
                with CURRENT.lock:
                    CURRENT.leases.discard(self.lock.fileno())
            self.lock.close()


class ChildScope:
    """Stop and reap process groups before any owned directory is removed."""
    def __init__(self):
        self.children = set()
        self.signalled = set()
        self.interrupted = False
        self.leases = set()
        self.lock = threading.RLock()
        self.cancelled = False
        self.handlers = {}

    def __enter__(self):
        global CURRENT
        if CURRENT is not None:
            raise ValueError("nested scratch child scope")
        CURRENT = self
        if threading.current_thread() is threading.main_thread():
            for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
                self.handlers[sig] = signal.signal(sig, self.interrupt)
        return self

    def interrupt(self, sig, frame):
        # Do not re-enter Popen.wait from its signal handler: its waitpid
        # mutex may be held by this same thread. Unwind first, then reap.
        if self.interrupted:
            return
        self.interrupted = True
        for watched in self.handlers:
            signal.signal(watched, signal.SIG_IGN)
        self.stop(wait=False)
        raise InterruptedError("release run interrupted by " + signal.Signals(sig).name)

    def stop(self, wait=True):
        with self.lock:
            self.cancelled = True
            children = list(self.children)
            newly_signalled = [child for child in children if child.pid not in self.signalled]
            self.signalled.update(child.pid for child in newly_signalled)
        for child in newly_signalled:
            try:
                os.killpg(child.pid, signal.SIGCONT)
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        if not wait:
            return
        # The nested compile guard itself gets 5s to kill/reap Cargo.
        # Its owning gate must finish that teardown before this parent kills it.
        deadline = time.monotonic() + TEARDOWN_WAIT_SECS
        for child in children:
            try:
                child.wait(timeout=max(.01, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait()

    def run(self, command, check=False, **kwargs):
        with self.lock:
            if self.cancelled:
                raise InterruptedError("release child admission cancelled")
            inherited = set(kwargs.pop("pass_fds", ())) | self.leases
            child = subprocess.Popen(command, start_new_session=True, pass_fds=tuple(inherited), **kwargs)
            self.children.add(child)
        try:
            status = child.wait()
            if check and status:
                raise subprocess.CalledProcessError(status, command)
            return subprocess.CompletedProcess(command, status)
        finally:
            if self.cancelled:
                self.stop()
            # Keep a interrupted child registered until stop() has reaped it.
            if child.poll() is not None:
                with self.lock:
                    self.children.discard(child)

    def __exit__(self, *exc):
        global CURRENT
        try:
            self.stop()
        finally:
            for sig, handler in self.handlers.items():
                signal.signal(sig, handler)
            CURRENT = None


def child_run(command, **kwargs):
    return CURRENT.run(command, **kwargs) if CURRENT else subprocess.run(command, **kwargs)


def register(path, owner_dir):
    # A child creates a directory then registers it before writing large data.
    path, owner_dir = Path(path).absolute(), Path(owner_dir)
    owner = json.loads((owner_dir / OWNER).read_text())
    (path / OWNER).write_text(json.dumps(dict(owner, lease=str(owner_dir / LOCK))))
    with (owner_dir / "paths").open("a") as stream:
        stream.write(json.dumps(str(path)) + "\n")


def sweep(repo, base, clean=False, env=None):
    env = os.environ if env is None else env
    cutoff = time.time() - positive(env, "CAS_RELEASE_SCRATCH_MAX_AGE_HOURS", 6) * 3600
    base = Path(base).absolute()
    parents = {base.parent: (base.name + ".", "assembly-clone-")}
    legacy = env.get("CAS_RELEASE_SCRATCH_EXTRA_BASES", "/home/cas-release-gate/base:/var/tmp/cas-release-gate:/Users/Shared/cas-release-gate")
    for value in filter(None, legacy.split(os.pathsep)):
        other = Path(value).absolute()
        parents.setdefault(other.parent, tuple())
        parents[other.parent] += (other.name + ".", "assembly-clone-")
    temp_parent = Path(env.get("TMPDIR") or tempfile.gettempdir()).absolute()
    parents.setdefault(temp_parent, tuple())
    parents[temp_parent] += ("cas-release-gate.",)
    protected = worktrees(repo)
    records = []
    for parent, prefixes in parents.items():
        if not parent.is_dir():
            continue
        # Serialize allocators/sweepers in this parent. Missing-owner legacy
        # directories are admitted only after the age and OS liveness checks.
        lock_path = parent / ".cas-scratch-sweep.lock"
        if lock_path.is_symlink():
            continue
        lock_context = lock_path.open("a+" if clean else "r") if clean or lock_path.exists() else contextlib.nullcontext()
        with lock_context as sweep_lock:
            if sweep_lock:
                fcntl.flock(sweep_lock, fcntl.LOCK_EX if clean else fcntl.LOCK_SH)
            for path in sorted(parent.iterdir()):
                if not path.name.startswith(prefixes) or path.is_symlink() or not path.is_dir():
                    continue
                stat = path.stat()
                if stat.st_uid != os.getuid():
                    continue
                reason = "recent"
                reclaimable = False
                lock = None
                try:
                    remap_git = path / "workspace-remap/.git"
                    clone_registry = path / "repo/.git/worktrees"
                    if (any(contains(path.resolve(), tree) for tree in protected)
                            or remap_git.is_file() or remap_git.is_symlink()
                            or (clone_registry.is_dir() and any(clone_registry.iterdir()))):
                        reason = "registered worktree"
                    elif stat.st_mtime <= cutoff:
                        owner_file = path / OWNER
                        owner = json.loads(owner_file.read_text()) if owner_file.exists() else {}
                        if owner and alive(owner.get("pid")):
                            reason = "live owner"
                        else:
                            lease = Path(owner.get("lease", path / LOCK))
                            safe_lease = lease == path / LOCK or (lease.name == LOCK and lease.parent.name.startswith("cas-release-gate."))
                            if not safe_lease or lease.is_symlink() or (path / LOCK).is_symlink():
                                reason = "unsafe lease"
                            else:
                                if not lease.exists():
                                    lease = path / LOCK
                                if clean or lease.exists():
                                    lock = lease.open("a+" if clean else "r")
                                    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                                if process_uses(path.resolve(), lock.fileno() if lock else None):
                                    reason = "live process"
                                else:
                                    reclaimable, reason = True, "dead owner past age bound"
                    record = {"path": str(path), "bytes": size(path), "reclaimable": reclaimable,
                              "reason": reason, "removed": False}
                    if clean and reclaimable:
                        # Recheck Git after lease admission; never recursively
                        # erase a registered checkout embedded in old scratch.
                        if not any(contains(path.resolve(), tree) for tree in worktrees(repo)):
                            shutil.rmtree(path)
                            record["removed"] = True
                    records.append(record)
                except (OSError, ValueError) as exc:
                    records.append({"path": str(path), "bytes": 0, "reclaimable": False,
                                    "reason": "protected: " + str(exc), "removed": False})
                finally:
                    if lock:
                        lock.close()
    if clean and any(row["removed"] for row in records):
        subprocess.run(["git", "-C", str(repo), "worktree", "prune"], check=True)
    return {"entries": records, "reclaimable_bytes": sum(row["bytes"] for row in records if row["reclaimable"]),
            "reclaimed_bytes": sum(row["bytes"] for row in records if row["removed"])}


class BoundedCache:
    def __init__(self, path, env, repo=None):
        self.path = Path(path)
        self.cap = int(positive(env, "CAS_ASSEMBLY_TARGET_MAX_GIB", 20) * 1024 ** 3)
        self.age = positive(env, "CAS_ASSEMBLY_TARGET_MAX_AGE_DAYS", 7) * 86400
        self.lock = None
        self.repo = repo

    def prune(self):
        if self.path.is_symlink():
            raise ValueError("assembly target must not be a symlink")
        if not self.path.exists():
            return
        if self.repo and any(contains(self.path.resolve(), tree) for tree in worktrees(self.repo)):
            raise ValueError("assembly target contains a registered worktree")
        used = size(self.path)
        stamp = self.path / ".cas-last-used"
        modified = stamp.stat().st_mtime if stamp.exists() else self.path.stat().st_mtime
        if used > self.cap or time.time() - modified > self.age:
            if process_uses(self.path.resolve()):
                raise ValueError("over-bound assembly target still has live users; eviction refused")
            print(f"assembly target eviction: {self.path} bytes={used} cap={self.cap} age_bound_s={self.age}", flush=True)
            shutil.rmtree(self.path)

    def __enter__(self):
        self.path.parent.mkdir(parents=True, exist_ok=True)
        lock_path = self.path.with_name(self.path.name + ".lock")
        if lock_path.is_symlink():
            raise ValueError("assembly target lease must not be a symlink")
        self.lock = lock_path.open("a+")
        fcntl.flock(self.lock, fcntl.LOCK_EX)
        if CURRENT is not None:
            with CURRENT.lock:
                CURRENT.leases.add(self.lock.fileno())
        try:
            self.prune()
        except BaseException:
            if CURRENT is not None:
                with CURRENT.lock:
                    CURRENT.leases.discard(self.lock.fileno())
            self.lock.close()
            raise
        return self.path

    def __exit__(self, *exc):
        try:
            self.prune()
            if self.path.exists():
                (self.path / ".cas-last-used").touch()
        finally:
            if CURRENT is not None:
                with CURRENT.lock:
                    CURRENT.leases.discard(self.lock.fileno())
            self.lock.close()


def guard(command, repo, base):
    sweep(repo, base, clean=True)
    lease = OwnedDirectory("cas-release-gate.", tempfile.gettempdir())
    with ChildScope() as scope, lease as owner_dir:
        try:
            env = dict(os.environ, CAS_RELEASE_GATE_SCRATCH_RUN_DIR=str(owner_dir))
            # Pass the lease through exec: surviving descendants keep scratch
            # protected even if the guardian itself receives SIGKILL.
            return scope.run(command, env=env, pass_fds=(lease.lock.fileno(),)).returncode
        finally:
            scope.stop()
            path_list = owner_dir / "paths"
            if path_list.exists():
                for line in reversed(path_list.read_text().splitlines()):
                    path = Path(json.loads(line))
                    # Exact registered paths, not globbing across other runs.
                    remap = path / "workspace-remap"
                    if remap.exists():
                        subprocess.run(["git", "-C", str(repo), "worktree", "remove", "--force", str(remap)],
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    if path.exists() and not path.is_symlink():
                        shutil.rmtree(path)
                subprocess.run(["git", "-C", str(repo), "worktree", "prune"], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("report", "clean", "register", "guard"))
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--base", type=Path, default=Path(os.environ.get("CAS_RELEASE_GATE_HOME_DIR") or "/var/tmp/cas-release-gate/base"))
    parser.add_argument("--owner-dir", type=Path)
    parser.add_argument("--path", type=Path)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.action == "register":
        register(args.path, args.owner_dir)
        return 0
    if args.action == "guard":
        return guard(args.command[1:] if args.command[:1] == ["--"] else args.command, args.repo, args.base)
    print(json.dumps(sweep(args.repo, args.base, clean=args.action == "clean"), sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.CalledProcessError) as exc:
        print("release scratch: " + str(exc), file=sys.stderr)
        sys.exit(1)
