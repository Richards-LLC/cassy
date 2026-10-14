#!/usr/bin/env python3
"""Consume the daemon's tested integration tip under its delivery-target lock."""
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import time


STALE_BASE_ERROR = "Main changed since integration; rerun the merge sweep"
DEFAULT_RECOVERY_TIMEOUT_SECS = 4 * 60 * 60


def integration_branch(root):
    # Match worktree/integration_branch.rs. Shared local Git config survives
    # checkout renames and applies to every linked worktree.
    configured = subprocess.run(["git", "-C", str(root), "config", "--local", "--get",
                                 "cas.integrationBranch"], capture_output=True, text=True)
    if configured.returncode == 0:
        branch = configured.stdout.strip()
    else:
        branches = git(root, "for-each-ref", "--format=%(refname:short)",
                       "refs/heads/integration/").splitlines()
        if len(branches) > 1:
            # cas-52de: the sweep receipt's tip names the branch the daemon
            # integrates; adopt the one branch whose head is that tip.
            branch = receipt_integration_branch(root, branches)
            if branch is None:
                raise RuntimeError("Multiple legacy integration branches; set git config --local "
                                   "cas.integrationBranch <branch> after reviewing the sweep receipt")
            print(f"Adopted {branch}: its head is the sweep receipt tip", file=sys.stderr)
        else:
            branch = branches[0] if branches else "integration/project"
        git(root, "config", "--local", "cas.integrationBranch", branch)
    if not branch.startswith("integration/"):
        raise RuntimeError("cas.integrationBranch must name an integration/ branch")
    git(root, "check-ref-format", "refs/heads/" + branch)
    return branch


def receipt_integration_branch(root, branches):
    """The one integration branch whose head is the sweep receipt's tip."""
    try:
        common = Path(git(root, "rev-parse", "--path-format=absolute", "--git-common-dir")).resolve()
        tip = json.loads((common.parent / ".cas/merge-sweeps/integration.json").read_text()).get("tip")
    except (OSError, ValueError, RuntimeError):
        return None
    if not isinstance(tip, str) or len(tip) != 40:
        return None
    heads = [branch for branch in branches
             if git(root, "rev-parse", "--verify", f"refs/heads/{branch}^{{commit}}") == tip]
    return heads[0] if len(heads) == 1 else None


def main_input(root):
    common = Path(git(root, "rev-parse", "--path-format=absolute", "--git-common-dir")).resolve()
    tip = git(root, "rev-parse", "--verify", "refs/remotes/origin/main^{commit}")
    return {"repository": str(common), "tip": tip, "base": tip,
            "mode": "from-main", "reason": "Release epics already merged to origin/main by PR; no sweep ran",
            "full_gate_required": True}


def release_input(root):
    previous = assembly_input(root)
    if previous and previous.get("mode") == "from-main":
        return main_input(root)
    common, receipt = integration_receipt(root)
    return {**receipt, "repository": str(common)}


def integration_receipt(root):
    common = Path(git(root, "rev-parse", "--path-format=absolute", "--git-common-dir")).resolve()
    return common, json.loads((common.parent / ".cas/merge-sweeps/integration.json").read_text())


def assembly_input_path():
    run = os.environ.get("CAS_RELEASE_TRAIN_RUN_DIR")
    return Path(run) / "assemble.integration.json" if run else None


def assembly_input(root):
    path = assembly_input_path()
    if path and path.exists():
        value = json.loads(path.read_text())
        if not isinstance(value, dict) or any(
                not isinstance(value.get(key), str) or len(value[key]) != 40
                or any(c not in "0123456789abcdef" for c in value[key]) for key in ("tip", "base")):
            raise RuntimeError("Assembly input receipt is malformed")
        common = Path(git(root, "rev-parse", "--path-format=absolute", "--git-common-dir")).resolve()
        if value.get("repository") != str(common):
            raise RuntimeError("Assembly input belongs to another repository")
        return value
    return None


