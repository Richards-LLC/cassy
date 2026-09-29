#!/usr/bin/env python3
"""Conservative CI test selection, additive failure history, and run receipts.

No Cargo is invoked by plan/history/record. Unknown inputs select the workspace;
module uncertainty selects the affected crate, never an empty successful run.
"""
import argparse
import io
import json
import os
from pathlib import Path
import re
import subprocess
import statistics
import sys
import time
import tomllib
import urllib.request
from urllib.parse import urlparse
import zipfile

VERSION = 1
SAFE = re.compile(r"^[A-Za-z0-9_][A-Za-z0-9_-]*$")


def git(root, *args):
    return subprocess.check_output(["git", *args], cwd=root, text=True).strip()


def write(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")


def workspace(root):
    manifest = tomllib.loads((root / "Cargo.toml").read_text())
    crates = {}
    for member in manifest["workspace"]["members"]:
        if any(char in member for char in "*?[ "):
            raise ValueError("unexpanded workspace member")
        data = tomllib.loads((root / member / "Cargo.toml").read_text())
        crates[data["package"]["name"]] = (member, data)
    edges = {name: set() for name in crates}
    common = manifest["workspace"].get("dependencies", {})
    for name, (_, data) in crates.items():
        sections = [data] + list(data.get("target", {}).values())
        for section in sections:
            for key in ["dependencies", "dev-dependencies", "build-dependencies"]:
                for dependency, config in section.get(key, {}).items():
                    if isinstance(config, dict) and config.get("workspace"):
                        config = common.get(dependency, config)
                    dependency = config.get("package", dependency) if isinstance(config, dict) else dependency
                    if dependency in crates:
                        edges[dependency].add(name)
    return crates, edges


def closure(seeds, edges):
    result = set(seeds)
    pending = list(seeds)
    while pending:
        for item in edges.get(pending.pop(), set()) - result:
            result.add(item)
            pending.append(item)
    return result


def inventory(root, data):
    helper = root / "scripts/cas-test-targets.py"
    if helper.exists():
        subprocess.check_call([sys.executable, str(helper), str(root / "cas-cli"), "--check"], stdout=subprocess.DEVNULL)
        output = subprocess.check_output([sys.executable, str(helper), str(root / "cas-cli")], text=True)
        mapping = dict(line.split("|", 1) for line in output.splitlines())
        mapping = {stem: target for stem, target in mapping.items() if (root / f"cas-cli/tests/{stem}.rs").is_file()}
        if not mapping or any(not SAFE.fullmatch(x) for pair in mapping.items() for x in pair):
            raise ValueError("invalid integration inventory")
        return mapping
    if data["package"].get("autotests", True) is False:
        raise ValueError("explicit harness inventory unavailable")
    return {path.stem: path.stem for path in (root / "cas-cli/tests").glob("*.rs")}


def modules(root):
    """Use top-level modules as conservative units, widening ambiguous imports.

    Reverse qualified imports and module-name references widen the production
    surface. Tests with opaque/root imports or subprocess entry points are
    always retained because their dependencies cannot be proved narrower.
    """
    edges = {}
    source = root / "cas-cli/src"
    names = {path.stem for path in source.glob("*.rs")} | {path.name for path in source.iterdir() if path.is_dir()}
    for path in source.rglob("*.rs"):
        relative = path.relative_to(source)
        owner = relative.parts[0].removesuffix(".rs")
        if owner in {"lib", "main"}:
            continue
        text = path.read_text()
        if re.search(r"\bcrate::(?:[A-Z]|\{|\*)", text):
            for dependency in names:
                edges.setdefault(dependency, set()).add(owner)
        for dependency in set(re.findall(r"\b([A-Za-z_][A-Za-z_0-9]*)\s*::", text)) & names:
            edges.setdefault(dependency, set()).add(owner)
    return edges


def full(plan, reason):
    plan.update(mode="workspace", reasons=plan["reasons"] + [reason], commands=[{"package": "*", "args": ["--workspace"], "stems": None}])
    return plan


def plan(root, base, zero, history):
    result = {"version": VERSION, "mode": "scoped", "head": git(root, "rev-parse", "HEAD"), "reasons": [], "commands": [], "paths": []}
    try:
        if not base or set(base) == {"0"}:
            base = zero
        result["base"] = git(root, "merge-base", base, "HEAD")
        output = subprocess.check_output(["git", "diff", "--name-only", "-z", result["base"], "HEAD"], cwd=root)
        result["paths"] = output.decode().split("\0")[:-1]
        crates, edges = workspace(root)
        mapping = inventory(root, crates["cas"][1])
        changes = {name: [] for name in crates}
        for path in result["paths"]:
            if path in {"Cargo.toml", "Cargo.lock", ".config/nextest.toml"} or path.startswith((".cargo/", "vendor/", ".github/", "scripts/")):
                return full(result, "shared build/test policy changed")
            owners = [name for name, (member, _) in crates.items() if path.startswith(member + "/")]
            if not owners:
                # Markdown outside embedded source is not compiled input.
                if path.endswith(".md") or path.startswith("docs/"):
                    continue
                return full(result, "unmapped changed path: " + path)
            changes[owners[0]].append(path)
        affected = closure({name for name, paths in changes.items() if paths}, edges)
        recorded = []
        if history and Path(history).exists():
            data = json.loads(Path(history).read_text())
            if data.get("uncertain"):
                return full(result, "failure history is incomplete")
            recorded = data["failures"]
            for failure in recorded:
                if failure["package"] not in crates or not SAFE.fullmatch(failure["binary"]):
                    return full(result, "failure history cannot be mapped")
                affected.add(failure["package"])
        for name in sorted(affected):
            paths = changes[name]
            if name != "cas" or not paths or any(not p.startswith("cas-cli/tests/") and not p.startswith("cas-cli/src/") for p in paths):
                result["commands"].append({"package": name, "args": ["-p", name], "stems": None})
                continue
            production = [p for p in paths if p.startswith("cas-cli/src/")]
            wide = any(p in {"cas-cli/src/lib.rs", "cas-cli/src/main.rs"} or not (root / p).exists() for p in paths) or any("macro_rules!" in (root / p).read_text() for p in production)
            changed_modules = {p.split("/")[2].removesuffix(".rs") for p in production}
            changed_modules = closure(changed_modules, modules(root))
            selected = set()
            for stem in mapping:
                files = [root / f"cas-cli/tests/{stem}.rs"] + list((root / f"cas-cli/tests/{stem}").rglob("*.rs"))
                text = "\n".join(p.read_text() for p in files if p.is_file())
                imports = set(re.findall(r"\bcas::([A-Za-z_][A-Za-z_0-9]*)::", text))
                opaque = not imports or re.search(r"\b(?:cargo_bin|Command|assert_cmd)\b|\bcas::(?:\{|\*)", text)
                if wide or (production and (opaque or imports & changed_modules)) or any(p.startswith(f"cas-cli/tests/{stem}/") or p == f"cas-cli/tests/{stem}.rs" for p in paths):
                    selected.add(stem)
            # Existing snapshot router remains an independent guarded target.
            # These archive/catalog guards protect embedded sources, not imports.
            if any(p.startswith("cas-cli/src/builtins/") for p in production):
                selected.update(mapping)
            if any(p.startswith("cas-cli/tests/") for p in paths):
                selected.add("builtin_archive_portability_test")
            for failure in recorded:
                if failure["package"] == "cas":
                    matches = {stem for stem, target in mapping.items() if target == failure["binary"]}
                    if not matches:
                        return full(result, "historical binary missing from inventory")
                    selected.update(matches)
            selected &= mapping.keys()
            args = ["-p", "cas"]
            expressions = []
            if production or wide:
                args.append("--lib")
                declared = set(re.findall(r"\bmod\s+(\w+)\s*;", (root / "cas-cli/src/lib.rs").read_text()))
                root_uncertain = "#[path" in (root / "cas-cli/src/lib.rs").read_text() or re.search(r"\bmod\s+tests\s*\{", (root / "cas-cli/src/lib.rs").read_text())
                unit_modules = sorted(changed_modules) if not wide and not root_uncertain and changed_modules <= declared else None
                expressions.append("binary(cas)" if unit_modules is None else "(binary(cas) and (" + " or ".join(f"test({module}::)" for module in unit_modules) + "))")
            for target in sorted({mapping[stem] for stem in selected}):
                args.extend(["--test", target])
            for stem in sorted(selected):
                target = mapping[stem]
                expressions.append(f"binary({target})" if target == stem else f"(binary({target}) and test({stem}::))")
            if not expressions:
                return full(result, "no provable affected test target")
            args.extend(["-E", " or ".join(expressions)])
            result["commands"].append({"package": "cas", "args": args, "stems": sorted(selected), "targets": sorted({mapping[s] for s in selected}), "modules": unit_modules if production or wide else []})
        if not result["commands"]:
            return full(result, "no Rust tests selected; caller requested Rust validation")
        result["selection_units"] = sum(1 + len(command.get("stems") or []) for command in result["commands"])
        return result
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        return full(result, "uncertain graph/diff/history: " + str(error))


def collect_history(destination):
    """Read bounded recent artifacts. History only adds tests or widens scope."""
    records = []
    uncertain = False
    try:
        repo = os.environ["GITHUB_REPOSITORY"]
        token = os.environ["GH_TOKEN"]
        if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo):
            raise ValueError("invalid repository")
        api = "https://api.github.com/repos/" + repo
        class SafeRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, request, response, code, message, headers, url):
                redirected = super().redirect_request(request, response, code, message, headers, url)
                if redirected and urlparse(url).hostname != urlparse(request.full_url).hostname:
                    redirected.remove_header("Authorization")
                return redirected
        opener = urllib.request.build_opener(SafeRedirect())
        def fetch(url):
            request = urllib.request.Request(url, headers={"Authorization": "Bearer " + token, "Accept": "application/vnd.github+json"})
            with opener.open(request, timeout=15) as response:
                data = response.read(5_000_001)
                if len(data) > 5_000_000:
                    raise ValueError("history response too large")
                return data
        artifacts = json.loads(fetch(api + "/actions/artifacts?per_page=100"))["artifacts"]
        selected = [a for a in artifacts if a["name"].startswith("ci-test-impact-") and not a["expired"]][:16]
        for artifact in selected:
            # Construct the endpoint from a numeric ID, not artifact-controlled URLs.
            archive = zipfile.ZipFile(io.BytesIO(fetch(api + f"/actions/artifacts/{int(artifact['id'])}/zip")))
            for info in archive.infolist():
                if info.filename.endswith("receipt.json") and info.file_size <= 1_000_000:
                    receipt = json.loads(archive.read(info))
                    records.extend(receipt["failures"])
                    uncertain |= receipt.get("uncertain", False)
    except (OSError, ValueError, KeyError, TypeError, RuntimeError, zipfile.BadZipFile) as error:
        print("History unavailable; widening selection: " + str(error), file=sys.stderr)
        uncertain = True
    unique = {json.dumps(record, sort_keys=True): record for record in records}
    write(destination, {"failures": list(unique.values()), "uncertain": uncertain})


