#!/usr/bin/env python3
"""Two-context release proof shared by assembly, recovery and the release gate.

A receipt records the exact tested tree. Its key masks release prose, member package versions and the generated ledger;
other manifest/lock content, sources and test commands remain inputs.
No PASS is published until native nextest and archive-mode in a plain clone
both pass. The gate's own diagnostic rows supply their zero-test guards.
"""
import argparse
import fcntl
import fnmatch
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
import tomllib

FORMAT = 1
MAX_AGE = 86400
IDENTITY = {"CAS_FACTORY_SESSION", "CAS_AGENT_ROLE", "CAS_AGENT_NAME",
            "CAS_SUPERVISOR_NAME", "CAS_AGENT_ID", "CAS_SESSION_ID", "CAS_ROOT"}
VOLATILE = {"_", "SHLVL", "PWD", "OLDPWD", "CARGO_BUILD_JOBS", "CARGO_TARGET_DIR",
            "VERIFIED_TEST_LOG",
            # Train output locations do not change the compiled/tested candidate.
            "CAS_RELEASE_ARTIFACTS_ROOT", "CAS_RELEASE_RECEIPTS_RUN_DIR"}


def digest(value):
    return hashlib.sha256(value).hexdigest()


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args], stderr=subprocess.PIPE)


def clean(root):
    return not git(root, "status", "--porcelain", "--untracked-files=all").strip()


def common_dir(root):
    return Path(git(root, "rev-parse", "--path-format=absolute", "--git-common-dir")
                .decode().strip()).resolve()


def mask_version(text, table):
    """Preserve every byte except the quoted version value in this table."""
    active = False
    result = []
    for line in text.splitlines(keepends=True):
        if line.lstrip().startswith("["):
            active = line.split("#", 1)[0].strip() == table
        if active:
            match = re.match(r"""^[ \t]*version[ \t]*=[ \t]*(["'])([^"'\r\n]*)\1""", line)
            if match:
                start, end = match.span(2)
                line = line[:start] + "assembly-member-version" + line[end:]
        result.append(line)
    return "".join(result)


def release_metadata(root, revision, entries):
    """Find actual workspace members in the tested revision, without Cargo."""
    paths = {entry.split(b"\t", 1)[1].decode() for entry in entries if entry}
    if "Cargo.toml" not in paths:
        return {}, {}
    root_text = git(root, "show", revision + ":Cargo.toml").decode()
    manifest = tomllib.loads(root_text)
    workspace = manifest.get("workspace", {})
    patterns = workspace.get("members", [])
    excludes = workspace.get("exclude", [])
    member_paths = []
    for path in paths:
        if path.endswith("/Cargo.toml"):
            directory = path.removesuffix("/Cargo.toml")
            if (any(fnmatch.fnmatchcase(directory, pattern.rstrip("/")) for pattern in patterns)
                    and not any(fnmatch.fnmatchcase(directory, pattern.rstrip("/")) for pattern in excludes)):
                member_paths.append(path)
    if "package" in manifest:
        member_paths.append("Cargo.toml")
    normalized, versions = {}, {}
    for path in member_paths:
        text = root_text if path == "Cargo.toml" else git(root, "show", revision + ":" + path).decode()
        package = tomllib.loads(text).get("package", {})
        version = package.get("version")
        if isinstance(version, str):
            versions[package["name"]] = version
            normalized[path] = mask_version(text, "[package]").encode()
    if "Cargo.lock" in paths:
        text = git(root, "show", revision + ":Cargo.lock").decode()
        # Each generated lock stanza is a separate TOML package table. A
        # registry/git entry with a colliding member name remains an input.
        parts = re.split(r"(?m)(?=^\[\[package\]\][ \t]*(?:#.*)?$)", text)
        for index, part in enumerate(parts):
            if part.startswith("[[package]]"):
                package = tomllib.loads(part)["package"][0]
                if ("source" not in package and package.get("name") in versions
                        and package.get("version") == versions[package["name"]]):
                    parts[index] = mask_version(part, "[[package]]")
        normalized["Cargo.lock"] = "".join(parts).encode()
    return normalized, versions


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
    normalized, _ = release_metadata(root, revision, entries)
    material = []
    for entry in entries:
        if not entry:
            continue
        path = entry.split(b"\t", 1)[1]
        if path == b"cas-cli/src/builtins/reference-history.json":
            continue
        if path.decode() in normalized:
            mode_type = entry.split(b" ", 2)[:2]
            entry = b" ".join(mode_type + [digest(normalized[path.decode()]).encode()]) + b"\t" + path
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
    material = environment_material(root, env)
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


def environment_material(root, env):
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
    return material


def receipt_path(root, expected):
    key = digest(json.dumps(expected, sort_keys=True).encode())
    return common_dir(root).parent / ".cas/merge-sweeps/assembly-proofs" / (key + ".json")


