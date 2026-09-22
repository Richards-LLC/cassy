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


def receipts_commit_for_base(root, base):
    run_dir = os.environ.get("CAS_RELEASE_RECEIPTS_RUN_DIR")
    candidates = []
    if run_dir:
        candidates.append(Path(run_dir) / "receipts.commit")
    common = Path(git(root, "rev-parse", "--path-format=absolute", "--git-common-dir")).resolve()
    artifacts = Path(os.environ.get("CAS_RELEASE_ARTIFACTS_ROOT", common.parent / ".cas" / "artifacts" / "release"))
    candidates.extend(sorted(artifacts.glob("v*-*/receipts.commit")))
    for path in candidates:
        try:
            fields = dict(
                line.split("=", 1)
                for line in path.read_text(encoding="utf-8").splitlines()
                if "=" in line
            )
        except OSError:
            continue
        if fields.get("BASE_SHA") == base:
            return fields.get("COMMIT_SHA", "unknown")
    return None


def warn_receipts_ahead(root, base, main_tip):
    if base == main_tip:
        return
    ancestor = subprocess.run(
        ["git", "-C", str(root), "merge-base", "--is-ancestor", base, main_tip],
        capture_output=True,
    )
    if ancestor.returncode != 0:
        return
    changed = git(root, "diff", "--name-only", base, main_tip).splitlines()
    if not changed or not all(path.startswith("docs/") for path in changed):
        return
    commit_sha = receipts_commit_for_base(root, base) or "unknown"
    print(
        "WARN assemble receipt docs: origin/main is ahead of the integration base "
        f"only under docs/; receipts commit {commit_sha} is the cause. "
        "Run the stale-base heal before assembling.",
        file=sys.stderr,
    )


def is_ancestor(root, older, newer):
    result = subprocess.run(
        ["git", "-C", str(root), "merge-base", "--is-ancestor", older, newer],
        capture_output=True,
    )
    return result.returncode == 0


def rebase_docs_only_release(root, main_tip, integration_tip):
    """Move release metadata commits from main onto the tested union tip."""
    current = git(root, "rev-parse", "HEAD")
    if is_ancestor(root, current, integration_tip):
        return
    branch = git(root, "branch", "--show-current")
    if not branch.startswith("release/") or not is_ancestor(root, main_tip, current):
        return
    changed = git(root, "diff", "--name-only", f"{main_tip}..{current}").splitlines()
    allowed = lambda path: path == "CHANGELOG.md" or path.startswith("docs/release-notes/")
    if not changed or not all(allowed(path) for path in changed):
        return
    result = subprocess.run(
        ["git", "-C", str(root), "rebase", "--onto", integration_tip, main_tip],
        capture_output=True,
        text=True,
    )
    if result.returncode:
        raise RuntimeError("Could not rebase docs-only release commits onto the integration tip")
    print("Rebased docs-only release commits onto integration tip", file=sys.stderr)


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


def recorded_run_fields():
    run_dir = os.environ.get("CAS_RELEASE_TRAIN_RUN_DIR")
    if run_dir:
        try:
            return dict(
                line.split("=", 1)
                for line in (Path(run_dir) / "run.env").read_text(encoding="utf-8").splitlines()
                if "=" in line
            )
        except OSError:
            pass
    return {}


def recorded_factory_session():
    session = recorded_run_fields().get("factory_session", "").strip()
    return session or os.environ.get("CAS_FACTORY_SESSION", "").strip()


def recorded_identity():
    fields = recorded_run_fields()
    identity = {}
    for field, variable in (
        ("agent_id", "CAS_AGENT_ID"),
        ("session_id", "CAS_SESSION_ID"),
        ("agent_name", "CAS_AGENT_NAME"),
        ("agent_role", "CAS_AGENT_ROLE"),
    ):
        value = fields.get(field, "").strip() or os.environ.get(variable, "").strip()
        if value:
            identity[variable] = value
    return identity


def heal_stale_assembly(root):
    """Re-sweep the current base/open-epic union once before assembly retry."""
    command = os.environ.get("CAS_RELEASE_TRAIN_CAS", "cas")
    env = {
        key: os.environ[key]
        for key in ("HOME", "PATH")
        if os.environ.get(key)
    }
    env["CAS_ROOT"] = str(root / ".cas")
    factory_session = recorded_factory_session()
    if factory_session:
        env["CAS_FACTORY_SESSION"] = factory_session
    env.update(recorded_identity())
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


class StaleBaseError(RuntimeError):
    """origin/main moved past the integration receipt base."""

    def __init__(self):
        super().__init__(STALE_BASE_ERROR)


def _assemble_locked(root):
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
    main_tip = git(root, "rev-parse", "refs/remotes/origin/main")
    if main_tip != receipt.get("base"):
        warn_receipts_ahead(root, receipt.get("base", ""), main_tip)
        raise StaleBaseError()
    for epic in receipt["epics"]:
        if resolve(root, epic["branch"]) != epic["tip"]:
            raise RuntimeError("An epic changed since integration; rerun the merge sweep")
    if git(root, "status", "--porcelain"):
        raise RuntimeError("Assembly checkout has changes; commit or move them first")
    current = git(root, "branch", "--show-current")
    if current and not current.startswith("release/"):
        raise RuntimeError("Use a detached checkout or a release/ branch for assembly")
    rebase_docs_only_release(root, main_tip, tip)
    git(root, "-c", "core.hooksPath=/dev/null", "merge", "--ff-only", tip)
    return tip


def _under_delivery_lock(root, action):
    """Run action(root) holding the integration branch's delivery-target lock.

    The lock is released when this returns or raises, so nothing started
    after it (notably integration-recover, which takes the same lock) can
    wait on this process.
    """
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
        try:
            return action(root)
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def assemble(root):
    try:
        return _under_delivery_lock(root, _assemble_locked)
    except StaleBaseError:
        pass
    # integration-recover takes the same delivery-target lock, so the heal must
    # run with it released (3.27.6 deadlocked here). The retry re-acquires the
    # lock and revalidates the receipt the recovery rewrote; a second stale
    # base is a hard failure rather than another heal.
    heal_stale_assembly(root)
    return _under_delivery_lock(root, _assemble_locked)


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
