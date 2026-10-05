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
PROTOCOL = "cas-scratch-v1"
REGENERABLE = ("suite.tar.zst", "extract", "tmp", "cargo-home", "bin")


def positive(env, key, default):
    value = float(env.get(key, default))
    if not 0 < value < float("inf"):
        raise ValueError(key + " must be finite and positive")
    return value


def process_identity(pid):
    """PID reuse must not make yesterday's directory belong to today's process."""
    pid = int(pid)
    if PROC_ROOT.is_dir():
        fields = (PROC_ROOT / str(pid) / "stat").read_text().rsplit(") ", 1)[1].split()
        if fields[0] in ("Z", "X"):
            raise ProcessLookupError(pid)
        boot = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
        return boot + ":" + fields[19]
    result = subprocess.run(["ps", "-p", str(pid), "-o", "lstart="], capture_output=True, text=True, check=True)
    if not result.stdout.strip():
        raise ProcessLookupError(pid)
    return result.stdout.strip()


def owner_record(path, lease):
    stat = path.stat()
    return {"protocol": PROTOCOL, "uid": os.getuid(), "pid": os.getpid(),
            "start": process_identity(os.getpid()), "created": time.time(),
            "path": str(path.resolve()), "device": stat.st_dev, "inode": stat.st_ino,
            "lease": str(lease.absolute())}


def read_owner(path):
    file = path / OWNER
    if file.is_symlink():
        raise ValueError("symlink owner record")
    if not file.exists():
        return None
    if file.stat().st_uid != os.getuid():
        raise ValueError("foreign owner record")
    owner = json.loads(file.read_text())
    stat = path.stat()
    if (owner.get("protocol") != PROTOCOL or owner.get("uid") != os.getuid()
            or owner.get("path") != str(path.resolve()) or owner.get("device") != stat.st_dev
            or owner.get("inode") != stat.st_ino or not owner.get("start")):
        # Old pid-only records have unknown provenance, not permission to adopt.
        return None
    return owner


def owner_live(owner):
    try:
        return process_identity(owner["pid"]) == owner["start"]
    except (FileNotFoundError, ProcessLookupError, subprocess.CalledProcessError):
        return False
    except (OSError, ValueError, KeyError):
        return True  # An unreadable recorded owner stays protected.


def open_lock(path, create=False):
    flags = os.O_RDWR if create else os.O_RDONLY
    flags |= getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0)
    if create:
        flags |= os.O_CREAT
    fd = os.open(path, flags, 0o600)
    stat = os.fstat(fd)
    import stat as file_stat
    if stat.st_uid != os.getuid() or not file_stat.S_ISREG(stat.st_mode):
        os.close(fd)
        raise ValueError("unsafe lease file")
    return os.fdopen(fd, "a+" if create else "r")


@contextlib.contextmanager
def admitted(path, owner, clean):
    lease = Path(owner["lease"]) if owner else path / LOCK
    if owner and lease != path / LOCK:
        # Registered children refer only to a real guardian's lease, with a
        # matching owner identity. Never follow an arbitrary JSON lease path.
        guardian = lease.parent
        if lease.name != LOCK or not guardian.name.startswith("cas-release-gate."):
            raise ValueError("unsafe guardian lease")
        if guardian.exists():
            guardian_owner = read_owner(guardian)
            if (not guardian_owner or guardian_owner["pid"] != owner["pid"]
                    or guardian_owner["start"] != owner["start"]):
                raise ValueError("guardian identity mismatch")
        else:
            lease = path / LOCK  # Dead guardian; serialize this orphan locally.
    if lease.is_symlink():
        raise ValueError("symlink lease")
    lock = open_lock(lease, create=clean) if clean or lease.exists() else None
    try:
        if lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if owner and owner_live(owner):
            raise ValueError("live owner with matching start time")
        # Only CAS-owned lease paths may ignore opaque unrelated processes.
        if process_uses(path.resolve(), lock.fileno() if lock else None, lease_managed=bool(owner)):
            raise ValueError("live process or unavailable process evidence")
        yield
    finally:
        if lock:
            lock.close()


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


