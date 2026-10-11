"""Host/user admission shared by assembly and worker browser/build suites.

Workers hold weighted counting slots and shared intent leases. Proofs hold
exclusive intent; a priority lock prevents new workers joining while they drain.
The legacy budget lock is retained for compatibility with older checkouts.
No browser is started merely to sample the budget. Only ancestry-validated
nested commands may reuse a live admission; an environment flag is insufficient.

Admitted commands never receive the lease descriptors (cas-7b7b9). A flock lives
as long as any process holds its open description, so a daemon or orphan that
inherited one (an sccache server, a test's `sleep 600`) held the budget after
the suite ended. A LeaseHolder process keeps the descriptors instead: it runs
no command, releases when its owner finishes, and if the owner is killed it
holds only while the owner's tracked command process groups still have members.
"""
from contextlib import contextmanager
import fcntl
import json
import os
import re
from pathlib import Path
import secrets
import shutil
import select
import signal
import stat
import subprocess
import sys
import tempfile
import time

DIRECTORY = Path('/var/tmp') / f'cas-host-memory-{os.getuid()}'
LEASE_ENV = 'CAS_HOST_MEMORY_LEASE'
GIB = 1024**3
HEADROOM_BYTES = 2 * GIB
DEFAULT_ESTIMATE_BYTES = 4 * GIB
HOLD_POLL_SECS = 0.2
STALE_SCAN_SECS = 5


def private_directory(directory):
    try:
        directory.mkdir(mode=0o700)
    except FileExistsError:
        pass
    info = directory.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700:
        raise ValueError('unsafe host memory lease directory')


def private_file(path, create=True):
    fd = os.open(path, os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC | (os.O_CREAT if create else 0), 0o600)
    info = os.fstat(fd)
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o600:
        os.close(fd)
        raise ValueError('unsafe host memory lease file')
    return os.fdopen(fd, 'r+')


def parent_pid(pid):
    if Path('/proc').is_dir():
        # comm can contain spaces/parentheses, so split after its last ')'.
        fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
        return int(fields[1])
    return int(subprocess.check_output(['ps', '-o', 'ppid=', '-p', str(pid)], timeout=1, text=True).strip())


def inherited(env, directory=None):
    directory = directory or DIRECTORY
    try:
        record = json.loads(env[LEASE_ENV])
        pid = int(record['pid'])
        token = record['token']
        if pid <= 1 or not isinstance(token, str) or len(token) != 32:
            return False
        with private_file(directory / f'lease-{pid}-{token}.json', False) as claim:
            if json.load(claim) != record:
                return False
        ancestor = os.getpid()
        for _ in range(64):
            if ancestor == pid:
                # A stale metadata file or inherited variable cannot waive admission.
                lock = record.get('slot', 'budget.lock')
                if lock != 'budget.lock' and not re.fullmatch(r'slot-[0-9]+\.lock', lock):
                    return False
                with private_file(directory / lock, False) as probe:
                    try:
                        fcntl.flock(probe, fcntl.LOCK_EX | fcntl.LOCK_NB)
                        return False
                    except BlockingIOError:
                        return record['role'] in ('proof', 'worker')
            if ancestor <= 1:
                return False
            ancestor = parent_pid(ancestor)
    except (KeyError, TypeError, ValueError, OSError, subprocess.SubprocessError):
        pass
    return False


def _process_name(pid):
    try:
        comm = Path(f'/proc/{pid}/comm').read_text().strip()
        argv = Path(f'/proc/{pid}/cmdline').read_bytes().split(b'\0')
        command = ' '.join(part.decode(errors='replace') for part in argv if part)[:160]
    except OSError:
        return None
    return f'{comm} (pid {pid}: {command})'


