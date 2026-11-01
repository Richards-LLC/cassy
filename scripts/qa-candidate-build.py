#!/usr/bin/env python3
"""Build an exact-tip QA binary with the release environment and owned scratch."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
import signal
import time
from urllib.parse import quote
from urllib.request import build_opener, ProxyHandler

import release_env
import release_scratch
import journey_dist
from concurrent.futures import ThreadPoolExecutor, TimeoutError as FutureTimeout


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def run(scope, command, **kwargs):
    print("candidate: " + " ".join(map(str, command)), file=sys.stderr, flush=True)
    scope.run(list(map(str, command)), check=True, **kwargs)


def verify_candidate_version(output, declared, revision):
    match = re.fullmatch(r"cas (\S+) \(([0-9a-f]{7}) (\d{4}-\d{2}-\d{2})\)", output)
    if not match:
        raise ValueError(f"candidate version has unexpected format: {output!r}")
    if match.group(1) != declared or not revision.startswith(match.group(2)):
        raise ValueError(
            f"candidate version {output!r} differs from revision's cas {declared} ({revision[:7]})"
        )


def verify_web(scope, binary, snapshot, owned):
    """Read embedded assets from this PID's private loopback hub, then retire it."""
    home, project = owned / 'web-home', owned / 'web-project'
    home.mkdir()
    (project / '.cas').mkdir(parents=True)
    env = {'HOME':str(home), 'XDG_CONFIG_HOME':str(home/'config'),
           'XDG_RUNTIME_DIR':str(home/'runtime'), 'CAS_ROOT':str(project/'.cas'),
           'PATH':os.environ.get('PATH',''), 'LANG':'C.UTF-8',
           'CAS_SKIP_FACTORY_TOOLING':'1', 'CAS_SUPPRESS_DUPLICATE_WARNING':'1'}
    run(scope, ['git','init','-q',project],env=env)
    run(scope, [binary,'init','--yes','--no-integrations'],cwd=project,env=env,
        stdout=sys.stderr,stderr=sys.stderr)
    served = owned / 'served-web'
    served.mkdir()
    opener = build_opener(ProxyHandler({}))
    with (owned/'candidate-hub.log').open('w') as log:
        pids = []
        scope.spawn_hooks.append(pids.append)
        executor = ThreadPoolExecutor(max_workers=1)
        launch = executor.submit(scope.run,[str(binary),'hub','serve','--no-tailscale-serve','--port','0'],
                                 cwd=project,env=env,stdout=log,stderr=log)
        try:
            deadline = time.monotonic()+15
            record = home/'.cas/hub/process.json'
            while True:
                if launch.done():
                    launch.result()
                    raise ValueError('candidate hub exited before serving its web receipt')
                if pids and record.is_file():
                    state = json.loads(record.read_text())
                    if state['pid'] != pids[0]:
                        raise ValueError('candidate hub receipt belongs to another PID')
                    origin = f'http://127.0.0.1:{int(state["port"])}'
                    try:
                        with opener.open(origin+'/commander/',timeout=1) as response:
                            response.read()
                        break
                    except OSError:
                        pass # Startup readiness, never a test or build retry.
                if time.monotonic()>=deadline:
                    raise ValueError('candidate hub did not serve its web receipt within15s')
                time.sleep(.05)
            for path in sorted(Path(snapshot['dist_path']).rglob('*')):
                if not path.is_file(): continue
                relative = path.relative_to(snapshot['dist_path'])
                target = served/relative
                target.parent.mkdir(parents=True,exist_ok=True)
                route = '/commander/' if relative.as_posix()=='index.html' else '/commander/'+quote(relative.as_posix())
                with opener.open(origin+route,timeout=5) as response:
                    target.write_bytes(response.read())
            actual = journey_dist.dist_hash(served)
            if actual != snapshot['dist_sha256']:
                raise ValueError('candidate served web digest differs from source-bound web digest')
            return {'source_sha':snapshot['source_sha'],'dist_sha256':snapshot['dist_sha256'],
                    'served_dist_sha256':actual}
        finally:
            try:
                if pids and not launch.done():
                    try: os.killpg(pids[0],signal.SIGTERM)
                    except ProcessLookupError: pass
                try: launch.result(timeout=3)
                except FutureTimeout:
                    if pids:
                        try: os.killpg(pids[0],signal.SIGKILL)
                        except ProcessLookupError: pass
                    launch.result()
            finally:
                executor.shutdown(wait=True)
                scope.spawn_hooks.remove(pids.append)


def verified_receipt(published, revision):
    """The published candidate's receipt, when its binary still matches it."""
    try:
        receipt = json.loads((published / "RECEIPT.json").read_text())
        if receipt["revision"] == revision and receipt["sha256"] == sha256(published / "cas"):
            return receipt
    except (OSError, ValueError, KeyError):
        pass
    return None


def retain_legacy_candidate(candidate_dir, candidates):
    """Before the per-revision layout `candidate` was a real directory."""
    if candidate_dir.is_symlink() or not candidate_dir.exists():
        return
    try:
        revision = json.loads((candidate_dir / "RECEIPT.json").read_text())["revision"]
    except (OSError, ValueError, KeyError):
        revision = None
    if revision and not (candidates / revision).exists():
        candidates.mkdir(exist_ok=True)
        candidate_dir.rename(candidates / revision)
    else:
        shutil.rmtree(candidate_dir)