def process_uses(path, own_lock_fd=None, lease_managed=False):
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
                if process.exists() and not lease_managed:
                    return True  # Missing evidence of a still-present process is unknown.
            except PermissionError:
                if not lease_managed:
                    return True
        return False
    try:
        result = subprocess.run(["lsof", "-nP", "-F0pfn", "+D", str(path)], capture_output=True)
        if result.returncode not in (0, 1) or (result.stderr and not lease_managed):
            return True
        pid, fd = None, None
        for field in result.stdout.replace(b"\n", b"\0").split(b"\0"):
            if field.startswith(b"p"):
                pid, fd = int(field[1:]), None
            elif field.startswith(b"f"):
                fd = field[1:].decode().rstrip("rwu")
            elif field.startswith(b"n"):
                if pid == os.getpid() and fd == str(own_lock_fd):
                    continue
                if contains(path, Path(os.fsdecode(field[1:]))):
                    return True
        return False
    except (OSError, ValueError):
        return not lease_managed


@contextlib.contextmanager
def defer_signals():
    watched = {signal.SIGINT, signal.SIGTERM, signal.SIGHUP}
    if hasattr(signal, "pthread_sigmask") and threading.current_thread() is threading.main_thread():
        previous = signal.pthread_sigmask(signal.SIG_BLOCK, watched)
        try:
            yield
        finally:
            signal.pthread_sigmask(signal.SIG_SETMASK, previous)
    else:
        yield


def inherited_leases(env=None):
    import stat
    env = os.environ if env is None else env
    descriptors = set()
    for value in filter(None, env.get("CAS_RELEASE_GATE_SCRATCH_LEASE_FDS", "").split(",")):
        fd = int(value)
        metadata = os.fstat(fd)
        if fd < 3 or metadata.st_uid != os.getuid() or not stat.S_ISREG(metadata.st_mode):
            raise ValueError("invalid inherited scratch lease")
        descriptors.add(fd)
    return descriptors


class OwnedDirectory:
    def __init__(self, prefix, parent):
        # Pending graceful signals may arrive as soon as masking is lifted.
        # Register a fallback cleanup with the enclosing scope first.
        with defer_signals():
            self.path = Path(tempfile.mkdtemp(prefix=prefix, dir=parent))
            try:
                self.lock = open_lock(self.path / LOCK, create=True)
                fcntl.flock(self.lock, fcntl.LOCK_EX)
                (self.path / OWNER).write_text(json.dumps(owner_record(self.path, self.path / LOCK)))
                if CURRENT is not None:
                    with CURRENT.lock:
                        CURRENT.leases.add(self.lock.fileno())
                        CURRENT.directories.add(self)
            except BaseException:
                shutil.rmtree(self.path)
                if getattr(self, "lock", None):
                    self.lock.close()
                raise

    def __enter__(self):
        return self.path

    def __exit__(self, *exc):
        if self.lock.closed:
            return
        try:
            if CURRENT is not None:
                CURRENT.stop()  # Reap before clone/base finally-cleanup unwinds.
            if self.path.exists():
                shutil.rmtree(self.path)
        finally:
            if CURRENT is not None:
                with CURRENT.lock:
                    CURRENT.leases.discard(self.lock.fileno())
                    CURRENT.directories.discard(self)
            self.lock.close()


class ChildScope:
    """Stop and reap process groups before any owned directory is removed."""
    def __init__(self):
        self.children = set()
        self.signalled = set()
        self.interrupted = False
        self.spawning = False
        self.leases = set()
        self.directories = set()
        self.caches = set()
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
        if not self.spawning:
            raise InterruptedError("release run interrupted by " + signal.Signals(sig).name)
        # Popen has not returned its PID yet. Finish registration before
        # unwinding; blocking signals here would leak the mask into exec.

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
            child_env = dict(kwargs.pop("env", os.environ))
            inherited = set(kwargs.pop("pass_fds", ())) | self.leases | inherited_leases(child_env)
            child_env["CAS_RELEASE_GATE_SCRATCH_LEASE_FDS"] = ",".join(map(str, sorted(inherited)))
            self.spawning = True
            try:
                child = subprocess.Popen(command, env=child_env, start_new_session=True, pass_fds=tuple(inherited), **kwargs)
                self.children.add(child)
            finally:
                self.spawning = False
            if self.cancelled:
                self.stop(wait=False)
                raise InterruptedError("release interrupted during child creation")
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
            for directory in list(self.directories):
                directory.__exit__(*exc)
            for cache in list(self.caches):
                cache.__exit__(*exc)
        finally:
            for sig, handler in self.handlers.items():
                signal.signal(sig, handler)
            CURRENT = None


