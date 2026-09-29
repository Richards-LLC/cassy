#!/usr/bin/env python3
"""Two-context release proof shared by assembly, recovery and the release gate.

A receipt records the exact tested tree. Its key omits only release prose;
source, embedded docs, manifests, workflows and test commands remain inputs.
No PASS is published until native nextest and archive-mode in a plain clone
both pass. The gate's own diagnostic rows supply their zero-test guards.
"""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import tempfile
import time

FORMAT = 1
MAX_AGE = 86400
IDENTITY = {"CAS_FACTORY_SESSION", "CAS_AGENT_ROLE", "CAS_AGENT_NAME",
            "CAS_SUPERVISOR_NAME", "CAS_AGENT_ID", "CAS_SESSION_ID", "CAS_ROOT"}
VOLATILE = {"_", "SHLVL", "PWD", "OLDPWD", "CARGO_BUILD_JOBS", "CARGO_TARGET_DIR",
            "VERIFIED_TEST_LOG"}


def digest(value):
    return hashlib.sha256(value).hexdigest()


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args], stderr=subprocess.PIPE)


def clean(root):
    return not git(root, "status", "--porcelain", "--untracked-files=all").strip()


def common_dir(root):
    return Path(git(root, "rev-parse", "--path-format=absolute", "--git-common-dir")
                .decode().strip()).resolve()


def code_input(root, revision="HEAD"):
    # Exclude release prose, retaining any docs referenced by Rust sources
    # (including embedded include_str!/include_bytes! fixtures).
    entries = git(root, "ls-tree", "-rz", revision).split(b"\0")
    references = subprocess.run(
        ["git", "-C", str(root), "grep", "-h", "-A", "3", "-e", "include_str!",
         "-e", "include_bytes!", revision, "--", "*.rs"],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    if references.returncode not in (0, 1):
        raise ValueError("cannot enumerate embedded documentation inputs")
    embedded = re.findall(rb'include_(?:str|bytes)!\s*\(\s*"[^"]*?(docs/[^"]+)"', references.stdout)
    material = []
    for entry in entries:
        if not entry:
            continue
        path = entry.split(b"\t", 1)[1]
        prose = path == b"CHANGELOG.md" or path.startswith(
            (b"docs/release-notes/", b"docs/release-reports/"))
        if not prose or any(path == item or path.startswith(item.rstrip(b"/") + b"/")
                            for item in embedded):
            material.append(entry)
    return digest(b"\0".join(material))


def test_environment(root):
    env = {key: value for key, value in os.environ.items() if key not in IDENTITY}
    env.setdefault("CAS_INIT_TIMEOUT_SECS", "900")
    if "ZIG" in env:
        zig = Path(env["ZIG"])
        if not zig.is_absolute():
            zig = root / zig
    else:
        zig = root / ".context/zig/zig"
        if not zig.is_file():
            zig = common_dir(root).parent / ".context/zig/zig"
    if zig.is_file():
        # Different hardlink paths in sibling worktrees represent the same Zig.
        env["ZIG"] = str(zig.resolve())
    return env


def inputs(root):
    if not clean(root):
        raise ValueError("assembly proof requires a clean checkout")
    env = test_environment(root)
    material = {}
    for key, value in env.items():
        if (key in VOLATILE or key.startswith("CAS_RELEASE_GATE_")
                or key.startswith("CAS_RELEASE_TRAIN_") or key == "ZIG"):
            continue
        material[key] = value
    zig = env.get("ZIG")
    if zig:
        material["ZIG_SHA256"] = digest(Path(zig).read_bytes())
    # Ignored local build inputs still invalidate a proof from another checkout.
    for name in (".env", "cas-cli/.env", ".cargo/config", ".cargo/config.toml"):
        path = root / name
        material["local:" + name] = digest(path.read_bytes()) if path.is_file() else "absent"
    cargo = env.get("CARGO", "cargo")
    tools = []
    for command in ([cargo, "--version"], [cargo, "nextest", "--version"], ["rustc", "-Vv"]):
        tools.append(subprocess.check_output(command, cwd=root, env=env, stderr=subprocess.STDOUT))
    result = {
        "format": FORMAT,
        "repository": str(common_dir(root)),
        "code_input": code_input(root),
        "environment": digest(json.dumps(material, sort_keys=True).encode()),
        "toolchain": digest(b"\0".join(tools)),
        "host": platform.system() + "/" + platform.machine(),
    }
    return result, env


def receipt_path(root, expected):
    key = digest(json.dumps(expected, sort_keys=True).encode())
    return common_dir(root).parent / ".cas/merge-sweeps/assembly-proofs" / (key + ".json")


def matching(root, expected):
    path = receipt_path(root, expected)
    try:
        record = json.loads(path.read_text())
        age = time.time() - record["completed_epoch"]
        if (record["inputs"] != expected or record["status"] != "PASS"
                or not 0 <= age <= MAX_AGE or len(record["tree"]) != 40
                or not isinstance(record["archive_size_bytes"], int)
                or record["archive_size_bytes"] <= 0
                or set(record["contexts"]) != {"worktree", "clone"}
                or any(record["contexts"][name]["status"] != "PASS"
                       or record["contexts"][name]["tree"] != record["tree"]
                       or record["contexts"][name]["passed"] <= 0
                       for name in ("worktree", "clone"))):
            return None
        # The object must still exist; recorded code inputs must belong to the
        # tested commit, rather than an arbitrary claim about the current tree.
        if (git(root, "rev-parse", record["head"] + "^{tree}").decode().strip() != record["tree"]
                or code_input(root, record["head"]) != expected["code_input"]):
            return None
        return record, path
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError):
        return None


