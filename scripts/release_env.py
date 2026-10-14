#!/usr/bin/env python3
"""One resolver for the release environment: Zig, the scratch base and the profile.

assembly-proof.py, check-lane-compile.py, release-gate.sh and release-train.sh
(with its preflight) all ask this module, so they agree on the same values and
nobody exports anything by hand (cas-9f61).

Zig, in order: $ZIG; the checkout's .context/zig/zig; the main checkout's
.context/zig/zig (symlinked into the checkout with --link-zig); the profile's
ZIG; `zig` on PATH. Otherwise the fix is ./scripts/bootstrap-zig.sh.

Scratch base, in order: an exported CAS_RELEASE_GATE_HOME_DIR; the profile's
entry for the checkout's filesystem (CAS_RELEASE_SCRATCH_BASE_<MOUNT>); a
legacy CAS_RELEASE_GATE_HOME_DIR line in the profile; <staging.scratch_root>/
cas-release-gate; <mount>/cas/scratch/cas-release-gate;
<mount>/cas-release-gate/base; the platform default; cas-release-gate beside the
checkout. A candidate counts only when its parent is an existing writable
directory on the checkout's filesystem with no .cas ancestor. Durable mode
(assembly-proof and the release train) also refuses system temporary roots,
because assembly-proof's plain clone must not look like a throwaway project.

The profile is the release env file (CAS_RELEASE_ENV_FILE, default
~/.cas/release.env). `setup` rewrites only its managed block; secrets and other
lines stay untouched, and values of unmanaged names are never printed.

Commands:
  resolve --worktree W [--durable] [--create] [--link-zig]   (shell assignments)
  doctor  --worktree W [--json]
  setup   --worktree W
"""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
from dataclasses import asdict, dataclass, field
import json
import os
from pathlib import Path
import platform
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time

GATE_VAR = "CAS_RELEASE_GATE_HOME_DIR"
PROFILE_SCRATCH_PREFIX = "CAS_RELEASE_SCRATCH_BASE_"
MANAGED_BEGIN = "# >>> cas release setup: managed block, regenerate with `cas release setup` >>>"
MANAGED_END = "# <<< cas release setup <<<"
HISTORY_NAME = ".cas-release-gate-last-archive-size-bytes"
TEMP_ROOTS = ("/tmp", "/var/tmp", "/private/tmp", "/private/var/tmp")
# Keep in sync with cut_preflight_merge_queue_query in release-train.d/preflight.sh
# (scripts/test-release-env.py asserts they match).
MERGE_QUEUE_QUERY = ('query { repository(owner: "Richards-LLC", name: "cassy") { mergeQueue(branch: "main") '
                     '{ entries(first: 100) { nodes { pullRequest { number title headRefName } } } } } }')
ENV_LINE = re.compile(r"^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)=(.*)$")


# --- host seams -----------------------------------------------------------


class Host:
    """Filesystem facts. Tests substitute device and mount answers."""

    def device(self, path: Path) -> int | None:
        # Same seam as release-portable.sh: a fake stat puts fixture paths on
        # separate devices without privileged mounts.
        fake = os.environ.get("CAS_RELEASE_PORTABLE_STAT")
        if fake:
            result = subprocess.run([fake, "-c", "%d", str(path)], capture_output=True, text=True)
            value = result.stdout.strip()
            return int(value) if result.returncode == 0 and value.isdigit() else None
        try:
            return os.stat(path).st_dev
        except OSError:
            return None

    def mount(self, path: Path) -> Path:
        path = Path(os.path.realpath(path))
        device = self.device(path)
        while path.parent != path and self.device(path.parent) == device:
            path = path.parent
        return path

    def writable_dir(self, path: Path) -> bool:
        return path.is_dir() and os.access(path, os.W_OK | os.X_OK)

    def free_bytes(self, path: Path) -> int | None:
        try:
            return shutil.disk_usage(path).free
        except OSError:
            return None


HOST = Host()


# --- profile ----------------------------------------------------------------