def child_run(command, **kwargs):
    return CURRENT.run(command, **kwargs) if CURRENT else subprocess.run(command, **kwargs)


def register(path, owner_dir):
    # Only the guardian may authorize removal of a fresh, direct scratch child.
    path, owner_dir = Path(path).absolute(), Path(owner_dir).absolute()
    owner = read_owner(owner_dir)
    if not owner or not owner_live(owner) or path.is_symlink() or not path.is_dir():
        raise ValueError("invalid scratch registration")
    parents = scratch_parents(Path(os.environ.get("CAS_RELEASE_GATE_HOME_DIR") or "/var/tmp/cas-release-gate/base"), os.environ)
    prefixes = parents.get(path.parent, ())
    stat = path.stat()
    if (not path.name.startswith(prefixes) or stat.st_uid != os.getuid()
            or stat.st_mode & 0o077 or list(path.iterdir())):
        raise ValueError("registration requires a fresh private recognized scratch directory")
    record = owner_record(path, owner_dir / LOCK)
    record.update(pid=owner["pid"], start=owner["start"], created=owner["created"])
    (path / OWNER).write_text(json.dumps(record))
    with (owner_dir / "paths").open("a") as stream:
        stream.write(json.dumps(str(path)) + "\n")


def scratch_parents(base, env):
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
    return parents


def registered(path, protected):
    remap = path / "workspace-remap/.git"
    registry = path / "repo/.git/worktrees"
    return (any(contains(path.resolve(), tree) for tree in protected)
            or remap.is_file() or remap.is_symlink()
            or (registry.is_dir() and any(registry.iterdir())))


def sweep(repo, base, clean=False, env=None):
    env = os.environ if env is None else env
    cutoff = time.time() - positive(env, "CAS_RELEASE_SCRATCH_MAX_AGE_HOURS", 6) * 3600
    records = []
    protected = worktrees(repo)
    for parent, prefixes in scratch_parents(base, env).items():
        if not parent.is_dir():
            continue
        lock_path = parent / ".cas-scratch-sweep.lock"
        if lock_path.is_symlink():
            continue
        context = open_lock(lock_path, create=clean) if clean or lock_path.exists() else contextlib.nullcontext()
        with context as sweep_lock:
            if sweep_lock:
                fcntl.flock(sweep_lock, fcntl.LOCK_EX if clean else fcntl.LOCK_SH)
            for path in sorted(parent.iterdir()):
                if not path.name.startswith(prefixes) or path.is_symlink() or not path.is_dir():
                    continue
                stat = path.stat()
                if stat.st_uid != os.getuid():
                    continue
                row = {"path": str(path), "bytes": size(path), "reclaimable_bytes": 0,
                       "reclaimed_bytes": 0, "retained_bytes": 0, "reclaimable": False,
                       "reason": "recent", "removed": False, "retained_base": False}
                try:
                    if stat.st_mtime <= cutoff:
                        owner = read_owner(path)
                        with admitted(path, owner, clean):
                            keep_base = registered(path, protected)
                            # Partial reclamation requires CAS provenance. A registered
                            # remap and its base always remain; only these siblings go.
                            candidates = ([path / name for name in REGENERABLE
                                           if (path / name).exists() and not (path / name).is_symlink()]
                                          if keep_base and owner else ([] if keep_base else [path]))
                            row["retained_base"] = keep_base
                            row["reason"] = "registered worktree retained; regenerable siblings only" if keep_base else "dead owner past age bound"
                            for candidate in candidates:
                                if any(contains(candidate.resolve(), tree) for tree in protected):
                                    continue
                                used = size(candidate) if candidate.is_dir() else candidate.stat().st_size
                                row["reclaimable_bytes"] += used
                                if clean:
                                    # Git identity is re-read immediately before mutation.
                                    if any(contains(candidate.resolve(), tree) for tree in worktrees(repo)):
                                        continue
                                    if candidate.is_dir():
                                        shutil.rmtree(candidate)
                                    else:
                                        candidate.unlink()
                                    row["reclaimed_bytes"] += used
                            row["reclaimable"] = row["reclaimable_bytes"] > 0
                            row["removed"] = not path.exists()
                except (OSError, ValueError) as exc:
                    row["reason"] = "protected: " + str(exc)
                row["retained_bytes"] = size(path) if path.exists() else 0
                records.append(row)
    # Never prune/unregister a stale remap here; follow-up cas-638d owns that.
    return {"entries": records,
            "reclaimable_bytes": sum(row["reclaimable_bytes"] for row in records),
            "reclaimed_bytes": sum(row["reclaimed_bytes"] for row in records),
            "retained_bytes": sum(row["retained_bytes"] for row in records)}


