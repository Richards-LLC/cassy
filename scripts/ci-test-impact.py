#!/usr/bin/env python3
"""Conservative CI test selection, additive failure history, and run receipts.

No builds are performed by plan/history/record. Unknown inputs select the workspace;
module uncertainty selects the affected crate, never an empty successful run.
"""
import argparse
import io
import json
import os
import platform
from pathlib import Path
import re
import signal
import subprocess
import statistics
import sys
import time
import tomllib
import urllib.request
from urllib.parse import urlparse
import zipfile
from rust_test_source import tokens, pairs

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


def integration_text(root, stem):
    """Conservatively include a suite's Rust modules and shared helpers."""
    tests = root / "cas-cli/tests"
    pending = [tests / f"{stem}.rs", *sorted((tests / stem).rglob("*.rs"))]
    seen, texts = set(), []
    while pending:
        path = pending.pop().resolve()
        if path in seen or not path.is_file():
            continue
        seen.add(path)
        text = path.read_text()
        texts.append(text)
        pending.extend(path.parent / relative for matches in re.findall(
            r'#\[path\s*=\s*"([^"]+)"\]|\binclude!\s*\(\s*"([^"]+)"', text)
            for relative in matches if relative)
        for module in re.findall(r"\bmod\s+(\w+)\s*;", text):
            # Both crate-root and ordinary module layouts; over-selection is safe.
            for directory in {path.parent, path.with_suffix("")}:
                pending.extend([directory / f"{module}.rs", directory / module / "mod.rs"])
    return "\n".join(texts)


# cas-rule: rule-176 scope: "(4) Run tests only for what changed. Plumbing changes get a smoke check, not the full suite."
def referencing_tests(text, paths):
    """Follow explicit file literals, local helpers and constants to test bodies."""
    if not any(Path(path).name in text for path in paths):
        return set()
    ts = tokens(text)
    ps = pairs(ts)
    tests = {match.start(1) for match in re.finditer(
        r"#\[(?:\w+::)*test(?:\([^]]*\))?\]\s*(?:#\[[^]]*\]\s*)*(?:async\s+)?fn\s+(\w+)\s*\(", text)}
    functions = []
    for i, token in enumerate(ts[:-1]):
        if token.string or token.value != "fn" or not SAFE.fullmatch(ts[i + 1].value):
            continue
        brace = next((j for j in range(i + 2, len(ts)) if not ts[j].string and ts[j].value in {"{", ";"}), None)
        if brace is not None and ts[brace].value == "{" and brace in ps:
            functions.append((ts[i + 1].value, brace, ps[brace], ts[i + 1].start in tests))
    hits = {i for i, token in enumerate(ts) if token.string and any(
        path in token.value or token.value.strip('"') == Path(path).name for path in paths)}
    dependent = {name for name, begin, end, _ in functions if any(begin < i < end for i in hits)}
    for hit in hits:
        if any(begin < hit < end for _, begin, end, _ in functions):
            continue
        owner = next((i for i in range(hit - 1, -1, -1) if not ts[i].string and ts[i].value in {"const", "static", ";", "}"}), None)
        if owner is not None and ts[owner].value in {"const", "static"}:
            dependent.add(ts[owner + 1].value)
    while True:
        expanded = dependent | {name for name, begin, end, _ in functions if any(
            not ts[i].string and ts[i].value in dependent for i in range(begin + 1, end))}
        if expanded == dependent:
            return {name for name, _, _, test in functions if test and name in dependent}
        dependent = expanded


# cas-rule: rule-176 scope: "(4) Run tests only for what changed. Plumbing changes get a smoke check, not the full suite."
def file_contracts(root, paths, crates, mapping, texts):
    commands, identities = [], []
    for package, (member, manifest) in crates.items():
        units = set()
        for source in (root / member / "src").rglob("*.rs"):
            units.update(referencing_tests(source.read_text(), paths))
        targets = {}
        suites = texts if package == "cas" else {source.stem: source.read_text() for source in (root / member / "tests").glob("*.rs")}
        for stem, text in suites.items():
            names = referencing_tests(text, paths)
            if names:
                targets.setdefault(mapping[stem] if package == "cas" else stem, set()).update(names)
        args, expressions = ["-p", package], []
        if units:
            args.append("--lib")
            targets[None] = units
        for target, names in sorted(targets.items(), key=lambda item: item[0] or ""):
            if target is not None:
                args.extend(["--test", target])
            binary = package if target is None else f"{package}::{target}"
            for name in sorted(names):
                expression = f"(binary_id(={binary}) and test(/(^|::){name}$/))"
                expressions.append(expression)
                identities.append({"package": package, "binary": binary, "test": name, "filter": expression})
        if expressions:
            commands.append({"package": package, "args": args + ["-E", " or ".join(expressions)], "stems": [], "targets": sorted(target for target in targets if target is not None), "modules": []})
    return commands, identities


