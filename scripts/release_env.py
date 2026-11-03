#!/usr/bin/env python3
"""One resolver for the release environment: Zig and the scratch base (cas-9f61).

assembly-proof.py and release-gate.sh
(with its preflight) all ask this module, so they agree without anything being
exported by hand. Resolution is deterministic and takes milliseconds, so it
runs every time and nothing is persisted beyond creating
<factory.scratch_root>/<project-key>/release-gate when that root is set.

Zig: $ZIG; `zig` on PATH; the checkout's .context/zig/zig; the main checkout's
(optionally symlinked in with --link-zig). This order matches ghostty_vt_sys's
build script. Otherwise the fix is ./scripts/bootstrap-zig.sh.

Scratch base: an exported CAS_RELEASE_GATE_HOME_DIR;
<factory.scratch_root>/<project-key>/release-gate/base; <staging.scratch_root>/
cas-release-gate; <mount>/cas/scratch/cas-release-gate;
<mount>/cas-release-gate/base; the platform default; cas-release-gate beside the
checkout. A candidate counts only when its parent is an existing writable
directory on the checkout's filesystem with no .cas ancestor. The one
exception is a base inside the configured factory.scratch_root (cas-baa8): it
may sit on another device, but it must share the scratch root's filesystem,
because assembly clones there hard-link the baseline from
<factory.scratch_root>/<project-key>/build-cache. Durable mode
(assembly-proof and the release train) also refuses system temporary roots,
because assembly-proof's plain clone must not look like a throwaway project.

  release_env.py resolve --worktree W [--durable] [--create] [--link-zig]
  release_env.py doctor  --worktree W
"""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
import hashlib
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import subprocess
import sys
import time

GATE_VAR = "CAS_RELEASE_GATE_HOME_DIR"
HISTORY_NAME = ".cas-release-gate-last-archive-size-bytes"
TEMP_ROOTS = ("/tmp", "/var/tmp", "/private/tmp", "/private/var/tmp")
# (scripts/test-release-env.py asserts they match).
MERGE_QUEUE_QUERY = ('query { repository(owner: "Richards-LLC", name: "cassy") { mergeQueue(branch: "main") '
                     '{ entries(first: 100) { nodes { pullRequest { number title headRefName } } } } } }')


class Host:
    """Filesystem facts. Tests substitute device answers."""

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


HOST = Host()


def git(checkout: Path, *args: str) -> str:
    result = subprocess.run(["git", "-C", str(checkout), *args], capture_output=True, text=True)
    return result.stdout.strip() if result.returncode == 0 else ""


def main_checkout(checkout: Path) -> Path | None:
    common = git(checkout, "rev-parse", "--path-format=absolute", "--git-common-dir")
    common_path = Path(common).resolve() if common else None
    return common_path.parent if common_path and common_path.name == ".git" else None


# --- Zig ----------------------------------------------------------------------


@dataclass
class ZigResolution:
    path: str
    source: str
    tried: list[str] = field(default_factory=list)
    fix: str = ""


def executable(path: Path) -> bool:
    return path.is_file() and os.access(path, os.X_OK)