class BoundedCache:
    def __init__(self, path, env, repo=None):
        self.path = Path(path).absolute()
        self.cap = int(positive(env, "CAS_ASSEMBLY_TARGET_MAX_GIB", 20) * 1024 ** 3)
        self.age = positive(env, "CAS_ASSEMBLY_TARGET_MAX_AGE_DAYS", 7) * 86400
        self.lock = None
        self.repo = repo
        self.events = []
        self.managed = False

    def inventory(self, clean=False, adopt=False):
        row = {"path": str(self.path), "bytes": 0, "reclaimed_bytes": 0,
               "retained_bytes": 0, "reclaimable_bytes": 0, "reason": "absent",
               "cap_bytes": self.cap, "max_age_secs": self.age}
        if self.path.is_symlink():
            raise ValueError("assembly target must not be a symlink")
        if not self.path.exists():
            self.events.append(row)
            return row
        row["bytes"] = row["retained_bytes"] = size(self.path)
        if self.repo and any(contains(self.path.resolve(), tree) for tree in worktrees(self.repo)):
            raise ValueError("assembly target contains a registered worktree")
        owner = read_owner(self.path)
        if adopt and not owner:
            # Explicit quiet-window operation; opaque evidence STILL refuses.
            if process_uses(self.path.resolve(), self.lock.fileno()):
                raise ValueError("legacy cache adoption refused: live users or unavailable process evidence")
            (self.path / OWNER).write_text(json.dumps(owner_record(self.path, self.path.with_name(self.path.name + ".lock"))))
            owner = read_owner(self.path)
            self.managed = True
            row["adopted"] = True
        if not owner:
            row["reason"] = "legacy cache retained: no CAS owner record; explicit quiet-window adoption required"
            self.events.append(row)
            return row
        if owner_live(owner) and not (self.managed and owner["pid"] == os.getpid()):
            row["reason"] = "live cache owner with matching start time"
            self.events.append(row)
            return row
        stamp = self.path / ".cas-last-used"
        modified = stamp.stat().st_mtime if stamp.exists() else self.path.stat().st_mtime
        row["reason"] = "within size and age bounds"
        if row["bytes"] > self.cap or time.time() - modified > self.age:
            if process_uses(self.path.resolve(), self.lock.fileno() if self.lock else None, lease_managed=True):
                row["reason"] = "over-bound cache retained: live users"
            else:
                row["reclaimable_bytes"] = row["bytes"]
                row["reason"] = "over size or age bound"
                if clean:
                    # Never evict a newly registered checkout under an old snapshot.
                    if self.repo and any(contains(self.path.resolve(), tree) for tree in worktrees(self.repo)):
                        raise ValueError("assembly target contains a registered worktree")
                    shutil.rmtree(self.path)
                    row["reclaimed_bytes"] = row["bytes"]
                    row["retained_bytes"] = 0
        self.events.append(row)
        return row

    def __enter__(self):
        with defer_signals():
            self.path.parent.mkdir(parents=True, exist_ok=True)
            self.lock = open_lock(self.path.with_name(self.path.name + ".lock"), create=True)
            fcntl.flock(self.lock, fcntl.LOCK_EX)
            if CURRENT is not None:
                with CURRENT.lock:
                    CURRENT.leases.add(self.lock.fileno())
                    CURRENT.caches.add(self)
            try:
                self.inventory(clean=True)
                if not self.path.exists():
                    self.path.mkdir(mode=0o700)
                    self.managed = True
                elif read_owner(self.path):
                    # Do not overwrite a live stale-owner record after its flock
                    # was lost. Ownership requires both admission checks to pass.
                    owner = read_owner(self.path)
                    if owner_live(owner):
                        raise ValueError("assembly target owner is still live")
                    self.managed = True
                if self.managed:
                    (self.path / OWNER).write_text(json.dumps(owner_record(self.path, self.path.with_name(self.path.name + ".lock"))))
            except BaseException:
                self.release()
                raise
            return self.path

    def release(self):
        if CURRENT is not None:
            with CURRENT.lock:
                CURRENT.leases.discard(self.lock.fileno())
                CURRENT.caches.discard(self)
        self.lock.close()

    def __exit__(self, *exc):
        if self.lock.closed:
            return
        try:
            if CURRENT is not None:
                CURRENT.stop()
            self.inventory(clean=True)
            if self.path.exists() and self.managed:
                (self.path / ".cas-last-used").touch()
                # The lease continues to protect an exiting owner while the
                # record is marked idle; descendants have already been reaped.
                owner = read_owner(self.path)
                owner.update(pid=0, start="idle")
                (self.path / OWNER).write_text(json.dumps(owner))
        finally:
            self.release()


