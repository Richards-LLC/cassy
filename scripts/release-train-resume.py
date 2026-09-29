#!/usr/bin/env python3
"""Bind dirty release evidence to the run that produced it, never arbitrary dirt."""

import hashlib
import json
from pathlib import Path
import re
import stat
import subprocess
import sys


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args])


def changed_paths(root):
    tracked = git(root, "diff", "--name-only", "-z", "HEAD")
    untracked = git(root, "ls-files", "--others", "--exclude-standard", "-z")
    return {p.decode() for p in (tracked + untracked).split(b"\0") if p}


def file_state(root, path):
    file = root / path
    mode = file.lstat().st_mode
    if not stat.S_ISREG(mode):
        raise ValueError(f"release output is not a regular file: {path}")
    return {
        "sha256": hashlib.sha256(file.read_bytes()).hexdigest(),
        "mode": stat.S_IMODE(mode),
        "index": hashlib.sha256(git(root, "ls-files", "--stage", "-z", "--", path)).hexdigest(),
    }


def legacy_filled_draft(root, run, draft, paths):
    """Allow exactly the old checksum writer's delta for pre-fix release runs."""
    if paths != {draft}:
        return False
    original = git(root, "show", f"HEAD:{draft}")
    expected = original
    receipt = (run / "release-published.receipt").read_text()
    for platform in ("LINUX", "MACOS"):
        token = ("{{" + platform + "_SHA256}}").encode()
        if token not in expected:
            continue
        match = re.search(rf"^{platform}_SHA256=([0-9a-f]{{64}})$", receipt, re.M)
        if not match:
            return False
        expected = expected.replace(token, match[1].encode())
    # The original awk writer prints each line, including a missing final LF.
    if expected != original and not expected.endswith(b"\n"):
        expected += b"\n"
    index = git(root, "show", f":{draft}")
    # No arbitrary staging, mode change, deletion, or extra prose is accepted.
    entry = git(root, "ls-files", "--stage", "--", draft).split()[0]
    head_mode = git(root, "ls-tree", "HEAD", "--", draft).split()[0]
    actual_mode = b"100755" if (root / draft).stat().st_mode & stat.S_IXUSR else b"100644"
    return expected != original and index == original and entry == actual_mode == head_mode \
        and (root / draft).read_bytes() == expected and not (root / draft).is_symlink()


def main():
    action, root_arg, run_arg, version, draft_arg = sys.argv[1:]
    root, run = Path(root_arg).resolve(), Path(run_arg).resolve()
    draft = str(Path(draft_arg).resolve().relative_to(root))
    landed = (run / "landed-main.sha").read_text().strip()
    identity = {"schema": 1, "worktree": str(root), "run": str(run),
                "version": version, "landed": landed, "draft": draft}
    receipt = run / "post-publication-outputs.json"
    paths = changed_paths(root)

    def allowed(path):
        return path == draft or (Path(path).parent == Path("docs/release-reports")
                                 and Path(path).name.startswith(f"v{version}"))

    if action == "record":
        # The caller records immediately after a writer, including a failed
        # writer. Unrelated files are deliberately never added to the receipt.
        outputs = {p: file_state(root, p) for p in sorted(paths) if allowed(p)}
        temp = receipt.with_suffix(".tmp")
        temp.write_text(json.dumps({**identity, "outputs": outputs}, sort_keys=True) + "\n")
        temp.replace(receipt)
        return
    if action != "check":
        raise ValueError(f"unknown action: {action}")
    if not paths:
        return
    if receipt.exists():
        data = json.loads(receipt.read_text())
        if not isinstance(data, dict) or not isinstance(data.get("outputs"), dict):
            raise ValueError("malformed post-publication output receipt")
        if any(data.get(key) != value for key, value in identity.items()):
            raise ValueError("post-publication output receipt belongs to a different release run")
        for path in sorted(paths):
            if not allowed(path) or data["outputs"].get(path) != file_state(root, path):
                raise ValueError(f"unrecorded or modified release output: {path}")
        return
    if not legacy_filled_draft(root, run, draft, paths):
        raise ValueError("unrecorded changes beyond the published draft checksum replacement")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        print(f"ERROR resume outputs: {error}", file=sys.stderr)
        sys.exit(1)