def write(path, record):
    temporary = path.with_suffix(".tmp." + str(os.getpid()))
    temporary.write_text(json.dumps(record, indent=2) + "\n")
    temporary.replace(path)


def no_cas_ancestor(path):
    for parent in (path, *path.parents):
        if parent.name == ".cas" or (parent / ".cas").exists():
            raise ValueError("plain clone must have no .cas ancestor: " + str(parent))


def run_row(root, row, env, log_dir):
    row_env = dict(env)
    row_env["CAS_RELEASE_GATE_LOG_DIR"] = str(log_dir / (row + "-rows"))
    row_env["CAS_RELEASE_GATE_ARCHIVE_SIZE_FILE"] = str(log_dir / "archive-size-bytes")
    log = log_dir / (row + ".log")
    print(f"assembly proof: {row} in {root}; log: {log}", flush=True)
    with log.open("w") as stream:
        result = subprocess.run(["bash", str(root / "scripts/release-gate.sh"),
                                 "0.0.0", "--only", row], cwd=root, env=row_env,
                                stdout=stream, stderr=subprocess.STDOUT)
    if result.returncode:
        print(log.read_text()[-6000:], flush=True)
        raise ValueError(f"assembly proof {row} failed: {log}")
    raw = (Path(row_env["CAS_RELEASE_GATE_LOG_DIR"]) / (row + ".log")).read_text()
    passed = re.findall(r"PASS: ([1-9][0-9]*) test\(s\) passed", raw)
    if not passed or not re.search(r"^PASS " + re.escape(row) + r" ", log.read_text(), re.M):
        raise ValueError(f"assembly {row} did not report a nonempty test pass: {log}")
    return {"status": "PASS", "row": row, "checkout": str(root), "log": str(log),
            "tree": git(root, "rev-parse", "HEAD^{tree}").decode().strip(),
            "passed": sum(map(int, passed))}


def prove(root):
    expected, env = inputs(root)
    path = receipt_path(root, expected)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.with_suffix(".lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        found = matching(root, expected)
        if found:
            return found
        head = git(root, "rev-parse", "HEAD").decode().strip()
        record = {"inputs": expected, "status": "RUNNING", "head": head,
                  "tree": git(root, "rev-parse", "HEAD^{tree}").decode().strip(), "contexts": {}}
        write(path, record)
        log_dir = path.parent / (path.stem + "-logs")
        log_dir.mkdir(exist_ok=True)
        record["contexts"]["worktree"] = run_row(root, "nextest", env, log_dir)
        # Default to a real disk, away from both the source .cas and ~/.cas.
        scratch = Path(env.get("CAS_RELEASE_GATE_HOME_DIR", "/var/tmp/cas-release-gate/base"))
        scratch.parent.mkdir(parents=True, exist_ok=True)
        no_cas_ancestor(scratch.parent)
        with tempfile.TemporaryDirectory(prefix="assembly-clone-", dir=scratch.parent) as directory:
            clone = Path(directory) / "repo"
            subprocess.run(["git", "clone", "--quiet", "--shared", "--no-checkout",
                            str(common_dir(root)), str(clone)], check=True)
            subprocess.run(["git", "-C", str(clone), "checkout", "--quiet", "--detach", head], check=True)
            no_cas_ancestor(clone)
            clone_env = dict(env)
            # Reuse compiled dependencies; Cargo invalidates producer paths.
            clone_env["CARGO_TARGET_DIR"] = str(path.parent.parent / "assembly-target")
            record["contexts"]["clone"] = run_row(clone, "archive-mode", clone_env, log_dir)
        current, _ = inputs(root)
        if current != expected or git(root, "rev-parse", "HEAD").decode().strip() != head:
            raise ValueError("assembly inputs changed while tests ran")
        record["status"] = "PASS"
        record["completed_epoch"] = int(time.time())
        size_file = log_dir / "archive-size-bytes"
        record["archive_size_bytes"] = int(size_file.read_text())
        if record["archive_size_bytes"] <= 0:
            raise ValueError("assembly archive is empty")
        write(path, record)
        return record, path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prove", "check", "input"))
    parser.add_argument("root", type=Path)
    args = parser.parse_args()
    try:
        root = args.root.resolve()
        if args.action == "input":
            print(code_input(root))
            return 0
        found = prove(root) if args.action == "prove" else matching(root, inputs(root)[0])
        if not found:
            return 1
        record, path = found
        print(f"PASS assembly receipt={path} source_sha={record['head']} tree={record['tree']} "
              f"code_input={record['inputs']['code_input']} contexts=worktree,clone "
              f"archive_size_bytes={record['archive_size_bytes']}")
        return 0
    except (OSError, ValueError, subprocess.CalledProcessError) as exc:
        if args.action == "prove":
            print("FAIL assembly proof: " + str(exc), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
