#!/usr/bin/env python3
"""Update this host to the released version and prove it converged.

Usage: release-host-update.py <version> <receipt-path> <release-worktree>

Runs `cas update --yes --json --version <version>` (CAS_RELEASE_TRAIN_CAS
names the binary; default `cas`), then proves that `cas --version`, the
running hub's version and the refresh receipt's refresh_binary_version all
equal <version>. Writes the evidence to <receipt-path> (host-update.json)
whether it passes or fails, and exits non-zero with a named BLOCKER line on
any failure. 3.27.6 recorded "host update proof deferred" while host and hub
stayed on the previous release; a deferred or no-op update is a failure here.

A cloud_sync phase failure in the refresh is recorded in the receipt but does
not fail the stage: cloud sync of an unrelated project cannot make the host
binary or hub stale. Any other failed refresh phase does fail it.

After convergence, refresh the worker cache from the released commit on main.
Cache failures warn without failing the published release. The receipt names
the completed snapshot; the cache helper publishes only after Cargo exits.
"""
import datetime
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile

STAGE = "host-update"
TOLERATED_PHASES = {"cloud_sync"}
PROJECT_PHASES = ("migration", "search_index", "skills", "membership", "cloud_sync")


def run(command, timeout):
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return None, "", f"timed out after {timeout}s"
    except OSError as exc:
        return None, "", f"could not start: {exc}"
    return result.returncode, result.stdout, result.stderr


def json_objects(stdout):
    objects = []
    for line in stdout.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            value = json.loads(line)
        except ValueError:
            continue
        if isinstance(value, dict):
            objects.append(value)
    return objects


def failed_phases(refresh):
    """(project, phase, summary) for every FAILED phase in a refresh receipt."""
    failures = []
    for project in refresh.get("projects") or []:
        if not isinstance(project, dict):
            continue
        for phase in PROJECT_PHASES:
            summary = project.get(phase)
            if isinstance(summary, str) and summary.startswith("FAILED"):
                failures.append((str(project.get("project", "?")), phase, summary))
    user_level = refresh.get("user_level_store")
    status = user_level.get("status") if isinstance(user_level, dict) else None
    if isinstance(status, str) and status.startswith("FAILED"):
        failures.append(("user-level store", "user_level_store", status))
    return failures


def run_cache_refresh(command, timeout, cwd, env):
    # Kill the whole writer group on timeout: terminating just its shell can
    # leave Cargo alive with captured pipes open and a checkout being removed.
    with subprocess.Popen(command, cwd=cwd, env=env, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, text=True, start_new_session=True) as child:
        try:
            stdout, stderr = child.communicate(timeout=timeout)
            return child.returncode, stdout, stderr
        except subprocess.TimeoutExpired:
            try:
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                stdout, stderr = child.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                stdout, stderr = child.communicate()
            return None, stdout, stderr + f"\ntimed out after {timeout}s"


def refresh_worker_cache(version, worktree, receipt_path):
    """Build a detached released tree, never a dirty lane or live target writer."""
    log = receipt_path.with_name("worker-build-cache.log")
    evidence = {"status": "WARN", "snapshot": None, "log": str(log)}
    try:
        def git(*args):
            return subprocess.run(
                ["git", "-C", str(worktree), *args], capture_output=True,
                text=True, check=True, timeout=60,
            ).stdout.strip()

        released = git("rev-parse", f"refs/tags/v{version}^{{commit}}")
        git("merge-base", "--is-ancestor", released, "refs/remotes/origin/main")
        evidence["source_commit"] = released
        common = Path(git("rev-parse", "--path-format=absolute", "--git-common-dir"))
        # The gate's exports live in its child shell. Standalone and resumed
        # host updates must resolve Zig again before entering a fresh checkout
        # that has no gitignored .context toolchain (same order as the gate).
        configured = os.environ.get("ZIG", "")
        candidates = []
        if configured:
            candidate = Path(configured)
            candidates.append(candidate if candidate.is_absolute() else worktree / candidate)
        candidates.extend((worktree / ".context/zig/zig", common.parent / ".context/zig/zig"))
        zig = next((path.resolve() for path in candidates
                    if path.is_file() and os.access(path, os.X_OK)), None)
        if zig is None:
            raise RuntimeError("no executable Zig in ZIG, release worktree or main checkout; "
                               "run ./scripts/bootstrap-zig.sh")
        evidence["ZIG"] = str(zig)
        cas_root = common.parent / ".cas"
        cache_root = cas_root / "build-cache"
        cache_root.mkdir(parents=True, exist_ok=True)
        command = [os.environ.get(
            "CAS_RELEASE_TRAIN_WORKER_CACHE_CMD",
            str(Path(__file__).with_name("refresh-worker-build-cache.sh")),
        )]
        evidence["command"] = command
        timeout = int(os.environ.get("CAS_RELEASE_TRAIN_WORKER_CACHE_TIMEOUT_SECS", "1800"))
        with tempfile.TemporaryDirectory(prefix="release-cache-", dir=cache_root) as checkout:
            git("worktree", "add", "--detach", checkout, released)
            try:
                env = dict(os.environ, CAS_ROOT=str(cas_root), ZIG=str(zig))
                code, stdout, stderr = run_cache_refresh(command, timeout, cwd=checkout, env=env)
                log.write_text(stdout + stderr, encoding="utf-8")
                evidence["exit"] = code
                if code != 0:
                    raise RuntimeError(f"refresh exited {code}: {stderr.strip()[-500:]}")
                match = re.search(r"^Published worker target baseline: ([A-Za-z0-9_.-]+)$",
                                  stdout, re.MULTILINE)
                if not match or match[1] in {".", ".."}:
                    raise RuntimeError("refresh reported no published snapshot")
                snapshot = match[1]
                metadata = (cache_root / "snapshots" / snapshot / ".cas-build-cache-metadata")
                if f"source_commit={released}" not in metadata.read_text().splitlines():
                    raise RuntimeError("published snapshot provenance differs from released commit")
                evidence.update(status="PASS", snapshot=snapshot)
            finally:
                git("worktree", "remove", "--force", checkout)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as exc:
        evidence.update(status="WARN", detail=str(exc))
        print(f"WARN {STAGE}: worker build-cache refresh failed (not blocking): {exc}",
              file=sys.stderr)
    return evidence