def explain_miss(root, expected):
    """Compare the newest nearby receipt without granting it authorization."""
    paths = sorted(receipt_path(root, expected).parent.glob("*.json"),
                   key=lambda path: path.stat().st_mtime, reverse=True)
    for path in paths[:100]:
        try:
            record = json.loads(path.read_text())
            recorded = record["inputs"]
            if record.get("status") != "PASS" or recorded.get("repository") != expected.get("repository"):
                continue
            for key in expected:
                if recorded.get(key) != expected[key]:
                    detail = ""
                    if key == "environment" and isinstance(record.get("environment_keys"), dict):
                        current = {name: digest(value.encode())
                                   for name, value in environment_material(root, test_environment(root)).items()}
                        prior = record["environment_keys"]
                        differing = next((name for name in sorted(set(prior) | set(current))
                                          if prior.get(name) != current.get(name)), None)
                        if differing:
                            detail = f" environment_key={differing}"
                    return f"MISS assembly key={key} reason=different{detail} receipt={path}"
        except (OSError, ValueError, KeyError, TypeError, AttributeError):
            continue
    return "MISS assembly key=receipt reason=no_matching_receipt"


def matching(root, expected, diagnostic=False):
    path = receipt_path(root, expected)

    def miss(key, reason):
        if diagnostic:
            print(f"MISS assembly key={key} reason={reason} receipt={path}", file=sys.stderr)
        return None

    try:
        record = json.loads(path.read_text())
        for key in expected:
            if record["inputs"].get(key) != expected[key]:
                return miss(key, "different")
        if record["inputs"] != expected:
            return miss("inputs", "unexpected_keys")
        if record["status"] != "PASS":
            return miss("status", "not_PASS")
        age = time.time() - record["completed_epoch"]
        if not 0 <= age <= MAX_AGE:
            return miss("completed_epoch", "future_or_expired")
        if len(record["tree"]) != 40:
            return miss("tree", "invalid")
        if (not isinstance(record["archive_size_bytes"], int)
                or record["archive_size_bytes"] <= 0):
            return miss("archive_size_bytes", "empty_or_invalid")
        if (set(record["contexts"]) != {"worktree", "clone"}
                or any(record["contexts"][name]["status"] != "PASS"
                       or record["contexts"][name]["tree"] != record["tree"]
                       or record["contexts"][name]["passed"] <= 0
                       for name in ("worktree", "clone"))):
            return miss("contexts", "incomplete_or_incoherent")
        if git(root, "rev-parse", record["head"] + "^{tree}").decode().strip() != record["tree"]:
            return miss("head", "tested_tree_differs")
        if code_input(root, record["head"]) != expected["code_input"]:
            return miss("code_input", "tested_commit_differs")
        return record, path
    except FileNotFoundError:
        if diagnostic:
            print(explain_miss(root, expected), file=sys.stderr)
        return None
    except (OSError, ValueError, KeyError, TypeError, AttributeError, subprocess.CalledProcessError):
        return miss("receipt", "malformed_or_unavailable_object")


def write(path, record):
    temporary = path.with_suffix(".tmp." + str(os.getpid()))
    temporary.write_text(json.dumps(record, indent=2) + "\n")
    temporary.replace(path)


def no_cas_ancestor(path):
    for parent in (path, *path.parents):
        if parent.name == ".cas" or (parent / ".cas").exists():
            raise ValueError("plain clone must have no .cas ancestor: " + str(parent))


def clone_scratch(env):
    scratch = Path(env.get("CAS_RELEASE_GATE_HOME_DIR") or "/var/tmp/cas-release-gate/base").resolve()
    # Keep the temp-root list in sync with known_repos::temp_root and
    # cloud::ephemeral_project:
    # a plain clone must exercise durable-project discovery, not a throwaway.
    roots = [Path(name) for name in ("/tmp", "/var/tmp", "/private/tmp", "/private/var/tmp")]
    if env.get("TMPDIR"):
        roots.append(Path(env["TMPDIR"]))
    for root in roots:
        for spelling in (root.absolute(), root.resolve()):
            if scratch == spelling or spelling in scratch.parents:
                raise ValueError(
                    f"plain clone scratch {scratch} is under Cassy disposable root {root}; "
                    "set CAS_RELEASE_GATE_HOME_DIR to a base on the checkout filesystem "
                    "outside system temporary roots, TMPDIR and every .cas ancestor")
    no_cas_ancestor(scratch.parent)
    return scratch


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
    # Refuse an unusable clone context before tool probing or the native suite.
    scratch = clone_scratch(os.environ)
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
                  "tree": git(root, "rev-parse", "HEAD^{tree}").decode().strip(), "contexts": {},
                  "environment_keys": {key: digest(value.encode())
                                       for key, value in environment_material(root, env).items()}}
        write(path, record)
        log_dir = path.parent / (path.stem + "-logs")
        log_dir.mkdir(exist_ok=True)
        record["contexts"]["worktree"] = run_row(root, "nextest", env, log_dir)
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
        found = prove(root) if args.action == "prove" else matching(root, inputs(root)[0], diagnostic=True)
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
        elif args.action == "check":
            print("MISS assembly key=checkout reason=" + str(exc), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