def env_file(env=os.environ) -> Path:
    configured = env.get("CAS_RELEASE_ENV_FILE")
    if configured:
        return Path(configured)
    return Path(env.get("HOME") or os.path.expanduser("~")) / ".cas/release.env"


def parse_value(raw: str) -> str:
    raw = raw.strip()
    try:
        parts = shlex.split(raw, comments=True)
    except ValueError:
        return raw.strip("\"'")
    return " ".join(parts)


def read_profile(path: Path) -> dict[str, str]:
    """Every NAME=value in the env file. Callers never print unmanaged values."""
    values: dict[str, str] = {}
    try:
        text = path.read_text()
    except OSError:
        return values
    for line in text.splitlines():
        match = ENV_LINE.match(line.strip())
        if match and match.group(1) not in values:
            values[match.group(1)] = parse_value(match.group(2))
    return values


def read_managed(path: Path) -> dict[str, str]:
    """Only the values inside the managed block."""
    values: dict[str, str] = {}
    try:
        lines = path.read_text().splitlines()
    except OSError:
        return values
    inside = False
    for line in lines:
        if line in (MANAGED_BEGIN, MANAGED_END):
            inside = line == MANAGED_BEGIN
            continue
        match = ENV_LINE.match(line.strip()) if inside else None
        if match:
            values[match.group(1)] = parse_value(match.group(2))
    return values


def profile_key(mount: Path) -> str:
    name = re.sub(r"[^A-Z0-9]+", "_", str(mount).upper()).strip("_")
    return PROFILE_SCRATCH_PREFIX + (name or "ROOT")


def write_profile(path: Path, managed: dict[str, str]) -> None:
    """Replace only the managed block; keep every other line, and keep the file 0600."""
    try:
        text = path.read_text()
    except FileNotFoundError:
        text = ""
    kept, inside = [], False
    for line in text.splitlines():
        if line == MANAGED_BEGIN:
            inside = True
            continue
        if line == MANAGED_END:
            inside = False
            continue
        if not inside:
            kept.append(line)
    while kept and not kept[-1].strip():
        kept.pop()
    block = [MANAGED_BEGIN,
             "# Discovered release environment; hand edits here are replaced on the next setup."]
    block += [f"export {name}={shlex.quote(value)}" for name, value in sorted(managed.items())]
    block.append(MANAGED_END)
    body = "\n".join(kept + ([""] if kept else []) + block) + "\n"
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".release-env.", dir=path.parent)
    try:
        with os.fdopen(fd, "w") as stream:
            stream.write(body)
        os.chmod(temporary, 0o600)  # the profile holds release secrets
        os.replace(temporary, path)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise


# --- git ----------------------------------------------------------------------


def git(checkout: Path, *args: str) -> str:
    result = subprocess.run(["git", "-C", str(checkout), *args], capture_output=True, text=True)
    return result.stdout.strip() if result.returncode == 0 else ""


def main_checkout(checkout: Path) -> Path | None:
    common = git(checkout, "rev-parse", "--path-format=absolute", "--git-common-dir")
    if not common:
        return None
    common_path = Path(common).resolve()
    return common_path.parent if common_path.name == ".git" else None


def worktrees(checkout: Path, env=os.environ) -> list[Path]:
    """Sibling checkouts that could host a release; temporary ones never do."""
    paths = []
    for line in git(checkout, "worktree", "list", "--porcelain").splitlines():
        if line.startswith("worktree "):
            path = Path(line[len("worktree "):])
            if path.is_dir() and under_temp_root(path, env) is None:
                paths.append(path)
    return paths


# --- Zig ----------------------------------------------------------------------


@dataclass
class ZigResolution:
    path: str
    source: str
    tried: list[str] = field(default_factory=list)
    fix: str = ""


def executable(path: Path) -> bool:
    return path.is_file() and os.access(path, os.X_OK)


