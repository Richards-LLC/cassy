#!/usr/bin/env bash
# Supervisor-only benchmark. Workers may syntax-check this file, never run its
# Rust workloads. Run backgrounded; a complete three-sample pass can take hours.
set -euo pipefail
exec python3 - "$@" <<'PY'
import argparse
import collections
import csv
import datetime
import json
import os
import pathlib
import shlex
import shutil
import statistics
import subprocess
import sys
import time
import tomllib
import uuid

parser = argparse.ArgumentParser(description="Compare cas-a4b1 base and extraction; three independent samples per metric")
parser.add_argument("base_ref")
parser.add_argument("spike_ref")
parser.add_argument("--nightly", action="store_true", help="Also try installed nightly LLVM, frontend-8, and Cranelift")
parser.add_argument("--keep-targets", action="store_true", help="Retain large build targets for compiler profiling")
args = parser.parse_args()

def capture(command, cwd=None):
    return subprocess.check_output(command, cwd=cwd, text=True).strip()

common = pathlib.Path(capture(["git", "rev-parse", "--path-format=absolute", "--git-common-dir"]))
repo = common.parent
if not (repo / "Cargo.toml").is_file():
    raise SystemExit("Run inside the cas-src checkout with a normal shared .git directory")
run_id = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-" + uuid.uuid4().hex[:8]
scratch = repo / ".cas" / "scratch" / ("cas-a4b1-timings-" + run_id)
output = pathlib.Path.home() / ".cas" / "artifacts" / "cas-a4b1" / run_id
scratch.mkdir(parents=True)
output.mkdir(parents=True)
references = {"base": capture(["git", "rev-parse", args.base_ref + "^{commit}"]),
              "spike": capture(["git", "rev-parse", args.spike_ref + "^{commit}"])}
for label, sha in references.items():
    if not capture(["git", "show", sha + ":Cargo.toml"]):
        raise SystemExit(f"{label} is not a workspace checkout")
skips, worktrees, rows, failures = [], {}, [], []
columns = ["ref", "sha", "variant", "measurement", "sample", "seconds", "exit_code", "command", "log", "target_dir"]
tsv_path = output / "timings.tsv"
with tsv_path.open("w") as file:
    csv.writer(file, delimiter="\t").writerow(columns)

# Resolve the repository's native-target flags explicitly before adding nightly
# options: setting RUSTFLAGS alone would otherwise discard its mold/CPU flags.
rust_version = capture(["rustc", "-vV"])
host = next(line.split(": ", 1)[1] for line in rust_version.splitlines() if line.startswith("host: "))
config_path = repo / ".cargo" / "config.toml"
config = tomllib.loads(config_path.read_text()) if config_path.is_file() else {}
if "CARGO_ENCODED_RUSTFLAGS" in os.environ:
    native_flags = os.environ["CARGO_ENCODED_RUSTFLAGS"].split("\x1f") if os.environ["CARGO_ENCODED_RUSTFLAGS"] else []
elif "RUSTFLAGS" in os.environ:
    native_flags = shlex.split(os.environ["RUSTFLAGS"])
else:
    native_flags = config.get("target", {}).get(host, {}).get("rustflags", config.get("build", {}).get("rustflags", []))
    if isinstance(native_flags, str):
        native_flags = shlex.split(native_flags)
variants = [("default-llvm", ["cargo"], [], {})]
if args.nightly:
    installed = capture(["rustup", "toolchain", "list"]) if shutil.which("rustup") else ""
    nightly = next((line.split()[0] for line in installed.splitlines()
                    if line.split()[0] in {"nightly", "nightly-" + host}), None)
    if nightly is None:
        skips.append("nightly: toolchain missing; no downloads attempted")
    else:
        probe = subprocess.run(["rustup", "run", nightly, "rustc", "-Zthreads=8", "--version"], capture_output=True)
        frontend = "-Zthreads=8"
        if probe.returncode:
            probe = subprocess.run(["rustup", "run", nightly, "rustc", "--jobs-frontend=8", "--version"], capture_output=True)
            frontend = "--jobs-frontend=8"
        variants.append(("nightly-llvm", ["cargo", "+" + nightly], [], {}))
        if not probe.returncode:
            variants.append(("nightly-frontend8", ["cargo", "+" + nightly], [frontend], {}))
        else:
            skips.append("nightly frontend-8: neither -Zthreads=8 nor --jobs-frontend=8 is supported")
        components = capture(["rustup", "component", "list", "--toolchain", nightly, "--installed"])
        if "rustc-codegen-cranelift" in components:
            variants.append(("nightly-cranelift", ["cargo", "+" + nightly, "-Zcodegen-backend"], [], {
                "CARGO_PROFILE_DEV_CODEGEN_BACKEND": "cranelift", "CARGO_PROFILE_TEST_CODEGEN_BACKEND": "cranelift"}))
        else:
            skips.append("Cranelift: installed nightly has no rustc-codegen-cranelift component; no downloads attempted")