def point_latest(candidate_dir, published):
    """Atomically aim `candidate` at the newest build, which stays where it is."""
    retain_legacy_candidate(candidate_dir, published.parent)
    link = candidate_dir.parent / f".candidate-link-{os.getpid()}"
    link.symlink_to(Path(published.parent.name) / published.name)
    try:
        os.replace(link, candidate_dir)
    except BaseException:
        link.unlink(missing_ok=True)
        raise


def build(repo, revision, candidate_dir):
    repo = repo.resolve()
    resolved = subprocess.check_output(
        ["git", "-C", str(repo), "rev-parse", "--verify", "--end-of-options", f"{revision}^{{commit}}"],
        text=True,
    ).strip()
    if revision != resolved:
        raise ValueError(f"--rev must be the full commit SHA ({resolved})")
    candidate_dir = candidate_dir.absolute()
    # Each revision publishes once, so a ledger's cited candidate is never replaced.
    task_dir = candidate_dir.parent
    candidates = task_dir / "candidates"
    published = candidates / revision
    task_dir.mkdir(parents=True, exist_ok=True)
    if task_dir.is_symlink() or candidates.is_symlink():
        raise ValueError("candidate artifact path must not be a symlink")
    if candidate_dir.is_symlink() and candidate_dir.resolve().parent != candidates.resolve():
        raise ValueError("candidate artifact path must not be a symlink")
    existing = verified_receipt(published, revision)
    if existing:
        point_latest(candidate_dir, published)
        return {"candidate_dir": str(published), "revision": revision,
                "version": existing["version"], "sha256": existing["sha256"]}
    scratch = release_env.resolve_scratch(repo, durable=True, create=True)
    if scratch.problem or not scratch.base:
        raise ValueError(f"release scratch unavailable: {scratch.problem}; {scratch.fix}")
    with release_scratch.ChildScope() as scope:
        with (
            release_scratch.OwnedDirectory(
                Path(scratch.base).name + ".qa-candidate-", Path(scratch.base).parent,
            ) as owned,
            release_scratch.BoundedCache(owned / "target", os.environ, repo) as target,
        ):
            # Private, size/age bounded output lives only for this candidate.
            # Legacy supervisor/main targets remain review-only.
            checkout = owned / "checkout"
            run(scope, ["git", "-C", repo, "worktree", "add", "--detach", checkout, revision])
            try:
                run(scope, ['npm','ci'],cwd=checkout/'hub-web',stdout=sys.stderr,stderr=sys.stderr)
                # Use the journey producer's source-bound production build,
                # then install its bytes only in this disposable checkout.
                web_artifacts = owned/'web'
                run(scope, [sys.executable,checkout/'scripts/journey_dist.py','build',
                            '--repo',checkout,'--artifacts',web_artifacts],stdout=sys.stderr,stderr=sys.stderr)
                snapshot = json.loads((web_artifacts/'journey-build.json').read_text())
                journey_dist.verify_build(checkout,snapshot,revision)
                shutil.rmtree(checkout/'hub-web/dist',ignore_errors=True)
                shutil.copytree(snapshot['dist_path'],checkout/'hub-web/dist')
                zig = release_env.resolve_zig(checkout, link=True)
                if not zig.path:
                    raise ValueError(f"Zig unavailable: {'; '.join(zig.tried)}; {zig.fix}")
                env = dict(os.environ, ZIG=zig.path, CARGO_TARGET_DIR=str(target),
                           PATH=str(Path(zig.path).parent) + os.pathsep + os.environ.get("PATH", ""))
                run(scope, ["cargo", "build", "-p", "cas", "--bin", "cas", "--locked"],
                    cwd=checkout, env=env, stdout=sys.stderr, stderr=sys.stderr)
                binary = target / "debug" / "cas"
                if not binary.is_file():
                    raise ValueError(f"candidate build produced no binary at {binary}")
                declared = tomllib.loads((checkout / "cas-cli/Cargo.toml").read_text())["package"]["version"]
                stage = Path(tempfile.mkdtemp(prefix=".candidate-", dir=task_dir))
                try:
                    shutil.copy2(binary, stage / "cas")
                    version = subprocess.check_output([str(stage / "cas"), "--version"], text=True).strip()
                    verify_candidate_version(version, declared, revision)
                    web = verify_web(scope,stage/'cas',snapshot,owned)
                    (stage / "VERSION.txt").write_text(f"{version}\nrevision={revision}\n")
                    binary_hash = sha256(stage / "cas")
                    version_hash = sha256(stage / "VERSION.txt")
                    receipt = {"revision": revision, "version": version,
                               "sha256": binary_hash, "zig": zig.path,
                               "zig_source": zig.source, "cargo_target": str(target), "web":web}
                    (stage / "RECEIPT.json").write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
                    receipt_hash = sha256(stage / "RECEIPT.json")
                    (stage / "SHA256SUMS").write_text(
                        f"{binary_hash}  cas\n{version_hash}  VERSION.txt\n"
                        f"{receipt_hash}  RECEIPT.json\n")
                    candidates.mkdir(exist_ok=True)
                    # Only an unverifiable, so uncitable, directory can be here.
                    if published.exists():
                        shutil.rmtree(published)
                    stage.rename(published)
                    point_latest(candidate_dir, published)
                finally:
                    if stage.exists():
                        shutil.rmtree(stage)
            finally:
                run(scope, ["git", "-C", repo, "worktree", "remove", "--force", checkout])
    return {"candidate_dir": str(published), "revision": revision,
            "version": version, "sha256": binary_hash}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--rev", required=True)
    parser.add_argument("--candidate-dir", type=Path, required=True)
    args = parser.parse_args()
    try:
        print(json.dumps(build(args.repo, args.rev, args.candidate_dir), sort_keys=True))
        return 0
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"candidate: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