# cas-rule: rule-176 scope: "(4) Run tests only for what changed. Plumbing changes get a smoke check, not the full suite."
def plan(root, base, zero, history, delivery=False):
    result = {"version": VERSION, "mode": "scoped", "head": git(root, "rev-parse", "HEAD"), "reasons": [], "commands": [], "paths": []}
    try:
        if not base or set(base) == {"0"}:
            base = zero
        base = git(root, "rev-parse", "--verify", f"{base}^{{commit}}")
        parents = git(root, "rev-list", "--parents", "-n", "1", "HEAD").split()[1:]
        if not delivery and len(parents) > 1 and base == parents[0]:
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
        texts = {stem: integration_text(root, stem) for stem in mapping}
        # Binary contracts depend on every cas source module, even when a
        # helper hides process construction behind command(). Keep a superset.
        subprocess_stems = sorted(stem for stem, text in texts.items() if re.search(
            r"\b(?:cargo_bin|cas_binary|Command|assert_cmd)\b|\bcommand\s*\(", text))
        result["subprocess_stems"] = subprocess_stems
        changes = {name: [] for name in crates}
        contract_paths = []
        for path in result["paths"]:
            if path.startswith((".claude/", ".codex/", ".grok/", "docs/", "changelog.d/")):
                continue
            if path.startswith(("scripts/", ".github/")):
                contract_paths.append(path)
                continue
            if path in {"Cargo.toml", "Cargo.lock", ".config/nextest.toml"} or path.startswith((".cargo/", "vendor/")):
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
            if production:
                selected.update(subprocess_stems)
            for stem in mapping:
                text = texts[stem]
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
        contracts, identities = file_contracts(root, contract_paths, crates, mapping, texts) if contract_paths else ([], [])
        result["file_contracts"] = identities
        for contract in contracts:
            previous = next((command for command in result["commands"] if command["package"] == contract["package"]), None)
            if previous is None:
                result["commands"].append(contract)
            elif previous["stems"] is not None:
                split = previous["args"].index("-E")
                prefix = previous["args"][:split]
                incoming = contract["args"][:contract["args"].index("-E")]
                for index, arg in enumerate(incoming[2:], 2):
                    if arg == "--lib" and arg not in prefix:
                        prefix.append(arg)
                    elif arg == "--test" and incoming[index + 1] not in previous.get("targets", []):
                        prefix.extend([arg, incoming[index + 1]])
                previous["args"] = prefix + ["-E", f"({previous['args'][split + 1]}) or ({contract['args'][-1]})"]
                previous["targets"] = sorted(set(previous.get("targets", [])) | set(contract["targets"]))
        if len(result["commands"]) > 1 and all(command["stems"] is None for command in result["commands"]):
            packages = sorted(command["package"] for command in result["commands"])
            result["commands"] = [{"package": "*", "packages": packages, "args": [arg for package in packages for arg in ["-p", package]], "stems": None}]
        result["selection_units"] = sum(len(command.get("packages", [command["package"]])) + len(command.get("stems") or []) for command in result["commands"])
        return result
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        return full(result, "uncertain graph/diff/history: " + str(error))


