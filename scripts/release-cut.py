#!/usr/bin/env python3
"""One manual/automatic cut lease and disposable checkout; never check out a branch."""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile


def cut(repo, branch, tip, args):
    def git(*argv):
        return subprocess.check_output(['git', '-C', str(repo), *argv], text=True).strip()
    tip = git('rev-parse', '--verify', f'{tip}^{{commit}}')
    common = Path(git('rev-parse', '--path-format=absolute', '--git-common-dir'))
    directory = common / 'cas' / 'release-cuts' / hashlib.sha256(branch.encode()).hexdigest()
    directory.mkdir(parents=True, exist_ok=True)
    with (directory / 'lease.lock').open('a+') as lease:
        try:
            fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            print(f'CUT_LEASE_BUSY: {branch}; inspect {directory / "owner.json"}', file=__import__('sys').stderr)
            return 71
        with tempfile.TemporaryDirectory(prefix='checkout-', dir=directory) as temporary:
            checkout = Path(temporary) / 'tree'
            subprocess.run(['git', '-C', str(repo), 'worktree', 'add', '--quiet', '--detach', str(checkout), tip], check=True)
            try:
                if '--notes' in args:
                    index = args.index('--notes') + 1
                    notes = Path(args[index])
                    if notes.is_absolute() and notes.is_relative_to(repo.resolve()):
                        relative = notes.relative_to(repo.resolve())
                        if not notes.exists() or subprocess.run(['git', '-C', str(checkout), 'cat-file', '-e', f'{tip}:{relative}'], stderr=subprocess.DEVNULL).returncode == 0:
                            args[index] = str(checkout / relative)
                (directory / 'owner.json').write_text(json.dumps({'pid': os.getpid(), 'branch': branch, 'tip': tip, 'checkout': str(checkout)}))
                env = dict(os.environ, CAS_RELEASE_CUT_OWNED='1', CAS_RELEASE_CUT_BRANCH=branch,
                           CAS_RELEASE_TRAIN_TIP=tip, CAS_ROOT=os.environ.get('CAS_ROOT', str(common.parent / '.cas')))
                return subprocess.run(['bash', 'scripts/release.sh', *args], cwd=checkout, env=env, pass_fds=(lease.fileno(),)).returncode
            finally:
                subprocess.run(['git', '-C', str(repo), 'worktree', 'remove', '--force', str(checkout)], check=True)
                (directory / 'owner.json').unlink(missing_ok=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', type=Path, required=True)
    parser.add_argument('--branch', required=True)
    parser.add_argument('--tip', required=True)
    parser.add_argument('args', nargs=argparse.REMAINDER)
    options = parser.parse_args()
    try:
        code = cut(options.repo, options.branch, options.tip,
                   options.args[1:] if options.args[:1] == ['--'] else options.args)
    except (OSError, subprocess.CalledProcessError) as error:
        print(f'CUT_OWNER_FAILED: {error}', file=__import__('sys').stderr)
        code = 1
    print(f"RELEASE_EXIT={code}", flush=True)
    raise SystemExit(code)
