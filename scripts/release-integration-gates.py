#!/usr/bin/env python3
"""No-build release rows for the rolling integration tip and train preflight."""
import argparse
import csv
import json
import os
from pathlib import Path
import re
import subprocess
import sys

FAST_ROWS = (
    "failure-log", "version-literals", "changelog-and-versions", "release-script",
    "release-notes-shell-injection", "procedure-guardrails", "test-targets",
    "markdown-lint", "test-shape", "test-env", "builtin-doc-hygiene",
    "journey-catalog", "builtin-skill-limits", "doctor-snapshot", "migration-registry",
    "ci-script-tests-changed",
)
REQUIRED_ROWS = (*FAST_ROWS, "ci-script-tests")


def blockers(receipt):
    proof = receipt.get("no_build") or {}
    if proof.get("tip") != receipt.get("tip") or not receipt.get("tip"):
        return ["integration-no-build-tip"]
    rows = proof.get("rows") or {}
    return ["integration-" + row for row in REQUIRED_ROWS if rows.get(row) != "PASS"]


def run(root, base, output):
    root, output = Path(root).resolve(), Path(output).resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    tip = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    proof = {"tip": tip, "rows": {row: "FAIL" for row in REQUIRED_ROWS}}
    # Invalidate before spawning anything, including failed retries.
    output.write_text(json.dumps(proof) + "\n")
    env = dict(os.environ)
    for key in tuple(env):
        if key.startswith(("CAS_RELEASE_TRAIN_", "CAS_RELEASE_GATE_", "RELEASE_GATE_")):
            env.pop(key)
    env.update(CAS_RELEASE_TRAIN_INVOCATION_KIND="internal",
               CAS_RELEASE_TRAIN_RUN_DIR=str(output.parent), CAS_RELEASE_TRAIN_STAGE="integration",
               CAS_RELEASE_TRAIN_FUTURE_CONTROL="integration-regression")
    # Export every production train knob, so the script tier continuously
    # exercises the namespace scrub with the full control set, including new ones.
    sources = [root / "scripts/release-train.sh", *(root / "scripts/release-train.d").glob("*.sh")]
    for source in sources:
        if source.is_file():
            for name in re.findall(r"\bCAS_RELEASE_TRAIN_[A-Z_0-9]+\b", source.read_text()):
                env.setdefault(name, "integration-control")
    version = re.search(r'^version\s*=\s*"([^"]+)"',
                        (root / "cas-cli/Cargo.toml").read_text(), re.M)
    if not version:
        raise ValueError("cannot read integration version")
    gate = root / "scripts/release-gate.sh"
    for mode, command, rows in (
        ("fast", ["--fast-rows", "--base", base], FAST_ROWS),
        ("scripts", [version[1], "--only", "ci-script-tests"], ("ci-script-tests",)),
    ):
        logs = output.parent / (output.stem + "-" + mode)
        logs.mkdir(parents=True, exist_ok=True)
        timing = logs / "timing.tsv"
        timing.unlink(missing_ok=True)
        child_env = dict(env, CAS_RELEASE_GATE_LOG_DIR=str(logs))
        with (logs / "gate.log").open("w") as log:
            result = subprocess.run(["bash", str(gate), *command], cwd=root,
                                    env=child_env, stdout=log, stderr=subprocess.STDOUT)
        seen = {}
        if timing.exists():
            with timing.open() as stream:
                seen = {entry["row"]: entry["status"] for entry in csv.DictReader(stream, delimiter="\t")}
        for row in rows:
            if seen.get(row) == "0":
                proof["rows"][row] = "PASS"
        # A failed gate without a failed row cannot manufacture a green receipt.
        if result.returncode and all(proof["rows"][row] == "PASS" for row in rows):
            proof["rows"][rows[0]] = "FAIL"
        output.write_text(json.dumps(proof) + "\n")
    failed = [row for row, status in proof["rows"].items() if status != "PASS"]
    print("integration no-build " + ("FAIL " + ",".join(failed) if failed else "PASS")
          + ": " + str(output))
    return bool(failed)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fast-rows", action="store_true")
    parser.add_argument("--check", type=Path)
    parser.add_argument("--run", nargs=3, metavar=("ROOT", "BASE", "OUTPUT"))
    args = parser.parse_args()
    if args.fast_rows:
        print(",".join(FAST_ROWS))
        return 0
    if args.check:
        receipt = json.loads(args.check.read_text())
        failed = blockers(receipt)
        if receipt.get("status") != "PASSED":
            failed.append("integration-receipt-status")
        for name in failed:
            print(name)
        return bool(failed)
    if args.run:
        return run(*args.run)
    parser.error("choose --fast-rows, --check or --run")


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ValueError, OSError, KeyError, subprocess.SubprocessError) as error:
        print("integration-no-build-receipt: " + str(error), file=sys.stderr)
        sys.exit(1)
