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
        # Retain source aliases in both layouts recognised by cas-test-targets.
        # Grouped harness names themselves are not source suites.
        tests = root / "cas-cli/tests"
        mapping = {stem: target for stem, target in mapping.items()
                   if (tests / f"{stem}.rs").is_file() or (tests / stem / "main.rs").is_file()}
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
        if re.search(r"\bcrate::(?:[A-Z]|\{|\*|[A-Za-z_]\w*\s*(?:;|,|\bas\b))", text):
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
        base = git(root, "rev-parse", "--verify", f"{base}^{{commit}}")
        parents = git(root, "rev-list", "--parents", "-n", "1", "HEAD").split()[1:]
        if len(parents) > 1 and base == parents[0]:
            # On a merge push, GitHub's before SHA is the first parent. That
            # parent can already contain the changed source, so diffing from it
            # hides the change from this merge's selection. Compare from the
            # parents' common base to keep affected tests such as cli::update.
            result["base"] = git(root, "merge-base", "--octopus", *parents)
        else:
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
                # Markdown outside embedded source is not compiled input, nor
                # are Commander package scripts (cas-eac2: the script tier
                # owns them through one named worker-memory assertion).
                if path.endswith(".md") or path.startswith("docs/") or path == "hub-web/package.json":
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
            test_paths = [p for p in paths if p.startswith("cas-cli/tests/")]
            shared_test_input = any(not any(p == f"cas-cli/tests/{stem}.rs" or p.startswith(f"cas-cli/tests/{stem}/") for stem in mapping) for p in test_paths)
            selected = set(mapping) if shared_test_input else set()
            for stem in mapping:
                files = [root / f"cas-cli/tests/{stem}.rs"] + list((root / f"cas-cli/tests/{stem}").rglob("*.rs"))
                text = "\n".join(p.read_text() for p in files if p.is_file())
                imports = set(re.findall(r"\bcas::([A-Za-z_][A-Za-z_0-9]*)::", text))
                opaque = not imports or re.search(r"\b(?:cargo_bin|Command|assert_cmd)\b|\bcas::(?:[A-Z]|\{|\*|[A-Za-z_]\w*\s*(?:;|,|\bas\b))", text)
                if wide or (production and (opaque or imports & changed_modules)) or any(p.startswith(f"cas-cli/tests/{stem}/") or p == f"cas-cli/tests/{stem}.rs" for p in paths):
                    selected.add(stem)
            # Existing snapshot router remains an independent guarded target.
            # These archive/catalog guards protect embedded sources, not imports.
            if any(p.startswith("cas-cli/src/builtins/") for p in production):
                selected.update(mapping)
            if any(p.startswith("cas-cli/tests/") for p in paths):
                selected.add("builtin_archive_portability_test")
            lib_failed = set()
            for failure in recorded:
                if failure["package"] == "cas" and failure["binary"] == "cas":
                    # A unit test in the cas lib: rerun exactly that test (cas-38ac).
                    lib_failed.add(failure["test"])
                elif failure["package"] == "cas":
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
            if lib_failed:
                if "--lib" not in args:
                    args.append("--lib")
                    unit_modules = []
                if unit_modules is not None:
                    unit_modules = sorted(set(unit_modules) | {test.split("::")[0] for test in lib_failed})
                expressions.append("(binary(cas) and (" + " or ".join(f"test(={test})" for test in sorted(lib_failed)) + "))")
            for target in sorted({mapping[stem] for stem in selected}):
                args.extend(["--test", target])
            for stem in sorted(selected):
                target = mapping[stem]
                expressions.append(f"binary({target})" if target == stem else f"(binary({target}) and test({stem}::))")
            if not expressions:
                return full(result, "no provable affected test target")
            args.extend(["-E", " or ".join(expressions)])
            result["commands"].append({"package": "cas", "args": args, "stems": sorted(selected), "targets": sorted({mapping[s] for s in selected}), "modules": unit_modules if production or wide or lib_failed else []})
        if not result["commands"]:
            return full(result, "no Rust tests selected; caller requested Rust validation")
        if len(result["commands"]) > 1 and all(command["stems"] is None for command in result["commands"]):
            packages = sorted(command["package"] for command in result["commands"])
            result["commands"] = [{"package": "*", "packages": packages, "args": [arg for package in packages for arg in ["-p", package]], "stems": None}]
        result["selection_units"] = sum(len(command.get("packages", [command["package"]])) + len(command.get("stems") or []) for command in result["commands"])
        return result
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        return full(result, "uncertain graph/diff/history: " + str(error))