def resolve_zig(checkout: Path, env=os.environ, *, link: bool = False) -> ZigResolution:
    checkout = Path(checkout).absolute()
    tried: list[str] = []
    configured = env.get("ZIG", "")
    if configured:
        path = Path(configured) if Path(configured).is_absolute() else checkout / configured
        if executable(path):
            return ZigResolution(str(path.resolve()), "ZIG (environment)", tried)
        tried.append(f"ZIG={configured} is not an executable file")
    # Same version authority as ghostty_vt_sys/build_support.rs, read once
    # for every PATH candidate (cas-43d2).
    required, pin_error = None, None
    try:
        manifest = Path(__file__).resolve().parent.parent / "crates/ghostty_vt_sys/zig/build.zig.zon"
        required = manifest.read_text().split('.minimum_zig_version = "', 1)[1].split('"', 1)[0]
    except (OSError, IndexError) as error:
        pin_error = error
    for directory in env.get("PATH", "").split(os.pathsep):
        on_path = checkout / directory / "zig"
        if not executable(on_path):
            continue
        if pin_error is not None:
            tried.append(f"{on_path} cannot validate pinned version: {pin_error}")
            continue
        try:
            version = subprocess.run([str(on_path), "version"], cwd=checkout, env=env,
                                     capture_output=True, text=True, timeout=5)
            if version.returncode == 0 and version.stdout.strip() == required:
                return ZigResolution(str(on_path.resolve()), "PATH", tried)
            tried.append(f"{on_path} version {version.stdout.strip()!r} (exit {version.returncode}), expected {required}")
        except (OSError, IndexError, subprocess.TimeoutExpired) as error:
            tried.append(f"{on_path} cannot validate pinned version: {error}")
    tried.append("no compatible zig on PATH")
    local = checkout / ".context/zig/zig"
    if executable(local):
        return ZigResolution(str(local.resolve()), "checkout .context/zig", tried)
    tried.append(f"{local} missing")
    main = main_checkout(checkout)
    if main is not None and main != checkout.resolve():
        shared = main / ".context/zig/zig"
        if executable(shared):
            if link:
                link_zig(checkout, main)
            return ZigResolution(str(shared.resolve()), "main checkout .context/zig", tried)
        tried.append(f"{shared} missing")
    root = shlex.quote(str(main or checkout))
    return ZigResolution("", "", tried, f"(cd {root} && ./scripts/bootstrap-zig.sh)")


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
    mount: str
    rejected: list[str] = field(default_factory=list)
    problem: str = ""
    fix: str = ""


def under_temp_root(path: Path, env) -> Path | None:
    # CAS_RELEASE_ENV_TEMP_ROOTS replaces the list for self-tests whose
    # fixtures necessarily live under /tmp.
    if "CAS_RELEASE_ENV_TEMP_ROOTS" in env:
        names = [name for name in env["CAS_RELEASE_ENV_TEMP_ROOTS"].split(":") if name]
    else:
        names = list(TEMP_ROOTS) + ([env["TMPDIR"]] if env.get("TMPDIR") else [])
    roots = {spelling for name in names for spelling in (Path(name).absolute(), Path(os.path.realpath(name)))}
    for spelling in (path.absolute(), Path(os.path.realpath(path))):
        for root in roots:
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


def staging_scratch_root(checkout: Path, env) -> str:
    """`cas config get staging.scratch_root`; CAS_RELEASE_ENV_SCRATCH_ROOT overrides it."""
    if "CAS_RELEASE_ENV_SCRATCH_ROOT" in env:
        return env["CAS_RELEASE_ENV_SCRATCH_ROOT"]
    cas = shutil.which("cas", path=env.get("PATH", ""))
    if not cas:
        return ""
    try:
        result = subprocess.run([cas, "config", "get", "staging.scratch_root"], cwd=checkout,
                                capture_output=True, text=True, timeout=5, env=dict(env))
    except (OSError, subprocess.SubprocessError):
        return ""
    lines = result.stdout.strip().splitlines() if result.returncode == 0 else []
    return lines[0] if lines and lines[0].startswith("/") else ""


# --- factory.scratch_root (cas-baa8) ---------------------------------------------

FACTORY_SCRATCH_VAR = "CAS_RELEASE_ENV_FACTORY_SCRATCH_ROOT"
RELEASE_GATE_DIR = "release-gate"


def project_key(cas_dir: Path) -> str:
    """`<label>-<sha256>` of a Cassy store, as cas_types::factory_scratch::project_key
    and refresh-worker-build-cache.sh derive it (pinned to one golden vector)."""
    cas_dir = Path(cas_dir)
    store = Path(os.path.realpath(cas_dir)) if cas_dir.exists() else cas_dir
    name = store.parent.name if store.parent != store else ""
    label = "".join(ch if ch.isascii() and (ch.isalnum() or ch in "-_") else "-"
                    for ch in (name or "project")[:48])
    return f"{label}-{hashlib.sha256(os.fsencode(store)).hexdigest()}"


def cas_dir_for(checkout: Path) -> Path:
    """The store of the main checkout every worktree of this repository shares."""
    return (main_checkout(checkout) or Path(checkout).absolute()) / ".cas"