# cas-rule: rule-176 scope: "(4) Run tests only for what changed. Plumbing changes get a smoke check, not the full suite."
def lane_split(root, selection):
    """Order the unchanged selection for lane CI, fail-fast (cas-2e1b).

    Own tests come first: unit tests of the directly changed top-level modules,
    edited integration suites, and directly changed crates. The ring follows:
    everything else the plan selected, including a whole workspace plan.
    """
    paths = selection["paths"]
    if selection["mode"] != "scoped":
        build, selected = ["--workspace"], "all()"
    else:
        build, selected = [], []
        multiple = len(selection["commands"]) > 1
        for command in selection["commands"]:
            packages = command.get("packages", [command["package"]])
            if command["stems"] is None:
                build += [arg for package in packages for arg in ("-p", package)]
                selected += [f"package({package})" for package in packages]
                continue
            args = command["args"]
            build += ["-p", command["package"]] if multiple else args[:args.index("-E")]
            selected.append(f"(package({command['package']}) and ({args[args.index('-E') + 1]}))")
        selected = " or ".join(selected)
    own = [row["filter"] for row in selection.get("file_contracts", [])]
    try:
        crates, _ = workspace(root)
        direct = sorted(name for name, (member, _) in crates.items() if any(p.startswith(member + "/") for p in paths))
        mapping = inventory(root, crates["cas"][1]) if "cas" in direct else {}
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
        direct = []
    for name in direct:
        if name != "cas":
            own.append(f"package({name})")
            continue
        modules = sorted({p.split("/")[2].removesuffix(".rs") for p in paths if p.startswith("cas-cli/src/")} - {"lib", "main"})
        parts = [f"(binary(cas) and test(/^{module}::/))" for module in modules if SAFE.fullmatch(module) and "-" not in module]
        for stem, target in sorted(mapping.items()):
            if any(p == f"cas-cli/tests/{stem}.rs" or p.startswith(f"cas-cli/tests/{stem}/") for p in paths):
                parts.append(f"binary({target})" if target == stem else f"(binary({target}) and test({stem}::))")
        if parts:
            own.append("(package(cas) and (" + " or ".join(parts) + "))")
    own = " or ".join(own) or None
    return {"build": build, "selected": selected, "own": own, "ring": f"({selected}) and not ({own})" if own else selected}

# cas-rule: rule-176 scope: "(4) Run tests only for what changed. Plumbing changes get a smoke check, not the full suite."
def lane_count(root, source, expression):
    """Count the non-ignored tests a filter selects, so no phase runs empty."""
    if not expression:
        return 0
    output = subprocess.check_output([os.environ.get("CARGO", "cargo"), "nextest", "list", *source, "-E", expression, "--message-format", "json"], cwd=root, text=True)
    suites = json.loads(output)["rust-suites"].values()
    return sum(1 for suite in suites for case in suite["testcases"].values() if case["filter-match"]["status"] == "matches" and not case.get("ignored"))


def lane_run(root, selection, source, expression, extra, destination, role):
    """Run one phase. Parsed test failures wait for base attribution; a
    failure without a parsed failed test fails the phase at once."""
    log = Path(destination).parent / f"{role}.log"
    log.parent.mkdir(parents=True, exist_ok=True)
    # Preserve archive compiler identity while binding the actual runtime.
    selection = dict(selection, configuration=proof_configuration(root, selection.get("configuration")))
    started = time.monotonic()
    env = dict(os.environ, VERIFIED_TEST_LOG=str(log.resolve()))
    status = subprocess.run([str(root / "scripts/run-verified-tests.sh"), "nextest", "run", *source, "-E", expression, *extra, "--no-fail-fast"], cwd=root, env=env).returncode
    receipt = record(selection, [str(log)], status, time.monotonic() - started, destination, role)
    # Attribution may clear the status only when every unsuccessful test is
    # named: the summary's run-minus-passed count must match the parsed set.
    counts = re.findall(r"Summary\s+\[.*?\]\s+(\d+) tests? run:\s+(\d+) passed", strip_ansi(log.read_text()))
    named = {(f["package"], f["binary"], f["test"]) for f in receipt["failures"]}
    unsuccessful = sum(int(run) - int(passed) for run, passed in counts)
    accounted = bool(named) and len(counts) > 0 and unsuccessful == len(named)
    if status and named and not accounted:
        print(f"Lane impact: {unsuccessful} unsuccessful tests but {len(named)} named; the unaccounted outcomes keep this phase red", file=sys.stderr)
    return status, bool(status and accounted)


