#!/usr/bin/env python3
"""Two-context release proof shared by assembly, recovery and the release gate.

A receipt records the exact tested tree. Its key masks release prose, member package versions and the generated ledger;
other manifest/lock content, sources and test commands remain inputs.
No PASS is published until CI script tests, native nextest and archive-mode
in a plain clone all pass. Rust rows supply their zero-test guards.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import csv
import fcntl
import fnmatch
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import tempfile
import time
import tomllib
# Also support the existing importlib fixture/receipt consumers.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import release_scratch

_target_spec = importlib.util.spec_from_file_location("proof_target", Path(__file__).with_name("proof_target.py"))
proof_target = importlib.util.module_from_spec(_target_spec)
_target_spec.loader.exec_module(proof_target)

FORMAT = 2
MAX_AGE = 86400
GIB = 1024 ** 3
# Soundwave serial proof 7e4c6f50874fb738e0a4d59dfe54a7cbe191b07466dfa8ba09dd7cc27d49e23e
# (abd6817b5), GNU time -v: maximum
# single-process RSS 7,293,348 KiB; MemAvailable fell ~6–7 GB at jobs=16.
# Round the large cas rustc/link unit up to 8 GiB per producer. Dependency
# jobs are much smaller; 256 MiB/job and 2 GiB for scripts are assumptions,
# to be checked against supervisor peak-RSS/PSI samples on both hosts.
COMPILE_JOB_BYTES = GIB // 4
PRODUCER_BYTES = 8 * GIB
SCRIPT_BYTES = 2 * GIB
TEST_FIXED_BYTES = 4 * GIB
TEST_THREAD_BYTES = GIB // 4
# Soundwave incremental cas relink, b86ec0c2e + train9, 2026-10-05:
# .cas/perf-98a0/link-rss.log, 0.5s ps sampler: max cc/mold 2.089 GiB,
# rustc 4.555 GiB; 30 link processes peaked at 9.75 GiB summed. Round links
# conservatively to 2.1 GiB/slot. Retain the cold-proof 8 GiB producer bound
# above rather than assume incremental rustc RSS covers cold code generation.
LINK_BYTES = math.ceil(2.1 * GIB)
GUARD_HEADROOM_BYTES = 2 * GIB
# Exact harness/session names: scrub these from the environment passed to every
# test row as well as its fingerprint. Do not ignore CAS_FACTORY_* wholesale;
# build controls and unknown future variables remain test inputs.
IDENTITY = {"CAS_FACTORY_SESSION", "CAS_AGENT_ROLE", "CAS_AGENT_NAME",
            "CAS_SUPERVISOR_NAME", "CAS_AGENT_ID", "CAS_SESSION_ID", "CAS_ROOT",
            "AI_AGENT", "CLAUDECODE", "CLAUDE_CODE_CHILD_SESSION", "CAS_CLONE_PATH",
            "CAS_FACTORY_MODE", "CAS_FACTORY_SUPERVISOR_CLI", "CAS_FACTORY_WORKER_CLI"}
VOLATILE = {"_", "SHLVL", "PWD", "OLDPWD", "CARGO_BUILD_JOBS", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR",
            "VERIFIED_TEST_LOG",
            # Train output locations do not change the compiled/tested candidate.
            "CAS_RELEASE_ARTIFACTS_ROOT", "CAS_RELEASE_RECEIPTS_RUN_DIR",
            # Publishing sources this file after assembly; any loaded build or
            # test variables are still included by their own names.
            "CAS_RELEASE_ENV_FILE"}

# cas-398c: the proof key is the whole scrubbed environment, so a proof made by
# the factory daemon in the background satisfies a cut run from a supervisor
# shell only when nothing test-relevant differs. Every name below is excluded
# from the key AND scrubbed from the environment the proof's test rows run in,
# so it cannot change a test outcome. Anything not listed stays a key input,
# including unknown future names; a miss then names the differing variable.
HARNESS_NOISE = {
    # Terminal and login-session plumbing of whoever launched the proof.
    "TERM": "terminal type of the launching shell",
    "COLORTERM": "terminal colour capability of the launching shell",
    "TERM_PROGRAM": "terminal emulator of the launching shell",
    "TERM_PROGRAM_VERSION": "terminal emulator of the launching shell",
    "TERM_SESSION_ID": "terminal session of the launching shell",
    "TMUX": "multiplexer socket of the launching shell",
    "TMUX_PANE": "multiplexer pane of the launching shell",
    "STY": "screen session of the launching shell",
    "WINDOW": "screen window of the launching shell",
    "WINDOWID": "X window of the launching shell",
    "DISPLAY": "graphical session of the launching shell",
    "WAYLAND_DISPLAY": "graphical session of the launching shell",
    "DBUS_SESSION_BUS_ADDRESS": "desktop bus of the login session",
    "XDG_SESSION_ID": "login session id",
    "XDG_SESSION_TYPE": "login session type",
    "XDG_SESSION_CLASS": "login session class",
    "XDG_VTNR": "login virtual terminal",
    "SSH_AUTH_SOCK": "ssh agent of the launching shell; proof rows use no remote",
    "SSH_AGENT_PID": "ssh agent of the launching shell; proof rows use no remote",
    "SSH_CLIENT": "remote login address",
    "SSH_CONNECTION": "remote login address",
    "SSH_TTY": "remote login terminal",
    # Interactive editors: a proof row never opens one.
    "EDITOR": "interactive editor; proof rows never open one",
    "VISUAL": "interactive editor; proof rows never open one",
    "GIT_EDITOR": "interactive editor; proof rows never open one",
    # Agent harness settings exported to the harness's own child shells.
    "COREPACK_ENABLE_AUTO_PIN": "Claude Code child-shell default; rows see corepack's default",
    "DISABLE_AUTOUPDATER": "Claude Code harness setting",
    "DISABLE_COST_WARNINGS": "Claude Code harness setting",
    "IS_DEMO": "Claude Code harness setting",
    "NoDefaultCurrentDirectoryInExePath": "Claude Code harness setting (Windows lookup)",
    # Factory worker spawn settings: they configure the agent, not the build.
    "CAS_FACTORY_WORKER_MODEL": "factory worker spawn setting (agent model)",
    "CAS_FACTORY_WORKER_EFFORT": "factory worker spawn setting (agent effort)",
    "CAS_FACTORY_WORKER_ACCOUNT_DIR": "factory worker spawn setting (agent account)",
    "CAS_FACTORY_CLAUDE_CONFIG_DIR_SOURCE": "factory worker spawn setting (agent config)",
    "CAS_FACTORY_NICE_WORKER": "factory worker CPU priority; changes scheduling, not results",
}
EXCLUDED_PREFIXES = (
    ("CLAUDE_", "Claude Code harness session plumbing"),
    ("CODEX_", "Codex harness session plumbing"),
)
CREDENTIAL_SUFFIXES = ("_TOKEN", "_API_KEY", "_SECRET", "_PASSWORD")


def exclusion_reason(name):
    """Why `name` is scrubbed from proof rows and excluded from the key."""
    if name in IDENTITY:
        return "harness/session identity"
    if name in HARNESS_NOISE:
        return HARNESS_NOISE[name]
    for prefix, reason in EXCLUDED_PREFIXES:
        if name.startswith(prefix):
            return reason
    if name.endswith(CREDENTIAL_SUFFIXES):
        return "credential; scrubbed so no proof row can reach a live service"
    return None


def key_only_exclusion(name):
    """Why `name` stays in the row environment but is not a key input."""
    if name in VOLATILE:
        return "output location, shell bookkeeping or build parallelism; not the tested candidate"
    if name.startswith("CAS_RELEASE_GATE_") or name.startswith("CAS_RELEASE_TRAIN_"):
        return "release gate/train orchestration control"
    if name == "ZIG":
        return "replaced by ZIG_SHA256 of the resolved binary"
    return None



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
            (b"docs/release-notes/", b"docs/release-reports/")) or (
                path.startswith(b"docs/qa/journey-evaluations/") and path.endswith(b".md"))
        if not prose or any(path == item or path.startswith(item.rstrip(b"/") + b"/")
                            for item in embedded):
            material.append(entry)
    return digest(b"\0".join(material))


def with_cargo_bin(env):
    """The PATH release-portable.sh gives the gate and the train.

    release_portable_path_add_cargo_bin appends an existing Cargo bin directory
    missing from PATH. A proof run from a shell without it (a macOS agent or
    launchd shell) otherwise keys a different PATH than the gate and train that
    look it up, so the receipt never matched there (cas-db34).
    """
    home = env.get("CARGO_HOME") or (env.get("HOME", "") + "/.cargo")
    cargo_bin = home + "/bin"
    path = env.get("PATH", "")
    if Path(cargo_bin).is_dir() and cargo_bin not in path.split(":"):
        env["PATH"] = f"{path}:{cargo_bin}" if path else cargo_bin
    return env


def test_environment(root):
    env = {key: value for key, value in os.environ.items() if exclusion_reason(key) is None}
    with_cargo_bin(env)
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
    """Hash the scrubbed test environment conservatively.

    Include every remaining variable, even unknown CAS_* names, except the
    explicit output/shell controls above and release gate/train orchestration.
    Local config bytes and resolved Zig bytes are additional inputs.
    """
    material = {}
    for key, value in env.items():
        if key_only_exclusion(key) or exclusion_reason(key):
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


def environment_policy(root):
    """Names only: which variables key this proof and why the rest do not."""
    env = test_environment(root)
    excluded = {}
    for name in os.environ:
        reason = exclusion_reason(name) or key_only_exclusion(name)
        if reason:
            excluded[name] = reason
    return {"included": sorted(environment_material(root, env)), "excluded": dict(sorted(excluded.items()))}


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
        if (record["script_tests"]["status"] != "PASS"
                or record["script_tests"]["row"] != "ci-script-tests"
                or record["script_tests"]["tree"] != record["tree"]):
            return miss("script_tests", "incomplete_or_incoherent")
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
    source = proof_target.prepare(root) if row != "ci-script-tests" else proof_target.identity(root)
    row_env = proof_target.environment(env, source)
    row_env["CAS_RELEASE_GATE_LOG_DIR"] = str(log_dir / (row + "-rows"))
    row_env["CAS_RELEASE_GATE_ARCHIVE_SIZE_FILE"] = str(log_dir / "archive-size-bytes")
    log = log_dir / (row + ".log")
    print(f"assembly proof: {row} in {root}; log: {log}", flush=True)
    with log.open("w") as stream:
        stream.write("PROOF_SOURCE: " + json.dumps(source, sort_keys=True) + "\n")
        stream.flush()
        result = release_scratch.child_run(["bash", str(root / "scripts/release-gate.sh"),
                                 "0.0.0", "--only", row], cwd=root, env=row_env,
                                stdout=stream, stderr=subprocess.STDOUT)
    if result.returncode:
        print(log.read_text()[-6000:], flush=True)
        raise ValueError(f"assembly proof {row} failed: {log}")
    raw = (Path(row_env["CAS_RELEASE_GATE_LOG_DIR"]) / (row + ".log")).read_text()
    if not re.search(r"^PASS " + re.escape(row) + r" ", log.read_text(), re.M):
        raise ValueError(f"assembly {row} did not report a pass: {log}")
    result = {"status": "PASS", "row": row, "checkout": str(root), "log": str(log),
              "tree": git(root, "rev-parse", "HEAD^{tree}").decode().strip(), "head": source["head"], "target": source["target"]}
    for filename, key in (("timing.tsv", "timing"), ("compile-timing.tsv", "compile_timing")):
        timing_path = Path(row_env["CAS_RELEASE_GATE_LOG_DIR"]) / filename
        if timing_path.is_file():
            with timing_path.open() as stream:
                timings = list(csv.DictReader(stream, delimiter="\t"))
            if len(timings) != 1 or timings[0]["row"] != row or timings[0]["status"] != "0":
                raise ValueError(f"assembly {row} has invalid {filename}: {timing_path}")
            result[key] = timings[0]
            wall, user, system = (float(timings[0][name]) for name in ("wall_s", "user_s", "system_s"))
            if any(not math.isfinite(value) or value < 0 for value in (wall, user, system)):
                raise ValueError(f"assembly {row} has invalid CPU timing: {timing_path}")
            result[key]["average_cores_busy"] = round((user + system) / wall, 3) if wall else 0
    admission_path = Path(row_env["CAS_RELEASE_GATE_LOG_DIR"]) / "memory-admission.json"
    if admission_path.is_file():
        result["test_memory"] = json.loads(admission_path.read_text())
    for filename, key in (("compile-memory.jsonl", "compile_memory"), ("link-rss.jsonl", "link_rss")):
        event_path = Path(row_env["CAS_RELEASE_GATE_LOG_DIR"]) / filename
        if event_path.is_file():
            events = [json.loads(line) for line in event_path.read_text().splitlines()]
            result[key] = events
            if any(event.get("action", "").endswith("abort") or event.get("estimate_exceeded") for event in events):
                raise ValueError(f"assembly {row} has unsafe memory evidence: {event_path}")
    if row == "ci-script-tests":
        return result
    passed = re.findall(r"PASS: ([1-9][0-9]*) test\(s\) passed", raw)
    if not passed:
        raise ValueError(f"assembly {row} did not report a nonempty test pass: {log}")
    result["passed"] = sum(map(int, passed))
    return result


def positive_knob(env, name):
    configured = env.get(name)
    if configured is not None:
        if not re.fullmatch(r"[1-9][0-9]*", configured):
            raise ValueError(name + " must be a positive integer")
        return int(configured)
    return None


def cpu_count():
    try:
        return max(1, len(os.sched_getaffinity(0)))
    except (AttributeError, OSError):
        return os.cpu_count() or 1


def memory_snapshot():
    if platform.system() == "Darwin":
        total = int(subprocess.check_output(["sysctl", "-n", "hw.memsize"]))
        vm = subprocess.check_output(["vm_stat"], text=True)
        page_size = int(re.search(r"page size of ([0-9]+) bytes", vm)[1])
        pages = {name: int(value) for name, value in
                 re.findall(r"^([^:]+):\s*([0-9]+)\.", vm, re.M)}
        # Do not count compressed or purgeable pages separately: they overlap
        # resident categories. This is a conservative vm_stat approximation.
        available = sum(pages[name] for name in
                        ("Pages free", "Pages inactive", "Pages speculative")) * page_size
        source = "sysctl hw.memsize + vm_stat free/inactive/speculative"
    else:
        fields = {name: int(value) * 1024 for name, value in
                  re.findall(r"^([^:]+):\s*([0-9]+) kB", Path("/proc/meminfo").read_text(), re.M)}
        total, available = fields["MemTotal"], fields["MemAvailable"]
        source = "/proc/meminfo MemTotal/MemAvailable"
    if not 0 < total or not 0 <= available <= total:
        raise ValueError("invalid physical memory snapshot")
    return {"total_bytes": total, "available_bytes": available, "source": source}


def memory_budget(env):
    try:
        snapshot = memory_snapshot()
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as exc:
        raise ValueError("cannot safely admit assembly without a memory snapshot: " + str(exc)) from exc
    configured = positive_knob(env, "CAS_RELEASE_GATE_ASSEMBLY_RESERVE_GIB")
    reserve = configured * GIB if configured is not None else max(8 * GIB, snapshot["total_bytes"] // 4)
    return dict(snapshot, reserve_bytes=reserve,
                budget_bytes=max(0, snapshot["available_bytes"] - reserve))


def execution_plan(env):
    positive_knob(env, "CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS")
    positive_knob(env, "CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLL_SECS")
    link_jobs = positive_knob(env, "CAS_RELEASE_GATE_ASSEMBLY_LINK_JOBS") or 8
    memory = memory_budget(env)
    requested = positive_knob(env, "CAS_RELEASE_GATE_ASSEMBLY_BUILD_JOBS")
    cores = cpu_count()
    cap = min(requested or cores, max(1, cores // 2))
    jobs = min(cap, max(0, (memory["budget_bytes"] - 2 * PRODUCER_BYTES - SCRIPT_BYTES
                           - LINK_BYTES - GUARD_HEADROOM_BYTES)
                        // (2 * COMPILE_JOB_BYTES)))
    mode = "concurrent" if jobs else "serial"
    reason = ("two producers and script tier fit above memory reserve" if jobs else
              "insufficient available memory for two producers plus script tier; using serial legs")
    return dict(memory, mode=mode, reason=reason, cores=cores, requested_compile_jobs=requested,
                compile_jobs=jobs, per_job_bytes=COMPILE_JOB_BYTES,
                producer_overhead_bytes=PRODUCER_BYTES, script_bytes=SCRIPT_BYTES,
                link_jobs=link_jobs, link_admission="fresh-memory shared pool", per_link_bytes=LINK_BYTES, guard_headroom_bytes=GUARD_HEADROOM_BYTES,
                link_estimate_source="soundwave b86ec0c2e + train9 incremental relink, 2026-10-05, .cas/perf-98a0/link-rss.log: max ld.mold 2190228 KiB (2.089 GiB), rustc 4775752 KiB (4.555 GiB), 0.5s ps; links rounded to 2.1 GiB; cold producer bound remains 8 GiB",
                estimate_source="8 GiB large unit rounded from measured 7293348 KiB max RSS, soundwave proof 7e4c6f50 (abd6817b5); 256 MiB/dependency job and 2 GiB scripts assumed",
                phases=[])


def admit_phase(env, execution, phase, compile_phase=False):
    wait_secs = positive_knob(env, "CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS") or 600
    poll_secs = positive_knob(env, "CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLL_SECS") or 2
    started = time.monotonic()
    while True:
        memory = memory_budget(env)
        if phase == "ci-script-tests":
            count = int(memory["budget_bytes"] >= SCRIPT_BYTES)
        elif compile_phase:
            cap = positive_knob(env, "CAS_RELEASE_GATE_ASSEMBLY_BUILD_JOBS") or cpu_count()
            count = min(cap, max(0, (memory["budget_bytes"] - PRODUCER_BYTES - LINK_BYTES
                                    - GUARD_HEADROOM_BYTES) // COMPILE_JOB_BYTES))
        else:
            count = min(cpu_count(), max(0, (memory["budget_bytes"] - TEST_FIXED_BYTES) // TEST_THREAD_BYTES))
        elapsed = time.monotonic() - started
        event = dict(memory, phase=phase, admitted=bool(count), elapsed_s=round(elapsed, 3),
                     deadline_s=wait_secs,
                     **({"compile_jobs": count} if compile_phase else {"test_threads": count}))
        execution["phases"].append(event)
        print("assembly memory admission: " + json.dumps(event, sort_keys=True), flush=True)
        if count:
            return str(count)
        if elapsed >= wait_secs:
            raise ValueError(f"assembly {phase} cannot fit above memory reserve after {wait_secs}s")
        time.sleep(min(poll_secs, wait_secs - elapsed))


# None selects the real host/user admission directory. In-process tests point
# it at a private directory so they never contend with live worker suites.
HOST_MEMORY_DIRECTORY = None


def run_contexts(root, clone, env, log_dir, clone_target, execution):
    # Worker suites and proofs use one host/user budget across worktrees and
    # clones. This is independent of the link-specific admission pool.
    spec = importlib.util.spec_from_file_location("host_memory", Path(__file__).with_name("host_memory.py"))
    host = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(host)
    wait = positive_knob(env, "CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS") or 600
    poll = positive_knob(env, "CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLL_SECS") or 1
    # Rows build with sccache; its server must not start inside the proof's
    # tree, where it would outlive the proof (cas-7b7b9).
    host.start_compiler_cache(env)
    with host.admission("proof", env, memory_budget, wait, poll, HOST_MEMORY_DIRECTORY) as (admitted_env, fds), \
         host.LeaseHolder(fds) as holder:
        # Rows never receive the proof's intent/budget descriptors, so an
        # sccache server or a test's orphan cannot keep them. The holder keeps
        # the lease across a killed proof while its row process groups run.
        # Nested admissions reuse it by ancestry and the live lock, not by FD.
        hooks = release_scratch.CURRENT.spawn_hooks if release_scratch.CURRENT else None
        if hooks is not None:
            hooks.append(holder.track)
        try:
            return _run_contexts(root, clone, admitted_env, log_dir, clone_target, execution)
        finally:
            if hooks is not None:
                hooks.remove(holder.track)


def _run_contexts(root, clone, env, log_dir, clone_target, execution):
    # Builds and script fixtures have independent checkouts/targets/logs. Test
    # groups only constrain one nextest process, and host ports/hub processes
    # are not all globally locked: serialize consumers after script admission.
    # Clone preparation may have taken time: admit against current memory,
    # not the earlier receipt snapshot. Knobs never bypass memory admission.
    clone_target = clone / "target"
    env = dict(env, CARGO_TARGET_DIR=str(root / "target"), CARGO_BUILD_TARGET_DIR=str(root / "target"),
               CAS_RELEASE_GATE_ASSEMBLY_LINK_GUARD_DIR=str(log_dir / "linker-guards"))
    execution.update(execution_plan(env))
    print("assembly scheduling: " + json.dumps(execution, sort_keys=True), flush=True)
    if execution["budget_bytes"] < SCRIPT_BYTES:
        admit_phase(env, execution, "ci-script-tests")
        samples = execution["phases"]
        execution.update(execution_plan(env))
        execution["phases"] = samples
    if execution["mode"] == "serial":
        scripts = run_row(root, "ci-script-tests", env, log_dir)
        results = []
        for checkout, row in ((root, "nextest"), (clone, "archive-mode")):
            row_env = dict(env, CARGO_BUILD_JOBS=admit_phase(env, execution, row + "-compile", True),
                           CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLICY=json.dumps(env_policy(env)))
            if row == "archive-mode":
                row_env["CARGO_TARGET_DIR"] = str(clone_target)
                row_env["CARGO_BUILD_TARGET_DIR"] = str(clone_target)
            results.append(run_row(checkout, row, row_env, log_dir))
        return scripts, *results
    with tempfile.TemporaryDirectory(prefix="assembly-sync-", dir=log_dir) as directory:
        sync = Path(directory)
        (sync / "owner").write_text(str(os.getpid()))
        native_env = dict(env, CAS_RELEASE_GATE_ASSEMBLY_SYNC_DIR=str(sync),
                          CARGO_BUILD_JOBS=str(execution["compile_jobs"]),
                          CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLICY=json.dumps(env_policy(env)))
        clone_env = dict(native_env, CARGO_TARGET_DIR=str(clone_target), CARGO_BUILD_TARGET_DIR=str(clone_target))
        with ThreadPoolExecutor(max_workers=3) as executor:
            scripts = executor.submit(run_row, root, "ci-script-tests", env, log_dir)
            native = executor.submit(run_row, root, "nextest", native_env, log_dir)
            archive = executor.submit(run_row, clone, "archive-mode", clone_env, log_dir)
            try:
                script_result = scripts.result()
                # Finish both producers before tests: no rustc/link peak can
                # overlap a consumer's debug processes and tmpfs fixtures.
                while not all((sync / ("compiled-" + row)).exists()
                              for row in ("nextest", "archive-mode")):
                    for future in (native, archive):
                        if future.done():
                            future.result()  # surface compile failure, abort waiter
                    time.sleep(0.1)
                write(sync / "release-nextest", int(admit_phase(env, execution, "nextest-tests")))
                native_result = native.result()
                write(sync / "release-archive-mode", int(admit_phase(env, execution, "archive-mode-tests")))
                return script_result, native_result, archive.result()
            finally:
                # Failed scripts/builds cannot strand the other consumer, or
                # grant a PASS; join children before removing clone and sync.
                (sync / "abort").touch()


def env_policy(env):
    """Only admission knobs cross the shell boundary, never the full env."""
    return {key: env[key] for key in ("CAS_RELEASE_GATE_ASSEMBLY_BUILD_JOBS",
                                    "CAS_RELEASE_GATE_ASSEMBLY_RESERVE_GIB",
                                    "CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS",
                                    "CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLL_SECS",
                                    "CAS_RELEASE_GATE_ASSEMBLY_LINK_JOBS") if key in env}


def prove(root):
    with release_scratch.ChildScope():
        return prove_owned(root)


def prove_owned(root):
    # Refuse an unusable clone context before tool probing or the native suite.
    scratch = clone_scratch(os.environ)
    scratch_report = release_scratch.sweep(root, scratch, clean=True)
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
                  "tree": git(root, "rev-parse", "HEAD^{tree}").decode().strip(), "contexts": {}, "scratch": scratch_report,
                  "environment_keys": {key: digest(value.encode())
                                       for key, value in environment_material(root, env).items()},
                  "environment_policy": environment_policy(root)}
        write(path, record)
        log_dir = path.parent / (path.stem + "-logs")
        log_dir.mkdir(exist_ok=True)
        record["execution"] = execution_plan(env)
        write(path, record)
        scratch.parent.mkdir(parents=True, exist_ok=True)
        no_cas_ancestor(scratch.parent)
        legacy_target = path.parent.parent / "assembly-target"
        # Inventory both old shared cache layouts without silently adopting
        # opaque or live outputs. They are no longer Cargo proof targets.
        record["legacy_cache"] = [release_scratch.cache_report(root, candidate, env=env)
                                  for candidate in (legacy_target, legacy_target.with_name(legacy_target.name + "-leased-v1"))]
        try:
            with release_scratch.OwnedDirectory("assembly-clone-", scratch.parent) as directory:
                clone = Path(directory) / "repo"
                release_scratch.child_run(["git", "clone", "--quiet", "--shared", "--no-checkout",
                                str(common_dir(root)), str(clone)], check=True)
                release_scratch.child_run(["git", "-C", str(clone), "checkout", "--quiet", "--detach", head], check=True)
                no_cas_ancestor(clone)
                # The private cache's sibling lifetime lock is administrative,
                # not a candidate source file. Ignore it only in this clone.
                with (clone / ".git/info/exclude").open("a") as exclude:
                    exclude.write("\n/target.lock\n")
                cache = release_scratch.BoundedCache(clone / "target", env, root)
                record["cache"] = cache.events
                with cache as clone_target:
                    # Every source root owns its output/freshness. Only immutable
                    # worker dependency snapshots seed the disposable clone.
                    proof_target.prepare(clone, cache=proof_target.cache_root(root))
                    try:
                        script_result, native_result, archive_result = run_contexts(
                            root, clone, env, log_dir, clone_target, record["execution"])
                    finally:
                        write(path, record)  # retain admission/fallback evidence on failure
                    record["script_tests"] = script_result
                    record["contexts"] = {"worktree": native_result, "clone": archive_result}
        finally:
            write(path, record)  # include cleanup/cache decisions even on interruption
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
              f"code_input={record['inputs']['code_input']} script_tests=PASS contexts=worktree,clone "
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