def factory_scratch_root(checkout: Path, env) -> Path | None:
    """`cas config get factory.scratch_root`; CAS_RELEASE_ENV_FACTORY_SCRATCH_ROOT
    overrides it. Only an absolute value relocates anything."""
    if FACTORY_SCRATCH_VAR in env:
        value = env[FACTORY_SCRATCH_VAR]
    else:
        cas = shutil.which("cas", path=env.get("PATH", ""))
        value = ""
        if cas:
            try:
                result = subprocess.run([cas, "config", "get", "factory.scratch_root"], cwd=checkout,
                                        capture_output=True, text=True, timeout=5, env=dict(env))
                lines = result.stdout.strip().splitlines() if result.returncode == 0 else []
                value = lines[0] if lines else ""
            except (OSError, subprocess.SubprocessError):
                value = ""
    value = value.strip()
    return Path(value) if value.startswith("/") else None


def project_scratch_dir(checkout: Path, env) -> Path | None:
    root = factory_scratch_root(checkout, env)
    return root / project_key(cas_dir_for(checkout)) if root else None


def build_cache_root(checkout: Path, env=os.environ) -> Path:
    """Where worker baselines are published: <project scratch>/build-cache, else <store>/build-cache."""
    project = project_scratch_dir(checkout, env)
    return project / "build-cache" if project else cas_dir_for(checkout) / "build-cache"


def nearest_existing(path: Path) -> Path:
    while not os.path.lexists(path) and path.parent != path:
        path = path.parent
    return path


def scratch_problem(base: Path, checkout_device, env, durable: bool, host: Host,
                    project: Path | None = None) -> str:
    if not base.is_absolute():
        return "not an absolute path"
    parent = base.parent
    if not (parent.is_dir() and os.access(parent, os.W_OK | os.X_OK)):
        return f"parent {parent} is not an existing writable directory"
    if base.exists() and not (base.is_dir() and os.access(base, os.W_OK | os.X_OK)):
        return "exists but is not a writable directory"
    device = host.device(parent)
    inside = project is not None and (base.absolute() == project or project in base.absolute().parents)
    if inside:
        # cas-baa8: the configured scratch root may be on another device, but the
        # base must share its filesystem: clones hard-link the build-cache there.
        root_device = host.device(nearest_existing(project.parent))
        if device != root_device:
            return (f"scratch root boundary (factory.scratch_root device {root_device}, "
                    f"scratch parent device {device})")
    elif checkout_device is None or device != checkout_device:
        return f"filesystem boundary (checkout device {checkout_device}, scratch parent device {device})"
    ancestor = cas_ancestor(parent)
    if ancestor is not None:
        return f".cas ancestor at {ancestor}"
    root = under_temp_root(base, env) if durable else None
    if root is not None:
        return f"under system temporary root {root}, which assembly-proof's plain clone refuses"
    return ""


def resolve_scratch(checkout: Path, env=os.environ, *, durable: bool = False, create: bool = False,
                    host: Host = HOST) -> ScratchResolution:
    checkout = Path(checkout).absolute()
    mount = host.mount(checkout)
    checkout_device = host.device(checkout)
    result = ScratchResolution("", "", str(mount))
    project = project_scratch_dir(checkout, env)
    explicit = env.get(GATE_VAR, "")
    if explicit:
        # Operator intent wins; callers still check it and name the problem.
        result.base, result.source = explicit, f"{GATE_VAR} (environment)"
        if project is not None and Path(explicit).absolute().is_relative_to(project):
            result.source += ", inside factory.scratch_root"
        result.problem = scratch_problem(Path(explicit), checkout_device, env, durable, host, project)
        if result.problem:
            result.fix = f"unset {GATE_VAR} so the resolver derives a base on the checkout's filesystem"
    else:
        candidates = []
        if project is not None:
            # Created beside the project's targets and build-cache; archives and
            # assembly clones are siblings of the base, inside release-gate/.
            if project.parent.is_dir() and os.access(project.parent, os.W_OK | os.X_OK):
                (project / RELEASE_GATE_DIR).mkdir(parents=True, exist_ok=True)
            candidates.append((project / RELEASE_GATE_DIR / "base", "factory.scratch_root"))
        scratch_root = staging_scratch_root(checkout, env)
        if scratch_root:
            candidates.append((Path(scratch_root) / "cas-release-gate", "staging.scratch_root"))
        candidates += [(mount / "cas/scratch/cas-release-gate", "checkout filesystem cas/scratch"),
                       (mount / "cas-release-gate/base", "checkout filesystem cas-release-gate"),
                       (Path("/Users/Shared/cas-release-gate" if platform.system() == "Darwin"
                             else "/var/tmp/cas-release-gate"), "platform default"),
                       (checkout.parent / "cas-release-gate", "beside the checkout")]
        for base, source in candidates:
            problem = scratch_problem(base, checkout_device, env, durable, host, project)
            if not problem:
                result.base, result.source = str(base), source
                break
            result.rejected.append(f"{source} {base}: {problem}")
        else:
            provision = shlex.quote(str(mount / "cas-release-gate"))
            result.problem = f"no usable scratch base on the checkout filesystem {mount}"
            result.fix = (f"mkdir -p {provision}/base" if os.access(mount, os.W_OK)
                          else f"sudo install -d -o \"$USER\" -m 0755 {provision} && mkdir {provision}/base")
    if create and result.base and not result.problem:
        try:
            Path(result.base).mkdir(exist_ok=True)
        except OSError as error:
            result.problem = f"could not create {result.base}: {error.strerror}"
    return result