def selected_failure(plan, failure):
    for command in plan["commands"]:
        if command["package"] in {"*", failure["package"]}:
            if command["stems"] is None:
                return True
            if failure["binary"] == "cas" and "--lib" in command["args"]:
                unit_modules = command.get("modules")
                return unit_modules is None or any(failure["test"].startswith(module + "::") for module in unit_modules)
            if failure["binary"] in command.get("targets", []):
                return any(failure["test"].startswith(stem + "::") or stem == failure["binary"] for stem in command["stems"])
    return False


def record(plan, logs, status, elapsed, destination, role):
    count = 0
    failures = []
    for path in logs:
        text = re.sub(r"\x1b\[[0-9;]*[A-Za-z]", "", Path(path).read_text())
        count += sum(map(int, re.findall(r"Summary\s+\[.*?\]\s+(\d+) tests? run:", text)))
        for binary, test in re.findall(r"^\s*FAIL\s+\[[^\]]*\]\s+(\S+)\s+(\S+)\s*$", text, re.M):
            package, separator, target = binary.partition("::")
            if not separator:
                target = package
            if SAFE.fullmatch(package) and SAFE.fullmatch(target):
                failures.append({"package": package, "binary": target, "test": test})
    misses = [failure for failure in failures if not selected_failure(plan, failure)] if role == "full" else []
    receipt = {"version": VERSION, "head": plan["head"], "base": plan.get("base"), "role": role, "plan": plan, "selected_test_count": count, "elapsed_seconds": elapsed, "status": status, "failures": failures, "uncertain": status != 0 and not failures, "post_merge_missed": misses, "recall": (len(failures) - len(misses)) / len(failures) if failures and role == "full" else None}
    write(destination, receipt)
    print(f"CI impact: {plan['mode']}, {count} tests, {elapsed:.2f}s, status={status}, misses={len(misses)}, recall={receipt['recall']}")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as handle:
            handle.write(f"\nCI impact ({role}): {count} tests; {elapsed:.2f}s; selection={plan['mode']}; post-merge misses={len(misses)}; recall={receipt['recall']} (null means no observed full-suite failures).\n")
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["plan", "run", "history", "record", "summarize"])
    parser.add_argument("--base-sha", default=os.environ.get("BASE_SHA", ""))
    parser.add_argument("--zero-base-ref", default=os.environ.get("ZERO_BASE_REF", ""))
    parser.add_argument("--history", default="target/ci-impact/history.json")
    parser.add_argument("--out", default="target/ci-impact/receipt.json")
    parser.add_argument("--log", action="append", default=[])
    parser.add_argument("--status", type=int, default=0)
    parser.add_argument("--elapsed", type=float, default=0)
    args = parser.parse_args()
    if args.mode == "summarize":
        receipts = [json.loads(Path(path).read_text()) for path in args.log]
        roles = {}
        for role in ["scoped", "full"]:
            rows = [receipt for receipt in receipts if receipt["role"] == role]
            failures = sum(len(row["failures"]) for row in rows)
            misses = sum(len(row["post_merge_missed"]) for row in rows)
            roles[role] = {"runs": len(rows), "median_seconds": statistics.median(row["elapsed_seconds"] for row in rows) if rows else None, "failures": failures, "misses": misses, "recall": (failures - misses) / failures if failures and role == "full" else None}
        write(args.out, roles)
        return 0
    if args.mode == "history":
        collect_history(args.history)
        return 0
    root = Path(git(Path.cwd(), "rev-parse", "--show-toplevel"))
    selection = plan(root, args.base_sha, args.zero_base_ref, args.history)
    if args.mode == "plan":
        write(args.out, selection)
        return 0
    if args.mode == "record":
        record(selection, args.log, args.status, args.elapsed, args.out, "full")
        return 0
    started = time.monotonic()
    logs = []
    status = 0
    for index, command in enumerate(selection["commands"]):
        log = Path(args.out).parent / f"run-{index}.log"
        log.parent.mkdir(parents=True, exist_ok=True)
        env = dict(os.environ, VERIFIED_TEST_LOG=str(log.resolve()))
        run = subprocess.run([str(root / "scripts/run-verified-tests.sh"), "nextest", "run", *command["args"], "--no-fail-fast"], cwd=root, env=env)
        logs.append(str(log))
        counts = re.findall(r"Summary\s+\[.*?\]\s+(\d+) tests? run:", log.read_text())
        if run.returncode and counts and not any(map(int, counts)) and "-E" in command["args"]:
            selection["reasons"].append("zero selected tests: widened " + command["package"])
            command.update(args=["-p", command["package"]], stems=None)
            retry_log = log.with_name(f"retry-{index}.log")
            env["VERIFIED_TEST_LOG"] = str(retry_log.resolve())
            run = subprocess.run([str(root / "scripts/run-verified-tests.sh"), "nextest", "run", *command["args"], "--no-fail-fast"], cwd=root, env=env)
            logs.append(str(retry_log))
        status = status or run.returncode
    record(selection, logs, status, time.monotonic() - started, args.out, "scoped")
    return status


if __name__ == "__main__":
    sys.exit(main())