def resolve_zig(checkout: Path, env=os.environ, *, profile: dict[str, str] | None = None,
                link: bool = False) -> ZigResolution:
    checkout = Path(checkout).absolute()
    profile = read_profile(env_file(env)) if profile is None else profile
    tried: list[str] = []

    def found(path: Path, source: str) -> ZigResolution:
        return ZigResolution(str(path.resolve()), source, tried)

    configured = env.get("ZIG", "")
    if configured:
        path = Path(configured)
        path = path if path.is_absolute() else checkout / path
        if executable(path):
            return found(path, "ZIG (environment)")
        tried.append(f"ZIG={configured} is not an executable file")
    local = checkout / ".context/zig/zig"
    if executable(local):
        return found(local, "checkout .context/zig")
    tried.append(f"{local} missing")
    main = main_checkout(checkout)
    if main is not None and main != checkout.resolve():
        shared = main / ".context/zig/zig"
        if executable(shared):
            if link:
                link_zig(checkout, main)
            return found(shared, "main checkout .context/zig")
        tried.append(f"{shared} missing")
    pinned = profile.get("ZIG", "")
    if pinned:
        if executable(Path(pinned)):
            return found(Path(pinned), f"profile ZIG ({env_file(env)})")
        tried.append(f"profile ZIG={pinned} is not an executable file")
    on_path = shutil.which("zig", path=env.get("PATH", ""))
    if on_path:
        return found(Path(on_path), "PATH")
    tried.append("no zig on PATH")
    root = main or checkout
    fix = f"(cd {shlex.quote(str(root))} && ./scripts/bootstrap-zig.sh) && cas release setup"
    return ZigResolution("", "", tried, fix)


def link_zig(checkout: Path, main: Path) -> None:
    """Symlink the main checkout's toolchain directory into a worktree."""
    target = checkout / ".context/zig"
    if target.is_symlink():
        target.unlink()
    elif target.exists():
        return
    target.parent.mkdir(parents=True, exist_ok=True)
    target.symlink_to(main / ".context/zig")


# --- scratch -------------------------------------------------------------------


@dataclass
class ScratchResolution:
    base: str
    source: str
    durable: bool
    checkout_mount: str
    profile_key: str
    rejected: list[str] = field(default_factory=list)
    problem: str = ""
    fix: str = ""


def temp_roots(env) -> list[Path]:
    # CAS_RELEASE_ENV_TEMP_ROOTS replaces the list for self-tests whose
    # fixtures necessarily live under /tmp.
    names = env["CAS_RELEASE_ENV_TEMP_ROOTS"].split(":") if "CAS_RELEASE_ENV_TEMP_ROOTS" in env else TEMP_ROOTS
    roots = [Path(name) for name in names if name]
    if env.get("TMPDIR") and "CAS_RELEASE_ENV_TEMP_ROOTS" not in env:
        roots.append(Path(env["TMPDIR"]))
    spellings = []
    for root in roots:
        spellings += [root.absolute(), Path(os.path.realpath(root))]
    return spellings


def under_temp_root(path: Path, env) -> Path | None:
    for spelling in (path.absolute(), Path(os.path.realpath(path))):
        for root in temp_roots(env):
            if spelling == root or root in spelling.parents:
                return root
    return None


def cas_ancestor(path: Path) -> Path | None:
    for parent in (path, *path.parents):
        if parent.name == ".cas":
            return parent
        if (parent / ".cas").exists():
            return parent / ".cas"
    return None


def platform_default() -> Path:
    if platform.system() == "Darwin":
        return Path("/Users/Shared/cas-release-gate")
    return Path("/var/tmp/cas-release-gate")


def staging_scratch_root(checkout: Path, env) -> str:
    if "CAS_RELEASE_ENV_SCRATCH_ROOT" in env:  # test seam and explicit override
        return env["CAS_RELEASE_ENV_SCRATCH_ROOT"]
    cas = shutil.which("cas", path=env.get("PATH", ""))
    if not cas:
        return ""
    try:
        result = subprocess.run([cas, "config", "get", "staging.scratch_root"], cwd=checkout,
                                capture_output=True, text=True, timeout=5,
                                env={key: value for key, value in env.items()})
    except (OSError, subprocess.SubprocessError):
        return ""
    value = result.stdout.strip().splitlines()[0] if result.returncode == 0 and result.stdout.strip() else ""
    return value if value.startswith("/") else ""