# --- commands ----------------------------------------------------------------


def command_resolve(args) -> int:
    zig = resolve_zig(args.worktree, link=args.link_zig)
    scratch = resolve_scratch(args.worktree, durable=args.durable, create=args.create)
    values = {
        "RELEASE_ENV_ZIG": zig.path,
        "RELEASE_ENV_ZIG_SOURCE": zig.source,
        "RELEASE_ENV_ZIG_TRIED": "; ".join(zig.tried),
        "RELEASE_ENV_ZIG_FIX": zig.fix,
        "RELEASE_ENV_SCRATCH": scratch.base,
        "RELEASE_ENV_SCRATCH_SOURCE": scratch.source,
        "RELEASE_ENV_SCRATCH_PROBLEM": scratch.problem,
        "RELEASE_ENV_SCRATCH_FIX": scratch.fix,
        "RELEASE_ENV_SCRATCH_REJECTED": "; ".join(scratch.rejected),
    }
    print("".join(f"{name}={shlex.quote(value)}\n" for name, value in values.items()), end="")
    return 0


@dataclass
class Check:
    name: str
    ok: bool
    detail: str
    fix: str = ""


def run_quiet(command: list[str], timeout: float) -> tuple[int, str]:
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=timeout)
    except FileNotFoundError:
        return 127, f"{command[0]} not found"
    except subprocess.TimeoutExpired:
        return 124, f"timed out after {timeout:.0f}s"
    return result.returncode, (result.stdout + result.stderr).strip()


def check_zig(checkout: Path) -> Check:
    zig = resolve_zig(checkout)
    if not zig.path:
        return Check("zig", False, "not found: " + "; ".join(zig.tried), zig.fix)
    code, output = run_quiet([zig.path, "version"], 10)
    if code:
        return Check("zig", False, f"{zig.path} does not run: {output[-200:]}", zig.fix)
    return Check("zig", True, f"{zig.path} {output} ({zig.source})")


def check_scratch(checkouts: list[Path], host: Host = HOST, env=os.environ) -> list[Check]:
    """One check per filesystem; any checkout on it that resolves a durable base wins."""
    groups: dict[int | None, list[Path]] = {}
    for checkout in checkouts:
        groups.setdefault(host.device(checkout), []).append(checkout)
    checks = []
    for group in groups.values():
        results = [resolve_scratch(checkout, env, durable=True, host=host) for checkout in group]
        scratch = next((result for result in results if result.base and not result.problem), results[0])
        name = f"scratch {scratch.mount}"
        if not scratch.base or scratch.problem:
            checks.append(Check(name, False, f"{scratch.problem}; tried {'; '.join(scratch.rejected)}",
                                scratch.fix))
            continue
        base = Path(scratch.base)
        free = shutil.disk_usage(base.parent).free
        try:
            need = 2 * int((base.parent / HISTORY_NAME).read_text().strip())
        except (OSError, ValueError):
            need = 0
        if need and free < need:
            checks.append(Check(name, False, f"{base} has {free} bytes free; need {need} (2x last archive)",
                                f"free space on {scratch.mount}"))
        else:
            checks.append(Check(name, True, f"{base} ({scratch.source}); {free} bytes free"))
    return checks