def proof_configuration(root, compiled=None):
    """Bind reusable results to the actual runner, tools and test profile.

    The receipt's exact head SHA binds repository inputs (including nextest
    configuration); these fields bind inputs outside that tree. Missing tool
    identity disables reuse rather than inventing a compatible baseline.
    """
    try:
        def version(*command):
            return subprocess.check_output(command, cwd=root, text=True, stderr=subprocess.DEVNULL, timeout=15).strip()
        return {"version": 1, "platform": platform.system(), "arch": platform.machine(),
                "image": [os.environ.get(key, "") for key in ("ImageOS", "ImageVersion")],
                "rustc": compiled["rustc"] if compiled else version("rustc", "-Vv"),
                "nextest": version(os.environ.get("CARGO", "cargo"), "nextest", "--version"),
                "profile": "default",
                "repository_config": compiled["repository_config"] if compiled else git(root, "ls-tree", "-r", "HEAD", "--", ".cargo", ".config/nextest.toml", "rust-toolchain", "rust-toolchain.toml"),
                "environment": {key: value for key, value in sorted(os.environ.items())
                    if key.startswith(("CARGO_PROFILE_", "NEXTEST_")) or key in {
                        "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTDOCFLAGS",
                        "CARGO_BUILD_TARGET", "RUST_TEST_THREADS", "CI", "GITHUB_ACTIONS"}}}
    except (OSError, subprocess.SubprocessError):
        return None


def github_fetcher():
    repo, token = os.environ["GITHUB_REPOSITORY"], os.environ["GH_TOKEN"]
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo):
        raise ValueError("invalid repository")
    class SafeRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, request, response, code, message, headers, url):
            redirected = super().redirect_request(request, response, code, message, headers, url)
            if redirected and urlparse(url).hostname != urlparse(request.full_url).hostname:
                redirected.remove_header("Authorization")
            return redirected
    opener = urllib.request.build_opener(SafeRedirect())
    def fetch(path):
        request = urllib.request.Request("https://api.github.com/repos/" + repo + path,
            headers={"Authorization": "Bearer " + token, "Accept": "application/vnd.github+json"})
        with opener.open(request, timeout=15) as response:
            data = response.read(5_000_001)
            if len(data) > 5_000_000:
                raise ValueError("proof response too large")
            return data
    return repo, fetch


def failure_key(failure):
    return failure["package"], failure["binary"], failure["test"]


# cas-rule: rule-176 scope: "(2) Each check runs once per change, at the cheapest point that catches its defect. Running the same check in more places is a defect."
def base_failure_proof(base, configuration, failures):
    """Only explicit failed identities at this exact SHA/config can be inherited.

    Read existing CI artifacts, never inferred selection or additive history.
    Old receipts without configuration and unavailable artifacts are misses.
    Bounded lookup and downloads leave the existing job time for a replay.
    """
    proven = {}
    if not configuration or not re.fullmatch(r"[0-9a-f]{40}", base):
        return []
    wanted = {failure_key(failure) for failure in failures}
    try:
        repo, fetch = github_fetcher()
        runs = json.loads(fetch(f"/actions/runs?head_sha={base}&status=completed&per_page=5"))["workflow_runs"]
        candidates = []
        for run in runs[:5]:
            if (run["head_sha"] != base or run["head_repository"]["full_name"] != repo
                    or run["path"] not in {".github/workflows/lane.yml", ".github/workflows/ci.yml"}):
                continue
            artifacts = json.loads(fetch(f"/actions/runs/{int(run['id'])}/artifacts?per_page=100"))["artifacts"]
            candidates.extend((artifact, run["id"], False) for artifact in artifacts)
        # Earlier lanes can have replayed this same recorded train base. Their
        # base-receipt is explicit proof at the base, even though the lane head
        # differs. Consult recent same-repository artifacts without a new job.
        recent = json.loads(fetch("/actions/artifacts?per_page=100"))["artifacts"]
        for artifact in recent:
            producer = artifact.get("workflow_run") or {}
            if producer.get("repository_id") and producer.get("repository_id") == producer.get("head_repository_id"):
                candidates.append((artifact, producer["id"], True))
        seen = set()
        for artifact, run_id, replay_only in candidates:
            if artifact["expired"] or not artifact["name"].startswith(("lane-impact-", "ci-test-impact-")) or artifact["name"] == "lane-impact-archive":
                continue
            if artifact["id"] in seen:
                continue
            if len(seen) == 12:
                break
            seen.add(artifact["id"])
            try:
                archive = zipfile.ZipFile(io.BytesIO(fetch(f"/actions/artifacts/{int(artifact['id'])}/zip")))
                for info in archive.infolist():
                    if not info.filename.endswith("receipt.json") or info.file_size > 1_000_000:
                        continue
                    if replay_only and Path(info.filename).name != "base-receipt.json":
                        continue
                    receipt = json.loads(archive.read(info))
                    if receipt.get("head") != base or receipt.get("configuration") != configuration or receipt.get("uncertain", True):
                        continue
                    for failure in receipt["failures"]:
                        key = failure_key(failure)
                        if key in wanted:
                            proven[key] = {"failure": failure, "run_id": run_id, "artifact_id": artifact["id"], "receipt": info.filename}
                if wanted <= proven.keys():
                    return list(proven.values())
            except (OSError, ValueError, KeyError, TypeError, RuntimeError, zipfile.BadZipFile) as error:
                print(f"Base proof artifact unavailable: {error}", file=sys.stderr)
    except (OSError, ValueError, KeyError, TypeError, RuntimeError) as error:
        print(f"Base proof unavailable; replay required: {error}", file=sys.stderr)
    return list(proven.values())


