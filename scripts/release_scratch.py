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
import re
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
REMAP_RECEIPT = ".cas-release-remap.json"
REGENERABLE = ("suite.tar.zst", "extract", "tmp", "cargo-home", "bin")
INVENTORY_ROOTS = set()


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
    result = subprocess.run(["ps", "-p", str(pid), "-o", "lstart="], env=dict(os.environ, LC_ALL="C"), capture_output=True, text=True, check=True)
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
    if not isinstance(owner, dict):
        return None
    stat = path.stat()
    if stat.st_uid != os.getuid() or stat.st_mode & 0o077:
        return None  # The lease protocol requires a private, same-user directory.
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
    except (OSError, ValueError, KeyError, IndexError):
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
    if owner and not (lease.name == LOCK and lease.parent.resolve() == path.resolve()):
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


def _file_size(item):
    # A live run may remove a file between enumeration and stat; a vanished
    # file occupies no space, so it counts as zero rather than aborting.
    try:
        return 0 if item.is_symlink() else item.stat(follow_symlinks=False).st_size
    except FileNotFoundError:
        return 0


def size(path):
    return sum(_file_size(Path(parent) / name)
               for parent, dirs, files in os.walk(path, followlinks=False)
               for name in files)


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
                if process.stat().st_uid != os.getuid() and lease_managed:
                    continue  # Verified private lease scratch excludes other users.
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
                arguments = (process / "cmdline").read_bytes().split(b"\0")
                for index, argument in enumerate(arguments):
                    value = os.fsdecode(argument).split("=", 1)[-1]
                    # The GC's --cache argument describes this inventory, not
                    # a user of its output. Keep own cwd/exe/maps/other FDs live.
                    if (int(process.name) == os.getpid() and index > 0
                            and arguments[index - 1] == b"--cache" and value in INVENTORY_ROOTS):
                        continue
                    if value.startswith("/") and contains(path, Path(value)):
                        return True
                for line in (process / "maps").read_text().splitlines():
                    fields = line.split(maxsplit=5)
                    if len(fields) == 6 and fields[5].startswith("/") and contains(path, Path(fields[5])):
                        return True
            except FileNotFoundError:
                if process.exists() and not lease_managed:
                    return True  # Missing evidence of a still-present process is unknown.
            except (PermissionError, ValueError, IndexError):
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