def check_toolchain() -> Check:
    missing = [tool for tool in ("cargo", "cargo-nextest", "cargo-zigbuild", "git", "jq", "gh")
               if not shutil.which(tool)]
    if not (shutil.which("setsid") or shutil.which("perl")):
        missing.append("setsid (or perl)")
    if not (shutil.which("sha256sum") or shutil.which("shasum")):
        missing.append("sha256sum or shasum")
    if not (shutil.which("gobjdump") or shutil.which("objdump")):
        missing.append("GNU objdump")
    if missing:
        return Check("toolchain", False, "missing: " + ", ".join(missing),
                     "cargo install --locked cargo-nextest cargo-zigbuild; install the rest with the host "
                     "package manager (macOS: brew install binutils jq gh)")
    if shutil.which("rustup"):
        code, output = run_quiet(["rustup", "target", "list", "--installed"], 10)
        if code == 0 and "x86_64-unknown-linux-gnu" not in output.split():
            return Check("toolchain", False, "Rust target x86_64-unknown-linux-gnu is not installed",
                         "rustup target add x86_64-unknown-linux-gnu")
    return Check("toolchain", True, "cargo, nextest, zigbuild, git, jq, gh, objdump present")


def check_gh() -> list[Check]:
    gh = os.environ.get("CAS_RELEASE_TRAIN_GH") or "gh"
    code, output = run_quiet([gh, "auth", "status"], 15)
    if code:
        return [Check("gh-auth", False, f"gh auth status failed: {output.splitlines()[-1] if output else code}",
                      "gh auth login")]
    code, output = run_quiet([gh, "api", "graphql", "-f", "query=" + MERGE_QUEUE_QUERY], 20)
    try:
        nodes = json.loads(output)["data"]["repository"]["mergeQueue"]["entries"]["nodes"] if code == 0 else None
    except (ValueError, KeyError, TypeError):
        nodes = None
    if not isinstance(nodes, list):
        return [Check("gh-auth", True, "gh is authenticated"),
                Check("merge-queue", False, f"merge-queue query failed: {output[-200:]}",
                      "gh auth refresh -s read:org,repo")]
    return [Check("gh-auth", True, "gh is authenticated"),
            Check("merge-queue", True, f"merge queue readable ({len(nodes)} entries)")]


def command_doctor(args) -> int:
    started = time.monotonic()
    checkout = Path(args.worktree).absolute()
    # Every sibling worktree's filesystem could host a release; temporary ones never do.
    checkouts = [checkout] + [Path(line[len("worktree "):])
                              for line in git(checkout, "worktree", "list", "--porcelain").splitlines()
                              if line.startswith("worktree ")]
    checkouts = [path for path in dict.fromkeys(checkouts)
                 if path.is_dir() and (path == checkout or under_temp_root(path, os.environ) is None)]
    env_file = Path(os.environ.get("CAS_RELEASE_ENV_FILE") or Path.home() / ".cas/release.env")
    with ThreadPoolExecutor(max_workers=3) as pool:
        gh, toolchain, zig = pool.submit(check_gh), pool.submit(check_toolchain), pool.submit(check_zig, checkout)
        checks = [Check("release-env", os.access(env_file, os.R_OK), f"{env_file} readable",
                        f"install -m 0600 /dev/null {shlex.quote(str(env_file))}")]
        checks += check_scratch(checkouts) + [zig.result(), toolchain.result()] + gh.result()
    gaps = 0
    for check in checks:
        print(f"{'ok  ' if check.ok else 'GAP '} {check.name}: {check.detail}")
        if not check.ok:
            gaps += 1
            print(f"     fix: {check.fix}")
    print(f"release doctor: {gaps} gap(s) in {time.monotonic() - started:.1f}s ({checkout})")
    return 1 if gaps else 0


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)
    resolve = commands.add_parser("resolve", help="print the resolved Zig and scratch base as shell assignments")
    resolve.add_argument("--worktree", type=Path, default=Path("."))
    resolve.add_argument("--durable", action="store_true", help="refuse system temporary roots")
    resolve.add_argument("--create", action="store_true", help="create the scratch base directory")
    resolve.add_argument("--link-zig", action="store_true", help="symlink the main checkout's Zig in")
    doctor = commands.add_parser("doctor", help="check every release prerequisite, one fix per gap")
    doctor.add_argument("--worktree", type=Path, default=Path("."))
    args = parser.parse_args(argv)
    return command_resolve(args) if args.command == "resolve" else command_doctor(args)


if __name__ == "__main__":
    sys.exit(main())