# cas-rule: rule-176 scope: "(3) Remove steps before adding them. A new check must fit an existing stage budget or replace something."
def lane_attribute(root, base, failures, destination, configuration=None):
    """Reuse exact-base failures, replay misses, and fail closed on incomplete work."""
    started = time.monotonic()
    # Twenty minutes for lookup/preparation/build/test, within the existing
    # 45-minute job. This is a bound, never permission to inherit a missing test.
    deadline = started + 20 * 60
    failures = list({failure_key(f): f for f in failures}.values())
    result = {"base": base, "inherited": [], "caused": [], "unresolved": failures,
              "reused": [], "status": "pending", "timings": {
                  "lookup_seconds": 0, "worktree_seconds": 0, "build_seconds": 0, "test_seconds": 0}}
    def save():
        result["elapsed_seconds"] = time.monotonic() - started
        write(destination, result)
    save()  # Durable even if preparation is interrupted.
    lookup = time.monotonic()
    result["reused"] = base_failure_proof(base, configuration, failures)
    result["timings"]["lookup_seconds"] = time.monotonic() - lookup
    known = {failure_key(row["failure"]) for row in result["reused"]}
    result["inherited"] = [f for f in failures if failure_key(f) in known]
    pending = [f for f in failures if failure_key(f) not in known]
    result["unresolved"] = pending
    if result["inherited"]:
        # Carry reusable proof forward so a busy train does not age its sole
        # successful base replay out of the bounded recent-artifact window.
        write(Path(destination).with_name("base-receipt.json"), {
            "version": VERSION, "head": base, "configuration": configuration,
            "failures": result["inherited"], "uncertain": False, "reused": result["reused"]})
    save()
    work = Path(os.environ.get("RUNNER_TEMP") or root / "target") / "lane-impact-base"
    def remaining():
        return max(0.001, deadline - time.monotonic())
    def timed_command(command, phase, log, env):
        phase_start = time.monotonic()
        try:
            # Stream output to disk throughout compilation/test execution.
            with open(log, "a") as handle:
                process = subprocess.Popen(command, cwd=work, env=env, stdout=handle, stderr=subprocess.STDOUT, start_new_session=True)
                try:
                    return process.wait(timeout=remaining())
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
                    raise
        finally:
            result["timings"][phase] = time.monotonic() - phase_start
            save()
    if pending:
        try:
            preparation = time.monotonic()
            try:
                subprocess.run(["git", "worktree", "remove", "--force", str(work)], cwd=root, capture_output=True, timeout=remaining())
                subprocess.check_call(["git", "worktree", "add", "--detach", str(work), base], cwd=root, timeout=remaining())
                subprocess.check_call(["git", "submodule", "update", "--init", "--recursive"], cwd=work, timeout=remaining())
            finally:
                result["timings"]["worktree_seconds"] = time.monotonic() - preparation
                save()
            packages = sorted({f["package"] for f in pending})
            build = ["--workspace"] if len(packages) > 1 else ["-p", packages[0]]
            if len(packages) == 1:
                targets = sorted({f["binary"] for f in pending})
                build += [arg for target in targets for arg in (["--lib"] if target == packages[0] else ["--test", target])]
            identity = lambda f: f["package"] if f["binary"] == f["package"] else f"{f['package']}::{f['binary']}"
            expression = " or ".join(f"(binary_id({identity(f)}) and test(={f['test']}))" for f in pending)
            env = dict(os.environ, CARGO_TARGET_DIR=str(root / "target"))
            cargo = os.environ.get("CARGO", "cargo")
            log = Path(destination).with_suffix(".log")
            log.with_suffix(".build.log").write_text("")
            # List performs the build separately so a stalled compiler and a
            # stalled test have distinct durations in the failure receipt.
            status = timed_command([cargo, "nextest", "list", *build, "-E", expression, "--message-format", "json"], "build_seconds", log.with_suffix(".build.log"), env)
            if status:
                raise RuntimeError(f"base build failed: {status}")
            log.write_text("")
            status = timed_command([cargo, "nextest", "run", *build, "-E", expression, "--no-fail-fast", "--no-tests=pass"], "test_seconds", log, env)
            text = strip_ansi(log.read_text())
            at_base = {failure_key(f) for f in parse_failures(text)}
            if status and not at_base:
                raise RuntimeError(f"base run failed without a test result: {status}")
            base_failures = [f for f in pending if failure_key(f) in at_base]
            base_configuration = proof_configuration(work)
            write(Path(destination).with_name("base-receipt.json"), {
                "version": VERSION, "head": base, "configuration": base_configuration,
                "failures": result["inherited"] + base_failures if base_configuration == configuration else base_failures,
                "uncertain": False, "reused": result["reused"]})
            result["inherited"] += base_failures
            result["caused"] = [f for f in pending if failure_key(f) not in at_base]
            result["unresolved"] = []
        except (OSError, subprocess.SubprocessError, RuntimeError) as error:
            result["error"] = str(error)
            print(f"Lane impact: unresolved base attribution: {error}", file=sys.stderr)
        finally:
            try:
                subprocess.run(["git", "worktree", "remove", "--force", str(work)], cwd=root, capture_output=True, timeout=15)
            except (OSError, subprocess.SubprocessError) as error:
                result["cleanup_error"] = str(error)
    result["status"] = "unresolved" if result["unresolved"] else "caused" if result["caused"] else "inherited"
    save()
    lines = [f"Lane impact: inherited from base {base[:12]} (fails there too): {f['binary']} {f['test']}" for f in result["inherited"]]
    lines += [f"Lane impact: caused by this delivery (passes or is absent at base): {f['binary']} {f['test']}" for f in result["caused"]]
    lines += [f"Lane impact: unresolved at base: {f['binary']} {f['test']}" for f in result["unresolved"]]
    print("\n".join(lines))
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as handle:
            handle.write("\n" + "\n".join(f"- {line}" for line in lines) + "\n")
    return 1 if result["caused"] or result["unresolved"] else 0


