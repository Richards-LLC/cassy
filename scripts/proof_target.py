#!/usr/bin/env python3
"""Private supervisor Cargo targets and source provenance (no Cargo in setup).

Run: python3 scripts/proof_target.py run <worktree> -- cargo check --workspace --tests
All commands use <worktree>/target. Caller --target-dir overrides are refused.
Immutable worker snapshots seed dependencies only; workspace units are rebuilt.
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tomllib

OWNER = '.cas-proof-owner.json'


def git(root, *args):
    return subprocess.check_output(['git', '-C', str(root), *args], stderr=subprocess.PIPE, text=True).strip()


def identity(root):
    root = Path(root).resolve()
    if Path(git(root, 'rev-parse', '--show-toplevel')).resolve() != root:
        raise ValueError('proof source must be the worktree root')
    return {'worktree': str(root), 'head': git(root, 'rev-parse', 'HEAD'), 'target': str(root / 'target'),
            'dirty': bool(git(root, 'status', '--porcelain', '--untracked-files=all'))}


def workspace_packages(root):
    manifest = root / 'Cargo.toml'
    if not manifest.exists():
        return set()
    config = tomllib.loads(manifest.read_text())
    paths = {manifest} if 'package' in config else set()
    for pattern in config.get('workspace', {}).get('members', []):
        paths.update(p / 'Cargo.toml' for p in root.glob(pattern) if p.is_dir())
    return {tomllib.loads(p.read_text())['package']['name'] for p in paths}


def workspace_unit(name, packages):
    stems = {name.split('.', 1)[0], name.removeprefix('lib').split('.', 1)[0]}
    return any(stem == pkg or stem.startswith(pkg + '-') or stem.startswith(pkg + '_')
               for stem in stems for package in packages for pkg in {package, package.replace('-', '_')})


def fingerprint_roots(target):
    # Cargo profiles can be custom; triples add one directory level.
    for first in target.iterdir():
        if first.is_symlink() and first.is_dir():
            raise ValueError('proof target profile/triple is a symlink')
        if not first.is_dir():
            continue
        for directory in (first, *[p for p in first.iterdir() if p.is_dir()]):
            if directory.is_symlink():
                raise ValueError('proof target profile/output directory is a symlink')
            root = directory / '.fingerprint'
            if root.exists():
                yield root


def invalidate_workspace(target, packages):
    for directory in fingerprint_roots(target):
        if directory.is_symlink():
            raise ValueError('proof target fingerprint directory is a symlink')
        for unit in directory.iterdir():
            if unit.is_symlink():
                raise ValueError('proof target fingerprint unit is a symlink')
            if unit.is_dir() and (workspace_unit(unit.name, packages) or
                                 any(p.is_file() and p.stat().st_nlink > 1 for p in unit.iterdir())):
                shutil.rmtree(unit)


def cache_root(root):
    common = Path(git(root, 'rev-parse', '--path-format=absolute', '--git-common-dir')).resolve()
    return common.parent / '.cas/build-cache'


def seed_dependencies(root, target, packages, cache):
    pointer = cache / 'current'
    if not pointer.exists() or (target / 'debug').exists():
        return None
    snapshot_name = pointer.read_text().strip()
    if not snapshot_name or snapshot_name in ('.', '..') or Path(snapshot_name).name != snapshot_name or '/' in snapshot_name or '\\' in snapshot_name:
        raise ValueError('invalid build-cache/current snapshot name')
    snapshot = cache / 'snapshots' / snapshot_name
    if snapshot.is_symlink() or not snapshot.is_dir():
        raise ValueError('build-cache snapshot must be a real directory')
    metadata = dict(line.split('=', 1) for line in
                    (snapshot / '.cas-build-cache-metadata').read_text().splitlines() if '=' in line)
    source = metadata.get('source_commit', '')
    # A baseline from unrelated history is not a source of trusted freshness.
    ancestor = subprocess.run(['git', '-C', str(root), 'merge-base', '--is-ancestor', source, 'HEAD'],
                              stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    if ancestor.returncode:
        return None
    with tempfile.TemporaryDirectory(prefix='.proof-seed-', dir=root) as directory:
        staging = Path(directory)
        for current, dirs, files in os.walk(snapshot, followlinks=False):
            current = Path(current)
            relative = current.relative_to(snapshot)
            # Workspace fingerprints/build outputs and incremental data never
            # enter another source root, even when mtimes claim to be fresh.
            dirs[:] = [d for d in dirs if d != 'incremental' and not (current / d).is_symlink()
                       and not (current.name in ('.fingerprint', 'build') and workspace_unit(d, packages))]
            destination = staging / relative
            destination.mkdir(parents=True, exist_ok=True)
            for name in files:
                original = current / name
                if original.is_symlink() or name in ('.cargo-lock', '.cas-build-cache-metadata', OWNER) or workspace_unit(name, packages):
                    continue
                output = destination / name
                if original.suffix == '.d':
                    output.write_text(original.read_text().replace(str(snapshot), str(target)))
                    shutil.copystat(original, output)
                elif '.fingerprint' not in relative.parts and original.suffix in ('.rlib', '.rmeta', '.so', '.dylib', '.a'):
                    try:
                        os.link(original, output)
                    except OSError:
                        shutil.copy2(original, output)
                else:
                    # Cargo mutates locks/freshness/build metadata in place.
                    shutil.copy2(original, output)
        for entry in staging.iterdir():
            destination = target / entry.name
            if not destination.exists():
                entry.rename(destination)
    return snapshot_name


def prepare(root, cache=None):
    source = identity(root)
    root = Path(source['worktree'])
    target = root / 'target'
    if target.is_symlink():
        raise ValueError('proof target is a symlink; refuse shared live Cargo outputs')
    target.mkdir(exist_ok=True)
    for child in target.iterdir():
        if child.is_symlink() and child.name in ('debug', 'release', OWNER, '.cas-proof-setup.lock'):
            raise ValueError('proof target metadata/profile is a symlink')
        if child.is_dir() and not child.is_symlink():
            for profile in ('debug', 'release'):
                if (child / profile).is_symlink():
                    raise ValueError('proof target cross-compile profile is a symlink')
    # The setup lock is private; Cargo owns execution locking afterwards.
    with (target / '.cas-proof-setup.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        owner = target / OWNER
        prior = json.loads(owner.read_text()) if owner.exists() else None
        if prior is not None and prior.get('worktree') != source['worktree']:
            raise ValueError('proof target last source root differs from this worktree')
        packages = workspace_packages(root)
        list(fingerprint_roots(target))  # validate output directories on every run
        if prior is None or prior.get('head') != source['head']:
            invalidate_workspace(target, packages)
        seed = seed_dependencies(root, target, packages, Path(cache) if cache else cache_root(root))
        source['seed_snapshot'] = seed
        temporary = owner.with_suffix('.tmp')
        temporary.write_text(json.dumps(source, sort_keys=True) + '\n')
        temporary.replace(owner)
    return source


def environment(env, source):
    # Environment beats config target-dir; command-line overrides are refused.
    return dict(env, CARGO_TARGET_DIR=source['target'], CARGO_BUILD_TARGET_DIR=source['target'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['run'])
    parser.add_argument('root', type=Path)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    try:
        if not command or any(arg == '--target-dir' or arg.startswith('--target-dir=') for arg in command):
            raise ValueError('proof runner owns --target-dir; supply a command without that override')
        source = prepare(args.root)
        print('PROOF_SOURCE: ' + json.dumps(source, sort_keys=True), flush=True)
        result = subprocess.call(command, cwd=source['worktree'], env=environment(os.environ, source))
        if identity(args.root)['head'] != source['head']:
            raise ValueError('proof source HEAD changed during execution')
        return result
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print('FAIL proof target: ' + str(error), file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
