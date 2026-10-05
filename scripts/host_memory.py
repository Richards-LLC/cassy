"""Host/user admission shared by assembly and worker browser/build suites.

Proofs hold shared budget leases; one worker suite holds an exclusive lease.
The intent lock gives a waiting proof priority over subsequent worker suites.
No browser is started merely to sample the budget. Only ancestry-validated
nested commands may reuse a live admission; an environment flag is insufficient.
"""
from contextlib import contextmanager
import fcntl
import json
import os
from pathlib import Path
import secrets
import stat
import subprocess
import time

DIRECTORY = Path('/var/tmp') / f'cas-host-memory-{os.getuid()}'
LEASE_ENV = 'CAS_HOST_MEMORY_LEASE'


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
                with private_file(directory / 'budget.lock', False) as probe:
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
def admission(role, env, memory_budget, wait_secs=600, poll_secs=1, directory=None, report=None):
    directory = directory or DIRECTORY
    report = report or default_report
    if role not in ('proof', 'worker'):
        raise ValueError('unknown host memory admission role')
    private_directory(directory)
    if inherited(env, directory):
        yield dict(env), ()
        return
    started = time.monotonic()
    with private_file(directory / 'intent.lock') as intent, private_file(directory / 'budget.lock') as budget:
        intent_held = budget_held = False
        try:
            while True:
                # Proof phases sample immediately after this lease; do not consume
                # a sample ahead of their detailed producer/link/test admission.
                memory = memory_budget(env) if role == 'worker' else {}
                reason = 'proof pending' if role == 'worker' else 'worker suite running'
                try:
                    if not intent_held:
                        fcntl.flock(intent, (fcntl.LOCK_EX if role == 'proof' else fcntl.LOCK_SH) | fcntl.LOCK_NB)
                        intent_held = True
                    reason = 'worker suite running'
                    fcntl.flock(budget, (fcntl.LOCK_SH if role == 'proof' else fcntl.LOCK_EX) | fcntl.LOCK_NB)
                    budget_held = True
                    # Worker estimate plus headroom; proofs still do their detailed phase admission.
                    if role == 'proof' or memory['budget_bytes'] >= 4 * 1024**3:
                        emit(role, 'admitted', started, report, memory)
                        break
                    reason = 'worker suite estimate + headroom exceeds fresh memory budget'
                    fcntl.flock(budget, fcntl.LOCK_UN)
                    budget_held = False
                except BlockingIOError:
                    if role == 'worker' and intent_held:
                        try:
                            fcntl.flock(budget, fcntl.LOCK_SH | fcntl.LOCK_NB)
                            reason = 'proof running'
                            fcntl.flock(budget, fcntl.LOCK_UN)
                        except BlockingIOError:
                            pass
                # A proof retains exclusive intent while awaiting the old worker.
                # A worker never retains shared intent across a wait.
                if role == 'worker' and intent_held:
                    fcntl.flock(intent, fcntl.LOCK_UN)
                    intent_held = False
                emit(role, reason, started, report, memory)
                elapsed = time.monotonic() - started
                if elapsed >= wait_secs:
                    raise ValueError(f'{role} host memory admission deadline expired after {wait_secs}s: {reason}; command was not started')
                time.sleep(min(poll_secs, wait_secs - elapsed))
            fcntl.flock(intent, fcntl.LOCK_UN)
            intent_held = False
            token = secrets.token_hex(16)
            claim_path = directory / f'lease-{os.getpid()}-{token}.json'
            record = {'pid': os.getpid(), 'token': token, 'role': role}
            with private_file(claim_path) as claim:
                json.dump(record, claim)
                claim.flush()
            try:
                yield dict(env, **{LEASE_ENV: json.dumps(record)}), (budget.fileno(),)
            finally:
                claim_path.unlink(missing_ok=True)
        finally:
            # Closing, rather than LOCK_UN, preserves the lease if a killed
            # wrapper still has descendants carrying its open description.
            if intent_held:
                fcntl.flock(intent, fcntl.LOCK_UN)