def cache_report(repo, path, clean=False, adopt=False, env=None):
    cache = BoundedCache(path, os.environ if env is None else env, repo)
    lock_path = cache.path.with_name(cache.path.name + ".lock")
    try:
        if not lock_path.exists() and not clean:
            return cache.inventory()
        if clean:
            lock_path.parent.mkdir(parents=True, exist_ok=True)
        cache.lock = open_lock(lock_path, create=clean)
        fcntl.flock(cache.lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        return cache.inventory(clean=clean, adopt=adopt)
    except (OSError, ValueError) as exc:
        used = size(cache.path) if cache.path.is_dir() and not cache.path.is_symlink() else 0
        return {"path": str(cache.path), "bytes": used, "retained_bytes": used,
                "reclaimable_bytes": 0, "reclaimed_bytes": 0, "reason": "protected: " + str(exc)}
    finally:
        if cache.lock:
            cache.lock.close()


def select_cache(path):
    """Leave unknown legacy output visible; new builds use a bounded lease cache."""
    path = Path(path)
    if path.exists() and not path.is_symlink() and read_owner(path) is None:
        return path.with_name(path.name + "-leased-v1")
    return path


def guard(command, repo, base):
    sweep(repo, base, clean=True)
    with ChildScope() as scope, OwnedDirectory("cas-release-gate.", tempfile.gettempdir()) as owner_dir:
        try:
            env = dict(os.environ, CAS_RELEASE_GATE_SCRATCH_RUN_DIR=str(owner_dir))
            # Pass the lease through exec: surviving descendants keep scratch
            # protected even if the guardian itself receives SIGKILL.
            return scope.run(command, env=env, pass_fds=tuple(scope.leases)).returncode
        finally:
            scope.stop()
            path_list = owner_dir / "paths"
            if path_list.exists():
                for line in reversed(path_list.read_text().splitlines()):
                    path = Path(json.loads(line))
                    owner = read_owner(path) if path.exists() and not path.is_symlink() else None
                    current = read_owner(owner_dir)
                    if not owner or owner["pid"] != current["pid"] or owner["start"] != current["start"] or owner["lease"] != str(owner_dir / LOCK):
                        continue  # Poisoned or replaced registry entries are not deletion authority.
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
    parser.add_argument("--cache", type=Path)
    parser.add_argument("--adopt-legacy-cache", action="store_true", help="explicit quiet-window adoption; clean only; live or opaque users refuse")
    parser.add_argument("--owner-dir", type=Path)
    parser.add_argument("--path", type=Path)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.action == "register":
        register(args.path, args.owner_dir)
        return 0
    if args.action == "guard":
        return guard(args.command[1:] if args.command[:1] == ["--"] else args.command, args.repo, args.base)
    if args.adopt_legacy_cache and (args.action != "clean" or args.cache is None):
        raise ValueError("--adopt-legacy-cache requires clean and --cache")
    report = sweep(args.repo, args.base, clean=args.action == "clean")
    if args.cache:
        report["caches"] = [cache_report(args.repo, path, clean=args.action == "clean",
                                              adopt=args.adopt_legacy_cache and path == args.cache)
                            for path in (args.cache, args.cache.with_name(args.cache.name + "-leased-v1"))]
        for field in ("reclaimable_bytes", "reclaimed_bytes", "retained_bytes"):
            report[field] += sum(row[field] for row in report["caches"])
    print(json.dumps(report, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.CalledProcessError) as exc:
        print("release scratch: " + str(exc), file=sys.stderr)
        sys.exit(1)