def record_assembly_input(root):
    path = assembly_input_path()
    if path is None:
        return
    receipt = release_input(root)
    tip = receipt["tip"]
    if (receipt.get("mode") != "from-main" and receipt.get("status") != "PASSED") or not is_ancestor(root, tip, "HEAD"):
        raise RuntimeError("Assembly did not consume the current passing integration tip")
    value = {key: receipt[key] for key in ("repository", "tip", "base")}
    if receipt.get("mode") == "from-main":
        value.update({key: receipt[key] for key in ("mode", "reason", "full_gate_required")})
    write_assembly_input(path, value)


def write_assembly_input(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = path.with_suffix(".tmp")
    temp.write_text(json.dumps(value) + "\n")
    temp.replace(path)


def metadata_input(root, revision):
    # Reuse the exact prep/ledger projection, without probing or running Cargo.
    spec = importlib.util.spec_from_file_location("assembly_proof", Path(__file__).with_name("assembly-proof.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.code_input(root, revision)


def legacy_assembly_base(root, head):
    # Old stage receipts record the assembled checkout, possibly with rebased
    # release prose. Strip only the release-metadata tail to find its code tip.
    value = metadata_input(root, head)
    while True:
        parent = subprocess.run(["git", "-C", str(root), "rev-parse", "--verify", head + "^"],
                                capture_output=True, text=True)
        if parent.returncode or metadata_input(root, parent.stdout.strip()) != value:
            return head
        head = parent.stdout.strip()


def refresh_resume(root):
    path = assembly_input_path()
    if path is None or not (path.parent / "stage.assemble.done").exists():
        return
    receipt = release_input(root)
    if receipt.get("mode") != "from-main" and receipt.get("status") != "PASSED":
        raise RuntimeError("Integration has no passing sweep; resolve the epic sweep report")
    old = assembly_input(root)
    if old is None:
        head = (path.parent / "stage.assemble.done").read_text().strip()
        old = {"tip": legacy_assembly_base(root, head), "base": receipt["base"],
               "repository": str(integration_receipt(root)[0])}
    if old["tip"] == receipt["tip"] and old["base"] == receipt["base"]:
        return
    if git(root, "status", "--porcelain"):
        raise RuntimeError("Release checkout has changes; commit or move them before refreshing integration")
    replay_base = receipt["tip"] if is_ancestor(root, receipt["tip"], "HEAD") else old["tip"]
    validate_release_metadata(root, replay_base, receipt["tip"])
    # Preserve evidence, but invalidate every downstream completion receipt.
    archive = path.parent / "superseded" / (str(time.time_ns()) + "-" + old["tip"][:12])
    archive.mkdir(parents=True)
    stages = "assemble prep ledger gate pr-body pipeline publish post-publication announce report receipts host-update".split()
    names = ["stage." + stage + ".done" for stage in stages]
    names += ["gate.done", "gate.green.epoch", "gate.full.sha", "pipeline.done", "landed-main.sha"]
    for name in names:
        file = path.parent / name
        if file.exists():
            file.replace(archive / name)
    # The old consumed tip remains available for the assemble body's rebase.
    write_assembly_input(path, old)
    print(f"Integration input changed {old['tip']} -> {receipt['tip']}; rerun assemble and later stages; evidence: {archive}")


def validate_release_metadata(root, old_tip, new_tip):
    current = git(root, "rev-parse", "HEAD")
    if (not git(root, "branch", "--show-current").startswith("release/")
            or not is_ancestor(root, old_tip, current)
            or metadata_input(root, old_tip) != metadata_input(root, current)):
        raise RuntimeError(
            "BLOCKER integration-release-metadata: checkout has changes beyond release prose, "
            "member versions or the generated ledger; preserve/review them before replay. "
            "Recovery: " + resume_rebase_command(root, old_tip, new_tip))


def resume_rebase_command(root, old_tip, new_tip):
    version = recorded_run_fields().get("version") or git(root, "branch", "--show-current").removeprefix("release/")
    rebase = ["git", "-C", str(root), "-c", "core.hooksPath=/dev/null", "rebase", "--onto", new_tip, old_tip]
    resume = [str(Path(__file__).with_name("release-train.sh")), version, str(root), "--cut", "--resume"]
    return shlex.join(rebase) + " && " + shlex.join(resume)


def git(root, *args):
    command = ["git", "-C", str(root), *args]
    result = subprocess.run(command, capture_output=True, text=True)
    if result.returncode:
        detail = "\n".join(value.strip() for value in (result.stderr, result.stdout) if value.strip())
        raise RuntimeError(f"Git command failed (exit {result.returncode})\nCommand: {shlex.join(command)}\n"
                           + (detail or "No Git diagnostic output"))
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


def metadata_already_applied(root, old_tip, integration_tip):
    """True when every path the release commits change already has its final
    content on the integration tip (cas-52de: docs copied onto the release
    branch in steps conflicted while the tip carried the same final bytes)."""
    changed = [path for path in git(root, "diff", "--name-only", "-z", "--no-renames",
                                   f"{old_tip}..HEAD").split("\0") if path]
    if not changed:
        return False

    def blob(rev, path):
        found = subprocess.run(["git", "-C", str(root), "rev-parse", "--verify", "--quiet",
                                f"{rev}:{path}"], capture_output=True, text=True)
        return found.stdout.strip() if found.returncode == 0 else None

    return all(blob("HEAD", path) == blob(integration_tip, path) for path in changed)


MERGE_DRIVERS = Path(__file__).resolve().with_name("cas-merge-drivers.py")


def install_merge_drivers(root, *revisions):
    """cas-7aa5: register Cassy's merge drivers (CHANGELOG [Unreleased]
    union, generated hub-web/dist) before replaying release metadata, when
    this checkout or any of `revisions` opts in through .gitattributes."""
    if not MERGE_DRIVERS.exists():
        return
    def marks(text):
        return "merge=cas-changelog" in text or "merge=cas-generated" in text
    local = Path(root) / ".gitattributes"
    opted = local.exists() and marks(local.read_text(errors="replace"))
    for revision in revisions:
        if opted:
            break
        shown = subprocess.run(["git", "-C", str(root), "show", f"{revision}:.gitattributes"],
                               capture_output=True, text=True)
        opted = shown.returncode == 0 and marks(shown.stdout)
    if opted:
        subprocess.run(["python3", str(MERGE_DRIVERS), "install", str(root)],
                       capture_output=True, text=True)


def rebase_release_metadata(root, old_tip, integration_tip):
    """Replay metadata with diagnostics and restore the checkout on conflicts."""
    install_merge_drivers(root, integration_tip, "HEAD")
    original = git(root, "rev-parse", "HEAD")
    if not git(root, "status", "--porcelain") and metadata_already_applied(root, old_tip, integration_tip):
        git(root, "reset", "--hard", integration_tip)
        print("Release metadata already on the integration tip; nothing to replay", file=sys.stderr)
        return False
    try:
        git(root, "-c", "core.hooksPath=/dev/null", "rebase", "--onto", integration_tip, old_tip)
    except RuntimeError as failure:
        aborted = subprocess.run(["git", "-C", str(root), "-c", "core.hooksPath=/dev/null", "rebase", "--abort"],
                                 capture_output=True, text=True)
        restored = (git(root, "rev-parse", "HEAD") == original and not git(root, "status", "--porcelain"))
        state = "checkout restored" if restored else "checkout needs rebase recovery"
        abort_detail = "" if restored or not aborted.returncode else "\nAbort diagnostic: " + aborted.stderr.strip()
        raise RuntimeError(
            "BLOCKER integration-release-metadata: rebase failed; " + state
            + ". Recovery:\n" + resume_rebase_command(root, old_tip, integration_tip)
            + "\nOr copy the release docs as one commit from the integration tip: "
            + "git checkout " + integration_tip + " -- CHANGELOG.md docs/release-notes, commit, rerun --cut"
            + "\n" + str(failure) + abort_detail) from failure
    return True


def rebase_docs_only_release(root, main_tip, integration_tip):
    """Move release metadata commits from main onto the tested union tip."""
    current = git(root, "rev-parse", "HEAD")
    if is_ancestor(root, current, integration_tip) or is_ancestor(root, integration_tip, current):
        return
    previous = assembly_input(root)
    if previous and previous["tip"] != integration_tip:
        old_tip = previous["tip"]
        validate_release_metadata(root, old_tip, integration_tip)
        if rebase_release_metadata(root, old_tip, integration_tip):
            print("Rebased release metadata onto updated integration tip", file=sys.stderr)
        return
    branch = git(root, "branch", "--show-current")
    if not branch.startswith("release/") or not is_ancestor(root, main_tip, current):
        return
    changed = [path for path in git(root, "diff", "--name-only", "-z", "--no-renames",
                                   f"{main_tip}..{current}").split("\0") if path]
    allowed = lambda path: (path == "CHANGELOG.md" or path.startswith("docs/release-notes/")
                            or (path.startswith("docs/qa/journey-evaluations/") and path.endswith(".md")))
    offending = [path for path in changed if not allowed(path)]
    if offending:
        raise RuntimeError(
            "BLOCKER integration-release-metadata: unsupported release paths\n"
            + "\n".join("  " + path for path in offending)
            + "\nPreserve the listed commits separately.\n"
            "Recovery: start release/<ver> from origin/main, then rerun --cut.\n"
            "prep carries the prior receipts commit after assemble.")
    if not changed:
        return
    if rebase_release_metadata(root, main_tip, integration_tip):
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
    """Re-sweep only the epic IDs recorded for this release, once."""
    _, receipt = integration_receipt(root)
    selected = []
    for epic in receipt["epics"]:
        epic_id = epic.get("id")
        if not isinstance(epic_id, str) or not epic_id or "," in epic_id:
            raise RuntimeError("Integration receipt lacks release epic IDs; rerun an explicitly selected sweep")
        selected.append(epic_id)
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
            [command, "factory", "integration-recover", "--base-only",
             "--release-epics", ",".join(selected)],
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
    branch = integration_branch(root)
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
    branch = integration_branch(root)
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


def _assemble_main_locked(root):
    value = main_input(root)
    if git(root, "status", "--porcelain"):
        raise RuntimeError("Assembly checkout has changes; commit or move them first")
    current = git(root, "branch", "--show-current")
    if current and not current.startswith("release/"):
        raise RuntimeError("Use a detached checkout or a release/ branch for assembly")
    previous = assembly_input(root)
    if previous and previous["tip"] != value["tip"]:
        validate_release_metadata(root, previous["tip"], value["tip"])
        rebase_release_metadata(root, previous["tip"], value["tip"])
    git(root, "-c", "core.hooksPath=/dev/null", "merge", "--ff-only", value["tip"])
    path = assembly_input_path()
    if path is None:
        raise RuntimeError("--from-main requires CAS_RELEASE_TRAIN_RUN_DIR for its audit receipt")
    write_assembly_input(path, value)
    return value["tip"]


def assemble(root, from_main=False):
    previous = assembly_input(root)
    if from_main or (previous and previous.get("mode") == "from-main"):
        return _under_delivery_lock(root, _assemble_main_locked)
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
        root = Path(sys.argv[1])
        action = sys.argv[2] if len(sys.argv) > 2 else "assemble"
        if action == "--record-input":
            _under_delivery_lock(root, record_assembly_input)
            return 0
        if action == "--resume-check":
            _under_delivery_lock(root, refresh_resume)
            return 0
        if action not in ("assemble", "--from-main"):
            raise RuntimeError("Unknown assembly action: " + action)
        tip = assemble(root, from_main=action == "--from-main")
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