def scratch_problem(base: Path, checkout_device, env, durable: bool, host: Host) -> str:
    if not base.is_absolute():
        return "not an absolute path"
    parent = base.parent
    if not host.writable_dir(parent):
        return f"parent {parent} is not an existing writable directory"
    if base.exists() and not host.writable_dir(base):
        return "exists but is not a writable directory"
    device = host.device(parent)
    if checkout_device is None or device != checkout_device:
        return f"filesystem boundary (checkout device {checkout_device}, scratch parent device {device})"
    ancestor = cas_ancestor(parent)
    if ancestor is not None:
        return f".cas ancestor at {ancestor}"
    root = under_temp_root(base, env) if durable else None
    if root is not None:
        return f"under system temporary root {root}, which assembly-proof's plain clone refuses"
    return ""


def provision_fix(mount: Path) -> str:
    base = mount / "cas-release-gate"
    if os.access(mount, os.W_OK):
        return f"mkdir -p {shlex.quote(str(base / 'base'))} && cas release setup"
    return (f"sudo install -d -o \"$USER\" -m 0755 {shlex.quote(str(base))} && "
            f"mkdir -p {shlex.quote(str(base / 'base'))} && cas release setup")


def resolve_scratch(checkout: Path, env=os.environ, *, durable: bool = False, create: bool = False,
                    profile: dict[str, str] | None = None, host: Host = HOST) -> ScratchResolution:
    checkout = Path(checkout).absolute()
    profile = read_profile(env_file(env)) if profile is None else profile
    mount = host.mount(checkout)
    key = profile_key(mount)
    checkout_device = host.device(checkout)
    result = ScratchResolution("", "", durable, str(mount), key)

    explicit = env.get(GATE_VAR, "")
    if explicit:
        # Operator intent wins; callers still check it and name the problem.
        result.base, result.source = explicit, f"{GATE_VAR} (environment)"
        result.problem = scratch_problem(Path(explicit), checkout_device, env, durable, host)
        if result.problem:
            result.fix = f"unset {GATE_VAR} so the resolver derives a base, or point it at a valid one"
        return finish(result, create)

    candidates: list[tuple[str, str]] = []
    if profile.get(key):
        candidates.append((profile[key], f"profile {key}"))
    # Another pin may name this filesystem under a different mount spelling
    # (a bind mount); the device check below keeps only same-filesystem pins.
    candidates += [(value, f"profile {name}") for name, value in sorted(profile.items())
                   if name.startswith(PROFILE_SCRATCH_PREFIX) and name != key and value]
    if profile.get(GATE_VAR):
        candidates.append((profile[GATE_VAR], f"profile {GATE_VAR} (legacy, one filesystem only)"))
    scratch_root = staging_scratch_root(checkout, env)
    if scratch_root:
        candidates.append((str(Path(scratch_root) / "cas-release-gate"), "staging.scratch_root"))
    candidates.append((str(mount / "cas/scratch/cas-release-gate"), "checkout filesystem cas/scratch"))
    candidates.append((str(mount / "cas-release-gate/base"), "checkout filesystem cas-release-gate"))
    candidates.append((str(platform_default()), "platform default"))
    candidates.append((str(checkout.parent / "cas-release-gate"), "beside the checkout"))

    for raw, source in candidates:
        problem = scratch_problem(Path(raw), checkout_device, env, durable, host)
        if not problem:
            result.base, result.source = raw, source
            return finish(result, create)
        result.rejected.append(f"{source} {raw}: {problem}")
    result.problem = f"no usable scratch base on the checkout filesystem {mount}"
    result.fix = provision_fix(mount)
    return result


def finish(result: ScratchResolution, create: bool) -> ScratchResolution:
    if create and result.base and not result.problem:
        try:
            Path(result.base).mkdir(exist_ok=True)
        except OSError as error:
            result.problem = f"could not create {result.base}: {error.strerror}"
    return result