def stale_holders(directory=None):
    """Name processes holding an admission lock whose locking process has exited.

    Each fdinfo "lock:" line names the PID that took that flock and appears only
    on descriptors sharing the locked open description, so it identifies real
    holders, not waiters that merely have the file open. A dead locking PID
    means an inheritor keeps the lock: a lease holder whose wrapper was killed,
    or a daemon/orphan that escaped an old suite. Returns [] without /proc.
    """
    directory = directory or DIRECTORY
    try:
        inodes = {}
        for path in directory.glob('*.lock'):
            info = path.stat()
            inodes[(info.st_dev, info.st_ino)] = path.name
        entries = [entry for entry in Path('/proc').iterdir() if entry.name.isdigit()]
    except OSError:
        return []
    holders = []
    for entry in entries:
        if int(entry.name) == os.getpid():
            continue
        try:
            descriptors = list((entry / 'fd').iterdir())
        except OSError:
            continue
        locks_held = set()
        for descriptor in descriptors:
            try:
                info = os.stat(descriptor)
                name = inodes.get((info.st_dev, info.st_ino))
                if not name:
                    continue
                details = (entry / 'fdinfo' / descriptor.name).read_text()
            except OSError:
                continue
            for line in details.splitlines():
                fields = line.split()
                # "lock:  1: FLOCK  ADVISORY  WRITE 1582598 fd:01:123456 0 EOF"
                if fields[:1] == ['lock:'] and len(fields) > 5 and fields[2] == 'FLOCK' \
                        and fields[5].isdigit() and not Path(f'/proc/{fields[5]}').exists():
                    locks_held.add(name)
        label = locks_held and _process_name(int(entry.name))
        if label:
            role = ('lease holder of a killed wrapper; it ends when that suite does'
                    if '--hold' in label and 'host_memory' in label else 'not an admitted suite')
            holders.append(f"{label} holds {', '.join(sorted(locks_held))} ({role})")
    return sorted(holders)


def default_report(event):
    if event['reason'] != 'admitted':
        print(f"waiting for host memory ({event['reason']}), {event['elapsed_s']:g} s", flush=True)
    print('host memory admission: ' + json.dumps(event, sort_keys=True), flush=True)


def emit(role, reason, started, report, memory=None):
    report({'phase': 'host-memory-admission', 'role': role, 'reason': reason,
            'elapsed_s': round(time.monotonic() - started, 3), **(memory or {})})