def lane_split(root, selection):
    """Order the unchanged selection for lane CI, fail-fast (cas-2e1b).

    Own tests come first: unit tests of the directly changed top-level modules,
    edited integration suites, and directly changed crates. The ring follows:
    everything else this plan selected for them. A workspace plan has no
    provable scope; it returns None and the train suite keeps that run.
    """
    if selection["mode"] != "scoped":
        return None
    crates, _ = workspace(root)
    paths = selection["paths"]
    direct = {name for name, (member, _) in crates.items() if any(p.startswith(member + "/") for p in paths)}
    multiple = len(selection["commands"]) > 1
    build, selected, own = [], [], []
    for command in selection["commands"]:
        packages = command.get("packages", [command["package"]])
        if command["stems"] is None:
            build += [arg for package in packages for arg in ("-p", package)]
            selected += [f"package({package})" for package in packages]
            own += [f"package({package})" for package in packages if package in direct]
            continue
        args = command["args"]
        expression = args[args.index("-E") + 1]
        build += ["-p", "cas"] if multiple else args[:args.index("-E")]
        mapping = inventory(root, crates["cas"][1])
        changed = sorted({p.split("/")[2].removesuffix(".rs") for p in paths if p.startswith("cas-cli/src/")} - {"lib", "main"})
        parts = []
        if "--lib" in args and changed and all(SAFE.fullmatch(name) for name in changed):
            parts.append("(binary(cas) and (" + " or ".join(f"test(/^{name}::/)" for name in changed) + "))")
        for stem in command["stems"]:
            if any(p == f"cas-cli/tests/{stem}.rs" or p.startswith(f"cas-cli/tests/{stem}/") for p in paths):
                target = mapping[stem]
                parts.append(f"binary({target})" if target == stem else f"(binary({target}) and test({stem}::))")
        guard = (lambda text: f"(package(cas) and ({text}))") if multiple else (lambda text: text)
        selected.append(guard(expression))
        if parts:
            own.append(guard(" or ".join(parts)))
    selected = " or ".join(selected)
    own = " or ".join(own) or None
    return {"build": build, "selected": selected, "own": own, "ring": f"({selected}) and not ({own})" if own else selected}


def lane_count(root, source, expression):
    """Count the non-ignored tests a filter selects, so no phase runs empty."""
    output = subprocess.check_output([os.environ.get("CARGO", "cargo"), "nextest", "list", *source, "-E", expression, "--message-format", "json"], cwd=root, text=True)
    suites = json.loads(output)["rust-suites"].values()
    return sum(1 for suite in suites for case in suite["testcases"].values() if case["filter-match"]["status"] == "matches" and not case.get("ignored"))


def lane_run(root, selection, source, expression, extra, destination, role):
    log = Path(destination).parent / f"{role}.log"
    log.parent.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    env = dict(os.environ, VERIFIED_TEST_LOG=str(log.resolve()))
    status = subprocess.run([str(root / "scripts/run-verified-tests.sh"), "nextest", "run", *source, "-E", expression, *extra, "--no-fail-fast"], cwd=root, env=env).returncode
    record(selection, [str(log)], status, time.monotonic() - started, destination, role)
    return status