def required_bytes(base: Path) -> int:
    try:
        size = int((base.parent / HISTORY_NAME).read_text().strip())
    except (OSError, ValueError):
        return 0
    return 2 * size


# --- commands ----------------------------------------------------------------


def shell_assignments(values: dict[str, str]) -> str:
    return "".join(f"{name}={shlex.quote(value)}\n" for name, value in values.items())


def command_resolve(args, env=os.environ) -> int:
    checkout = Path(args.worktree).absolute()
    profile = read_profile(env_file(env))
    zig = resolve_zig(checkout, env, profile=profile, link=args.link_zig)
    scratch = resolve_scratch(checkout, env, durable=args.durable, create=args.create, profile=profile)
    print(shell_assignments({
        "RELEASE_ENV_ZIG": zig.path,
        "RELEASE_ENV_ZIG_SOURCE": zig.source,
        "RELEASE_ENV_ZIG_TRIED": "; ".join(zig.tried),
        "RELEASE_ENV_ZIG_FIX": zig.fix,
        "RELEASE_ENV_SCRATCH": scratch.base,
        "RELEASE_ENV_SCRATCH_SOURCE": scratch.source,
        "RELEASE_ENV_SCRATCH_PROBLEM": scratch.problem,
        "RELEASE_ENV_SCRATCH_FIX": scratch.fix,
        "RELEASE_ENV_SCRATCH_REJECTED": "; ".join(scratch.rejected),
    }), end="")
    return 0


@dataclass
class Check:
    name: str
    status: str  # ok | warn | gap
    detail: str
    fix: str = ""


def run_quiet(command: list[str], timeout: float, env) -> tuple[int, str]:
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=timeout, env=dict(env))
    except FileNotFoundError:
        return 127, f"{command[0]} not found"
    except subprocess.TimeoutExpired:
        return 124, f"timed out after {timeout:.0f}s"
    return result.returncode, (result.stdout + result.stderr).strip()


def check_env_file(env) -> list[Check]:
    path = env_file(env)
    if not path.exists():
        return [Check("release-env", "gap", f"{path} does not exist",
                      f"install -m 0600 /dev/null {shlex.quote(str(path))} && cas release setup")]
    if not os.access(path, os.R_OK):
        return [Check("release-env", "gap", f"{path} is not readable", f"chmod 0600 {shlex.quote(str(path))}")]
    names = sorted(read_profile(path))  # names only: the file holds secrets
    return [Check("release-env", "ok", f"{path} names: {' '.join(names) or '(none)'}")]


def check_zig(checkout: Path, env, profile) -> list[Check]:
    zig = resolve_zig(checkout, env, profile=profile)
    if not zig.path:
        return [Check("zig", "gap", "not found: " + "; ".join(zig.tried), zig.fix)]
    code, output = run_quiet([zig.path, "version"], 10, env)
    if code:
        return [Check("zig", "gap", f"{zig.path} ({zig.source}) does not run: {output[-200:]}", zig.fix)]
    return [Check("zig", "ok", f"{zig.path} {output.splitlines()[0] if output else ''} ({zig.source})")]


def by_filesystem(checkouts: list[Path], host: Host = HOST) -> list[list[Path]]:
    """Group checkouts by device, keeping first-seen order."""
    groups: dict[int | None, list[Path]] = {}
    for checkout in checkouts:
        groups.setdefault(host.device(checkout), []).append(checkout)
    return list(groups.values())


def resolve_filesystem(group: list[Path], env, profile, *, create: bool = False,
                       host: Host = HOST) -> tuple[Path, ScratchResolution]:
    """The first checkout on this filesystem that resolves a durable base wins.

    A bind mount (for example an artifacts directory mounted from a scratch
    drive) has no provisioned candidates of its own, but its sibling checkouts
    on the real mount do.
    """
    first = None
    for checkout in group:
        scratch = resolve_scratch(checkout, env, durable=True, create=create, profile=profile, host=host)
        if scratch.base and not scratch.problem:
            return checkout, scratch
        first = first or (checkout, scratch)
    return first