def main():
    if len(sys.argv) != 4:
        print(__doc__.strip().splitlines()[2], file=sys.stderr)
        return 2
    version, receipt_path = sys.argv[1], Path(sys.argv[2])
    cas = os.environ.get("CAS_RELEASE_TRAIN_CAS", "cas")
    timeout = int(os.environ.get("CAS_RELEASE_TRAIN_HOST_UPDATE_TIMEOUT_SECS", "1800"))
    blockers = []
    evidence = {
        "stage": STAGE,
        "version": version,
        "cas_command": cas,
        "checked_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    }

    command = [cas, "update", "--yes", "--json", "--version", version]
    code, stdout, stderr = run(command, timeout)
    evidence["update"] = {"command": command, "exit": code}
    objects = json_objects(stdout)
    refresh = next((o for o in reversed(objects) if "refresh_binary_version" in o), None)
    binary = next((o for o in objects if "binary_updated" in o), None)
    if binary is not None:
        evidence["update"]["binary_updated"] = binary.get("binary_updated")
        evidence["update"]["reported_version"] = binary.get("version")
    evidence["refresh_binary_version"] = refresh.get("refresh_binary_version") if refresh else None
    evidence["refresh_status"] = refresh.get("refresh_status") if refresh else None
    failures = failed_phases(refresh) if refresh else []
    tolerated = [f for f in failures if f[1] in TOLERATED_PHASES]
    fatal = [f for f in failures if f[1] not in TOLERATED_PHASES]
    evidence["cloud_sync_failures"] = [
        {"project": project, "detail": summary} for project, _, summary in tolerated
    ]
    evidence["failed_phases"] = [
        {"project": project, "phase": phase, "detail": summary} for project, phase, summary in fatal
    ]
    if code is None:
        blockers.append(f"cas update {stderr}")
    elif refresh is None:
        tail = (stderr or stdout).strip().splitlines()
        blockers.append(
            "cas update printed no refresh receipt (refresh_binary_version); the update "
            "did not run or was deferred" + (f": {tail[-1]}" if tail else "")
        )
    elif fatal:
        blockers.append(
            "cas update refresh failed: "
            + "; ".join(f"{project} {phase} {summary}" for project, phase, summary in fatal)
        )
    elif code != 0 and not tolerated:
        tail = (stderr or stdout).strip().splitlines()
        blockers.append(f"cas update exited {code}" + (f": {tail[-1]}" if tail else ""))

    code, stdout, stderr = run([cas, "--version"], 60)
    match = re.search(r"\b(\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)\b", stdout or "")
    evidence["cas_version"] = match.group(1) if match else None
    if code is None or code != 0 or not match:
        blockers.append(f"cas --version did not report a version: {(stderr or stdout).strip()}")

    code, stdout, stderr = run([cas, "hub", "status", "--json"], 60)
    hub = json_objects(stdout)
    hub = hub[-1] if hub else {}
    record = hub.get("record") if isinstance(hub.get("record"), dict) else {}
    evidence["hub_running"] = hub.get("running")
    evidence["hub_version"] = record.get("version")
    if code is None or code != 0 or not hub:
        blockers.append(f"cas hub status --json failed: {(stderr or stdout).strip()}")
    elif hub.get("running") is not True:
        blockers.append("hub is not running after the update")

    for field in ("cas_version", "hub_version", "refresh_binary_version"):
        observed = evidence.get(field)
        if observed is None:
            blockers.append(f"{field} is missing; cannot prove {version} is installed")
        elif observed != version:
            blockers.append(f"{field}={observed} does not equal the released {version}")

    evidence["status"] = "FAIL" if blockers else "PASS"
    evidence["blockers"] = blockers
    receipt_path.parent.mkdir(parents=True, exist_ok=True)
    if not blockers:
        evidence["worker_build_cache"] = refresh_worker_cache(version, Path(sys.argv[3]), receipt_path)
    else:
        evidence["worker_build_cache"] = {"status": "SKIP", "snapshot": None,
                                          "detail": "host update has blockers"}
    receipt_path.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")

    for project, _, summary in tolerated:
        print(f"WARN {STAGE}: cloud_sync failed for {project} (recorded, not blocking): {summary}",
              file=sys.stderr)
    if blockers:
        for blocker in blockers:
            print(f"BLOCKER {STAGE}: {blocker}", file=sys.stderr)
        print(f"receipt: {receipt_path}", file=sys.stderr)
        return 1
    print(f"PASS {STAGE}: cas, hub and refresh all report {version}; receipt {receipt_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