def lane(root, args):
    """Lane CI phases: plan, own (builds and runs own tests), ring shards."""
    out = Path(args.out)
    outputs = {}
    if args.phase == "plan":
        selection = plan(root, args.base_sha, args.zero_base_ref, args.history)
        split = lane_split(root, selection)
        write(out, {"plan": selection, "split": split})
        outputs = {"scoped": str(split is not None).lower(), "build-args": " ".join(split["build"]) if split else "", "reason": "; ".join(selection["reasons"])}
        print(f"Lane impact: {selection['mode']}" + (f" ({outputs['reason']}); the train suite owns this delivery's Rust tests" if split is None else f"; own: {split['own']}; ring: {split['ring']}"))
        status = 0
    else:
        saved = json.loads(out.read_text())
        selection, split = saved["plan"], saved["split"]
        source = ["--archive-file", args.archive, "--workspace-remap", str(root)] if args.phase == "ring" else split["build"]
        receipt = out.with_name(f"{args.phase}-receipt.json")
        if args.phase == "ring":
            status = lane_run(root, selection, source, split["ring"], ["--partition", f"count:{args.shard}"], receipt, "lane-ring")
        else:
            status = 0
            if split["own"] and lane_count(root, source, split["own"]):
                status = lane_run(root, selection, source, split["own"], [], receipt, "lane-own")
            else:
                print("Lane impact: no own tests; the ring follows")
            ring = lane_count(root, source, split["ring"]) if status == 0 else 0
            shards = min(3, -(-ring // 1500))
            outputs = {"ring-shards": json.dumps(list(range(1, shards + 1))), "ring-total": str(shards), "ring-tests": str(ring)}
    if args.github_output:
        with open(args.github_output, "a") as handle:
            handle.writelines(f"{key}={value}\n" for key, value in outputs.items())
    return status


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
            logged = []
            receipt_uncertain = False
            for info in archive.infolist():
                if info.filename.endswith("receipt.json") and info.file_size <= 1_000_000:
                    receipt = json.loads(archive.read(info))
                    records.extend(receipt["failures"])
                    receipt_uncertain |= receipt.get("uncertain", False)
                elif info.filename.endswith(".log") and info.file_size <= 50_000_000:
                    logged.extend(parse_failures(strip_ansi(archive.read(info).decode(errors="replace"))))
            # Receipts written before the progress-counter fix recorded a failed
            # run as uncertain; its uploaded log still names the failures.
            if receipt_uncertain and logged:
                records.extend(logged)
            else:
                uncertain |= receipt_uncertain
    except (OSError, ValueError, KeyError, TypeError, RuntimeError, zipfile.BadZipFile) as error:
        print("History unavailable; widening selection: " + str(error), file=sys.stderr)
        uncertain = True
    unique = {json.dumps(record, sort_keys=True): record for record in records}
    write(destination, {"failures": list(unique.values()), "uncertain": uncertain})


def selected_failure(plan, failure):
    for command in plan["commands"]:
        packages = command.get("packages")
        includes_package = failure["package"] in packages if packages is not None else command["package"] in {"*", failure["package"]}
        if includes_package:
            if command["stems"] is None:
                return True
            if failure["binary"] == "cas" and "--lib" in command["args"]:
                unit_modules = command.get("modules")
                return unit_modules is None or any(failure["test"].startswith(module + "::") for module in unit_modules)
            if failure["binary"] in command.get("targets", []):
                return any(failure["test"].startswith(stem + "::") or stem == failure["binary"] for stem in command["stems"])
    return False


def strip_ansi(text):
    return re.sub(r"\x1b\[[0-9;]*[A-Za-z]", "", text)


def parse_failures(text):
    """FAIL lines of an ANSI-stripped nextest log. nextest 0.9.100+ prints a
    `( 3528/11850)` progress counter after the duration (cas-38ac)."""
    failures = []
    for binary, test in re.findall(r"^\s*FAIL\s+\[[^\]]*\]\s+(?:\(\s*\d+/\d+\)\s+)?(\S+)\s+(\S+)\s*$", text, re.M):
        package, separator, target = binary.partition("::")
        if not separator:
            target = package
        if SAFE.fullmatch(package) and SAFE.fullmatch(target):
            failures.append({"package": package, "binary": target, "test": test})
    return failures


def record(plan, logs, status, elapsed, destination, role):
    count = 0
    failures = []
    for path in logs:
        text = strip_ansi(Path(path).read_text())
        count += sum(map(int, re.findall(r"Summary\s+\[.*?\]\s+(\d+) tests? run:", text)))
        failures.extend(parse_failures(text))
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
    parser.add_argument("mode", choices=["plan", "run", "history", "record", "summarize", "lane"])
    parser.add_argument("--phase", choices=["plan", "own", "ring"], default="plan")
    parser.add_argument("--archive", default="lane-impact.tar.zst")
    parser.add_argument("--shard", default="1/1")
    parser.add_argument("--github-output", default="")
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
    if args.mode == "lane":
        return lane(root, args)
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