@contextmanager
def admission(role, env, memory_budget, wait_secs=600, poll_secs=1, directory=None, report=None,
              estimate_bytes=DEFAULT_ESTIMATE_BYTES):
    directory = directory or DIRECTORY
    report = report or default_report
    if role not in ('proof', 'worker'):
        raise ValueError('unknown host memory admission role')
    if type(estimate_bytes) is not int or estimate_bytes <= 0:
        raise ValueError('invalid worker memory estimate')
    private_directory(directory)
    if inherited(env, directory):
        # Preserve the open descriptions through nested Popen(close_fds=True).
        # Legacy claims have no FD list and retain their old inheritance shape.
        record = json.loads(env[LEASE_ENV])
        recorded_fds = tuple(record.get('fds', ()))
        fds = []
        paths = ('intent.lock', record.get('slot', 'budget.lock'))
        if recorded_fds:
            if len(recorded_fds) != 2 or any(type(fd) is not int or fd < 0 for fd in recorded_fds):
                raise ValueError('invalid inherited host memory descriptors')
            for fd, path in zip(recorded_fds, paths):
                with private_file(directory / path, False) as probe:
                    expected = os.fstat(probe.fileno())
                try:
                    info = os.fstat(fd)
                except OSError:
                    continue
                if (info.st_dev, info.st_ino) == (expected.st_dev, expected.st_ino):
                    fds.append(fd)
        # npm/Node may close extra descriptors between wrappers. The validated
        # live ancestor still owns admission; never pass a reused unrelated FD.
        yield dict(env), tuple(fds)
        return
    started = time.monotonic()
    with private_file(directory / 'priority.lock') as priority, \
         private_file(directory / 'intent.lock') as intent, private_file(directory / 'budget.lock') as budget:
        priority_held = intent_held = False
        slot = None
        next_scan, holders = time.monotonic(), []
        try:
            while True:
                # Proof phases sample immediately after this lease; do not consume
                # a sample ahead of their detailed producer/link/test admission.
                memory = {}
                reason = 'proof pending' if role == 'worker' else 'worker suite running'
                try:
                    if not priority_held:
                        fcntl.flock(priority, (fcntl.LOCK_EX if role == 'proof' else fcntl.LOCK_SH) | fcntl.LOCK_NB)
                        priority_held = True
                    if not intent_held:
                        if role == 'worker': reason = 'proof running'
                        fcntl.flock(intent, (fcntl.LOCK_EX if role == 'proof' else fcntl.LOCK_SH) | fcntl.LOCK_NB)
                        intent_held = True
                    reason = 'worker suite running'
                    # EX also drains old proofs, which only hold budget SH.
                    fcntl.flock(budget, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    # Workers use budget.lock only as an allocation mutex. Old
                    # worker EX and proof SH leases still exclude new admissions.
                    if role == 'worker':
                        memory = memory_budget(env)
                        slot, reserved, slot_name = counting_slot(directory)
                        memory = dict(memory, reserved_bytes=reserved, estimate_bytes=estimate_bytes,
                                      headroom_bytes=HEADROOM_BYTES)
                    if role == 'proof' or memory['budget_bytes'] >= reserved + estimate_bytes + HEADROOM_BYTES:
                        if slot:
                            slot.write(str(estimate_bytes))
                            slot.truncate()
                            slot.flush()
                            fcntl.flock(budget, fcntl.LOCK_UN)
                        emit(role, 'admitted', started, report, memory)
                        break
                    slot.close()
                    slot = None
                    reason = 'worker suite estimate + headroom exceeds fresh memory budget'
                    fcntl.flock(budget, fcntl.LOCK_UN)
                except BlockingIOError:
                    if role == 'worker' and priority_held:
                        try:
                            fcntl.flock(budget, fcntl.LOCK_SH | fcntl.LOCK_NB)
                            reason = 'proof running'
                            fcntl.flock(budget, fcntl.LOCK_UN)
                        except BlockingIOError:
                            pass
                # Proof keeps priority while draining old/new workers. Workers
                # release all transient gates across a wait, so light commands
                # may use the remaining budget ahead of a larger waiter.
                if role == 'worker' and intent_held:
                    fcntl.flock(intent, fcntl.LOCK_UN)
                    intent_held = False
                if role == 'worker' and priority_held:
                    fcntl.flock(priority, fcntl.LOCK_UN)
                    priority_held = False
                if time.monotonic() >= next_scan:
                    next_scan = time.monotonic() + STALE_SCAN_SECS
                    holders = stale_holders(directory)
                if holders:
                    # Name the inheritor instead of blaming a suite that ended.
                    reason = 'admission lock held after its suite exited by ' + '; '.join(holders)
                emit(role, reason, started, report, memory)
                elapsed = time.monotonic() - started
                if elapsed >= wait_secs:
                    raise ValueError(f'{role} host memory admission deadline expired after {wait_secs}s: {reason}; command was not started')
                time.sleep(min(poll_secs, wait_secs - elapsed))
            fcntl.flock(priority, fcntl.LOCK_UN)
            priority_held = False
            token = secrets.token_hex(16)
            claim_path = directory / f'lease-{os.getpid()}-{token}.json'
            record = {'pid': os.getpid(), 'token': token, 'role': role}
            if slot:
                record['slot'] = slot_name
            fds = (intent.fileno(), slot.fileno()) if slot else (intent.fileno(), budget.fileno())
            record['fds'] = list(fds)
            with private_file(claim_path) as claim:
                json.dump(record, claim)
                claim.flush()
            try:
                yield dict(env, **{LEASE_ENV: json.dumps(record)}), fds
            finally:
                claim_path.unlink(missing_ok=True)
        finally:
            # Closing, rather than LOCK_UN, preserves the lease if a killed
            # wrapper still has descendants carrying its open description.
            if slot:
                slot.close()
            if priority_held:
                fcntl.flock(priority, fcntl.LOCK_UN)


def counting_slot(directory):
    """Under budget.lock, count live FD reservations and lock one reusable slot.

    Metadata alone never reserves capacity. Never unlink slot files: descendants
    may retain their locks even after an abruptly killed wrapper disappears.
    """
    reserved = 0
    candidate = None
    try:
        paths = sorted(directory.glob('slot-*.lock'))
        for path in paths:
            if not re.fullmatch(r'slot-[0-9]+\.lock', path.name):
                raise ValueError('unsafe host memory slot name')
            stream = private_file(path, False)
            try:
                fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                try:
                    weight = int(stream.read())
                    if weight <= 0:
                        raise ValueError('invalid live host memory reservation')
                    reserved += weight
                finally:
                    stream.close()
            else:
                if candidate is None:
                    candidate = stream
                    candidate_name = path.name
                else:
                    stream.close()
        if candidate is None:
            # All existing slots are live; find a new name without overwriting.
            index = 0
            while (directory / f'slot-{index}.lock').exists():
                index += 1
            candidate = private_file(directory / f'slot-{index}.lock')
            candidate_name = f'slot-{index}.lock'
            fcntl.flock(candidate, fcntl.LOCK_EX | fcntl.LOCK_NB)
        return candidate, reserved, candidate_name
    except BaseException:
        if candidate:
            candidate.close()
        raise


class LeaseHolder:
    """Keep admission descriptors alive in a process that runs no command.

    The owner passes nothing to its commands. It tracks each command's process
    group here. close() ends the hold at once; if the owner dies without
    closing, or closes with release=False on an error path, the holder keeps
    the lease only while a tracked group has members, so a killed wrapper
    cannot admit a proof over its running suite.
    """

    def __init__(self, fds, poll_secs=HOLD_POLL_SECS):
        self.process = self._write = None
        if not fds:
            return  # nested reuse: the live ancestor's holder already holds it
        read, self._write = os.pipe()  # both ends close-on-exec in the owner
        try:
            self.process = subprocess.Popen(
                [sys.executable, str(Path(__file__).resolve()), '--hold', str(read), str(poll_secs)],
                pass_fds=(read, *fds), start_new_session=True,
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        except BaseException:
            os.close(self._write)
            self._write = None
            raise
        finally:
            os.close(read)

    def track(self, pgid):
        if self._write is None:
            return
        try:
            os.write(self._write, f'pgid {int(pgid)}\n'.encode())
        except OSError:
            pass  # a dead holder only loses the killed-wrapper cover; the owner still holds

    def close(self, release=True):
        """release=False keeps the lease until tracked groups end (error paths)."""
        if self._write is None:
            return
        try:
            if release:
                os.write(self._write, b'release\n')
        except OSError:
            pass
        os.close(self._write)
        self._write = None
        if release:
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()

    def __enter__(self):
        return self

    def __exit__(self, kind, value, traceback):
        self.close(release=kind is None)


def _group_alive(pgid):
    try:
        os.killpg(pgid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def hold(read_fd, poll_secs=HOLD_POLL_SECS):
    """The LeaseHolder process: hold inherited descriptors until released."""
    for sig in (signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, signal.SIG_IGN)
    groups, pending = set(), b''
    while True:
        select.select([read_fd], [], [])
        chunk = os.read(read_fd, 4096)
        if not chunk:
            break  # owner exited without releasing
        pending += chunk
        while b'\n' in pending:
            line, pending = pending.split(b'\n', 1)
            if line == b'release':
                return 0
            if line.startswith(b'pgid '):
                groups.add(int(line[5:]))
    while any(_group_alive(group) for group in groups):
        time.sleep(poll_secs)
    return 0


LEASE_ENV_KEYS = (LEASE_ENV, 'CAS_RELEASE_GATE_SCRATCH_LEASE_FDS')


def start_compiler_cache(env):
    """Start sccache's server outside every lease before admitted builds.

    sccache's client daemonizes a server on first use, and that server would
    otherwise be a child of the admitted build. Starting it here, with no lease
    variables and no inherited descriptors, keeps it out of every admission.
    "Address in use" means a server already runs; any failure is ignored and
    the build still runs normally.
    """
    wrapper = _sccache_wrapper(env)
    if not wrapper:
        return False
    clean = {key: value for key, value in env.items() if key not in LEASE_ENV_KEYS}
    # cas-3a29: the server outlives this build and serves every later one,
    # so it must never inherit a run's temporary TMPDIR: once that directory
    # is deleted, every compile through it fails "Failed to create temp dir".
    stable = compiler_cache_tmpdir(env)
    try:
        stable.mkdir(parents=True, exist_ok=True)
        clean['TMPDIR'] = str(stable)
    except OSError:
        pass
    try:
        subprocess.run([wrapper, '--start-server'], env=clean, stdin=subprocess.DEVNULL,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                       start_new_session=True, timeout=30)
    except (OSError, subprocess.SubprocessError):
        return False
    return True


SCCACHE_RESTART_HINT = 'sccache --stop-server'


def _sccache_wrapper(env):
    wrapper = env.get('RUSTC_WRAPPER') or env.get('CARGO_BUILD_RUSTC_WRAPPER')
    return wrapper if wrapper and Path(wrapper).name == 'sccache' else None


def compiler_cache_tmpdir(env):
    """The TMPDIR the shared sccache server runs under (cas-3a29).

    A sibling of the sccache cache directory, so it lives as long as the
    cache, never under a run's temporary directory and never inside the cache
    itself (which sccache sizes and evicts). `CAS_SCCACHE_TMPDIR` overrides.
    """
    override = env.get('CAS_SCCACHE_TMPDIR')
    if override:
        return Path(override)
    cache = env.get('SCCACHE_DIR') or str(Path(env.get('HOME') or Path.home()) / '.cache' / 'sccache')
    return Path(cache.rstrip('/') + '-tmp')


def _compiler_cache_canary(env, wrapper):
    """Compile a one-line crate through the server: ``(ok, output)``.

    A compile is what needs the server's temporary directory, so only a
    compile proves the server healthy; ``--show-stats`` does not.
    """
    rustc = env.get('RUSTC') or shutil.which('rustc', path=env.get('PATH'))
    if not rustc:
        return True, 'no rustc on PATH; compiler cache not probed'
    clean = {key: value for key, value in env.items() if key not in LEASE_ENV_KEYS}
    parent = compiler_cache_tmpdir(env)
    try:
        parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix='cas-sccache-canary-', dir=parent) as scratch:
            source = Path(scratch) / 'canary.rs'
            source.write_text('pub fn canary() {}\n')
            result = subprocess.run(
                [wrapper, rustc, '--crate-name', 'cas_sccache_canary', '--crate-type', 'lib',
                 '--emit=metadata', '--out-dir', scratch, str(source)],
                env=clean, stdin=subprocess.DEVNULL, capture_output=True, text=True,
                start_new_session=True, timeout=120)
    except (OSError, subprocess.SubprocessError) as error:
        return False, f'sccache: canary could not run: {error}'
    return result.returncode == 0, (result.stderr + result.stdout).strip()


def ensure_compiler_cache(env):
    """Start sccache's server and prove it can compile (cas-3a29).

    Returns ``(status, detail)``:

    - ``off``: no sccache wrapper, nothing to do.
    - ``ok``: the server compiled the canary.
    - ``restarted``: the canary failed in sccache (typically a server whose
      TMPDIR was deleted); the server was stopped and restarted under the
      stable TMPDIR, and then compiled. ``detail`` is the original failure.
    - ``blocked``: sccache still cannot compile. ``detail`` is a named
      environment blocker with the restart command, for the caller to report
      instead of a build or test failure.

    A canary failure that is not sccache's own (rustc itself failing) is
    reported ``ok`` with its output, so the build runs and fails on its own
    terms.
    """
    wrapper = _sccache_wrapper(env)
    if not wrapper:
        return 'off', ''
    # A wrapper that does not exist is not a server to heal: there is nothing
    # to restart, and a real build fails on its own terms (cas-3a29: CI
    # runners export RUSTC_WRAPPER=sccache without installing it).
    if not (os.path.isfile(wrapper) or shutil.which(wrapper, path=env.get('PATH'))):
        return 'off', f'{wrapper} is not installed'
    start_compiler_cache(env)
    ok, output = _compiler_cache_canary(env, wrapper)
    if ok:
        return 'ok', ''
    if 'sccache' not in output.lower():
        return 'ok', output
    clean = {key: value for key, value in env.items() if key not in LEASE_ENV_KEYS}
    try:
        subprocess.run([wrapper, '--stop-server'], env=clean, stdin=subprocess.DEVNULL,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                       start_new_session=True, timeout=30)
    except (OSError, subprocess.SubprocessError):
        pass
    start_compiler_cache(env)
    retried, retry_output = _compiler_cache_canary(env, wrapper)
    if retried:
        return 'restarted', output
    first = next((line for line in (retry_output or output).splitlines() if line.strip()), 'no output')
    return 'blocked', (
        f'ENVIRONMENT BLOCKER: the sccache compiler cache cannot compile ({first}), even after a '
        f'restart under {compiler_cache_tmpdir(env)}. This is a host problem, not a build or test '
        f'failure. Fix: run `{SCCACHE_RESTART_HINT}` (the next build starts a fresh server), or set '
        f'RUSTC_WRAPPER= to build uncached, then rerun.')


if __name__ == '__main__':
    if sys.argv[1:2] == ['--hold'] and len(sys.argv) == 4:
        sys.exit(hold(int(sys.argv[2]), float(sys.argv[3])))
    sys.exit('usage: host_memory.py --hold <fd> <poll-secs>')
