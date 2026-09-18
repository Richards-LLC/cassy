#!/usr/bin/env python3
"""Consume the daemon's tested integration tip under its delivery-target lock."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


STALE_BASE_ERROR = "Main changed since integration; rerun the merge sweep"
DEFAULT_RECOVERY_TIMEOUT_SECS = 4 * 60 * 60


def git(root, *args):
    result = subprocess.run(["git", "-C", str(root), *args], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError("Git could not validate or fast-forward the assembly checkout")
    return result.stdout.strip()


def resolve(root, branch):
    tips = []
    for ref in (f"refs/heads/{branch}", f"refs/remotes/origin/{branch}"):
        result = subprocess.run(["git", "-C", str(root), "rev-parse", "--verify", f"{ref}^{{commit}}"],
                                capture_output=True, text=True)
        if result.returncode == 0:
            tips.append(result.stdout.strip())
    if not tips:
        raise RuntimeError("An input branch is missing; rerun the merge sweep")
    if len(tips) == 2:
        for older, newer in (tips, tips[::-1]):
            result = subprocess.run(["git", "-C", str(root), "merge-base", "--is-ancestor", older, newer],
                                    capture_output=True)
            if result.returncode == 0:
                return newer
        raise RuntimeError("Local and remote epic diverged; reconcile the epic first")
    return tips[0]


def recovery_timeout_secs():
    raw = os.environ.get("CAS_RELEASE_TRAIN_RECOVERY_TIMEOUT_SECS", "")
    if not raw:
        return DEFAULT_RECOVERY_TIMEOUT_SECS
    try:
        value = int(raw)
    except ValueError as exc:
        raise RuntimeError("CAS_RELEASE_TRAIN_RECOVERY_TIMEOUT_SECS must be an integer") from exc
    if value <= 0:
        raise RuntimeError("CAS_RELEASE_TRAIN_RECOVERY_TIMEOUT_SECS must be positive")
    return value


def heal_stale_assembly(root):
    """Re-sweep the current base/open-epic union once before assembly retry."""
    command = os.environ.get("CAS_RELEASE_TRAIN_CAS", "cas")
    env = os.environ.copy()
    env["CAS_ROOT"] = str(root / ".cas")
    try:
        result = subprocess.run(
            [command, "factory", "integration-recover", "--base-only"],
            cwd=root,
            env=env,
            capture_output=True,
            text=True,
            timeout=recovery_timeout_secs(),
        )
    except subprocess.TimeoutExpired as exc:
        raise RuntimeError(
            f"Integration recovery timed out after {recovery_timeout_secs()}s"
        ) from exc
    except OSError as exc:
        raise RuntimeError(f"Integration recovery could not start: {exc}") from exc
    if result.returncode:
        detail = (result.stderr or result.stdout).strip().splitlines()
        raise RuntimeError(
            "Integration recovery failed"
            + (f": {detail[-1]}" if detail else "")
        )


def _assemble_locked(root, lock, allow_heal=True):
    """Validate and consume an integration receipt while holding its lock."""
    common = Path(git(root, "rev-parse", "--path-format=absolute", "--git-common-dir")).resolve()
    project = "".join(c if c.isascii() and (c.isalnum() or c == "-") else "-"
                      for c in common.parent.name)
    branch = "integration/" + project
    cas = common.parent / ".cas"
    receipt = json.loads((cas / "merge-sweeps" / "integration.json").read_text())
    if receipt.get("status") != "PASSED":
        raise RuntimeError("Integration has no passing sweep; resolve the epic sweep report")
    tip = git(root, "rev-parse", "--verify", f"refs/heads/{branch}^{{commit}}")
    if tip != receipt.get("tip"):
        raise RuntimeError("Integration tip changed since its sweep; rerun the merge sweep")
    try:
        if git(root, "rev-parse", "refs/remotes/origin/main") != receipt.get("base"):
            raise RuntimeError(STALE_BASE_ERROR)
    except RuntimeError as exc:
        if not allow_heal or str(exc) != STALE_BASE_ERROR:
            raise
        heal_stale_assembly(root)
        return _assemble_locked(root, lock, allow_heal=False)
    for epic in receipt["epics"]:
        if resolve(root, epic["branch"]) != epic["tip"]:
            raise RuntimeError("An epic changed since integration; rerun the merge sweep")
    if git(root, "status", "--porcelain"):
        raise RuntimeError("Assembly checkout has changes; commit or move them first")
    current = git(root, "branch", "--show-current")
    if current and not current.startswith("release/"):
        raise RuntimeError("Use a detached checkout or a release/ branch for assembly")
    git(root, "-c", "core.hooksPath=/dev/null", "merge", "--ff-only", tip)
    return tip


def assemble(root):
    common = Path(git(root, "rev-parse", "--path-format=absolute", "--git-common-dir")).resolve()
    project = "".join(c if c.isascii() and (c.isalnum() or c == "-") else "-"
                      for c in common.parent.name)
    branch = "integration/" + project
    cas = common.parent / ".cas"
    key = hashlib.sha256(b"cas-0a21/delivery-target-lock/v1\0" + os.fsencode(common)
                         + b"\0" + branch.encode()).hexdigest()
    locks = cas / "locks" / "delivery-target"
    locks.mkdir(parents=True, exist_ok=True)
    with (locks / (key + ".lock")).open("a") as lock:
        # Never make release tooling silently wait behind a workspace build.
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise RuntimeError("Integration sweep is running; retry assembly after it finishes") from exc
        return _assemble_locked(root, lock)


def main():
    try:
        tip = assemble(Path(sys.argv[1]))
    except (OSError, ValueError, KeyError, IndexError, RuntimeError) as exc:
        print("FAIL release assembly", file=sys.stderr)
        print(str(exc), file=sys.stderr)
        return 1
    print("PASS release assembly")
    print("Next: run release-train.sh with --gate")
    print("Tip: " + tip)
    return 0


if __name__ == "__main__":
    sys.exit(main())