def finish_lease(resource):
    """Drop our own open-file description before testing for surviving heirs."""
    if resource.finished:
        return not resource.deferred
    resource.finished = True
    if CURRENT is not None:
        with CURRENT.lock:
            CURRENT.leases.discard(resource.lock.fileno())
    lease_path = resource.lease_path
    resource.lock.close()
    resource.lock = open_lock(lease_path, create=False)
    try:
        fcntl.flock(resource.lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        return True
    except BlockingIOError:
        resource.deferred = True
        resource.lock.close()
        print(json.dumps({"scratch_retained": str(resource.path),
                          "retained_bytes": size(resource.path),
                          "reason": "surviving descendant holds inherited lifetime lease"}), file=sys.stderr)
        return False


class OwnedDirectory:
    def __init__(self, prefix, parent):
        # Pending graceful signals may arrive as soon as masking is lifted.
        # Register a fallback cleanup with the enclosing scope first.
        with defer_signals():
            self.path = Path(tempfile.mkdtemp(prefix=prefix, dir=parent))
            self.lease_path = self.path / LOCK
            self.finished = self.deferred = False
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
            if not finish_lease(self):
                return
            if self.path.exists():
                shutil.rmtree(self.path)
        finally:
            if CURRENT is not None:
                with CURRENT.lock:
                    if not self.lock.closed:
                        CURRENT.leases.discard(self.lock.fileno())
                    CURRENT.directories.discard(self)
            self.lock.close()


class ChildScope:
    """Stop and reap process groups before any owned directory is removed."""
    def __init__(self):
        self.children = set()
        # Called with each child's process group id right after it starts.
        self.spawn_hooks = []
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
                for hook in list(self.spawn_hooks):
                    hook(child.pid)
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


def registered(path, protected, generated_remap=False):
    remap = path / "workspace-remap/.git"
    registry = path / "repo/.git/worktrees"
    linked = any((Path(parent) / '.git').is_file() or (Path(parent) / '.git').is_symlink()
                 for parent, dirs, files in os.walk(path, followlinks=False)
                 if not (generated_remap and Path(parent) == path / 'workspace-remap'))
    return (any(contains(path.resolve(), tree) for tree in protected)
            or ((remap.is_file() or remap.is_symlink()) and not generated_remap)
            or linked or (registry.is_dir() and any(registry.iterdir())))


def git_output(repo, *args):
    env = {key: value for key, value in os.environ.items()
           if key not in ('GIT_DIR', 'GIT_WORK_TREE', 'GIT_COMMON_DIR', 'GIT_INDEX_FILE')}
    env['GIT_OPTIONAL_LOCKS'] = '0'
    return subprocess.check_output(['git', '-C', str(repo), *args], env=env, stderr=subprocess.PIPE)


def git_rows(common):
    raw = git_output(common, '--git-dir=' + str(common), 'worktree', 'list', '--porcelain', '-z')
    rows, row = {}, {}
    for field in raw.split(b'\0'):
        if not field:
            if row:
                rows[row['worktree']] = row
                row = {}
        else:
            key, _, value = os.fsdecode(field).partition(' ')
            row[key] = str(Path(value).resolve()) if key == 'worktree' else value
    return rows


def file_identity(path, directory=False):
    import stat
    metadata = path.lstat()
    if (metadata.st_uid != os.getuid() or stat.S_ISLNK(metadata.st_mode)
            or (not stat.S_ISDIR(metadata.st_mode) if directory else not stat.S_ISREG(metadata.st_mode))):
        raise ValueError('unsafe remap identity: ' + str(path))
    return [metadata.st_dev, metadata.st_ino]


def admin_identity(admin, common):
    """Validate the registry path before Git or Python can follow its metadata."""
    if admin.parent != common / 'worktrees' or admin.name in ('', '.', '..'):
        raise ValueError('generated remap belongs to another Git registry')
    registry_identity = file_identity(admin.parent, directory=True)
    identity = file_identity(admin, directory=True)
    # Git reads HEAD, gitdir, commondir and index; reject any unexpected links
    # anywhere in this admin entry, including auxiliary config/log metadata.
    for parent, dirs, files in os.walk(admin, followlinks=False):
        for name in dirs:
            file_identity(Path(parent) / name, directory=True)
        for name in files:
            file_identity(Path(parent) / name)
    for name in ('HEAD', 'gitdir', 'commondir'):
        file_identity(admin / name)
    if (admin / (admin / 'commondir').read_text().strip()).resolve() != common:
        raise ValueError('generated remap common directory changed')
    return registry_identity, identity


def validate_remap_receipt(receipt):
    if not isinstance(receipt, dict) or receipt.get('protocol') != 'cas-release-remap-v1':
        raise ValueError('invalid generated remap receipt')
    for key in ('remap', 'common', 'admin'):
        value = receipt.get(key)
        if (not isinstance(value, str) or not value or '\0' in value
                or not Path(value).is_absolute() or str(Path(value)) != value
                or '..' in Path(value).parts):
            raise ValueError('invalid generated remap receipt path: ' + key)
    for key in ('remap_identity', 'git_file_identity', 'common_identity', 'registry_identity', 'admin_identity'):
        value = receipt.get(key)
        if (not isinstance(value, list) or len(value) != 2
                or any(type(item) is not int or item < 0 for item in value)):
            raise ValueError('invalid generated remap receipt identity: ' + key)
    if not isinstance(receipt.get('head'), str) or not re.fullmatch(r'[0-9a-f]{40}|[0-9a-f]{64}', receipt['head']):
        raise ValueError('invalid generated remap receipt HEAD')


def remap_owner_identity(owner):
    return {key: owner[key] for key in ('protocol', 'uid', 'pid', 'start', 'path', 'device', 'inode', 'lease')}


def remap_identity(base, common, receipt=None):
    remap = base / 'workspace-remap'
    present = remap.exists() or remap.is_symlink()
    if present:
        file_identity(remap, directory=True)
    if any(parent.name == '.cas' for parent in remap.parents) or (remap / '.cas').exists() or (remap / '.cas').is_symlink():
        raise ValueError('Cassy checkout is never a reclaimable generated remap')
    git_file = remap / '.git'
    if not present and receipt:
        admin = Path(receipt['admin'])
    else:
        file_identity(git_file)
        pointer = git_file.read_text().strip()
        if not pointer.startswith('gitdir: '):
            raise ValueError('invalid generated remap Git pointer')
        admin = Path(pointer[8:])
        if not admin.is_absolute():
            admin = remap / admin
    registry_identity, identity = admin_identity(admin, common)
    row = git_rows(common).get(str(remap))
    if not row or 'detached' not in row or 'branch' in row or 'locked' in row:
        raise ValueError('remap is not an unlocked generated detached worktree')
    if Path((admin / 'gitdir').read_text().strip()) != git_file:
        raise ValueError('generated remap Git backlink changed')
    if (admin / 'HEAD').read_text().strip() != row['HEAD']:
        raise ValueError('generated remap HEAD is not detached')
    if remap.exists() and git_output(remap, 'status', '--porcelain', '--untracked-files=all'):
        raise ValueError('generated remap has delivery changes')
    return {'remap': str(remap), 'remap_identity': file_identity(remap, directory=True) if remap.exists() else receipt['remap_identity'],
            'git_file_identity': file_identity(git_file) if remap.exists() else receipt['git_file_identity'], 'common': str(common),
            'common_identity': file_identity(common, directory=True), 'admin': str(admin),
            'registry_identity': registry_identity, 'admin_identity': identity, 'head': row['HEAD']}


def register_remap(base, repo):
    """Record only a fresh generated detached checkout under a live held owner."""
    base = Path(base).resolve()
    owner = read_owner(base)
    if not owner or not owner_live(owner):
        raise ValueError('remap registration requires a live verified scratch owner')
    with open_lock(Path(owner['lease'])) as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            pass
        else:
            raise ValueError('remap registration requires a held lifetime lease')
    common = Path(os.fsdecode(git_output(repo, 'rev-parse', '--path-format=absolute', '--git-common-dir')).strip()).resolve()
    identity = remap_identity(base, common)
    receipt = dict(identity, protocol='cas-release-remap-v1', owner=remap_owner_identity(owner))
    # Exclusive creation prevents a stale receipt being silently overwritten.
    with (base / REMAP_RECEIPT).open('x') as stream:
        json.dump(receipt, stream)
        stream.flush()
        os.fsync(stream.fileno())
    directory = os.open(base, os.O_RDONLY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def unregister_generated_remap(base, owner, clean):
    """Caller already holds the dead/released owner's exclusive lifetime lease."""
    base = base.resolve()
    receipt_path = base / REMAP_RECEIPT
    if not owner or (not receipt_path.exists() and not receipt_path.is_symlink()):
        return False
    receipt_identity = file_identity(receipt_path)
    with open_lock(receipt_path) as stream:
        receipt = json.load(stream)
    validate_remap_receipt(receipt)
    if receipt.get('owner') != remap_owner_identity(owner):
        raise ValueError('generated remap owner receipt changed')
    if read_owner(base) != owner:
        raise ValueError('generated remap base identity changed')
    common = Path(receipt['common'])
    if common.resolve() != common or file_identity(common, directory=True) != receipt.get('common_identity'):
        raise ValueError('generated remap registry changed')
    remap = base / 'workspace-remap'
    admin = Path(receipt['admin'])
    if admin.parent != common / 'worktrees' or receipt.get('remap') != str(remap):
        raise ValueError('generated remap receipt escapes its registry/base')
    registry = common / 'worktrees'
    if registry.exists() or registry.is_symlink():
        if file_identity(registry, directory=True) != receipt['registry_identity']:
            raise ValueError('generated remap registry identity changed')
    if not remap.exists() and not remap.is_symlink() and not admin.exists() and not admin.is_symlink() and str(remap) not in git_rows(common):
        return True  # Targeted Git removal completed before a prior interruption.
    identity = remap_identity(base, common, receipt)
    if any(receipt.get(key) != value for key, value in identity.items()):
        raise ValueError('generated remap Git identity changed')
    if not clean:
        return True
    # Targeted Git removal also prunes this exact admin entry. Never globally
    # prune unrelated missing parked checkouts or force-remove dirty worktrees.
    with open_lock(receipt_path) as stream:
        current_receipt = json.load(stream)
    if (read_owner(base) != owner or file_identity(receipt_path) != receipt_identity
            or current_receipt != receipt or remap_identity(base, common, receipt) != identity):
        raise ValueError('generated remap changed before unregistration')
    git_output(common, '--git-dir=' + str(common), 'worktree', 'remove', identity['remap'])
    if Path(identity['admin']).exists() or Path(identity['admin']).is_symlink() or identity['remap'] in git_rows(common):
        raise ValueError('generated remap unregister did not remove exact metadata')
    return True


def sweep(repo, base, clean=False, env=None):
    env = os.environ if env is None else env
    cutoff = time.time() - positive(env, "CAS_RELEASE_SCRATCH_MAX_AGE_HOURS", 6) * 3600
    records = []
    protected = worktrees(repo)
    for parent, prefixes in scratch_parents(base, env).items():
        if not parent.is_dir():
            continue
        lock_path = parent / f".cas-scratch-sweep-{os.getuid()}.lock"
        if lock_path.is_symlink():
            continue
        context = open_lock(lock_path, create=clean) if clean or lock_path.exists() else contextlib.nullcontext()
        with context as sweep_lock:
            if sweep_lock:
                fcntl.flock(sweep_lock, fcntl.LOCK_EX if clean else fcntl.LOCK_SH)
            for path in sorted(parent.iterdir()):
                if not path.name.startswith(prefixes) or path.is_symlink() or not path.is_dir():
                    continue
                path = path.resolve()
                stat = path.stat()
                if stat.st_uid != os.getuid():
                    continue
                row = {"path": str(path), "bytes": size(path), "reclaimable_bytes": 0,
                       "reclaimed_bytes": 0, "retained_bytes": 0, "reclaimable": False,
                       "reason": "recent", "removed": False, "retained_base": False}
                try:
                    owner = read_owner(path)
                    if owner is not None or stat.st_mtime <= cutoff:
                        with admitted(path, owner, clean):
                            generated = unregister_generated_remap(path, owner, clean)
                            protected = worktrees(repo)
                            if generated and not clean:
                                protected = [tree for tree in protected if tree != path / 'workspace-remap']
                            keep_base = registered(path, protected, generated_remap=generated and not clean)
                            # Only exact generated receipts may unregister a worktree;
                            # unknown/worker/parked checkouts retain their whole base.
                            candidates = ([path / name for name in REGENERABLE
                                           if (path / name).exists() and not (path / name).is_symlink()]
                                          if keep_base and owner else ([] if keep_base else [path]))
                            row["retained_base"] = keep_base
                            row["reason"] = "registered worktree retained; regenerable siblings only" if keep_base else ("dead verified owner" if owner else "unknown provenance past age bound")
                            for candidate in candidates:
                                if any(contains(candidate.resolve(), tree) for tree in protected):
                                    continue
                                used = size(candidate) if candidate.is_dir() else _file_size(candidate)
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
                except (OSError, ValueError, subprocess.CalledProcessError, KeyError, TypeError) as exc:
                    row["reason"] = "protected: " + str(exc)
                row["retained_bytes"] = size(path) if path.exists() else 0
                records.append(row)
    # Targeted Git removal cleans only receipt-matching admin metadata; never
    # globally prune missing parked/delivery worktrees from another run.
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
        self.finished = self.deferred = False
        self.lease_path = self.path.with_name(self.path.name + ".lock")

    def inventory(self, clean=False, adopt=False):
        row = {"path": str(self.path), "bytes": 0, "reclaimed_bytes": 0,
               "retained_bytes": 0, "reclaimable_bytes": 0, "reason": "absent",
               "cap_bytes": self.cap, "max_age_secs": self.age}
        if self.path.is_symlink():
            raise ValueError("assembly target must not be a symlink")
        if not self.path.exists():
            self.events.append(row)
            return row
        if self.path.stat().st_uid != os.getuid():
            raise ValueError("assembly target belongs to another user")
        row["bytes"] = row["retained_bytes"] = size(self.path)
        if self.repo and any(contains(self.path.resolve(), tree) for tree in worktrees(self.repo)):
            raise ValueError("assembly target contains a registered worktree")
        owner = read_owner(self.path)
        if adopt and not owner:
            # Explicit quiet-window operation; opaque evidence STILL refuses.
            if process_uses(self.path.resolve(), self.lock.fileno()):
                raise ValueError("legacy cache adoption refused: live users or unavailable process evidence")
            self.path.chmod(0o700)
            (self.path / OWNER).write_text(json.dumps(owner_record(self.path, self.path.with_name(self.path.name + ".lock"))))
            owner = read_owner(self.path)
            self.managed = True
            row["adopted"] = True
        if owner and Path(owner["lease"]) != self.path.with_name(self.path.name + ".lock"):
            raise ValueError("cache owner references an unexpected lease")
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
                if not self.lock.closed:
                    CURRENT.leases.discard(self.lock.fileno())
                CURRENT.caches.discard(self)
        self.lock.close()

    def __exit__(self, *exc):
        if self.lock.closed:
            return
        try:
            if CURRENT is not None:
                CURRENT.stop()
            if not finish_lease(self):
                used = size(self.path)
                self.events.append({"path": str(self.path), "bytes": used,
                                    "retained_bytes": used, "reclaimed_bytes": 0,
                                    "reason": "surviving descendant holds cache lease"})
                return
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
    with ChildScope() as scope:
        lease = OwnedDirectory("cas-release-gate.", tempfile.gettempdir())
        with lease as owner_dir:
            try:
                env = dict(os.environ, CAS_RELEASE_GATE_SCRATCH_RUN_DIR=str(owner_dir))
                return scope.run(command, env=env).returncode
            finally:
                scope.stop()
                # Escaped descendants retaining the lease preserve all their
                # paths. Cleanup never outruns even a detached surviving heir.
                if finish_lease(lease):
                    path_list = owner_dir / "paths"
                    if path_list.exists():
                        for line in reversed(path_list.read_text().splitlines()):
                            path = Path(json.loads(line))
                            owner = read_owner(path) if path.exists() and not path.is_symlink() else None
                            current = read_owner(owner_dir)
                            if not owner or owner["pid"] != current["pid"] or owner["start"] != current["start"] or owner["lease"] != str(owner_dir / LOCK):
                                continue
                            try:
                                if process_uses(path.resolve(), lease_managed=True):
                                    raise ValueError('live output retained')
                                unregister_generated_remap(path, owner, True)
                                if registered(path, worktrees(repo)) or process_uses(path.resolve(), lease_managed=True):
                                    raise ValueError('registered checkout or live output retained')
                                if path.exists() and not path.is_symlink():
                                    shutil.rmtree(path)
                            except (OSError, ValueError, subprocess.CalledProcessError, KeyError, TypeError) as exc:
                                print(json.dumps({'scratch_retained': str(path), 'retained_bytes': size(path),
                                                  'reason': str(exc)}), file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("report", "clean", "register", "register-remap", "guard"))
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
    if args.action == 'register-remap':
        register_remap(args.path, args.repo)
        return 0
    if args.action == "guard":
        return guard(args.command[1:] if args.command[:1] == ["--"] else args.command, args.repo, args.base)
    if args.adopt_legacy_cache and (args.action != "clean" or args.cache is None):
        raise ValueError("--adopt-legacy-cache requires clean and --cache")
    report = sweep(args.repo, args.base, clean=args.action == "clean")
    if args.cache:
        INVENTORY_ROOTS.add(str(args.cache.absolute()))
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