def check_scratch(checkouts: list[Path], env, profile, host: Host = HOST) -> list[Check]:
    checks = []
    for group in by_filesystem(checkouts, host):
        checkout, scratch = resolve_filesystem(group, env, profile, host=host)
        name = f"scratch {scratch.checkout_mount}"
        if not scratch.base or scratch.problem:
            detail = scratch.problem + ("; tried " + "; ".join(scratch.rejected) if scratch.rejected else "")
            checks.append(Check(name, "gap", detail, scratch.fix))
            continue
        base = Path(scratch.base)
        free = host.free_bytes(base.parent)
        need = required_bytes(base)
        if free is not None and need and free < need:
            checks.append(Check(name, "gap", f"{base} has {free} bytes free; need {need} (2x last archive)",
                                f"free space on {scratch.checkout_mount}"))
            continue
        status, fix = "ok", ""
        if not scratch.source.startswith("profile " + PROFILE_SCRATCH_PREFIX) and GATE_VAR not in env:
            status, fix = "warn", "cas release setup"
        detail = f"{base} ({scratch.source}); {free if free is not None else 'unknown'} bytes free"
        if status == "warn":
            detail += "; not yet pinned in the profile"
        checks.append(Check(name, status, detail, fix))
    return checks


TOOLS = ("cargo", "cargo-nextest", "cargo-zigbuild", "git", "jq", "python3", "gh")


def check_toolchain(env) -> list[Check]:
    path = env.get("PATH", "")
    missing = [tool for tool in TOOLS if not shutil.which(tool, path=path)]
    if not (shutil.which("setsid", path=path) or shutil.which("perl", path=path)):
        missing.append("setsid (or perl)")
    if not (shutil.which("sha256sum", path=path) or shutil.which("shasum", path=path)):
        missing.append("sha256sum or shasum")
    objdump = shutil.which("gobjdump", path=path) or shutil.which("objdump", path=path)
    if not objdump:
        missing.append("GNU objdump")
    if missing:
        hint = "brew install binutils jq gh" if platform.system() == "Darwin" else "install the missing tools"
        if any(tool.startswith("cargo-") for tool in missing):
            hint = "cargo install cargo-nextest cargo-zigbuild --locked; " + hint
        return [Check("toolchain", "gap", "missing: " + ", ".join(missing), hint)]
    rustup = shutil.which("rustup", path=path)
    if rustup:
        code, output = run_quiet([rustup, "target", "list", "--installed"], 10, env)
        if code == 0 and "x86_64-unknown-linux-gnu" not in output.split():
            return [Check("toolchain", "gap", "Rust target x86_64-unknown-linux-gnu is not installed",
                          "rustup target add x86_64-unknown-linux-gnu")]
    return [Check("toolchain", "ok", "cargo, nextest, zigbuild, git, jq, gh, objdump present")]


def check_gh(env) -> list[Check]:
    gh = env.get("CAS_RELEASE_TRAIN_GH") or shutil.which("gh", path=env.get("PATH", ""))
    if not gh:
        return [Check("gh-auth", "gap", "GitHub CLI is not installed", "install gh, then gh auth login")]
    code, output = run_quiet([gh, "auth", "status"], 15, env)
    if code:
        return [Check("gh-auth", "gap", f"gh auth status failed: {output.splitlines()[-1] if output else code}",
                      "gh auth login")]
    checks = [Check("gh-auth", "ok", "gh is authenticated")]
    code, output = run_quiet([gh, "api", "graphql", "-f", "query=" + MERGE_QUEUE_QUERY], 20, env)
    try:
        nodes = json.loads(output)["data"]["repository"]["mergeQueue"]["entries"]["nodes"] if code == 0 else None
    except (ValueError, KeyError, TypeError):
        nodes = None
    if not isinstance(nodes, list):
        checks.append(Check("merge-queue", "gap", f"merge-queue query failed: {output[-200:]}",
                            "gh auth refresh -s read:org,repo"))
    else:
        checks.append(Check("merge-queue", "ok", f"merge queue readable ({len(nodes)} entries)"))
    return checks


