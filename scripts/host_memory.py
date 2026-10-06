"""Host/user admission shared by assembly and worker browser/build suites.

Workers hold weighted counting slots and shared intent leases. Proofs hold
exclusive intent; a priority lock prevents new workers joining while they drain.
The legacy budget lock is retained for compatibility with older checkouts.
No browser is started merely to sample the budget. Only ancestry-validated
nested commands may reuse a live admission; an environment flag is insufficient.
"""
from contextlib import contextmanager
import fcntl
import json
import os
import re
from pathlib import Path
import secrets
import stat
import subprocess
import time

DIRECTORY = Path('/var/tmp') / f'cas-host-memory-{os.getuid()}'
LEASE_ENV = 'CAS_HOST_MEMORY_LEASE'
GIB = 1024**3
HEADROOM_BYTES = 2 * GIB
DEFAULT_ESTIMATE_BYTES = 4 * GIB


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
        fds = tuple(record.get('fds', ()))
        paths = ('intent.lock', record.get('slot', 'budget.lock'))
        if fds:
            if len(fds) != 2 or any(type(fd) is not int or fd < 0 for fd in fds):
                raise ValueError('invalid inherited host memory descriptors')
            for fd, path in zip(fds, paths):
                info = os.fstat(fd)
                with private_file(directory / path, False) as probe:
                    expected = os.fstat(probe.fileno())
                if (info.st_dev, info.st_ino) != (expected.st_dev, expected.st_ino):
                    raise ValueError('invalid inherited host memory descriptors')
        yield dict(env), fds
        return
    started = time.monotonic()
    with private_file(directory / 'priority.lock') as priority, \
         private_file(directory / 'intent.lock') as intent, private_file(directory / 'budget.lock') as budget:
        priority_held = intent_held = budget_held = False
        slot = None
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
                        fcntl.flock(intent, (fcntl.LOCK_EX if role == 'proof' else fcntl.LOCK_SH) | fcntl.LOCK_NB)
                        intent_held = True
                    reason = 'worker suite running'
                    fcntl.flock(budget, (fcntl.LOCK_SH if role == 'proof' else fcntl.LOCK_EX) | fcntl.LOCK_NB)
                    budget_held = True
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
                            budget_held = False
                        emit(role, 'admitted', started, report, memory)
                        break
                    slot.close()
                    slot = None
                    reason = 'worker suite estimate + headroom exceeds fresh memory budget'
                    fcntl.flock(budget, fcntl.LOCK_UN)
                    budget_held = False
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