def lane_base(root, recorded_ref, inferred):
    """Resolve the recorded attribution base without changing the impact range.

    A record that is missing or not an ancestor of HEAD falls back to the
    inferred base; the returned source names which one was used.
    """
    if not recorded_ref:
        return inferred, "inferred: no record requested"
    found = subprocess.run(["git", "rev-parse", "--verify", "--quiet", f"{recorded_ref}^{{commit}}"], cwd=root, capture_output=True, text=True)
    if found.returncode:
        return inferred, f"inferred: no record at {recorded_ref}"
    recorded = found.stdout.strip()
    if subprocess.run(["git", "merge-base", "--is-ancestor", recorded, "HEAD"], cwd=root).returncode:
        return inferred, f"inferred: recorded start base {recorded[:12]} is not an ancestor of HEAD"
    return recorded, "recorded start base"


# cas-rule: rule-176 scope: "(4) Run tests only for what changed. Plumbing changes get a smoke check, not the full suite."
def lane(root, args):
    """Lane CI phases: plan, own (builds and runs own tests), ring shards and
    attribute (rerun a phase's failures at the base)."""
    out = Path(args.out)
    outputs = {}
    if args.phase == "plan":
        base, source = lane_base(root, args.recorded_base_ref, args.base_sha)
        selection = plan(root, args.base_sha, args.zero_base_ref, args.history, delivery=True)
        selection["attribution_base"] = base
        selection["base_source"] = "live target merge-base"
        selection["attribution_base_source"] = source
        print(f"Lane impact base: {selection.get('base', args.base_sha)} (live target merge-base)")
        print(f"Lane attribution base: {base or args.zero_base_ref} ({source})")
        selection["configuration"] = proof_configuration(root)
        split = lane_split(root, selection)
        write(out, {"plan": selection, "split": split})
        outputs = {"mode": selection["mode"], "build-args": " ".join(split["build"]), "base-source": selection["base_source"]}
        print(f"Lane impact: {selection['mode']} ({'; '.join(selection['reasons']) or 'delivery range'}); own: {split['own']}; ring: {split['ring']}")
        print("Subprocess suites: " + ", ".join(selection.get("subprocess_stems", [])))
        status = 0
    else:
        saved = json.loads(out.read_text())
        selection, split = saved["plan"], saved["split"]
        if args.phase == "attribute":
            receipt = json.loads(out.with_name(f"{args.receipt}-receipt.json").read_text())
            return lane_attribute(root, selection.get("attribution_base", selection["base"]), receipt["failures"], out.with_name(f"{args.receipt}-attribution.json"), receipt.get("configuration"))
        source = ["--archive-file", args.archive, "--workspace-remap", str(root)] if args.phase == "ring" else split["build"]
        receipt = out.with_name(f"{args.phase}-receipt.json")
        status, failed = 0, False
        if args.phase == "ring":
            status, failed = lane_run(root, selection, source, split["ring"], ["--partition", f"count:{args.shard}"], receipt, "lane-ring")
        elif split["own"] and lane_count(root, source, split["own"]):
            status, failed = lane_run(root, selection, source, split["own"], [], receipt, "lane-own")
        else:
            print("Lane impact: no own tests; the ring follows")
        outputs = {"failed": str(failed).lower()}
        if args.phase == "own" and (status == 0 or failed):
            ring = lane_count(root, source, split["ring"])
            shards = min(3, -(-ring // 1500))
            outputs.update({"ring-shards": json.dumps(list(range(1, shards + 1))), "ring-total": str(shards), "ring-tests": str(ring)})
        if failed:
            status = 0
    if args.github_output:
        with open(args.github_output, "a") as handle:
            handle.writelines(f"{key}={value}\n" for key, value in outputs.items())
    return status


def collect_history(destination):
    """Read bounded recent artifacts. History only adds tests or widens scope."""
    records = []
    uncertain = False
    try:
        _, fetch = github_fetcher()
        artifacts = json.loads(fetch("/actions/artifacts?per_page=100"))["artifacts"]
        selected = [a for a in artifacts if a["name"].startswith("ci-test-impact-") and not a["expired"]][:16]
        for artifact in selected:
            # Construct the endpoint from a numeric ID, not artifact-controlled URLs.
            archive = zipfile.ZipFile(io.BytesIO(fetch(f"/actions/artifacts/{int(artifact['id'])}/zip")))
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
    """Unsuccessful-test lines of an ANSI-stripped nextest log: FAIL, TIMEOUT,
    a signal (SIGSEGV, SIGABRT, ...), ABORT and LEAK-FAIL (cas-2e1b). nextest
    0.9.100+ prints a `( 3528/11850)` progress counter after the duration
    (cas-38ac)."""
    failures = []
    for binary, test in re.findall(r"^\s*(?:FAIL|TIMEOUT|SIG[A-Z]+|ABORT|LEAK-FAIL)\s+\[[^\]]*\]\s+(?:\(\s*\d+/\d+\)\s+)?(\S+)\s+(\S+)\s*$", text, re.M):
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
    receipt = {"version": VERSION, "head": plan["head"], "base": plan.get("base"), "role": role, "configuration": plan.get("configuration"), "plan": plan, "selected_test_count": count, "elapsed_seconds": elapsed, "status": status, "failures": failures, "uncertain": status != 0 and not failures, "post_merge_missed": misses, "recall": (len(failures) - len(misses)) / len(failures) if failures and role == "full" else None}
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
    parser.add_argument("--phase", choices=["plan", "own", "ring", "attribute"], default="plan")
    parser.add_argument("--receipt", choices=["own", "ring"], default="own")
    parser.add_argument("--archive", default="lane-impact.tar.zst")
    parser.add_argument("--shard", default="1/1")
    parser.add_argument("--github-output", default="")
    parser.add_argument("--recorded-base-ref", default="")
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
        # An externally executed archive/profile needs its own producer identity;
        # legacy recall-only receipts cannot safely stand in for base proof.
        record(selection, args.log, args.status, args.elapsed, args.out, "full")
        return 0
    selection["configuration"] = proof_configuration(root)
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