def doctor(checkout: Path, env=os.environ, host: Host = HOST) -> list[Check]:
    profile = read_profile(env_file(env))
    checkouts = [checkout] + [path for path in worktrees(checkout, env) if path != checkout]
    with ThreadPoolExecutor(max_workers=4) as pool:
        futures = [pool.submit(check_gh, env), pool.submit(check_toolchain, env),
                   pool.submit(check_zig, checkout, env, profile)]
        checks = check_env_file(env) + check_scratch(checkouts, env, profile, host)
        for future in futures:
            checks += future.result()
    return checks


def command_doctor(args, env=os.environ) -> int:
    started = time.monotonic()
    checkout = Path(args.worktree).absolute()
    checks = doctor(checkout, env)
    gaps = [check for check in checks if check.status == "gap"]
    elapsed = time.monotonic() - started
    if args.json:
        print(json.dumps({"checks": [asdict(check) for check in checks], "gaps": len(gaps),
                          "elapsed_secs": round(elapsed, 2)}, indent=2))
        return 1 if gaps else 0
    label = {"ok": "ok  ", "warn": "warn", "gap": "GAP "}
    for check in checks:
        print(f"{label[check.status]} {check.name}: {check.detail}")
        if check.fix:
            print(f"     fix: {check.fix}")
    verdict = f"{len(gaps)} gap(s)" if gaps else "no gaps"
    print(f"release doctor: {verdict} in {elapsed:.1f}s ({checkout})")
    return 1 if gaps else 0


def command_setup(args, env=os.environ) -> int:
    checkout = Path(args.worktree).absolute()
    path = env_file(env)
    profile = read_profile(path)
    # Keep pins for filesystems this run cannot see (another host's worktrees).
    managed = read_managed(path)
    failures = []
    zig = resolve_zig(checkout, env, profile=profile, link=True)
    if zig.path:
        managed["ZIG"] = zig.path
        print(f"zig: {zig.path} ({zig.source})")
    else:
        failures.append(f"zig: {'; '.join(zig.tried)}; fix: {zig.fix}")
    # Pin every filesystem a release checkout lives on, not only this one.
    scratch_env = {key: value for key, value in env.items() if key != GATE_VAR}
    for group in by_filesystem([checkout] + worktrees(checkout, env)):
        _, scratch = resolve_filesystem(group, scratch_env, profile, create=True)
        mount = scratch.checkout_mount
        if scratch.base and not scratch.problem:
            managed[scratch.profile_key] = scratch.base
            print(f"scratch {mount}: {scratch.base} ({scratch.source})")
        else:
            failures.append(f"scratch {mount}: {scratch.problem}; fix: {scratch.fix}")
    write_profile(path, managed)
    print(f"wrote {len(managed)} managed value(s) to {path}")
    for failure in failures:
        print(f"unresolved {failure}", file=sys.stderr)
    return 1 if failures else 0


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)
    resolve = commands.add_parser("resolve", help="print the resolved Zig and scratch base")
    resolve.add_argument("--worktree", default=".")
    resolve.add_argument("--durable", action="store_true", help="refuse system temporary roots")
    resolve.add_argument("--create", action="store_true", help="create the scratch base directory")
    resolve.add_argument("--link-zig", action="store_true", help="symlink the main checkout's Zig in")
    check = commands.add_parser("doctor", help="check every release prerequisite, one fix per gap")
    check.add_argument("--worktree", default=".")
    check.add_argument("--json", action="store_true")
    setup = commands.add_parser("setup", help="persist discovered values in the release env file")
    setup.add_argument("--worktree", default=".")
    args = parser.parse_args(argv)
    handler = {"resolve": command_resolve, "doctor": command_doctor, "setup": command_setup}[args.command]
    return handler(args)


if __name__ == "__main__":
    sys.exit(main())
