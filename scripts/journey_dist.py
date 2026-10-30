#!/usr/bin/env python3
"""Build and verify source-bound journey snapshots without touching tracked dist."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import subprocess
import sys
import tempfile


def git(repo: Path, *args: str) -> str:
    return subprocess.run(['git', *args], cwd=repo, check=True, capture_output=True,
                          text=True).stdout.strip()



# Only the opt-in comparison may overlay these producer-owned helpers from
# its reviewed head onto the base checkout. Product, bundle and test inputs
# remain the evaluated revision; record the helper revision explicitly.
CAPTURE_INSTRUMENTATION = (
    'scripts/journey-eval.sh', 'scripts/journey-receipt.py',
    'scripts/journey_dist.py', 'scripts/journey-bundles.py',
    'hub-web/e2e/journeys/serve-dist.mjs',
    'hub-web/e2e/journeys/journey.ts',
    'hub-web/e2e/journeys/receipt-directory.mjs',
    'hub-web/e2e/journeys/receipt-directory.d.mts',
)
GUARDED_INPUTS = ('hub-web', 'docs/qa/journeys.md', 'scripts/journey-eval.sh',
                  'scripts/journey-receipt.py', 'scripts/journey-bundles.py',
                  'scripts/journeys-for-diff.py', 'scripts/journey_dist.py', 'scripts/journey-background.py',
                  'cas-cli/Cargo.toml', 'docs/design/design-tokens.json')


def comparison(repo: Path, refs: list[str], evaluated: str) -> dict:
    base, head = [git(repo, 'rev-parse', '--verify', f'{ref}^{{commit}}') for ref in refs]
    if evaluated not in (base, head):
        raise ValueError('comparison must evaluate exactly its base or head checkout')
    dirty = set(git(repo, 'diff', '--name-only', 'HEAD', '--', *GUARDED_INPUTS).splitlines())
    dirty.update(git(repo, 'ls-files', '--others', '--exclude-standard', '--', *GUARDED_INPUTS).splitlines())
    if dirty - set(CAPTURE_INSTRUMENTATION):
        raise ValueError('comparison product/catalog/test inputs must be committed')
    for path in CAPTURE_INSTRUMENTATION:
        source = subprocess.run(['git', 'show', f'{head}:{path}'], cwd=repo, capture_output=True)
        if source.returncode == 0:
            if not (repo / path).is_file() or (repo / path).read_bytes() != source.stdout:
                raise ValueError(f'comparison instrumentation must match head: {path}')
        elif path in dirty:
            raise ValueError(f'comparison instrumentation absent from head: {path}')
    return {'base_sha': base, 'head_sha': head, 'instrumentation_sha': head}



def check_source(repo: Path, source: str, compared: dict | None = None) -> None:
    if git(repo, 'rev-parse', 'HEAD^{commit}') != source:
        raise ValueError('source tip changed; rebuild the journey snapshot')
    if compared:
        if comparison(repo, [compared['base_sha'], compared['head_sha']], source) != compared:
            raise ValueError('comparison instrumentation changed')
    elif git(repo, 'status', '--porcelain', '--', *GUARDED_INPUTS):
        raise ValueError('commit product/catalog/runner inputs before building a journey snapshot')


def dist_hash(dist: Path) -> str:
    digest = hashlib.sha256(b'cassy-journey-dist-v1\0')
    if not (dist / 'index.html').is_file():
        raise ValueError('journey snapshot has no index.html')
    for path in sorted(dist.rglob('*')):
        if path.is_symlink():
            raise ValueError(f'journey snapshot contains a symlink: {path}')
        if not path.is_file():
            continue
        name, data = os.fsencode(path.relative_to(dist).as_posix()), path.read_bytes()
        digest.update(len(name).to_bytes(8, 'big'))
        digest.update(name)
        digest.update(len(data).to_bytes(8, 'big'))
        digest.update(data)
    return digest.hexdigest()


def verify_build(repo: Path, build: dict, source: str) -> dict:
    if build.get('schema') != 1 or build.get('source_sha') != source:
        raise ValueError('journey snapshot source does not match the evaluated tip')
    check_source(repo, source, build.get('comparison'))
    dist = Path(build['dist_path'])
    if not dist.is_absolute() or dist_hash(dist) != build.get('dist_sha256'):
        raise ValueError('journey snapshot bytes changed; retain the failed run and rebuild')
    return build


def build_snapshot(repo: Path, source: str, artifacts: Path, compared: dict | None = None) -> dict:
    check_source(repo, source, compared)
    artifacts = artifacts.resolve()
    # Build output must never become a tracked lane input, even when callers
    # choose an artifact directory inside the checkout.
    if artifacts.is_relative_to(repo) and subprocess.run(
            ['git', 'check-ignore', '--quiet', str(artifacts)], cwd=repo).returncode != 0:
        raise ValueError('journey build artifacts must be outside the checkout or Git-ignored')
    builds = artifacts / 'journey-builds'
    builds.mkdir(parents=True, exist_ok=True)
    snapshot = Path(tempfile.mkdtemp(prefix=source + '-', dir=builds))
    dist = snapshot / 'dist'
    log = snapshot / 'build.log'
    with log.open('w') as output:
        result = subprocess.run(['npm', 'run', 'build', '--', '--outDir', str(dist)],
                                cwd=repo / 'hub-web', stdout=output, stderr=subprocess.STDOUT)
    if result.returncode:
        raise ValueError(f'journey snapshot build failed (exit {result.returncode}); see {log}')
    check_source(repo, source, compared)
    value = {'schema': 1, 'source_sha': source, 'dist_path': str(dist),
             'dist_sha256': dist_hash(dist)}
    if compared:
        value['comparison'] = compared
    (artifacts / 'journey-build.json').write_text(json.dumps(value, indent=2) + '\n')
    return value


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['build', 'verify'])
    parser.add_argument('--repo', required=True, type=Path)
    parser.add_argument('--plan', type=Path)
    parser.add_argument('--manifest', type=Path)
    parser.add_argument('--artifacts', type=Path)
    args = parser.parse_args()
    repo = args.repo.resolve()
    plan = json.loads(args.plan.read_text()) if args.plan else {}
    source = plan.get('head_sha') or git(repo, 'rev-parse', 'HEAD^{commit}')
    if args.action == 'verify':
        if not args.manifest:
            parser.error('verify requires --manifest')
        value = verify_build(repo, json.loads(args.manifest.read_text()), source)
    else:
        artifacts = args.artifacts or repo / 'target/journey-dist'
        value = build_snapshot(repo, source, artifacts, plan.get('comparison'))
    print(json.dumps(value))
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (ValueError, KeyError, TypeError, OSError, subprocess.CalledProcessError) as error:
        print(f'journey-dist: {error}', file=sys.stderr)
        sys.exit(2)