metadata = {"base_ref": args.base_ref, "spike_ref": args.spike_ref, "commits": references,
            "run_id": run_id, "rustc": rust_version, "cargo": capture(["cargo", "--version"]),
            "host": host, "jobs": os.environ.get("CARGO_BUILD_JOBS", config.get("build", {}).get("jobs")),
            "rustflags": native_flags, "variants": [variant[0] for variant in variants],
            "variant_commands": {name: {"cargo": command, "extra_flags": flags, "profile_env": profile}
                                 for name, command, flags, profile in variants},
            "sccache": "disabled via empty compiler wrappers", "cold": "fresh Cargo target; OS/download caches are shared",
            "scratch": str(scratch), "skipped": skips}
(output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")

def summary():
    groups = collections.defaultdict(list)
    for row in rows:
        if row["exit_code"] == 0:
            groups[row["ref"], row["variant"], row["measurement"]].append(float(row["seconds"]))
    # Sum matched sample durations before taking a median. Summing medians
    # can manufacture a sample that never existed.
    successful = {(row["ref"], row["variant"], row["measurement"], row["sample"]): float(row["seconds"])
                  for row in rows if row["exit_code"] == 0}
    equivalents = {"cold_check_tests": "moved_tests_check_companion",
                   "incremental_check_split": "incremental_check_split_companion",
                   "incremental_check_core": "incremental_check_core_companion",
                   "test_no_run_after_split_edit": "moved_tests_build_companion"}
    for (label, variant, metric, sample), duration in successful.items():
        if metric not in equivalents:
            continue
        companion = successful.get((label, variant, equivalents[metric], sample))
        if label == "base" or companion is not None:
            groups[label, variant, metric + "_equivalent_coverage"].append(duration + (companion or 0))
    text = ["# Crate split timings", "", f"Base: {references['base']}", f"Spike: {references['spike']}", "",
            "Cold means an empty Cargo target, not a cold filesystem or empty dependency download cache.",
            "Three independent targets are used per ref/variant. Order alternates by sample.",
            "Each incremental row follows a private function-body edit; no public API changes are injected.",
            "Root-only checks/test builds exclude moved dependency unit tests. Companion rows compile them; compare the sum for equivalent coverage.", "",
            "| Ref | Variant | Measurement | Successful samples | Median seconds |",
            "| --- | --- | --- | ---: | ---: |"]
    for (label, variant, metric), values in sorted(groups.items()):
        median = f"{statistics.median(values):.3f}" if len(values) == 3 else "INCOMPLETE"
        text.append(f"| {label} | {variant} | {metric} | {len(values)} | {median} |")
    text += ["", "Skipped: " + ("; ".join(skips) or "none"), "Failures: " + ("; ".join(failures) or "none"),
             "", f"Raw TSV: {tsv_path}", "Cargo output and per-command --timings HTML are under this run's logs/timings directories.",
             "Default builds remain on their existing LLVM toolchain; nightly alternatives are measurement-only."]
    (output / "summary.md").write_text("\n".join(text) + "\n")

def measure(label, variant, sample, metric, command, cwd, target, env):
    log = output / "logs" / f"{label}-{variant}-{sample}-{metric}.log"
    log.parent.mkdir(exist_ok=True)
    started = time.perf_counter()
    with log.open("wb") as file:
        result = subprocess.run(command, cwd=cwd, env=env, stdout=file, stderr=subprocess.STDOUT)
    elapsed = time.perf_counter() - started
    row = dict(zip(columns, [label, references[label], variant, metric, sample, f"{elapsed:.6f}", result.returncode,
                            shlex.join(command), str(log), str(target)]))
    rows.append(row)
    with tsv_path.open("a") as file:
        csv.DictWriter(file, fieldnames=columns, delimiter="\t").writerow(row)
    timings = target / "cargo-timings"
    if timings.exists():
        shutil.copytree(timings, output / "timings" / log.stem, dirs_exist_ok=True)
    summary()
    print(f"{label} {variant} sample={sample} {metric}: {elapsed:.3f}s exit={result.returncode}", flush=True)
    if result.returncode:
        failures.append(f"{log.stem}: exit {result.returncode}; see {log}")
        summary()
        raise RuntimeError(f"measurement failed: {log}")

try:
    for label, sha in references.items():
        worktree = scratch / label
        subprocess.run(["git", "worktree", "add", "--detach", str(worktree), sha], cwd=repo, check=True)
        worktrees[label] = worktree
    for variant, cargo, flags, extra_env in variants:
        for sample in range(1, 4):
            for label in (["base", "spike"] if sample % 2 else ["spike", "base"]):
                worktree = worktrees[label]
                extracted = (worktree / "crates/cas-hub-state/src/runtime.rs").is_file()
                split_file = worktree / ("crates/cas-hub-state/src/runtime.rs" if extracted else "cas-cli/src/hub/runtime.rs")
                core_file = worktree / "cas-cli/src/agent_id.rs"
                originals = {path: path.read_text() for path in [split_file, core_file]}
                target = scratch / f"target-{label}-{variant}-{sample}"
                target.mkdir()
                env = dict(os.environ, CARGO_TARGET_DIR=str(target), RUSTC_WRAPPER="", RUSTC_WORKSPACE_WRAPPER="")
                env.pop("RUSTFLAGS", None)
                env.pop("CARGO_PROFILE_DEV_CODEGEN_BACKEND", None)
                env.pop("CARGO_PROFILE_TEST_CODEGEN_BACKEND", None)
                env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(native_flags + flags)
                env.update(extra_env)
                def edit(path, value):
                    path.write_text(originals[path] + f"\n#[allow(dead_code)]\nfn cas_a4b1_body_edit() -> usize {{ {value} }}\n")
                def run(metric, *tail):
                    measure(label, variant, sample, metric, cargo + list(tail) + ["--locked", "--timings"], worktree, target, env)
                try:
                    run("cold_check_tests", "check", "-p", "cas", "--tests")
                    if extracted:
                        run("moved_tests_check_companion", "check", "-p", "cas-hub-state", "--tests")
                    edit(split_file, 1)
                    run("incremental_check_split", "check", "-p", "cas", "--tests")
                    if extracted:
                        run("incremental_check_split_companion", "check", "-p", "cas-hub-state", "--tests")
                    edit(core_file, 1)
                    run("incremental_check_core", "check", "-p", "cas", "--tests")
                    if extracted:
                        run("incremental_check_core_companion", "check", "-p", "cas-hub-state", "--tests")
                    run("build", "build", "-p", "cas")
                    edit(split_file, 2)
                    run("incremental_build_split", "build", "-p", "cas")
                    edit(core_file, 2)
                    run("incremental_build_core", "build", "-p", "cas")
                    edit(split_file, 3)
                    run("test_no_run_after_split_edit", "test", "--no-run", "-p", "cas")
                    if extracted:
                        run("moved_tests_build_companion", "test", "--no-run", "-p", "cas-hub-state")
                finally:
                    for path, text in originals.items():
                        path.write_text(text)
                    if not args.keep_targets:
                        shutil.rmtree(target)
finally:
    summary()
    for label, worktree in worktrees.items():
        result = subprocess.run(["git", "worktree", "remove", "--force", str(worktree)], cwd=repo)
        if result.returncode:
            print(f"Worktree cleanup failed; inspect {worktree}", file=sys.stderr)
    print(f"Results: {output}", flush=True)
PY
