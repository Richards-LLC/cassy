#!/usr/bin/env bash
# Fixture-driven self-test for scripts/release-gate.sh.
#
# The fixtures deliberately keep Cargo and nextest fake: the gate's contract is
# about dispatching every release check and failing closed with a named reason,
# not about spending a release's build time in its own unit test.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/release-portable.sh
source "$script_dir/release-portable.sh"
release_portable_define_sha256sum
gate="${RELEASE_GATE_TEST_GATE:-$script_dir/release-gate.sh}"
# cas-db34: the physical spelling. macOS's $TMPDIR is under /var, a symlink
# to /private/var, and the gate reports the resolved checkout path; fixtures
# compare against that spelling.
tmp="$(cd "$(mktemp -d)" && pwd -P)"
trap 'rm -rf "$tmp"' EXIT

# The gate refuses any scratch base with a .cas ancestor. Its default used to be
# $HOME/.cache/cas-release-gate — which on a developer machine sits under the
# user-level ~/.cas — so unset, this self-test died mid-run on the two rows that
# build a scratch base, printing no summary and reading as a broken script
# rather than the host condition it is (cas-4ccc). cas-c736 moved the same
# default into release-gate.sh itself, so this line is now belt-and-braces
# rather than a prerequisite; it is kept so the harness is deterministic even
# when an operator has the variable exported to somewhere else. The
# default-scratch-base fixture below deliberately runs with it UNSET.
: "${CAS_RELEASE_GATE_HOME_DIR:=$tmp/gate-scratch/base}"
export CAS_RELEASE_GATE_HOME_DIR
mkdir -p "$CAS_RELEASE_GATE_HOME_DIR"

# Fail loudly rather than silently reintroducing the same class: if the chosen
# base has a .cas ancestor, every scratch row would refuse and the reader would
# be back to debugging the gate instead of the release.
probe="$CAS_RELEASE_GATE_HOME_DIR"
while [[ "$probe" != "/" && -n "$probe" ]]; do
    if [[ -d "$probe/.cas" ]]; then
        printf 'CAS_RELEASE_GATE_HOME_DIR=%s has a .cas ancestor at %s; pick a path with none\n' \
            "$CAS_RELEASE_GATE_HOME_DIR" "$probe/.cas" >&2
        exit 1
    fi
    probe="$(dirname "$probe")"
done
unset probe

pass=0
fail=0

ok() { printf 'ok   %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL %s\n' "$1"; fail=$((fail + 1)); }

new_fixture() {
    local name="$1" repo
    repo="$tmp/$name"
    mkdir -p "$repo/scripts" "$repo/hub-web/scripts" "$repo/cas-cli/src" "$repo/cas-cli/tests" "$repo/crates" \
        "$repo/.github/workflows" \
        "$repo/.context/zig"
    cp "$gate" "$repo/scripts/release-gate.sh"
    cp "$script_dir/assembly-proof.py" "$repo/scripts/assembly-proof.py"
    cp "$script_dir/proof_target.py" "$repo/scripts/proof_target.py"
    cp "$script_dir/assembly-memory.py" "$repo/scripts/assembly-memory.py"
    cp "$script_dir/host_memory.py" "$repo/scripts/host_memory.py"
    # Only copied fixture code selects a private pool. Keep real locking and
    # inherited-lease validation in subprocesses and clones; production has no
    # environment knob that redirects its host/user admission directory.
    python3 - "$repo/scripts/host_memory.py" "$tmp/host-memory" <<'PY_HOST_MEMORY_FIXTURE' || return 1
from pathlib import Path
import sys
path, pool = map(Path, sys.argv[1:])
body = path.read_text()
selector = "DIRECTORY = Path('/var/tmp') / f'cas-host-memory-{os.getuid()}'"
if body.count(selector) != 1:
    raise SystemExit('host memory fixture selector changed; refusing production pool')
path.write_text(body.replace(selector, f'DIRECTORY = Path({str(pool)!r})'))
PY_HOST_MEMORY_FIXTURE
    # The producer and its guard share deterministic physical-memory fixtures.
    python3 - "$repo/scripts/assembly-proof.py" <<'PY_MEMORY_GUARD_FIXTURE'
from pathlib import Path
import sys
path = Path(sys.argv[1])
path.write_text(path.read_text().replace("def memory_snapshot():", "def memory_snapshot():\n    return {'total_bytes': 64 * GIB, 'available_bytes': 60 * GIB, 'source': 'fixture'}"))
PY_MEMORY_GUARD_FIXTURE
    cp "$script_dir/release_scratch.py" "$repo/scripts/release_scratch.py"
    # Sweeper integration has isolated real-filesystem regressions; these gate
    # fixtures must never reclaim the host's production scratch.
    python3 - "$repo/scripts/release_scratch.py" <<'PY_SCRATCH_SWEEP_FIXTURE'
from pathlib import Path
import sys
path = Path(sys.argv[1])
path.write_text(path.read_text().replace("def sweep(repo, base, clean=False, env=None):", "def sweep(repo, base, clean=False, env=None):\n    return {'entries': [], 'reclaimable_bytes': 0, 'reclaimed_bytes': 0}"))
PY_SCRATCH_SWEEP_FIXTURE
    # Cargo is fake here: bypass only durable-location classification in the
    # copied producer. Production guard behavior has its own Python regressions.
    python3 - "$repo/scripts/assembly-proof.py" <<'PY_SCRATCH'
from pathlib import Path
import sys
path = Path(sys.argv[1])
path.write_text(path.read_text().replace("scratch = clone_scratch(os.environ)",
    "scratch = Path(os.environ['CAS_RELEASE_GATE_HOME_DIR']).resolve()").replace(
    "snapshot = memory_snapshot()",
    "snapshot = {'total_bytes': 64 * GIB, 'available_bytes': 60 * GIB, 'source': 'fixture'}"))
PY_SCRATCH
    cp "$script_dir/release-portable.sh" "$repo/scripts/release-portable.sh"
    cp "$script_dir/release-test-env.sh" "$script_dir/release-integration-gates.py" "$repo/scripts/"
    for helper in test-check-portable-x86_64-isa check-portable-x86_64-isa check-portable-x86_64-dependencies check-blake3-no-avx512-build; do
        cp "$script_dir/$helper.sh" "$repo/scripts/$helper.sh"
    done
    # Real defects in the new rows are covered by test-fast-release-rows.py.
    for helper in cas-test-targets check-changed-markdown check-test-shape check-test-env check-builtin-doc-hygiene check-builtin-contract-phrases \
        journeys-for-diff check-builtin-skill-limits check-doctor-snapshot check-migration-registry ci-script-tests-for-diff; do
        printf '#!/usr/bin/env python3\n' >"$repo/scripts/$helper.py"
    done
    cat >"$repo/cas-cli/Makefile" <<'EOF'
.PHONY: test-ci-tiers
test-ci-tiers:
	cd .. && python3 scripts/ci-script-fixture.py
EOF
    cat >"$repo/scripts/ci-script-fixture.py" <<'EOF'
import os
import json
import subprocess
import tomllib
import unittest

class ScriptTier(unittest.TestCase):
    def test_nested_gate_receipts(self):
        if os.environ.get("GATE_FIXTURE_NESTED_GATE_TEST") != "1":
            return
        # A deliberately failing nested row is an expected self-test result.
        # It must not overwrite the outer row's timing or receipt destinations.
        with open("cas-cli/Cargo.toml", "rb") as manifest:
            version = tomllib.load(manifest)["package"]["version"]
        result = subprocess.run(["bash", "scripts/release-gate.sh", version,
                                 "--only", "hub-web-tests"],
                                env=dict(os.environ, NPM="/usr/bin/false"),
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("FAIL hub-web-tests", result.stdout)
        with open(os.environ["GATE_FIXTURE_NESTED_ENV_FILE"], "w") as stream:
            json.dump({key: value for key, value in os.environ.items()
                       if key.startswith("CAS_RELEASE_GATE_") or key in
                       ("CAS_RELEASE_ARTIFACTS_ROOT", "CAS_RELEASE_RECEIPTS_RUN_DIR",
                        "VERIFIED_TEST_COUNT_FILE", "VERIFIED_TEST_LOG")}, stream)

    def test_train_controls_absent(self):
        self.assertFalse([key for key in os.environ if key.startswith(("CAS_RELEASE_TRAIN_", "CAS_RELEASE_GATE_"))])

    def test_seeded_ci_script_failure(self):
        for key in ("CAS_FACTORY_SESSION", "CAS_AGENT_ROLE", "CAS_AGENT_NAME",
                    "CAS_SUPERVISOR_NAME", "CAS_AGENT_ID", "CAS_SESSION_ID", "CAS_ROOT"):
            self.assertNotIn(key, os.environ)
        self.assertNotEqual(os.environ.get("GATE_FIXTURE_CI_SCRIPT_FAIL"), "1",
                            "seeded script failure before queue admission")

unittest.main()
EOF
    cp "$script_dir/check-workflow-run-interpolation.py" "$repo/scripts/check-workflow-run-interpolation.py"
    cat > "$repo/.github/workflows/release.yml" <<'EOF'
jobs:
  publish:
    steps:
      - env:
          NOTES: ${{ steps.notes.outputs.notes }}
        run: echo "$NOTES"
EOF
    cp "$script_dir/release-integrate.py" "$repo/scripts/release-integrate.py"
    cp "$script_dir/release-train.sh" "$repo/scripts/release-train.sh"
    cp "$script_dir/release-learning.py" "$repo/scripts/release-learning.py"
    cat >"$repo/scripts/check-release-publish-toolchain.py" <<'PY_ZIG_FIXTURE'
import os, sys
if os.environ.get('GATE_FIXTURE_ZIGBUILD_FAIL') == '1':
    print('zigbuild rejects build.jobs config')
    sys.exit(1)
print('zigbuild config parsed (gate dispatch fixture)')
PY_ZIG_FIXTURE

    cp -R "$script_dir/release-train.d" "$repo/scripts/"
    cp "$script_dir/test-release-integration.py" "$repo/scripts/test-release-integration.py"
    # The nested integration fixtures have their own release version. Keep it
    # distinct from this gate's 9.99.7 so source-literal checks stay meaningful.
    python3 - "$repo/scripts/test-release-integration.py" <<'PY_NESTED_VERSION'
from pathlib import Path
import sys
path = Path(sys.argv[1])
path.write_text(path.read_text().replace('9.99.7', '9.98.7'))
PY_NESTED_VERSION
    cp "$script_dir/run-verified-tests.sh" "$repo/scripts/run-verified-tests.sh"
cat >"$repo/.gitignore" <<'EOF'
.context/zig/
.cas/
target/
__pycache__/
EOF
    printf '%s\n' '#!/usr/bin/env bash' 'echo fixture-zig-1.0' >"$repo/.context/zig/zig"
    chmod +x "$repo/.context/zig/zig"
    cat >"$repo/Cargo.toml" <<'EOF'
[workspace]
members = ["cas-cli", "crates/cas-types", "crates/cas-search", "crates/cas-store", "crates/cas-core", "crates/cas-mcp"]
EOF
    cat >"$repo/scripts/gen-builtin-reference-history.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${GATE_FIXTURE_REFERENCE_FAIL:-}" == 1 ]]; then
  printf 'changed ledger\n' > cas-cli/src/builtins/reference-history.json
else
  mkdir -p cas-cli/src/builtins
  touch cas-cli/src/builtins/reference-history.json
fi
EOF
    chmod +x "$repo/scripts"/*.sh
    mkdir -p "$repo/cas-cli/src/builtins"
    : >"$repo/cas-cli/src/builtins/reference-history.json"
    for mirror in \
        "$repo/cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md"; do
        mkdir -p "$(dirname "$mirror")"
        printf '%s\n' '- 2026-09-02 — **version-literals** — Symptom: fixture source literal. Root cause: fixture. Release: fixture.' >"$mirror"
    done
    cat >"$repo/cas-cli/src/builtins/skills/cas-cut-release/SKILL.md" <<'EOF'
Use when cutting a release.
Run the full suite on the assembled tree. Use nohup, kill -0, stranded_branch_override,
release-published-receipt.sh --write-draft, and cas --version.
Require Scoped Validation; the ledger is the last prep step; record a cause class.
Use 9.99.x fixtures; workers never poll CI; commit a reviewed snapshot update.
Check for a competing release with the merge-queue GraphQL query. Read CAS_RELEASE_ENV_FILE.
Require the annotated tag peels, four Slack POSTED receipts, and refresh_binary_version.
Keep release.tag-complete.epoch separate from the verified release-published.receipt.
EOF
    cat >"$repo/scripts/release.sh" <<'EOF'
#!/usr/bin/env bash
./scripts/release.sh                 # local audit only
# Pre-warming rule: in a tag worktree, use only the bare ./scripts/release.sh;
# it is audit-only and remote-safe.
target/$target/release/build"/blake3-*
target/$target/release/.fingerprint"/blake3-*
EOF
    cat >"$repo/cas-cli/src/version.rs" <<'EOF'
// fixture source
EOF
    cat >"$repo/cas-cli/tests/smoke.rs" <<'EOF'
// fixture test
EOF
    for crate in cas-cli cas-types cas-search cas-store cas-core cas-mcp; do
        local file
        if [[ "$crate" == cas-cli ]]; then file="$repo/cas-cli/Cargo.toml"; else file="$repo/crates/$crate/Cargo.toml"; fi
        mkdir -p "$(dirname "$file")"
        printf '[package]\nname = "%s"\nversion = "9.99.7"\n' "$crate" >"$file"
    done
    cat >"$repo/CHANGELOG.md" <<'EOF'
## [Unreleased]

## [9.99.7] - 2026-09-02

- Fixture release.
EOF
    cat >"$repo/Cargo.lock" <<'EOF'
# fixture lockfile
EOF
    cat >"$repo/scripts/cargo-stub" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${GATE_FIXTURE_CARGO_LOG:?}"
if [[ "${GATE_FIXTURE_EXPECT_CLEAN:-}" == 1 ]]; then
  python3 -c 'import os; assert not [k for k in os.environ if k.startswith(("CAS_RELEASE_TRAIN_", "CAS_RELEASE_GATE_"))]'
fi
if [[ "$*" == *'--no-run'* ]]; then printf 'fixture compile stderr\n' >&2; fi
# cas-c0411: every gate child must see the raised `cas init` watchdog budget,
# because the child that hit the 300s default was a test's `cas init`, several
# processes below the gate.
printf 'CAS_INIT_TIMEOUT_SECS=%s :: %s\n' "${CAS_INIT_TIMEOUT_SECS:-unset}" "$*" \
  >>"${GATE_FIXTURE_ENV_LOG:-/dev/null}"
printf 'ZIG=%s :: %s\n' "${ZIG:-unset}" "$*" \
  >>"${GATE_FIXTURE_ZIG_LOG:-/dev/null}"
printf 'INSTA_WORKSPACE_ROOT=%s :: %s\n' "${INSTA_WORKSPACE_ROOT:-unset}" "$*" >>"${GATE_FIXTURE_ARCHIVE_ENV_LOG:-/dev/null}"
printf 'RUSTC_WRAPPER=%s CARGO_HOME=%s :: %s\n' "${RUSTC_WRAPPER:-unset}" "${CARGO_HOME:-unset}" "$*" \
  >>"${GATE_FIXTURE_ARCHIVE_ENV_LOG:-/dev/null}"
printf 'TMPDIR=%s NEXTEST_TEST_THREADS=%s :: %s\n' "${TMPDIR:-unset}" "${NEXTEST_TEST_THREADS:-unset}" "$*" \
  >>"${GATE_FIXTURE_ARCHIVE_ENV_LOG:-/dev/null}"
printf 'CAS_FACTORY_SESSION=%s CAS_AGENT_ROLE=%s CAS_AGENT_NAME=%s CAS_SUPERVISOR_NAME=%s CAS_AGENT_ID=%s :: %s\n' \
  "${CAS_FACTORY_SESSION:-unset}" "${CAS_AGENT_ROLE:-unset}" "${CAS_AGENT_NAME:-unset}" \
  "${CAS_SUPERVISOR_NAME:-unset}" "${CAS_AGENT_ID:-unset}" "$*" \
  >>"${GATE_FIXTURE_FACTORY_ENV_LOG:-/dev/null}"
if [[ "$*" == 'zigbuild --version' ]]; then echo fixture-zigbuild-1.0; fi
if [[ "$*" == 'tree --locked -p cas --target x86_64-unknown-linux-gnu --edges normal,build,features' ]]; then
  printf '%s\n' 'rustls feature "ring"' 'blake3 feature "no_avx512"' 'blake3 v1.8.6 (/repo/vendor/blake3-1.8.6)'
fi
if [[ "$*" == 'zigbuild -p cas --release --target x86_64-unknown-linux-gnu --locked' ]]; then
  [[ "$(command -v zig)" == "${ZIG:?}" ]] || {
    echo 'release binary fixture: selected Zig is not on PATH' >&2; exit 1;
  }
  # run_gate captures the caller's flags for the ISA controls. Older manual
  # fixture invocations test other rows and do not supply these expectations.
  if [[ -n "${GATE_FIXTURE_ISA_ORIGINAL_ENCODED+x}" ]]; then
    [[ "${CARGO_ENCODED_RUSTFLAGS-__unset__}" == "$GATE_FIXTURE_ISA_ORIGINAL_ENCODED" \
        && "${RUSTFLAGS-__unset__}" == "$GATE_FIXTURE_ISA_ORIGINAL_RUSTFLAGS" ]] || {
    echo 'release binary fixture: publisher rustflags/linker were changed' >&2; exit 1;
    }
  fi
  [[ "${CFLAGS_x86_64_unknown_linux_gnu:-}" == -march=x86_64 && "${CXXFLAGS_x86_64_unknown_linux_gnu:-}" == -march=x86_64 ]] || {
    echo 'release binary fixture: missing baseline C/C++ flags' >&2; exit 1;
  }
  if [[ "${GATE_FIXTURE_ISA_BUILD_FAIL:-}" == 1 ]]; then exit 1; fi
  destination="${CARGO_TARGET_DIR:-target}/x86_64-unknown-linux-gnu/release"
  mkdir -p "$destination"
  cat >"$destination/fixture.S" <<'ASM'
.text
.globl main
.type main, @function
main:
  xor %eax, %eax
  ret
.section .note.GNU-stack,"",@progbits
ASM
  if [[ "${GATE_FIXTURE_ISA_EVEX:-}" == 1 ]] || grep -qF 'name = "aes"' Cargo.lock; then
    sed 's/xor %eax, %eax/.byte 0x62, 0xf1, 0xff, 0x08, 0x78, 0xc8/' "$destination/fixture.S" >"$destination/seeded.S"
    mv "$destination/seeded.S" "$destination/fixture.S"
  fi
  source "$(dirname "$0")/release-portable.sh"
  release_portable_x86_64_linux_cc
  "${RELEASE_PORTABLE_X86_64_CC[@]}" "$destination/fixture.S" -o "$destination/cas"
  if [[ "${GATE_FIXTURE_ISA_MISSING:-}" == 1 ]]; then rm "$destination/cas"; fi
fi
if [[ "$*" == 'check --workspace --tests' && "${GATE_FIXTURE_CHECK_FAIL:-}" == 1 ]]; then exit 1; fi
if [[ "$*" == 'check --workspace --tests --target aarch64-apple-darwin' && "${GATE_FIXTURE_MACOS_FAIL:-}" == 1 ]]; then exit 1; fi
if [[ "$*" == 'check --workspace --tests --target aarch64-apple-darwin' ]]; then
  [[ -z "${RUSTC_WRAPPER:-}" ]] || {
    printf 'macOS fixture: target check must clear RUSTC_WRAPPER\n' >&2
    exit 1
  }
  [[ -x "${CC_aarch64_apple_darwin:-}" ]] || {
    printf 'macOS fixture: target CC shim is missing\n' >&2
    exit 1
  }
  printf '%s\n' 'int cas_release_gate_fixture(void) { return 0; }' |
    "$CC_aarch64_apple_darwin" -arch arm64 -mmacosx-version-min=11.0 \
    -x c -c -o "${GATE_FIXTURE_CC_OBJECT:?}" -
  [[ -s "${GATE_FIXTURE_CC_OBJECT:?}" ]] || {
    printf 'macOS fixture: target CC shim produced no object\n' >&2
    exit 1
  }
fi
if [[ "$*" == 'nextest run --workspace'* && "${GATE_FIXTURE_NEXTEST_FAIL:-}" == 1 ]]; then exit 1; fi
if [[ "$*" == *'builtin_archive_portability_test'* && "${GATE_FIXTURE_FIXTURE_PATHS_FAIL:-}" == 1 ]]; then exit 1; fi
if [[ "$*" == 'test -p cas --doc' && "${GATE_FIXTURE_DOCTEST_FAIL:-}" == 1 ]]; then exit 1; fi
if [[ "${GATE_FIXTURE_SNAPSHOT_FAIL:-}" == 1 ]]; then
  case "$*" in *component_output_test*) exit 1;; esac
fi
if [[ "${GATE_FIXTURE_DRIFT_FAIL:-}" == 1 ]]; then
  case "$*" in *builtin_flavor_drift_test*) exit 1;; esac
fi
if [[ "$*" == 'nextest archive --workspace'* ]]; then
  archive_file=''
  for arg in "$@"; do [[ "$arg" == *.tar.zst ]] && archive_file="$arg"; done
  [[ -n "$archive_file" ]] && printf archive >"$archive_file"
  exit 0
fi
if [[ "$*" == 'test -p cas --doc' && "${GATE_FIXTURE_EMPTY_SUITE:-}" != 1 ]]; then
  printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n'
fi
if [[ "$*" == 'nextest run '* && "${GATE_FIXTURE_EMPTY_SUITE:-}" != 1 ]]; then
  printf 'Summary [0.001s] 1 test run: 1 passed, 0 skipped\n'
fi
if [[ "$*" == 'nextest run --archive-file '* ]]; then
  extract_to='' previous=''
  for arg in "$@"; do
    [[ "$previous" != --extract-to ]] || extract_to="$arg"
    previous="$arg"
  done
  python3 - "$extract_to" <<'PY_ARCHIVE_EXTRACT'
from pathlib import Path
import stat
import sys
assert sys.argv[1], 'archive fixture: --extract-to is missing'
extract = Path(sys.argv[1]).resolve(strict=True)
assert extract.is_dir(), 'archive fixture: extraction destination is not a directory'
destination, base = extract.stat(), extract.parent.stat()
assert (destination.st_uid, destination.st_gid) == (base.st_uid, base.st_gid)
assert stat.S_IMODE(destination.st_mode) == stat.S_IMODE(base.st_mode) == 0o700
(extract / 'fixture-extracted-test').write_text('extracted archive fixture\n')
PY_ARCHIVE_EXTRACT
  printf 'EXTRACT_DIR_READY=%s :: %s\n' "$extract_to" "$*" \
    >>"${GATE_FIXTURE_ARCHIVE_ENV_LOG:-/dev/null}"
  [[ "${RUSTC_WRAPPER:-}" == /nonexistent/sccache ]] || { printf 'archive fixture: wrapper=%s\n' "${RUSTC_WRAPPER:-unset}" >&2; exit 1; }
  [[ -d "${CARGO_HOME:-}" ]] || { printf 'archive fixture: CARGO_HOME is not a directory: %s\n' "${CARGO_HOME:-unset}" >&2; exit 1; }
  [[ -z "$(find "$CARGO_HOME" -mindepth 1 -print -quit)" ]] || { printf 'archive fixture: CARGO_HOME is not empty: %s\n' "$CARGO_HOME" >&2; exit 1; }
  if [[ "${GATE_FIXTURE_ARCHIVE_FAIL:-}" == 1 ]]; then exit 1; fi
fi
EOF
    chmod +x "$repo/scripts/cargo-stub"
    printf '%s\n' '#!/usr/bin/env bash' 'echo fixture-zigbuild-1.0' >"$repo/scripts/cargo-zigbuild"
    chmod +x "$repo/scripts/cargo-zigbuild"
    cat >"$repo/scripts/rustup-stub" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${GATE_FIXTURE_RUSTUP_LOG:?}"
if [[ "$*" != 'target add aarch64-apple-darwin' ]]; then
  printf 'unexpected rustup invocation: %s\n' "$*" >&2
  exit 1
fi
if [[ "${GATE_FIXTURE_RUSTUP_FAIL:-}" == 1 ]]; then
  printf 'rustup fixture: target installation failed\n' >&2
  exit 1
fi
EOF
    chmod +x "$repo/scripts/rustup-stub"
    cat >"$repo/scripts/hub-web-visual-qa-stub" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "${1:-missing-artifact-dir}" >>"${GATE_FIXTURE_VISUAL_QA_LOG:?}"
if [[ "${GATE_FIXTURE_HUB_WEB_VISUAL_QA_FAIL:-}" == 1 ]]; then exit 1; fi
EOF
    chmod +x "$repo/scripts/hub-web-visual-qa-stub"
    git -C "$repo" init -q
    git -C "$repo" config user.email release-gate@example.test
    git -C "$repo" config user.name release-gate-test
    git -C "$repo" add .
    git -C "$repo" commit -qm 'release gate fixture'
    git -C "$repo" branch -M epic/release-gate-fixture
    printf '%s' "$repo"
}

run_gate() {
    local repo="$1" failure_variable="${2:-}"
    shift 2
    if [[ -n "$failure_variable" ]]; then
        (cd "$repo" && \
          env -u ZIG -u CAS_RELEASE_EPIC_REF -u CAS_RELEASE_TRAIN_BRANCH \
          "$failure_variable=1" \
          GATE_FIXTURE_CARGO_LOG="$tmp/cargo.log" \
          PATH="$repo/scripts:$PATH" \
          GATE_FIXTURE_ISA_ORIGINAL_ENCODED="${CARGO_ENCODED_RUSTFLAGS-__unset__}" \
          GATE_FIXTURE_ISA_ORIGINAL_RUSTFLAGS="${RUSTFLAGS-__unset__}" \
          GATE_FIXTURE_RUSTUP_LOG="$tmp/rustup.log" \
          GATE_FIXTURE_CC_OBJECT="$tmp/macos-check.o" \
          CARGO="$repo/scripts/cargo-stub" \
          RUSTUP="$repo/scripts/rustup-stub" \
          GATE_FIXTURE_VISUAL_QA_LOG="$tmp/visual-qa.log" \
          RELEASE_GATE_HUB_WEB_VISUAL_QA="$repo/scripts/hub-web-visual-qa-stub" \
          RELEASE_GATE_GEN_REFERENCE_HISTORY="$repo/scripts/gen-builtin-reference-history.sh" \
          "$@")
    else
        (cd "$repo" && \
          env -u ZIG -u CAS_RELEASE_EPIC_REF -u CAS_RELEASE_TRAIN_BRANCH \
          GATE_FIXTURE_CARGO_LOG="$tmp/cargo.log" \
          PATH="$repo/scripts:$PATH" \
          GATE_FIXTURE_ISA_ORIGINAL_ENCODED="${CARGO_ENCODED_RUSTFLAGS-__unset__}" \
          GATE_FIXTURE_ISA_ORIGINAL_RUSTFLAGS="${RUSTFLAGS-__unset__}" \
          GATE_FIXTURE_RUSTUP_LOG="$tmp/rustup.log" \
          GATE_FIXTURE_CC_OBJECT="$tmp/macos-check.o" \
          CARGO="$repo/scripts/cargo-stub" \
          RUSTUP="$repo/scripts/rustup-stub" \
          GATE_FIXTURE_VISUAL_QA_LOG="$tmp/visual-qa.log" \
          RELEASE_GATE_HUB_WEB_VISUAL_QA="$repo/scripts/hub-web-visual-qa-stub" \
          RELEASE_GATE_GEN_REFERENCE_HISTORY="$repo/scripts/gen-builtin-reference-history.sh" \
          "$@")
    fi
}

# Export every train control named by production, including future names.
# Fake Cargo/npm run at the real suite boundaries and reject leaked controls.
test_child_environment() (
    local key repo output row
    while IFS= read -r key; do
        export "$key=fixture-control"
    done < <(rg -o --no-filename 'CAS_RELEASE_TRAIN_[A-Z_0-9]+' "$script_dir/release-train.sh" "$script_dir/release-train.d" | sort -u)
    export CAS_RELEASE_TRAIN_FUTURE_CONTROL=fixture CAS_RELEASE_GATE_FUTURE_CONTROL=fixture
    repo="$(new_fixture train-child-env)"
    printf '%s\n' '{"name":"env-fixture","private":true}' >"$repo/hub-web/package.json"
    cat >"$repo/scripts/npm-env-stub" <<'EOF'
#!/usr/bin/env bash
python3 -c 'import os; assert not [k for k in os.environ if k.startswith(("CAS_RELEASE_TRAIN_", "CAS_RELEASE_GATE_"))]'
EOF
    chmod +x "$repo/scripts/npm-env-stub"
    local failed=0
    for row in ci-script-tests nextest doctests archive-mode snapshot-portability builtin-projections hub-web-tests; do
        # Suite-only diagnostics need no branch discovery; pass every train
        # variable through, including BRANCH which run_gate normally removes.
        output="$(cd "$repo" && env -u ZIG \
            GATE_FIXTURE_EXPECT_CLEAN=1 GATE_FIXTURE_CARGO_LOG="$tmp/cargo.log" \
            NPM="$repo/scripts/npm-env-stub" CARGO="$repo/scripts/cargo-stub" \
            PATH="$repo/scripts:$PATH" \
            "$repo/scripts/release-gate.sh" 9.99.7 --only "$row" 2>&1 || true)"
        if grep -qF "PASS $row" <<<"$output"; then
            printf 'ok   %s child environment has no train/gate controls\n' "$row"
        else
            printf 'FAIL %s child environment: %s\n' "$row" "$output"
            failed=$((failed + 1))
        fi
    done
    [[ "$failed" == 0 ]]
)
if [[ "${1:-}" == --test-child-env-only ]]; then
    test_child_environment
    exit $?
fi

assert_named_failure() {
    local name="$1" output="$2"
    if grep -qF "FAIL $name" <<<"$output" && grep -qF "RELEASE GATE FAILED" <<<"$output"; then
        ok "$name fails closed with a named receipt row"
    else
        bad "$name did not produce a named failure (output: $output)"
    fi
}

assert_all_pass() {
    local output="$1"
    for name in scratch-base epic-worktree-fresh epic-worktree-zig failure-log ancestor-proxy-config assemble-stale-base \
        version-literals release-binary-isa fixture-paths workspace-tests macos-check nextest doctests archive-mode snapshot-portability \
        builtin-projections changelog-and-versions release-script release-notes-shell-injection procedure-guardrails working-tree test-targets markdown-lint test-shape test-env ci-script-tests builtin-doc-hygiene \
        journey-catalog builtin-skill-limits doctor-snapshot migration-registry ci-script-tests-changed fixture-paths-src \
        hub-web-tests hub-web-dist-drift hub-web-visual-qa; do
        if ! grep -qF "PASS $name" <<<"$output"; then
            bad "passing fixture omitted PASS $name"
            return
        fi
    done
    if grep -qF 'RELEASE GATE PASSED' <<<"$output"; then
        ok 'clean fixture passes every release gate row'
    else
        bad 'clean fixture did not print a passing gate receipt'
    fi
}

run_scenario() {
    local name="$1" variable="$2" repo output
    repo="$(new_fixture "$name")"
    output="$(run_gate "$repo" "$variable" "$repo/scripts/release-gate.sh" 9.99.7 2>&1 || true)"
    assert_named_failure "$3" "$output"
}

# The new row must audit the staged executable, including code introduced by
# dependencies, before the train can authorize pr-body/pipeline.
repo="$(new_fixture release-binary-isa)"
output="$(run_gate "$repo" GATE_FIXTURE_ISA_EVEX "$repo/scripts/release-gate.sh" 9.99.7 --only release-binary-isa 2>&1 || true)"
if grep -qF 'FAIL release-binary-isa' <<<"$output" \
    && grep -qF 'forbidden EVEX/AVX-512' <<<"$output" \
    && grep -qi 'vcvttsd2usi' <<<"$output"; then
    ok 'release-binary-isa refuses seeded EVEX with the first instruction finding'
else
    bad "release-binary-isa missed the seeded final ELF: $output"
fi
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --only release-binary-isa 2>&1 || true)"
if grep -qF 'PASS release-binary-isa' <<<"$output" \
    && grep -qF 'RELEASE GATE PASSED' <<<"$output"; then
    ok 'release-binary-isa accepts a baseline executable with locked zigbuild and release flags'
else
    bad "release-binary-isa rejected the baseline final ELF: $output"
fi
for control in GATE_FIXTURE_ISA_BUILD_FAIL GATE_FIXTURE_ISA_MISSING; do
    output="$(run_gate "$repo" "$control" "$repo/scripts/release-gate.sh" 9.99.7 --only release-binary-isa 2>&1 || true)"
    assert_named_failure release-binary-isa "$output"
done
isa_target="$tmp/custom-cargo-target"
output="$(CARGO_TARGET_DIR="$isa_target" CARGO_ENCODED_RUSTFLAGS=$'-C\x1ftarget-cpu=x86-64' RUSTFLAGS='-C debuginfo=1' \
    run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --only release-binary-isa 2>&1 || true)"
if grep -qF 'PASS release-binary-isa' <<<"$output" && [[ -f "$isa_target/x86_64-unknown-linux-gnu/release/cas" ]]; then
    ok 'release-binary-isa preserves publisher linker flags, selected Zig and configured target directory'
else
    bad "release-binary-isa ignored the configured target directory: $output"
fi
isa_run="$tmp/isa-learning"
mkdir -p "$isa_run"
printf 'publish\n' >"$isa_run/blockers.log"
output="$(cd "$repo" && "$repo/scripts/release-gate.sh" --learn 'publish ISA audit found seeded EVEX' 'dependency backend introduced AVX-512' release-binary-isa --run-dir "$isa_run" --evidence blockers.log:1 2>&1)"
if grep -qF 'learn=release-binary-isa' "$isa_run/blockers.log" \
    && python3 "$repo/scripts/release-learning.py" --check "$repo" "$isa_run"; then
    ok 'release-binary-isa is an executable learned row for blockers.log'
else
    bad "release-binary-isa could not map the publish rescue: $output"
fi
if [[ "${1:-}" == --release-binary-isa-only ]]; then
    printf '\n%s passed, %s failed\n' "$pass" "$fail"
    test "$fail" -eq 0
    exit
fi

# cas-728e: copied producers must use real admission in a private pool. The
# wait case also proves that isolation did not become an admission bypass.
repo="$(new_fixture host-memory-admission)"
if python3 - "$repo/scripts" "$script_dir" "$tmp/host-memory" <<'PY_HOST_MEMORY_REGRESSION'
import importlib.util
import os
from pathlib import Path
import subprocess
import sys

fixture, production, pool = map(Path, sys.argv[1:])
def load(path):
    spec = importlib.util.spec_from_file_location('host_memory', path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

host = load(fixture / 'host_memory.py')
original = load(production / 'host_memory.py')
assert original.DIRECTORY == Path('/var/tmp') / f'cas-host-memory-{os.getuid()}'
assert host.DIRECTORY == pool, (host.DIRECTORY, pool)
assert host.DIRECTORY != original.DIRECTORY
command = [sys.executable, '-c', """
import importlib.util
from pathlib import Path
import os
import sys
spec = importlib.util.spec_from_file_location('proof', Path(sys.argv[1]) / 'assembly-proof.py')
p = importlib.util.module_from_spec(spec)
spec.loader.exec_module(p)
p._run_contexts = lambda *args: print('fixture contexts started')
p.run_contexts(None, None, dict(os.environ), None, None, {})
""", str(fixture)]
env = dict(os.environ, CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS='1',
           CAS_HOST_MEMORY_DIRECTORY=str(pool / 'operator-override'))
env.pop(host.LEASE_ENV, None)
high = {'total_bytes': 64 * 1024**3, 'available_bytes': 60 * 1024**3,
        'reserve_bytes': 16 * 1024**3, 'budget_bytes': 44 * 1024**3, 'source': 'fixture'}
with host.admission('worker', env, lambda _: high, report=lambda _: None):
    blocked = subprocess.run(command, env=env, capture_output=True, text=True, timeout=15)
    assert blocked.returncode != 0, blocked.stdout + blocked.stderr
    assert 'worker suite running' in blocked.stdout + blocked.stderr
    assert 'fixture contexts started' not in blocked.stdout
admitted = subprocess.run(command, env=env, capture_output=True, text=True, timeout=15)
assert admitted.returncode == 0, admitted.stdout + admitted.stderr
assert 'fixture contexts started' in admitted.stdout
assert not (pool / 'operator-override').exists()
PY_HOST_MEMORY_REGRESSION
then
    ok 'fixture subprocess proofs use a private pool and still wait for its worker budget'
else
    bad 'fixture subprocess proof admission pool is not isolated or does not enforce leases'
fi

repo="$(new_fixture publish-toolchain)"
output="$(GATE_FIXTURE_ZIGBUILD_FAIL=1 run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --only publish-toolchain 2>&1 || true)"
assert_named_failure publish-toolchain "$output"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --only publish-toolchain 2>&1)"
if grep -qF 'PASS publish-toolchain' <<<"$output"; then
    ok 'publish-toolchain is independently selectable and fail closed'
else
    bad "publish-toolchain row failed: $output"
fi

# 1-7. Each mechanical or command-backed failure is isolated in its own repo.
repo="$(new_fixture release-notes-shell-injection)"
cat > "$repo/.github/workflows/release.yml" <<'EOF'
jobs:
  publish:
    steps:
      - run: |
          gh release create "$VERSION" --notes "${{ steps.notes.outputs.notes }}"
EOF
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --only release-notes-shell-injection 2>&1 || true)"
assert_named_failure release-notes-shell-injection "$output"

repo="$(new_fixture version-literal)"
printf 'const VERSION: &str = "9.99.7-rc.1";\n' >"$repo/cas-cli/src/version.rs"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 2>&1 || true)"
assert_named_failure version-literals "$output"

# The learned assemble-stale-base diagnosis must remain an executable,
# independently selectable row, not only a parsed failure-log marker.
repo="$(new_fixture assemble-stale-base-row)"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --only assemble-stale-base 2>&1 || true)"
if grep -qF 'PASS assemble-stale-base' <<<"$output" \
    && grep -qF 'RELEASE GATE PASSED: selected checks are green' <<<"$output"; then
    ok 'assemble-stale-base runs the identity-safe stale-base fixture as an executable row'
else
    bad "assemble-stale-base row did not pass its fixture (output: $output)"
fi

# A gitignored build cache that embeds the checkout path (for example a worktree
# named after the release) must not trip version-literals in a git checkout.
repo="$(new_fixture version-literal-ignored-cache)"
(
    cd "$repo" \
        && git init -q \
        && git -c user.email=gate@fixture -c user.name=gate add -A >/dev/null \
        && printf 'crates/vendor-cache/\n' >>.gitignore \
        && mkdir -p crates/vendor-cache \
        && printf 'cache entry for /tmp/release-9.99.7-assembly/src\n' >crates/vendor-cache/paths.txt \
        && git -c user.email=gate@fixture -c user.name=gate add -A >/dev/null \
        && git -c user.email=gate@fixture -c user.name=gate commit -qm fixture
)
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --only version-literals,scratch-base 2>&1 || true)"
if grep -qF 'PASS version-literals' <<<"$output"; then
    ok 'version-literals ignores gitignored caches in a git checkout'
else
    bad "version-literals scanned a gitignored cache (output: $output)"
fi

if test_child_environment; then
    ok 'every suite child scrubs all declared and future train/gate control variables'
else
    bad 'suite child environment contains train/gate control variables'
fi

# Real make must retain a failing Python test name and stop before Cargo.
# Inherited dry-run/ignore-error/touch modes cannot manufacture a PASS.
repo="$(new_fixture ci-script-tier)"
output="$(CAS_FACTORY_SESSION=fixture CAS_AGENT_ROLE=supervisor CAS_AGENT_NAME=fixture \
    CAS_SUPERVISOR_NAME=fixture CAS_AGENT_ID=fixture CAS_SESSION_ID=fixture CAS_ROOT=fixture \
    run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --only ci-script-tests 2>&1 || true)"
if grep -qF 'PASS ci-script-tests' <<<"$output"; then
    ok 'CI script row runs real make with every factory identity key scrubbed'
else
    bad "CI script identity scrub failed: $output"
fi
# Reproduce the assembly failure: a passing script tier contains an expected
# failed nested web-gate row. The outer receipt must contain only its own row,
# and all parent output/synchronization controls must be absent in the child.
printf '%s\n' '{"name":"nested-web-fixture","private":true}' >"$repo/hub-web/package.json"
nested_outer_logs="$tmp/nested-outer-rows"
nested_child_env="$tmp/nested-child-env.json"
output="$(CAS_RELEASE_GATE_LOG_DIR="$nested_outer_logs" \
    CAS_RELEASE_GATE_ARCHIVE_SIZE_FILE="$tmp/outer-archive-size" \
    CAS_RELEASE_GATE_CACHE_DIR="$tmp/outer-cache" \
    CAS_RELEASE_GATE_SWEEP_CACHE_DIR="$tmp/outer-sweep-cache" \
    CAS_RELEASE_GATE_SWEEP_RECEIPT="$tmp/outer-sweep.json" \
    CAS_RELEASE_GATE_ASSEMBLY_SYNC_DIR="$tmp/outer-sync" \
    CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLICY='{}' \
    CAS_RELEASE_GATE_ASSEMBLY_BUILD_JOBS=16 CAS_RELEASE_GATE_ASSEMBLY_RESERVE_GIB=16 \
    CAS_RELEASE_ARTIFACTS_ROOT="$tmp/outer-artifacts" \
    CAS_RELEASE_RECEIPTS_RUN_DIR="$tmp/outer-receipts" \
    VERIFIED_TEST_COUNT_FILE="$tmp/outer-count" VERIFIED_TEST_LOG="$tmp/outer-test.log" \
    GATE_FIXTURE_NESTED_GATE_TEST=1 GATE_FIXTURE_NESTED_ENV_FILE="$nested_child_env" \
    run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --only ci-script-tests 2>&1 || true)"
if grep -qF 'PASS ci-script-tests' <<<"$output" \
    && python3 - "$nested_outer_logs/timing.tsv" "$nested_child_env" <<'PY_NESTED_RECEIPTS'
import csv
import json
import sys
with open(sys.argv[1]) as stream:
    rows = list(csv.DictReader(stream, delimiter="\t"))
assert len(rows) == 1 and rows[0]["row"] == "ci-script-tests" and rows[0]["status"] == "0", rows
with open(sys.argv[2]) as stream:
    assert json.load(stream) == {}, "nested tests inherited parent receipt controls"
PY_NESTED_RECEIPTS
then
    ok 'nested gate self-tests cannot overwrite outer timing, receipts, caches or assembly controls'
else
    bad "nested gate receipt isolation failed: $output"
fi
for make_mode in -n -i -t; do
    : >"$tmp/cargo.log"
    output="$(MAKEFLAGS="$make_mode" GNUMAKEFLAGS="$make_mode" MFLAGS="$make_mode" \
        run_gate "$repo" GATE_FIXTURE_CI_SCRIPT_FAIL "$repo/scripts/release-gate.sh" 9.99.7 2>&1 || true)"
    assert_named_failure ci-script-tests "$output"
    if grep -qF 'test_seeded_ci_script_failure' <<<"$output" && [[ ! -s "$tmp/cargo.log" ]]; then
        ok "CI script failure keeps its test name and stops before Cargo despite make $make_mode"
    else
        bad "CI script failure was hidden or reached Cargo: $output"
    fi
done

run_scenario workspace-check GATE_FIXTURE_CHECK_FAIL workspace-tests
run_scenario macos-check-run GATE_FIXTURE_MACOS_FAIL macos-check
run_scenario macos-target-install GATE_FIXTURE_RUSTUP_FAIL macos-check
run_scenario nextest-run GATE_FIXTURE_NEXTEST_FAIL nextest
run_scenario doctest-run GATE_FIXTURE_DOCTEST_FAIL doctests
run_scenario archive-run GATE_FIXTURE_ARCHIVE_FAIL archive-mode
# A missing archive command must fail the row by name, never become its PATH.
cat >"$tmp/hide-archive-command.sh" <<'EOF'
command() {
    if [[ "$1" == -v && "$2" == cargo-nextest ]]; then return 1; fi
    builtin command "$@"
}
EOF
repo="$(new_fixture archive-missing-command)"
output="$(BASH_ENV="$tmp/hide-archive-command.sh" \
    run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --only archive-mode 2>&1 || true)"
if grep -qF 'archive-mode: required archive command is missing: cargo-nextest' <<<"$output" \
    && grep -qF 'FAIL archive-mode' <<<"$output"; then
    ok 'archive-mode names a missing command and fails the row'
else
    bad "archive-mode swallowed a missing command into PATH: $output"
fi
run_scenario snapshot-run GATE_FIXTURE_SNAPSHOT_FAIL snapshot-portability
run_scenario projection-run GATE_FIXTURE_DRIFT_FAIL builtin-projections
run_scenario fixture-paths-run GATE_FIXTURE_FIXTURE_PATHS_FAIL fixture-paths
run_scenario visual-qa-run GATE_FIXTURE_HUB_WEB_VISUAL_QA_FAIL hub-web-visual-qa

# The normal visual-QA scenarios above inject a runner stub so the self-test
# stays fast. Keep one real-row fixture as well: its npm stub records `ci` and
# refuses to run the visual-QA command unless dependency installation happened
# first. This catches a missing npm ci that the runner stub would conceal.
repo="$(new_fixture visual-qa-dependency-install)"
printf '%s\n' '{"name":"hub-web-fixture","private":true}' >"$repo/hub-web/package.json"
printf '%s\n' 'export {}' >"$repo/hub-web/scripts/visual-qa.mjs"
cat >"$repo/scripts/npm-stub" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${GATE_FIXTURE_NPM_LOG:?}"
if [[ "$1" == ci ]]; then
  : >"${GATE_FIXTURE_NPM_CI_MARKER:?}"
  exit 0
fi
if [[ "$1" == run && "${2:-}" == build ]]; then
  mkdir -p dist
  printf 'built: ' >dist/app.js
  cat src/main.ts >>dist/app.js
  exit 0
fi
if [[ "$1" == exec && "$*" == *'playwright install chromium'* ]]; then
  exit 0
fi
if [[ "$1" == exec && "$*" == *'node scripts/visual-qa.mjs'* ]]; then
  [[ -f "${GATE_FIXTURE_NPM_CI_MARKER:?}" ]] || {
    printf 'visual-QA runner started before npm ci\n' >&2
    exit 1
  }
  printf '%s\n' "${1:-missing-artifact-dir}" >>"${GATE_FIXTURE_NPM_RUNNER_LOG:?}"
  exit 0
fi
printf 'unexpected npm invocation: %s\n' "$*" >&2
exit 1
EOF
chmod +x "$repo/scripts/npm-stub"
output="$(
    cd "$repo" && \
    env -u ZIG -u CAS_RELEASE_EPIC_REF -u CAS_RELEASE_TRAIN_BRANCH -u RELEASE_GATE_HUB_WEB_VISUAL_QA \
      CARGO="$repo/scripts/cargo-stub" \
      NPM="$repo/scripts/npm-stub" \
      GATE_FIXTURE_CARGO_LOG="$tmp/cargo.log" \
      GATE_FIXTURE_NPM_LOG="$tmp/npm.log" \
      GATE_FIXTURE_NPM_CI_MARKER="$tmp/npm-ci.marker" \
      GATE_FIXTURE_NPM_RUNNER_LOG="$tmp/npm-runner.log" \
      "$repo/scripts/release-gate.sh" 9.99.7 --only hub-web-visual-qa
)"
if grep -qF 'PASS hub-web-visual-qa' <<<"$output" && \
   grep -qF 'ci --no-audit --no-fund' "$tmp/npm.log" && \
   grep -qF 'exec --yes --package=playwright -- node scripts/visual-qa.mjs' "$tmp/npm.log" && \
   [[ -f "$tmp/npm-ci.marker" ]]; then
    ok 'hub-web-visual-qa installs dependencies before invoking the runner'
else
    bad "hub-web-visual-qa dependency install contract failed (output: $output; npm log: $(cat "$tmp/npm.log" 2>/dev/null || true))"
fi

# cas-83ff. Building Commander web assets must prove committed dist stays in
# sync with src, and the later visual-QA row must reuse the same npm install.
repo="$(new_fixture hub-web-dist-drift)"
mkdir -p "$repo/hub-web/src" "$repo/hub-web/dist"
printf '%s\n' '{"name":"hub-web-fixture","private":true,"scripts":{"build":"fixture-build"}}' \
    >"$repo/hub-web/package.json"
printf '%s\n' '{"name":"hub-web-fixture","lockfileVersion":3,"packages":{"":{"name":"hub-web-fixture"}}}' \
    >"$repo/hub-web/package-lock.json"
printf '%s\n' 'initial source' >"$repo/hub-web/src/main.ts"
printf '%s\n' 'built: initial source' >"$repo/hub-web/dist/app.js"
printf '%s\n' 'export {}' >"$repo/hub-web/scripts/visual-qa.mjs"
cat >"$repo/scripts/npm-stub" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${GATE_FIXTURE_NPM_LOG:?}"
if [[ "$1" == ci ]]; then
  : >"${GATE_FIXTURE_NPM_CI_MARKER:?}"
  exit 0
fi
if [[ "$1" == run && "${2:-}" == build ]]; then
  printf 'built: ' >dist/app.js
  cat src/main.ts >>dist/app.js
  exit 0
fi
if [[ "$1" == exec && "$*" == *'playwright install chromium'* ]]; then
  exit 0
fi
if [[ "$1" == exec && "$*" == *'node scripts/visual-qa.mjs'* ]]; then
  [[ -f "${GATE_FIXTURE_NPM_CI_MARKER:?}" ]]
  exit 0
fi
printf 'unexpected npm invocation: %s\n' "$*" >&2
exit 1
EOF
chmod +x "$repo/scripts/npm-stub"
git -C "$repo" add hub-web
git -C "$repo" commit -qm 'seed hub web committed dist'
printf '%s\n' 'changed source' >"$repo/hub-web/src/main.ts"
dist_drift_npm_log="$tmp/dist-drift-npm.log"
dist_drift_marker="$tmp/dist-drift-npm-ci.marker"
dist_drift_runner_log="$tmp/dist-drift-runner.log"
output="$({
    cd "$repo" && \
    env -u ZIG -u CAS_RELEASE_EPIC_REF -u CAS_RELEASE_TRAIN_BRANCH -u RELEASE_GATE_HUB_WEB_VISUAL_QA \
      CARGO="$repo/scripts/cargo-stub" \
      NPM="$repo/scripts/npm-stub" \
      GATE_FIXTURE_CARGO_LOG="$tmp/cargo.log" \
      GATE_FIXTURE_NPM_LOG="$dist_drift_npm_log" \
      GATE_FIXTURE_NPM_CI_MARKER="$dist_drift_marker" \
      GATE_FIXTURE_NPM_RUNNER_LOG="$dist_drift_runner_log" \
      "$repo/scripts/release-gate.sh" 9.99.7 --only hub-web-dist-drift,hub-web-visual-qa
} 2>&1 || true)"
assert_named_failure hub-web-dist-drift "$output"
if ! grep -qF 'FAIL hub-web-visual-qa' <<<"$output" && \
   [[ "$(grep -c '^ci ' "$dist_drift_npm_log")" -eq 1 ]]; then
    ok 'hub-web-dist-drift fails stale dist and visual QA reuses npm ci'
else
    bad "hub-web-dist-drift did not isolate stale dist or repeated npm ci (output: $output; npm log: $(cat "$dist_drift_npm_log" 2>/dev/null || true))"
fi

(cd "$repo/hub-web" && \
    GATE_FIXTURE_NPM_LOG="$dist_drift_npm_log" \
    GATE_FIXTURE_NPM_CI_MARKER="$dist_drift_marker" \
    "$repo/scripts/npm-stub" run build)
git -C "$repo" add hub-web/dist
git -C "$repo" commit -qm 'regenerate hub web committed dist'
: >"$dist_drift_npm_log"
output="$({
    cd "$repo" && \
    env -u ZIG -u CAS_RELEASE_EPIC_REF -u CAS_RELEASE_TRAIN_BRANCH -u RELEASE_GATE_HUB_WEB_VISUAL_QA \
      CARGO="$repo/scripts/cargo-stub" \
      NPM="$repo/scripts/npm-stub" \
      GATE_FIXTURE_CARGO_LOG="$tmp/cargo.log" \
      GATE_FIXTURE_NPM_LOG="$dist_drift_npm_log" \
      GATE_FIXTURE_NPM_CI_MARKER="$dist_drift_marker" \
      GATE_FIXTURE_NPM_RUNNER_LOG="$dist_drift_runner_log" \
      "$repo/scripts/release-gate.sh" 9.99.7 --only hub-web-dist-drift,hub-web-visual-qa || true
})"
if grep -qF 'PASS hub-web-dist-drift' <<<"$output" && \
   grep -qF 'PASS hub-web-visual-qa' <<<"$output" && \
   grep -qF 'RELEASE GATE PASSED' <<<"$output" && \
   [[ "$(grep -c '^ci ' "$dist_drift_npm_log")" -eq 1 ]]; then
    ok 'hub-web-dist-drift passes after dist regeneration'
else
    bad "hub-web-dist-drift did not pass with regenerated dist (output: $output; npm log: $(cat "$dist_drift_npm_log" 2>/dev/null || true))"
fi

# cas-1f6e. A src-side test module that reads the producer checkout at runtime
# through CARGO_MANIFEST_DIR passes on the build host and fails on the
# merge-queue shard runner; the fixture-paths row must name the file and line.
repo="$(new_fixture src-runtime-manifest-dir)"
mkdir -p "$repo/cas-cli/src/inspect"
cat >"$repo/cas-cli/src/inspect/mod.rs" <<'EOF'
#[cfg(test)]
mod tests {
    #[test]
    fn reads_producer_copy() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let _ = std::fs::read_to_string(root.join("docs/design/design-tokens.json"));
    }
}
EOF
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --only fixture-paths 2>&1 || true)"
assert_named_failure fixture-paths "$output"
grep -qF 'cas-cli/src/inspect/mod.rs:5' <<<"$output" && ok 'fixture-paths names the runtime CARGO_MANIFEST_DIR read' \
    || bad "fixture-paths did not name the src runtime read: $output"

# The compile-time form is shard-safe and must stay green; the sanctioned
# cas::test_paths probe is allowlisted by path.
repo="$(new_fixture src-compile-time-include)"
mkdir -p "$repo/cas-cli/src/inspect"
cat >"$repo/cas-cli/src/inspect/mod.rs" <<'EOF'
#[cfg(test)]
mod tests {
    const DOC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/design/design-tokens.json"));
    const BYTES: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../casdemo.png"));
    #[test]
    fn embedded() { assert!(!DOC.is_empty() && !BYTES.is_empty()); }
}
EOF
cat >"$repo/cas-cli/src/test_paths.rs" <<'EOF'
pub fn workspace_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}
EOF
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --only fixture-paths 2>&1 || true)"
grep -qF 'PASS fixture-paths' <<<"$output" && ok 'fixture-paths allows compile-time includes and the test_paths probe' \
    || bad "fixture-paths rejected a shard-safe include: $output"

# 8. Ledger regeneration must be compared to the committed file.
run_scenario reference-ledger GATE_FIXTURE_REFERENCE_FAIL builtin-projections

# 9. Changelog/version contract and clean-tree contract are independent.
repo="$(new_fixture changelog-failure)"
# Portable in-place edit: GNU and BSD sed disagree on -i (cas-fed5).
grep -v 'Fixture release' "$repo/CHANGELOG.md" >"$repo/CHANGELOG.md.tmp" || true
mv "$repo/CHANGELOG.md.tmp" "$repo/CHANGELOG.md"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 2>&1 || true)"
assert_named_failure changelog-and-versions "$output"

repo="$(new_fixture dirty-tree)"
printf 'untracked\n' >"$repo/untracked.txt"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 2>&1 || true)"
assert_named_failure working-tree "$output"

repo="$(new_fixture invalid-failure-log)"
printf '%s\n' '- 2026-09-02 — **not-a-gate-check** — Symptom: unparseable. Root cause: fixture. Release: fixture.' >>"$repo/cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 2>&1 || true)"
assert_named_failure failure-log "$output"

# cas-8b90. A worktree that is dirty or no longer matches its claimed epic ref
# must not enter a release gate. The reset command is printed so the operator
# can refresh the exact worktree that failed the freshness check.
repo="$(new_fixture stale-epic-worktree)"
printf 'stale worktree\n' >"$repo/stale.txt"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 2>&1 || true)"
assert_named_failure epic-worktree-fresh "$output"
if grep -qF "reset command: git -C $repo reset --hard HEAD" <<<"$output"; then
    ok 'stale epic worktree receipt names the exact reset command'
else
    bad "stale epic worktree receipt omitted its reset command (output: $output)"
fi

# The normal fixture has an ignored Zig installation, proving the worktree
# candidate. Remove it to prove the refusal is named and actionable.
repo="$(new_fixture missing-epic-zig)"
rm -f "$repo/.context/zig/zig"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 2>&1 || true)"
assert_named_failure epic-worktree-zig "$output"
if grep -qF './scripts/bootstrap-zig.sh' <<<"$output"; then
    ok 'missing epic-worktree Zig receipt names bootstrap-zig.sh'
else
    bad "missing epic-worktree Zig receipt omitted bootstrap-zig.sh (output: $output)"
fi

# A detached fresh worktree has no ignored .context/zig of its own. Its Git
# common directory still points at the main checkout, so the main-checkout
# fallback must export the executable and keep the gate green.
repo="$(new_fixture main-checkout-zig)"
epic_worktree="$tmp/main-checkout-zig-worktree"
zig_log="$tmp/main-checkout-zig.log"
git -C "$repo" worktree add --detach "$epic_worktree" HEAD >/dev/null
output="$(cd "$epic_worktree" && \
    env -u ZIG \
    CAS_RELEASE_EPIC_REF=refs/heads/epic/release-gate-fixture \
    GATE_FIXTURE_CARGO_LOG="$tmp/cargo.log" \
    GATE_FIXTURE_RUSTUP_LOG="$tmp/rustup.log" \
    GATE_FIXTURE_CC_OBJECT="$tmp/macos-check.o" \
    GATE_FIXTURE_ZIG_LOG="$zig_log" \
    CARGO="$epic_worktree/scripts/cargo-stub" \
    RUSTUP="$epic_worktree/scripts/rustup-stub" \
    RELEASE_GATE_GEN_REFERENCE_HISTORY="$epic_worktree/scripts/gen-builtin-reference-history.sh" \
    "$epic_worktree/scripts/release-gate.sh" 9.99.7 2>&1 || true)"
git -C "$repo" worktree remove --force "$epic_worktree" >/dev/null
if grep -qF 'PASS epic-worktree-zig' <<<"$output" \
    && grep -qF "ZIG=$repo/.context/zig/zig ::" "$zig_log"; then
    ok 'fresh epic worktree resolves Zig from the main checkout'
else
    bad "fresh epic worktree did not use the main-checkout Zig fallback (output: $output)"
fi

repo="$(new_fixture learn-mode)"
learn_output="$(cd "$repo" && GATE_FIXTURE_REFERENCE_FAIL=1 \
    "$repo/scripts/release-gate.sh" --learn 'new release symptom' 'new release cause' 'procedure-guardrails' 2>&1)"
grep -qF 'Learned release failure in the failure log' <<<"$learn_output"
grep -qF 'Regenerated builtin reference history after --learn' <<<"$learn_output"
grep -qF 'changed ledger' "$repo/cas-cli/src/builtins/reference-history.json"
grep -qF 'new release symptom' "$repo/cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md"
[[ ! -e "$repo/cas-cli/src/builtins/codex/skills/cas-cut-release/references/failure-log.md" ]]
ok '--learn appends a dated failure entry to the one failure log'

# cas-6df6. Keep the release diagnosis in the executable nextest failure-log
# category and prove --learn accepts the exact operator-reported cause.
repo="$(new_fixture learn-nextest-factory-session)"
nextest_cause='gate inherited the supervisor shell'"'"'s CAS_FACTORY_SESSION; a test agent registered under it routed lifecycle pushes to a supervisor absent from the fixture'
learn_output="$(cd "$repo" && \
    "$repo/scripts/release-gate.sh" --learn 'nextest inherited factory identity' "$nextest_cause" nextest 2>&1)"
if grep -qF 'Learned release failure in the failure log' <<<"$learn_output" \
    && grep -qF "$nextest_cause" "$repo/cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md"; then
    ok '--learn records the nextest factory-session diagnosis in the failure log'
else
    bad "--learn did not record the nextest factory-session diagnosis: $learn_output"
fi

# cas-7715. A worker's scoped proof can pass while archive-mode catches a
# builtin size/phrase guardrail that the proof surface failed to require. Keep
# that diagnosis attached to a real executable release row, and exercise the
# exact operator text through --learn so the failure log stays durable.
repo="$(new_fixture learn-scoped-proof-surface)"
scoped_proof_symptom='worker proof passed while a skill-size guardrail test failed in the gate'
scoped_proof_cause='proof surface mapped files to the tests that mention them, not to the guardrail binaries that read them'
learn_output="$(cd "$repo" && \
    "$repo/scripts/release-gate.sh" --learn "$scoped_proof_symptom" "$scoped_proof_cause" archive-mode 2>&1)"
if grep -qF 'Learned release failure in the failure log' <<<"$learn_output" \
    && grep -qF "**archive-mode**" \
        "$repo/cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md" \
    && grep -qF "Symptom: $scoped_proof_symptom Root cause: $scoped_proof_cause" \
        "$repo/cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md"; then
    ok '--learn records the scoped-proof guardrail diagnosis on archive-mode'
else
    bad "--learn did not record the scoped-proof guardrail diagnosis: $learn_output"
fi

# cas-77c1. Integration spawn fixtures must carry the build-guard override into
# isolated children: otherwise a saturated host makes them read live
# /proc/loadavg and refuse a request that is healthy under the test contract.
repo="$(new_fixture learn-nextest-factory-build-guard)"
nextest_symptom='nextest: spawn_workers integration tests refused by build guard under host load'
nextest_cause='integration harness never set CAS_FACTORY_BUILD_GUARD=off; guard read live /proc/loadavg during full-suite run'
learn_output="$(cd "$repo" && \
    "$repo/scripts/release-gate.sh" --learn "$nextest_symptom" "$nextest_cause" nextest 2>&1)"
if grep -qF 'Learned release failure in the failure log' <<<"$learn_output" \
    && grep -qF "Symptom: $nextest_symptom Root cause: $nextest_cause" \
        "$repo/cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md"; then
    ok '--learn records the nextest factory build-guard diagnosis in the failure log'
else
    bad "--learn did not record the nextest factory build-guard diagnosis: $learn_output"
fi

# cas-4ccc. A populated .cas/proxy.toml ABOVE the worktree is readable by any
# test that resolves project config by walking up from its cwd. The gate must
# neutralize it and name it — never refuse, because blocking a release on the
# operator's own MCP configuration is how the original hour was lost.
repo="$(new_fixture ancestor-proxy)"
mkdir -p "$(dirname "$repo")/.cas"
printf '[servers.violet]\ntype = "http"\nurl = "https://example.invalid/mcp"\n' \
    >"$(dirname "$repo")/.cas/proxy.toml"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 2>&1 || true)"
if grep -qF 'FAIL ancestor-proxy-config' <<<"$output"; then
    bad 'ancestor proxy.toml must be neutralized, not refused'
else
    ok 'ancestor proxy.toml is neutralized rather than blocking the release'
fi
if grep -qF '.cas/proxy.toml' <<<"$output" && grep -qF 'CAS_ROOT=' <<<"$output"; then
    ok 'ancestor proxy.toml is named in the receipt with the override that neutralized it'
else
    bad 'ancestor proxy.toml was not named with its override in the receipt'
fi
# Remove the whole directory, not just the file: later scenarios assert that no
# ancestor of their scratch base holds a .cas store at all, and an empty
# leftover would fail them for this fixture's reason.
rm -rf "$(dirname "$repo")/.cas"

# The repository's OWN .cas/proxy.toml is where a project config belongs and
# must not be treated as an ancestor leak.
repo="$(new_fixture own-proxy)"
mkdir -p "$repo/.cas"
printf '[servers.local]\ntype = "http"\nurl = "https://example.invalid/mcp"\n' \
    >"$repo/.cas/proxy.toml"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 2>&1 || true)"
if grep -qF 'FAIL ancestor-proxy-config' <<<"$output"; then
    bad "the repository's own .cas/proxy.toml must not trip the ancestor check"
else
    ok "the repository's own .cas/proxy.toml is not treated as an ancestor leak"
fi

# cas-c736. With CAS_RELEASE_GATE_HOME_DIR UNSET the gate must pick its own
# scratch base rather than $HOME/.cache/cas-release-gate, which has a .cas
# ancestor on every machine with a user-level store and made archive-mode and
# snapshot-portability refuse before they ran. This is the fixture that proves
# the default path is the one taken, so the variable is an override and not a
# prerequisite for cutting a release.
run_gate_unset_home() {
    local repo="$1"
    (cd "$repo" && \
      env -u CAS_RELEASE_GATE_HOME_DIR \
      CAS_RELEASE_GATE_CHECKOUT_DEVICE=1 CAS_RELEASE_GATE_SCRATCH_DEVICE=1 \
      GATE_FIXTURE_CARGO_LOG="$tmp/cargo.log" \
      GATE_FIXTURE_RUSTUP_LOG="$tmp/rustup.log" \
      GATE_FIXTURE_CC_OBJECT="$tmp/macos-check.o" \
      CARGO="$repo/scripts/cargo-stub" \
      RUSTUP="$repo/scripts/rustup-stub" \
      RELEASE_GATE_GEN_REFERENCE_HISTORY="$repo/scripts/gen-builtin-reference-history.sh" \
      "$repo/scripts/release-gate.sh" 9.99.7)
}

repo="$(new_fixture default-scratch-base)"
output="$(run_gate_unset_home "$repo" 2>&1 || true)"
# cas-db34: the default is per host (/Users/Shared on macOS, where /var/tmp is
# a Cassy disposable root); ask the gate's own helper which one applies.
default_scratch_base="$(bash -c 'source "$1"; release_portable_default_scratch_base' _ "$repo/scripts/release-portable.sh")"
if grep -qF "scratch base: $default_scratch_base (from default)" <<<"$output" \
    && grep -qF 'PASS archive-mode' <<<"$output" \
    && grep -qF 'PASS snapshot-portability' <<<"$output"; then
    ok 'an unset CAS_RELEASE_GATE_HOME_DIR takes the gate default, and the scratch rows run'
else
    bad "unset CAS_RELEASE_GATE_HOME_DIR did not take the gate default (output: $output)"
fi

# ...and an explicit value still wins, named in the receipt so a reader can see
# which base a release was actually gated against.
override="$tmp/scratch-override"
mkdir -p "$override"
repo="$(new_fixture explicit-scratch-base)"
output="$(cd "$repo" && env CAS_RELEASE_GATE_HOME_DIR="$override/base" \
    GATE_FIXTURE_CARGO_LOG="$tmp/cargo.log" \
    GATE_FIXTURE_RUSTUP_LOG="$tmp/rustup.log" \
    GATE_FIXTURE_CC_OBJECT="$tmp/macos-check.o" \
    CARGO="$repo/scripts/cargo-stub" \
    RUSTUP="$repo/scripts/rustup-stub" \
    RELEASE_GATE_GEN_REFERENCE_HISTORY="$repo/scripts/gen-builtin-reference-history.sh" \
    "$repo/scripts/release-gate.sh" 9.99.7 2>&1 || true)"
if grep -qF "scratch base: $override/base (from CAS_RELEASE_GATE_HOME_DIR)" <<<"$output"; then
    ok 'an explicit CAS_RELEASE_GATE_HOME_DIR still wins over the default'
else
    bad "explicit CAS_RELEASE_GATE_HOME_DIR was not honoured (output: $output)"
fi

# The scratch preflight is the first receipt row and rejects each host
# condition without dispatching Cargo.
repo="$(new_fixture scratch-preflight)"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --only scratch-base 2>&1)"
first_row="$(grep -m1 -E '^(PASS|FAIL) ' <<<"$output")"
if [[ "$first_row" == PASS\ scratch-base* ]]; then
    ok 'scratch-base is the first release-gate receipt row'
else
    bad "scratch-base was not first: $output"
fi

output="$(cd "$repo" && CAS_RELEASE_GATE_HOME_DIR="$tmp/gate-scratch/base" \
    CAS_RELEASE_GATE_PARENT_WRITABLE=0 "$repo/scripts/release-gate.sh" 9.99.7 --only scratch-base 2>&1 || true)"
assert_named_failure scratch-base "$output"
grep -qF 'parent' <<<"$output" && ok 'scratch-base names an unwritable parent' \
    || bad "scratch-base omitted the unwritable parent: $output"

# The full gate aborts after this first preflight failure. Neither Cargo nor
# archive receipt mutation may occur after the host was already proven unsafe.
preflight_cargo_sentinel="$tmp/preflight-abort-cargo.log"
preflight_archive_sentinel="$tmp/preflight-abort-archive-size"
rm -f "$preflight_cargo_sentinel" "$preflight_archive_sentinel"
output="$(cd "$repo" && CAS_RELEASE_GATE_HOME_DIR="$tmp/gate-scratch/base" \
    CAS_RELEASE_GATE_PARENT_WRITABLE=0 \
    CAS_RELEASE_GATE_ARCHIVE_SIZE_FILE="$preflight_archive_sentinel" \
    GATE_FIXTURE_CARGO_LOG="$preflight_cargo_sentinel" \
    CARGO="$repo/scripts/cargo-stub" \
    RELEASE_GATE_GEN_REFERENCE_HISTORY="$repo/scripts/gen-builtin-reference-history.sh" \
    "$repo/scripts/release-gate.sh" 9.99.7 2>&1 || true)"
if [[ ! -e "$preflight_cargo_sentinel" && ! -e "$preflight_archive_sentinel" ]] \
    && ! grep -qE '^(PASS|FAIL) (fixture-paths|workspace-tests|nextest|archive-mode)' <<<"$output"; then
    ok 'scratch-base failure aborts before Cargo and archive rows run'
else
    bad "scratch-base failure continued into costly rows: $output"
fi

output="$(cd "$repo" && CAS_RELEASE_GATE_HOME_DIR="$tmp/gate-scratch/base" \
    CAS_RELEASE_GATE_CHECKOUT_DEVICE=11 CAS_RELEASE_GATE_SCRATCH_DEVICE=22 \
    "$repo/scripts/release-gate.sh" 9.99.7 --only scratch-base 2>&1 || true)"
assert_named_failure scratch-base "$output"
grep -qF 'filesystem boundary' <<<"$output" && ok 'scratch-base names a cross-device base' \
    || bad "scratch-base omitted the filesystem boundary: $output"

output="$(cd "$repo" && CAS_RELEASE_GATE_HOME_DIR="$tmp/gate-scratch/base" \
    CAS_RELEASE_GATE_CHECKOUT_DEVICE=11 CAS_RELEASE_GATE_SCRATCH_DEVICE=11 \
    CAS_RELEASE_GATE_LAST_ARCHIVE_BYTES=100 CAS_RELEASE_GATE_FREE_BYTES=199 \
    "$repo/scripts/release-gate.sh" 9.99.7 --only scratch-base 2>&1 || true)"
assert_named_failure scratch-base "$output"
if grep -qF 'need at least 200 (2x last archive 100)' <<<"$output"; then
    ok 'scratch-base enforces free bytes >= 2x the recorded archive size'
else
    bad "scratch-base omitted the capacity formula: $output"
fi

unsafe="$tmp/unsafe-scratch"
mkdir -p "$unsafe/.cas" "$unsafe/child"
output="$(cd "$repo" && CAS_RELEASE_GATE_HOME_DIR="$unsafe/child/base" \
    "$repo/scripts/release-gate.sh" 9.99.7 --only scratch-base 2>&1 || true)"
assert_named_failure scratch-base "$output"
grep -qF '.cas ancestor' <<<"$output" && ok 'scratch-base rejects a .cas ancestor' \
    || bad "scratch-base omitted the .cas ancestor: $output"

# --only validates its selection synchronously, executes in canonical row
# order, and names the selected set in the terminal receipt.
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --only version-literals,scratch-base 2>&1)"
rows="$(grep '^PASS ' <<<"$output" | awk '{print $2}' | paste -sd, -)"
if [[ "$rows" == 'scratch-base,version-literals' ]] \
    && grep -qF 'selected checks are green for 9.99.7: scratch-base,version-literals' <<<"$output"; then
    ok '--only preserves canonical row order and prints the selected-row summary'
else
    bad "--only row order or summary drifted: $output"
fi
for invalid in '' not-a-row; do
    output="$(cd "$repo" && "$repo/scripts/release-gate.sh" 9.99.7 --only "$invalid" 2>&1 || true)"
    if grep -qE 'requires at least one|unknown --only' <<<"$output"; then
        ok "--only rejects ${invalid:-an empty row list}"
    else
        bad "--only accepted invalid rows '$invalid': $output"
    fi
done

archive_receipt="$tmp/archive-size-bytes"
archive_env_log="$tmp/archive-environment.log"
: >"$archive_env_log"
output="$(cd "$repo" && GATE_FIXTURE_ARCHIVE_ENV_LOG="$archive_env_log" \
    GATE_FIXTURE_CARGO_LOG="$tmp/cargo.log" \
    CARGO="$repo/scripts/cargo-stub" CAS_RELEASE_GATE_ARCHIVE_SIZE_FILE="$archive_receipt" \
    "$repo/scripts/release-gate.sh" 9.99.7 --only archive-mode 2>&1 || true)"
archive_gate_output="$output"
if [[ "$(cat "$archive_receipt" 2>/dev/null)" == 7 ]] \
    && grep -qF "per-run=$archive_receipt" <<<"$output"; then
    ok 'archive-mode records the measured archive size in the per-run receipt source'
else
    bad "archive-mode did not record its measured size: $output"
fi
output="$(cd "$repo" && CAS_RELEASE_GATE_FREE_BYTES=13 \
    "$repo/scripts/release-gate.sh" 9.99.7 --only scratch-base 2>&1 || true)"
if grep -qF 'need at least 14 (2x last archive 7)' <<<"$output"; then
    ok 'scratch-base reads the last archive-size source written by archive-mode'
else
    bad "scratch-base did not read the recorded archive size: $output"
fi

if grep -qE '^RUSTC_WRAPPER=/nonexistent/sccache CARGO_HOME=.*/cargo-home :: nextest run --archive-file ' \
    "$archive_env_log"; then
    ok 'archive-mode runs the extracted suite with a missing wrapper and empty CARGO_HOME'
else
    bad "archive-mode did not reproduce the shard environment: $(cat "$archive_env_log") (output: $output)"
fi

if grep -qF 'PASS archive-mode' <<<"$archive_gate_output" \
    && grep -qE '^EXTRACT_DIR_READY=.*/extract :: nextest run --archive-file ' "$archive_env_log"; then
    ok 'archive extraction exists with the base owner and private mode before nextest runs'
else
    bad "archive extraction was not ready before nextest: $archive_gate_output"
fi

if python3 - "$archive_env_log" "$CAS_RELEASE_GATE_HOME_DIR" <<'PY_TEMP_PLACEMENT'
from pathlib import Path
import re
import sys
text = Path(sys.argv[1]).read_text()
match = re.search(r'^TMPDIR=(\S+) .* :: nextest run --archive-file .* --extract-to (\S+) --workspace-remap (\S+)', text, re.M)
assert match, 'missing explicit test TMPDIR or disk extraction'
temp, extract, remap = map(Path, match.groups())
assert temp.name == 'archive-test-tmp'
assert temp.parent.name.startswith('cas-release-gate.')
assert extract.parent == remap.parent
assert extract.parent.parent == Path(sys.argv[2]).parent
assert extract.parent not in temp.parents
assert not temp.exists(), 'test temp was not cleaned'
assert not extract.exists(), 'extraction was not cleaned'
PY_TEMP_PLACEMENT
then
    ok 'archive test temp is separate from durable extraction/remap and both are cleaned'
else
    bad 'archive TMPDIR and extraction placement regressed'
fi

# cas-6df6. A release gate launched inside a factory supervisor must not let
# its shell identity become the registered session for integration fixtures.
# Both the ordinary nextest row and both archive-mode cargo invocations must
# receive a scrubbed factory identity, while the archive row keeps its existing
# CAS_ROOT isolation.
factory_env_log="$tmp/factory-environment.log"
: >"$factory_env_log"
output="$(cd "$repo" && \
    CAS_FACTORY_SESSION=foreign-supervisor-session \
    CAS_AGENT_ROLE=supervisor \
    CAS_AGENT_NAME=foreign-supervisor \
    CAS_SUPERVISOR_NAME=foreign-supervisor \
    CAS_AGENT_ID=foreign-agent-id \
    GATE_FIXTURE_FACTORY_ENV_LOG="$factory_env_log" \
    GATE_FIXTURE_CARGO_LOG="$tmp/cargo.log" \
    CARGO="$repo/scripts/cargo-stub" \
    RELEASE_GATE_GEN_REFERENCE_HISTORY="$repo/scripts/gen-builtin-reference-history.sh" \
    "$repo/scripts/release-gate.sh" 9.99.7 --only nextest,doctests,archive-mode 2>&1 || true)"
if grep -qF 'CAS_FACTORY_SESSION=unset CAS_AGENT_ROLE=unset CAS_AGENT_NAME=unset CAS_SUPERVISOR_NAME=unset CAS_AGENT_ID=unset :: nextest run --workspace' \
    "$factory_env_log" \
    && grep -qF 'CAS_FACTORY_SESSION=unset CAS_AGENT_ROLE=unset CAS_AGENT_NAME=unset CAS_SUPERVISOR_NAME=unset CAS_AGENT_ID=unset :: nextest archive --workspace' \
    "$factory_env_log" \
    && grep -qF 'CAS_FACTORY_SESSION=unset CAS_AGENT_ROLE=unset CAS_AGENT_NAME=unset CAS_SUPERVISOR_NAME=unset CAS_AGENT_ID=unset :: nextest run --archive-file' \
    "$factory_env_log"; then
    if grep -qF 'CAS_FACTORY_SESSION=unset CAS_AGENT_ROLE=unset CAS_AGENT_NAME=unset CAS_SUPERVISOR_NAME=unset CAS_AGENT_ID=unset :: test -p cas --doc' "$factory_env_log"; then
        ok 'doctests scrub inherited factory identity'
    else
        bad 'doctests leaked inherited factory identity'
    fi
    ok 'nextest and archive-mode scrub inherited factory identity'
else
    bad "nextest or archive-mode leaked factory identity: $(cat "$factory_env_log") (output: $output)"
fi

# cas-c0411. The `cas init` watchdog budget the gate hands its children is the
# fix for a release that failed on wall clock: a test's child `cas init` hit the
# 300s default while the box was saturated, and the archive-mode row died with
# it. The budget must reach the children — including the archive run, which
# rebuilds its environment with `env -u COLUMNS HOME=... PATH=...` — and must be
# named in the receipt so a reader can see which budget a release was gated on.
run_gate_with_env_log() {
    local repo="$1" env_log="$2"
    shift 2
    (cd "$repo" && \
      env -u CAS_INIT_TIMEOUT_SECS "$@" \
      GATE_FIXTURE_CARGO_LOG="$tmp/cargo.log" \
      GATE_FIXTURE_ENV_LOG="$env_log" \
      GATE_FIXTURE_RUSTUP_LOG="$tmp/rustup.log" \
      GATE_FIXTURE_CC_OBJECT="$tmp/macos-check.o" \
      CARGO="$repo/scripts/cargo-stub" \
      RUSTUP="$repo/scripts/rustup-stub" \
      RELEASE_GATE_GEN_REFERENCE_HISTORY="$repo/scripts/gen-builtin-reference-history.sh" \
      "$repo/scripts/release-gate.sh" 9.99.7)
}

env_log="$tmp/init-timeout.log"
: >"$env_log"
repo="$(new_fixture init-watchdog-budget)"
output="$(run_gate_with_env_log "$repo" "$env_log" 2>&1 || true)"
if grep -qF 'init watchdog budget: 900s (from release-gate; cas init clamps at 3600s)' <<<"$output"; then
    ok 'the receipt names the cas init watchdog budget the children ran with'
else
    bad "the receipt did not name the gate's cas init watchdog budget (output: $output)"
fi
if [[ -s "$env_log" ]] && ! grep -q 'CAS_INIT_TIMEOUT_SECS=unset' "$env_log"; then
    ok 'every gate child inherits the raised cas init watchdog budget'
else
    bad "a gate child ran without the raised cas init budget (log: $(cat "$env_log"))"
fi
if grep -q '^CAS_INIT_TIMEOUT_SECS=900 :: nextest run --archive-file ' "$env_log"; then
    ok "the archive-mode row's rebuilt environment keeps the raised budget"
else
    bad "the archive run lost the raised budget (log: $(cat "$env_log"))"
fi

# ...and an operator who wants a different budget still wins, named as such.
env_log="$tmp/init-timeout-override.log"
: >"$env_log"
repo="$(new_fixture init-watchdog-override)"
output="$(run_gate_with_env_log "$repo" "$env_log" CAS_INIT_TIMEOUT_SECS=1234 2>&1 || true)"
if grep -qF 'init watchdog budget: 1234s (from CAS_INIT_TIMEOUT_SECS; cas init clamps at 3600s)' <<<"$output" \
    && grep -q '^CAS_INIT_TIMEOUT_SECS=1234 :: ' "$env_log"; then
    ok 'an explicit CAS_INIT_TIMEOUT_SECS overrides the gate default for its children'
else
    bad "an explicit CAS_INIT_TIMEOUT_SECS was not honoured (output: $output; log: $(cat "$env_log"))"
fi

# cas-c736. The gate must not accumulate scratch directories under its base.
# snapshot-portability leaked one <base>.snap.XXXXXX per invocation where
# archive-mode has always cleaned up after itself; harmless while the base was
# opt-in, but the default base is now /var/tmp/cas-release-gate on every host,
# so every run and every fixture below would pile up there forever.
scratch_leftovers() {
    find "$(dirname "$CAS_RELEASE_GATE_HOME_DIR")" -maxdepth 1 \
        -name "$(basename "$CAS_RELEASE_GATE_HOME_DIR").*" 2>/dev/null | wc -l
}
repo="$(new_fixture scratch-cleanup)"
leftovers_before="$(scratch_leftovers)"
run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 >/dev/null 2>&1 || true
leftovers_after="$(scratch_leftovers)"
if [[ "$leftovers_after" -eq "$leftovers_before" ]]; then
    ok 'a gate run leaves no scratch directory behind under its base'
else
    bad "gate run leaked $((leftovers_after - leftovers_before)) scratch dir(s) under $CAS_RELEASE_GATE_HOME_DIR"
fi

repo="$(new_fixture passing)"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 2>&1)"
assert_all_pass "$output"

# macos-check must install the target before dispatching the exact workspace
# test compile, and run_check must retain its measured timing in timing.tsv.
repo="$(new_fixture macos-check-receipt)"
macos_log_dir="$tmp/macos-check-logs"
# cas-db34: judge only this run's dispatch, not earlier fixtures' calls.
: >"$tmp/rustup.log"
: >"$tmp/cargo.log"
rm -f "$tmp/macos-check.o"
output="$(CAS_RELEASE_GATE_LOG_DIR="$macos_log_dir" run_gate "$repo" '' \
    "$repo/scripts/release-gate.sh" 9.99.7 --only macos-check 2>&1)"
if grep -qF 'PASS macos-check' <<<"$output" \
    && grep -qxF 'target add aarch64-apple-darwin' "$tmp/rustup.log" \
    && grep -qxF 'check --workspace --tests --target aarch64-apple-darwin' "$tmp/cargo.log" \
    && [[ -s "$tmp/macos-check.o" ]] \
    && (( $(wc -l <"$macos_log_dir/timing.tsv") == 2 )) \
    && awk -F '\t' '$1 == "macos-check" && $7 == 0 && $4 ~ /^[0-9]+\.[0-9]+$/ {found=1} END {exit !found}' \
        "$macos_log_dir/timing.tsv"; then
    ok 'macos-check installs the Darwin target, compiles the workspace, and records timing'
else
    bad "macos-check dispatch or timing receipt failed (output: $output; rustup: $(cat "$tmp/rustup.log" 2>/dev/null || true); cargo: $(cat "$tmp/cargo.log" 2>/dev/null || true))"
fi

repo="$(new_fixture macos-rustup-unavailable)"
output="$(cd "$repo" && \
    env -u ZIG -u CAS_RELEASE_EPIC_REF -u CAS_RELEASE_TRAIN_BRANCH \
      RUSTUP="$repo/scripts/rustup-not-installed" \
      CARGO="$repo/scripts/cargo-stub" \
      GATE_FIXTURE_CARGO_LOG="$tmp/cargo.log" \
      GATE_FIXTURE_RUSTUP_LOG="$tmp/rustup.log" \
      "$repo/scripts/release-gate.sh" 9.99.7 --only macos-check 2>&1 || true)"
assert_named_failure macos-check "$output"
if grep -qF 'macos-check: rustup is unavailable' <<<"$output"; then
    ok 'macos-check fails clearly when rustup is unavailable'
else
    bad "macos-check hid the unavailable-rustup cause: $output"
fi

# Whole gate executes the workspace complement only once; a focused nextest
# diagnostic still executes the complete in-tree suite.
repo="$(new_fixture suite-coverage)"
: >"$tmp/cargo.log"
run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 >"$tmp/coverage.log" 2>&1
if grep -qF "nextest run --workspace --filterset binary_id(~component_output_test)" "$tmp/cargo.log" \
    && ! grep -qxF 'nextest run --workspace --no-fail-fast' "$tmp/cargo.log" \
    && [[ "$(grep -c '^nextest archive --workspace ' "$tmp/cargo.log")" == 1 ]] \
    && grep -qF -- '--filterset not binary_id(~component_output_test)' "$tmp/cargo.log"; then
    ok 'full gate builds one archive and runs complementary suite filters'
else
    bad "full gate duplicated or lost suite coverage: $(cat "$tmp/cargo.log")"
fi
: >"$tmp/cargo.log"
run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --only nextest >"$tmp/diagnostic.log" 2>&1
if grep -qxF 'nextest run --workspace --no-fail-fast' "$tmp/cargo.log"; then
    ok 'focused nextest diagnostic retains whole-workspace execution'
else
    bad 'focused nextest diagnostic lost whole-workspace coverage'
fi

output="$(run_gate "$repo" GATE_FIXTURE_EMPTY_SUITE "$repo/scripts/release-gate.sh" 9.99.7 --only nextest,doctests,archive-mode 2>&1 || true)"
assert_named_failure nextest "$output"
assert_named_failure archive-mode "$output"
assert_named_failure doctests "$output"
if grep -qE '^INSTA_WORKSPACE_ROOT=.*/workspace-remap :: nextest run --archive-file .*--no-fail-fast' "$archive_env_log"; then
    ok 'archive consumer pins snapshot workspace and completes all binaries like CI'
else
    bad 'archive consumer drifted from CI snapshot-root or no-fail-fast contract'
fi

# Receipts from real fixture executions, never forged PASS to prove success.
repo="$(new_fixture row-cache)"
: >"$tmp/cargo.log"
export CAS_RELEASE_GATE_CACHE_DIR="$tmp/pass-cache"
export CAS_RELEASE_GATE_LOG_DIR="$tmp/row-logs"
run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 >"$tmp/cache-first.log" 2>&1 || { cat "$tmp/cache-first.log"; exit 1; }
expected_timing_rows=$(( $(grep -c '^PASS ' "$tmp/cache-first.log") + 1 ))
if (( $(wc -l <"$CAS_RELEASE_GATE_LOG_DIR/timing.tsv") == expected_timing_rows )) \
    && [[ -s "$CAS_RELEASE_GATE_LOG_DIR/archive-mode.log" ]] \
    && grep -qE '^  timing: wall=[0-9]+\.[0-9]+s user=' "$tmp/cache-first.log"; then
    ok 'every row retains wall/CPU timing and successful raw logs'
else
    bad 'row timing or successful logs missing'
fi
run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --reuse >"$tmp/cache-second.log" 2>&1
if [[ "$(awk -F '\t' '$7 == "REUSED" {n++} END {print n+0}' "$CAS_RELEASE_GATE_LOG_DIR/timing.tsv")" == 11 ]]; then
    ok 'unchanged full gate reuses eleven eligible PASS receipts'
    if [[ "$(grep -cxF 'zigbuild -p cas --release --target x86_64-unknown-linux-gnu --locked' "$tmp/cargo.log")" == 1 ]]; then
        ok 'unchanged code and lock reuse ISA evidence without another release build'
    else
        bad 'unchanged release ISA receipt rebuilt the executable'
    fi
else
    bad "unchanged full gate did not reuse eligible rows: $(cat "$tmp/cache-second.log")"
fi
if awk -F '\t' '$1 ~ /scratch-base|epic-worktree|builtin-projections|working-tree/ && $7 == "REUSED" {bad=1} END {exit bad}' "$CAS_RELEASE_GATE_LOG_DIR/timing.tsv"; then
    ok 'live preconditions, ledger regeneration and final cleanliness never reuse'
else
    bad 'a live precondition reused stale evidence'
fi
mkdir -p "$repo/docs/release-notes"
printf 'release prose\n' >"$repo/docs/release-notes/cache.md"
git -C "$repo" add docs/release-notes/cache.md
git -C "$repo" commit -qm 'fixture release prose'
run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 >"$tmp/cache-docs.log" 2>&1
if [[ "$(awk -F '\t' '$7 == "REUSED" {n++} END {print n+0}' "$CAS_RELEASE_GATE_LOG_DIR/timing.tsv")" == 11 ]]; then
    ok 'train row cache automatically reuses unchanged code proof after a release-prose commit'
else
    bad "release prose reran unchanged code rows: $(cat "$CAS_RELEASE_GATE_LOG_DIR/timing.tsv")"
fi
# Tool updates at the same selected path invalidate the artifact PASS too.
printf '%s\n' '#!/usr/bin/env bash' 'echo fixture-zig-2.0' >"$repo/.context/zig/zig"
run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --reuse >"$tmp/cache-isa-zig.log" 2>&1
if awk -F '\t' '$1 == "release-binary-isa" && $7 == "0" {found=1} END {exit !found}' "$CAS_RELEASE_GATE_LOG_DIR/timing.tsv"; then
    ok 'selected Zig version change invalidates the release artifact PASS'
else
    bad 'updated Zig reused stale release artifact evidence'
fi
printf '%s\n' '#!/usr/bin/env bash' 'echo fixture-zig-1.0' >"$repo/.context/zig/zig"

# A graph change must invalidate an earlier artifact PASS without relying on
# environment changes. Fake Cargo emits the incident EVEX when aes is locked.
printf '\n[[package]]\nname = "aes"\nversion = "0.9.3"\n' >>"$repo/Cargo.lock"
git -C "$repo" add Cargo.lock
git -C "$repo" commit -qm 'fixture dependency introduces EVEX'
if run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --reuse >"$tmp/cache-isa-evex.log" 2>&1; then
    bad 'changed Cargo.lock reused a release binary PASS containing seeded EVEX'
elif grep -qF 'FAIL release-binary-isa' "$tmp/cache-isa-evex.log" \
    && grep -qi 'vcvttsd2usi' "$tmp/cache-isa-evex.log" \
    && ! grep -qF 'PASS workspace-tests' "$tmp/cache-isa-evex.log"; then
    ok 'changed Cargo.lock invalidates release ISA PASS and refuses before later rows'
else
    bad "locked EVEX did not fail before pipeline: $(cat "$tmp/cache-isa-evex.log")"
fi
git -C "$repo" checkout HEAD~1 -- Cargo.lock
git -C "$repo" commit -qm 'restore baseline locked dependency graph'
run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --reuse >"$tmp/cache-isa-restored.log" 2>&1
if awk -F '\t' '$1 == "release-binary-isa" && $7 == "REUSED" {found=1} END {exit !found}' "$CAS_RELEASE_GATE_LOG_DIR/timing.tsv"; then
    ok 'restored locked code reuses the genuine prior release ISA PASS'
else
    bad 'restored baseline did not reuse the matching artifact receipt'
fi
printf '// Rust-only fix\n' >>"$repo/cas-cli/tests/smoke.rs"
git -C "$repo" add .
git -C "$repo" commit -qm 'fixture Rust fix'
run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --reuse >"$tmp/cache-rust.log" 2>&1
if [[ "$(awk -F '\t' '$7 == "REUSED" {print $1}' "$CAS_RELEASE_GATE_LOG_DIR/timing.tsv")" == $'hub-web-tests\nhub-web-dist-drift\nhub-web-visual-qa' ]]; then
    ok 'Rust-only commit reuses web evidence and reruns all Rust-dependent rows'
else
    bad "Rust change cache invalidation failed: $(cat "$CAS_RELEASE_GATE_LOG_DIR/timing.tsv")"
fi
printf '// web change\n' >"$repo/hub-web/scripts/changed.mjs"
git -C "$repo" add .
git -C "$repo" commit -qm 'fixture web fix'
run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --reuse >"$tmp/cache-web.log" 2>&1
if ! grep -q REUSED "$CAS_RELEASE_GATE_LOG_DIR/timing.tsv"; then
    ok 'web change invalidates web evidence and conservative Rust evidence'
else
    bad 'web change retained stale web evidence'
fi
# Corrupt, expired and future receipts are cache misses, not authorization.
for bad_epoch in 1000000000 9999999999 malformed; do
    for receipt in "$CAS_RELEASE_GATE_CACHE_DIR"/*; do
        read -r key sha epoch status <"$receipt"
        printf '%s %s %s PASS\n' "$key" "$sha" "$bad_epoch" >"$receipt"
    done
    run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --reuse >"$tmp/cache-invalid.log" 2>&1
    if ! grep -q REUSED "$CAS_RELEASE_GATE_LOG_DIR/timing.tsv"; then
        ok "cache rejects receipt epoch $bad_epoch"
    else
        bad "cache trusted receipt epoch $bad_epoch"
    fi
done
# --only neither reads nor populates the full-gate cache.
before="$(sha256sum "$CAS_RELEASE_GATE_CACHE_DIR"/*)"
run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --only nextest >"$tmp/cache-only.log" 2>&1
after="$(sha256sum "$CAS_RELEASE_GATE_CACHE_DIR"/*)"
if [[ "$before" == "$after" ]] && ! grep -q REUSED "$CAS_RELEASE_GATE_LOG_DIR/timing.tsv"; then
    ok 'diagnostic rows cannot read or modify full-gate row receipts'
else
    bad 'diagnostic row modified the authorization cache'
fi
# Environment mutations and failed fresh attempts cannot inherit old PASS.
GATE_FIXTURE_DOCTEST_FAIL=1 run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --reuse >"$tmp/cache-env.log" 2>&1 && bad 'changed failing environment reused PASS'
assert_named_failure doctests "$(cat "$tmp/cache-env.log")"
# An uncommitted source edit must never be vouched for with old evidence.
printf '// dirty edit\n' >>"$repo/cas-cli/tests/smoke.rs"
run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --reuse >"$tmp/cache-dirty.log" 2>&1 && bad 'dirty tree authorized'
if ! grep -q REUSED "$CAS_RELEASE_GATE_LOG_DIR/timing.tsv"; then
    ok 'dirty tree cannot reuse prior PASS'
else
    bad 'dirty tree reused prior evidence'
fi
unset CAS_RELEASE_GATE_CACHE_DIR CAS_RELEASE_GATE_LOG_DIR

# cas-1925. A passing rolling assembly sweep writes the same row receipt shape
# as the gate, but under the shared merge-sweep directory because its detached
# checkout is not the release worktree. The gate may consume only the
# equivalent nextest row when integration.json authorizes this exact tip.
repo="$(new_fixture assembly-row-cache)"
assembly_gate_cache="$tmp/assembly-gate-cache"
assembly_gate_logs="$tmp/assembly-gate-logs"
export CAS_RELEASE_GATE_CACHE_DIR="$assembly_gate_cache"
export CAS_RELEASE_GATE_LOG_DIR="$assembly_gate_logs"
run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 >"$tmp/assembly-first.log" 2>&1 || {
    cat "$tmp/assembly-first.log"
    exit 1
}
assembly_receipt="$(find "$assembly_gate_cache" -maxdepth 1 -type f -name 'nextest.*' -print -quit)"
mkdir -p "$repo/.cas/merge-sweeps/row-cache"
cp "$assembly_receipt" "$repo/.cas/merge-sweeps/row-cache/"
printf '{"status":"PASSED","tip":"%s"}\n' "$(git -C "$repo" rev-parse HEAD)" \
    >"$repo/.cas/merge-sweeps/integration.json"
rm -f "$assembly_receipt"
rm -rf "$assembly_gate_cache"
mkdir -p "$assembly_gate_cache"
run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --reuse >"$tmp/assembly-reuse.log" 2>&1
if grep -qF 'source=assembly sweep' "$assembly_gate_logs/nextest.log" \
    && grep -qF 'source_sha='"$(git -C "$repo" rev-parse HEAD)" "$assembly_gate_logs/nextest.log" \
    && [[ "$(awk -F '\t' '$1 == "nextest" && $7 == "REUSED" {n++} END {print n+0}' "$assembly_gate_logs/timing.tsv")" == 1 ]]; then
    ok 'assembly sweep receipt reuses the matching nextest row and names its SHA'
else
    bad "assembly sweep receipt was not consumed: $(cat "$assembly_gate_logs/nextest.log" 2>/dev/null || true)"
fi

# cas-846f: a from-main release's full gate reuses nothing, even with a green
# sweep receipt and a matching row receipt in place, and refuses --reuse.
if CAS_RELEASE_GATE_NO_REUSE=1 run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --reuse \
    >"$tmp/no-reuse-refused.log" 2>&1; then
    bad 'CAS_RELEASE_GATE_NO_REUSE accepted --reuse'
elif grep -qF -- '--reuse refused' "$tmp/no-reuse-refused.log"; then
    ok 'CAS_RELEASE_GATE_NO_REUSE refuses --reuse'
else
    bad "CAS_RELEASE_GATE_NO_REUSE failed without naming --reuse: $(cat "$tmp/no-reuse-refused.log")"
fi
CAS_RELEASE_GATE_NO_REUSE=1 run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 \
    >"$tmp/no-reuse-fresh.log" 2>&1
if [[ "$(awk -F '\t' '$7 == "REUSED" {n++} END {print n+0}' "$assembly_gate_logs/timing.tsv")" == 0 ]] \
    && grep -q '^nextest' "$assembly_gate_logs/timing.tsv"; then
    ok 'a no-reuse full gate runs every row fresh beside a green sweep receipt'
else
    bad "a no-reuse full gate reused evidence: $(cat "$assembly_gate_logs/timing.tsv")"
fi

printf '{"status":"FAILED","tip":"%s"}\n' "$(git -C "$repo" rev-parse HEAD)" \
    >"$repo/.cas/merge-sweeps/integration.json"
run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --reuse >"$tmp/assembly-red.log" 2>&1
if [[ "$(awk -F '\t' '$1 == "nextest" && $7 == "REUSED" {n++} END {print n+0}' "$assembly_gate_logs/timing.tsv")" == 0 ]]; then
    ok 'a non-green assembly sweep cannot authorize an old row receipt'
else
    bad 'a non-green assembly sweep authorized a stale row receipt'
fi

printf '// changed input\n' >>"$repo/cas-cli/tests/smoke.rs"
git -C "$repo" add cas-cli/tests/smoke.rs
git -C "$repo" commit -qm 'fixture assembly input change'
printf '{"status":"PASSED","tip":"%s"}\n' "$(git -C "$repo" rev-parse HEAD~1)" \
    >"$repo/.cas/merge-sweeps/integration.json"
run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --reuse >"$tmp/assembly-changed.log" 2>&1
if [[ "$(awk -F '\t' '$1 == "nextest" && $7 == "REUSED" {n++} END {print n+0}' "$assembly_gate_logs/timing.tsv")" == 0 ]]; then
    ok 'a changed workspace input invalidates the assembly row receipt'
else
    bad 'a changed workspace input retained the assembly row receipt'
fi
unset CAS_RELEASE_GATE_CACHE_DIR CAS_RELEASE_GATE_LOG_DIR

# Full train entry point: assemble prove -> prep -> ledger -> detached gate.
repo="$(new_fixture train-proof)"
cp -R "$script_dir/release-train.d" "$repo/scripts/"
cp "$script_dir/release-train-resume.py" "$repo/scripts/"
# This fixture proves train/assembly receipt consumption, while the ISA cases
# above exercise real ELF refusal and auditing. Supply its already-proved ISA
# receipt at the copied gate's key seam: prep changes Cargo.lock, so a receipt
# seeded before prep would belong to a different input. Keep the real cache
# reader and all nine receipt fields, without adding a production bypass.
python3 - "$repo/scripts/release-gate.sh" <<'PY_TRAIN_ISA_RECEIPT'
from pathlib import Path
import sys
path = Path(sys.argv[1])
body = path.read_text()
seam = '    if "$reuse_rows" && [[ -n "$key" ]]; then\n'
assert body.count(seam) == 1, 'train ISA receipt seam changed'
seed = '''    if [[ "$name" == release-binary-isa && -n "$key" && -n "$cache_dir" ]]; then
        printf '%s %s %s PASS %s %s %s %s %s\\n' \\
            "$key" "$cache_head" "$(date +%s)" "$cache_checkout_identity" \\
            "$input_hash" "$env_fingerprint" "$cache_toolchain" \\
            "$cache_implementation_digest" >"$cache_dir/$name.$key"
    fi
'''
path.write_text(body.replace(seam, seed + seam))
PY_TRAIN_ISA_RECEIPT
# Any accidental execution of the auditor fails this fixture. The separate
# seeded-EVEX/baseline cases retain the real auditor and zigbuild stub.
cat >"$repo/scripts/test-check-portable-x86_64-isa.sh" <<'EOF'
#!/usr/bin/env bash
echo 'train fixture unexpectedly executed the ISA auditor' >&2
exit 1
EOF
# This regression exercises assembly onward, with no GitHub/toolchain preflight.
# Keep helper functions used by the nested integration fixtures while skipping
# this train fixture's GitHub/toolchain stage.
printf '\ncut_stage_preflight() { return 0; }\n' >>"$repo/scripts/release-train.d/preflight.sh"
cat >"$repo/scripts/bump-release-version.sh" <<'EOF'
#!/usr/bin/env bash
python3 - "$1" <<'PY_BUMP'
from pathlib import Path
import sys
for path in [Path('cas-cli/Cargo.toml'), *Path('crates').glob('*/Cargo.toml'), Path('Cargo.lock'), Path('CHANGELOG.md')]:
    path.write_text(path.read_text().replace('9.99.7', sys.argv[1]))
PY_BUMP
EOF
cat >"$repo/scripts/gen-builtin-reference-history.sh" <<'EOF'
#!/usr/bin/env bash
printf '{"train-ledger": []}\n' >cas-cli/src/builtins/reference-history.json
EOF
chmod +x "$repo/scripts/bump-release-version.sh" "$repo/scripts/gen-builtin-reference-history.sh"
mkdir -p "$repo/docs/release-notes"
printf 'reviewed release draft\n' >"$repo/docs/release-notes/2099-01-02-v9.99.8-slack.md"
printf 'version = 4\n' >"$repo/Cargo.lock"
for manifest in "$repo/cas-cli/Cargo.toml" "$repo"/crates/*/Cargo.toml; do
    name="$(sed -n 's/^name = "\([^"]*\)"/\1/p' "$manifest")"
    printf '\n[[package]]\nname = "%s"\nversion = "9.99.7"\n' "$name" >>"$repo/Cargo.lock"
done
git -C "$repo" add .
git -C "$repo" commit -qm 'seed real train proof sequence'
git -C "$repo" branch -M release/9.99.8
train_proof_sha="$(git -C "$repo" rev-parse HEAD)"
git -C "$repo" update-ref refs/remotes/origin/main "$train_proof_sha"
git -C "$repo" branch "integration/$(basename "$repo")" "$train_proof_sha"
mkdir -p "$repo/.cas/merge-sweeps"
printf '{"status":"PASSED","tip":"%s","base":"%s","epics":[]}\n' "$train_proof_sha" "$train_proof_sha" \
    >"$repo/.cas/merge-sweeps/integration.json"
: >"$tmp/cargo.log"
# The supervisor proof precedes the train's choice of output directories.
run_gate "$repo" '' python3 "$repo/scripts/assembly-proof.py" prove "$repo" >"$tmp/supervisor-proof.log" 2>&1
run_train_proof() {
    local stop="$1"
    shift
    run_gate "$repo" '' env CAS_RELEASE_ARTIFACTS_ROOT="$tmp/train-artifacts" \
        CAS_RELEASE_TRAIN_CAS=/bin/false CAS_RELEASE_TRAIN_CARGO="$repo/scripts/cargo-stub" \
        CAS_RELEASE_TRAIN_DATE=2099-01-02 CAS_RELEASE_TRAIN_PREFLIGHT_CMD=true \
        CAS_RELEASE_TRAIN_CUT_STOP_AFTER="$stop" CAS_RELEASE_TRAIN_CUT_POLL_SECS=1 \
        CAS_RELEASE_TRAIN_CUT_GATE_TRIES=15 "$repo/scripts/release-train.sh" 9.99.8 "$repo" --cut "$@"
}
# Resume the successful assembly receipt with a newly selected receipt output
# location: neither suite may re-run at assemble or the first full gate.
run_train_proof assemble >"$tmp/train-proof.log" 2>&1 || true
CAS_RELEASE_RECEIPTS_RUN_DIR="$tmp/train-artifacts/receipt-output" \
    run_train_proof gate --resume >>"$tmp/train-proof.log" 2>&1 || true
if grep -q 'stopped after stage assemble' "$tmp/train-proof.log" \
    && grep -q 'stopped after stage gate' "$tmp/train-proof.log"; then
    train_run="$tmp/train-artifacts/v9.99.8-train-proof"
    if [[ "$(grep -c '^nextest run --workspace.*--no-fail-fast' "$tmp/cargo.log")" == 1 ]] \
        && [[ "$(grep -c 'reused PASS assembly' "$train_run/gate.log")" == 3 ]] \
        && [[ "$(awk -F '\t' '$1 == "ci-script-tests" && $7 == "REUSED" {n++} END {print n+0}' "$train_run"/rows/*/timing.tsv)" == 1 ]] \
        && [[ "$(grep -c '^nextest run --archive-file ' "$tmp/cargo.log")" == 1 ]] \
        && [[ "$(grep -c '^zigbuild ' "$tmp/cargo.log" || true)" == 0 ]] \
        && grep -q '^Reused PASS ' "$train_run"/rows/*/release-binary-isa.log \
        && [[ "$(awk -F '\t' '$1 == "release-binary-isa" && $4 == 0 && $7 == "REUSED" {n++} END {print n+0}' "$train_run"/rows/*/timing.tsv)" == 1 ]] \
        && grep -q 'stage prep: done' "$tmp/train-proof.log" \
        && grep -q 'stage ledger: done' "$tmp/train-proof.log"; then
        ok 'real train assemble, prep, ledger and detached gate reuse both assembly contexts and the script tier'
    else
        bad "train sequence missed assembly reuse: $(cat "$tmp/cargo.log"); $(cat "$train_run/gate.log")"
    fi
else
    bad "real train proof sequence failed: $(cat "$tmp/train-proof.log"); gate: $(cat "$tmp/train-artifacts/v9.99.8-train-proof/gate.log" 2>/dev/null)"
fi

# Real two-context producer with Cargo stubbed, then the real first full gate.
repo="$(new_fixture two-context-proof)"
python3 - "$repo" <<'PYFIX'
from pathlib import Path
import sys
root = Path(sys.argv[1])
packages = []
for manifest in [root / 'cas-cli/Cargo.toml', *root.glob('crates/*/Cargo.toml')]:
    text = manifest.read_text()
    name = text.split('name = "', 1)[1].split('"', 1)[0]
    packages.append(f'[[package]]\nname = "{name}"\nversion = "9.99.7"\n')
    manifest.write_text(text + '[dependencies]\nthird-party = "0.2.0"\n')
(root / 'Cargo.lock').write_text('version = 4\n\n' + '\n'.join(packages) +
    '\n[[package]]\nname = "third-party"\nversion = "0.2.0"\n' +
    '\n[[package]]\nname = "non-member"\nversion = "0.4.0"\n')
PYFIX
git -C "$repo" add .
git -C "$repo" commit -qm 'seed prep manifest and lock fixture'
proof_sha="$(git -C "$repo" rev-parse HEAD)"
export CAS_RELEASE_GATE_LOG_DIR="$tmp/proof-gate-logs"
: >"$tmp/cargo.log"
run_gate "$repo" '' python3 "$repo/scripts/assembly-proof.py" prove "$repo" >"$tmp/proof.log" 2>&1 || { cat "$tmp/proof.log"; exit 1; }
if [[ "$(grep -c '^nextest run .*--no-fail-fast' "$tmp/cargo.log")" == 2 ]] \
    && grep -qF 'contexts=worktree,clone' "$tmp/proof.log"; then
    ok 'assembly proves native nextest and archive-mode in a plain clone exactly twice'
else
    bad "assembly suite count: $(cat "$tmp/cargo.log")"
fi
if python3 - "$tmp/proof.log" <<'PY_PHASE_RECEIPT'
import json
from pathlib import Path
import re
import sys
path = re.search(r'PASS assembly receipt=(\S+)', Path(sys.argv[1]).read_text())[1]
record = json.loads(Path(path).read_text())
assert record['inputs']['format'] == 2
assert record['execution']['mode'] == 'concurrent'
assert set(record['contexts']) == {'worktree', 'clone'}
for row in (record['script_tests'], *record['contexts'].values()):
    assert row['timing']['row'] == row['row']
    assert float(row['timing']['wall_s']) >= 0
    assert row['timing']['average_cores_busy'] >= 0
    assert row['timing']['started_utc'] <= row['timing']['ended_utc']
for row in record['contexts'].values():
    assert row['compile_timing']['row'] == row['row']
    assert float(row['compile_timing']['user_s']) >= 0
    assert float(row['compile_timing']['system_s']) >= 0
    assert row['compile_timing']['average_cores_busy'] >= 0
assert [item['phase'] for item in record['execution']['phases']] == ['nextest-tests', 'archive-mode-tests']
PY_PHASE_RECEIPT
then
    ok 'assembly receipt keeps all legs and captures per-leg/compile intervals plus memory admission'
else
    bad 'assembly phase timing or memory receipt regressed'
fi
run_gate "$repo" '' python3 "$repo/scripts/assembly-proof.py" prove "$repo" >"$tmp/proof-retry.log" 2>&1
if [[ "$(grep -c '^nextest run .*--no-fail-fast' "$tmp/cargo.log")" == 2 ]] \
    && grep -qF "source_sha=$proof_sha" "$tmp/proof-retry.log"; then
    ok 'assemble prove reuses the supervisor receipt and runs no additional suite'
else
    bad 'assemble prove reran an existing matching receipt'
fi
run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 >"$tmp/proof-gate.log" 2>&1
if [[ "$(awk -F '\t' '$7 == "REUSED" {n++} END {print n+0}' "$CAS_RELEASE_GATE_LOG_DIR/timing.tsv")" == 3 ]] \
    && grep -qF 'PASS assembly receipt=' "$CAS_RELEASE_GATE_LOG_DIR/archive-mode.log" \
    && grep -qF 'PASS assembly receipt=' "$CAS_RELEASE_GATE_LOG_DIR/ci-script-tests.log" \
    && [[ "$(grep -c '^nextest run --archive-file ' "$tmp/cargo.log")" == 1 ]]; then
    ok 'assembly retry and first full gate cite all three proved rows (cas-398c: ci-script-tests too) without rerunning the archive suite'
else
    bad "first gate missed assembly proof: $(cat "$tmp/proof-gate.log")"
fi
# Exact real-cut prep/ledger delta must retain the original proof SHA.
python3 - "$repo" <<'PYFIX'
from pathlib import Path
import sys
root = Path(sys.argv[1])
for manifest in [root / 'cas-cli/Cargo.toml', *root.glob('crates/*/Cargo.toml')]:
    manifest.write_text(manifest.read_text().replace('version = "9.99.7"', 'version = "9.99.8"'))
lock = root / 'Cargo.lock'
lock.write_text(lock.read_text().replace('version = "9.99.7"', 'version = "9.99.8"'))
(root / 'cas-cli/src/builtins/reference-history.json').write_text('{"prep": []}\n')
changelog = root / 'CHANGELOG.md'
changelog.write_text(changelog.read_text().replace('9.99.7', '9.99.8'))
PYFIX
git -C "$repo" add .
git -C "$repo" commit -qm 'simulate prep and ledger'
run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.8 >"$tmp/proof-prep.log" 2>&1
if [[ "$(awk -F '\t' '$1 ~ /^(nextest|archive-mode|ci-script-tests)$/ && $7 == "REUSED" {n++} END {print n+0}' "$CAS_RELEASE_GATE_LOG_DIR/timing.tsv")" == 3 ]] \
    && grep -qF "source_sha=$proof_sha" "$CAS_RELEASE_GATE_LOG_DIR/nextest.log" \
    && grep -qF "source_sha=$proof_sha" "$CAS_RELEASE_GATE_LOG_DIR/ci-script-tests.log" \
    && grep -qF "source_sha=$proof_sha" "$CAS_RELEASE_GATE_LOG_DIR/archive-mode.log" \
    && [[ "$(grep -c '^nextest run --archive-file ' "$tmp/cargo.log")" == 1 ]]; then
    ok 'prep member versions, lock and ledger reuse all three proved rows and cite the original proof SHA'
else
    bad "real-cut prep missed assembly proof: $(cat "$tmp/proof-prep.log")"
fi
for change in manifest-dependency lock-dependency non-member; do
    changed_file=Cargo.lock
    [[ "$change" != manifest-dependency ]] || changed_file=cas-cli/Cargo.toml
    python3 - "$repo/$changed_file" "$change" <<'PYFIX'
from pathlib import Path
import sys
path = Path(sys.argv[1])
text = path.read_text()
if sys.argv[2] == 'manifest-dependency':
    text = text.replace('third-party = "0.2.0"', 'third-party = "0.3.0"')
elif sys.argv[2] == 'lock-dependency':
    text = text.replace('name = "third-party"\nversion = "0.2.0"', 'name = "third-party"\nversion = "0.3.0"')
else:
    text = text.replace('name = "non-member"\nversion = "0.4.0"', 'name = "non-member"\nversion = "0.5.0"')
path.write_text(text)
PYFIX
    git -C "$repo" add "$changed_file"
    git -C "$repo" commit -qm "fixture $change changed"
    run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.8 >"$tmp/proof-$change.log" 2>&1
    if [[ "$(awk -F '\t' '$1 ~ /^(nextest|archive-mode|ci-script-tests)$/ && $7 == "REUSED" {n++} END {print n+0}' "$CAS_RELEASE_GATE_LOG_DIR/timing.tsv")" == 0 ]]; then
        ok "$change version invalidates every assembly row"
    else
        bad "$change version incorrectly reused assembly proof"
    fi
    git -C "$repo" restore --source=HEAD~1 -- "$changed_file"
    git -C "$repo" add "$changed_file"
    git -C "$repo" commit -qm 'restore fixture dependency'
done
mkdir -p "$repo/docs/release-notes"
printf 'release prose\n' >"$repo/docs/release-notes/new.md"
git -C "$repo" add docs/release-notes/new.md
git -C "$repo" commit -qm 'release prose'
if run_gate "$repo" '' python3 "$repo/scripts/assembly-proof.py" check "$repo" >"$tmp/proof-docs.log" 2>&1; then
    ok 'release prose commit retains the tested code proof'
else
    bad 'release prose invalidated assembly proof'
fi
: >"$tmp/cargo.log"
printf '// code change\n' >>"$repo/cas-cli/src/version.rs"
git -C "$repo" add cas-cli/src/version.rs
git -C "$repo" commit -qm 'code changed'
if run_gate "$repo" '' python3 "$repo/scripts/assembly-proof.py" check "$repo" >/dev/null 2>&1; then
    bad 'changed code reused assembly proof'
else
    ok 'changed code misses the assembly proof'
fi
run_gate "$repo" '' python3 "$repo/scripts/assembly-proof.py" prove "$repo" >"$tmp/proof-changed.log" 2>&1
if [[ "$(grep -c '^nextest run --archive-file ' "$tmp/cargo.log")" == 1 ]]; then
    ok 'changed candidate reruns both contexts'
else
    bad 'changed candidate did not rerun clone proof'
fi
if run_gate "$repo" GATE_FIXTURE_ARCHIVE_FAIL python3 "$repo/scripts/assembly-proof.py" prove "$repo" >"$tmp/proof-fail.log" 2>&1; then
    bad 'failed clone run published a PASS'
else
    ok 'failed clone run cannot publish a PASS receipt'
fi
if run_gate "$repo" GATE_FIXTURE_ARCHIVE_FAIL python3 "$repo/scripts/assembly-proof.py" check "$repo" >/dev/null 2>&1; then
    bad 'failed clone run was reused'
else
    ok 'failed or incomplete two-context proof is a cache miss'
fi
if run_gate "$repo" GATE_FIXTURE_EMPTY_SUITE python3 "$repo/scripts/assembly-proof.py" prove "$repo" >"$tmp/proof-empty.log" 2>&1; then
    bad 'zero-test producer published proof'
else
    ok 'zero-test assembly cannot publish a PASS receipt'
fi
: >"$tmp/cargo.log"
run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --only nextest >/dev/null
if [[ "$(grep -c '^nextest run --workspace' "$tmp/cargo.log")" == 1 ]]; then
    ok 'diagnostic nextest always runs despite matching assembly proof'
else
    bad 'diagnostic consumed an assembly proof'
fi
unset CAS_RELEASE_GATE_LOG_DIR

# Admission barriers exercise the real shell seam with fake Cargo. A compiled
# producer must wait for script PASS/test serialization, then abort promptly
# when another leg fails, keeping compile stderr out of numeric timing fields.
repo="$(new_fixture assembly-admission)"
for row in nextest archive-mode; do
    sync="$tmp/sync-$row"
    logs="$tmp/sync-$row-logs"
    mkdir -p "$sync"
    printf '%s' "$$" >"$sync/owner"
    : >"$tmp/cargo.log"
    CAS_RELEASE_GATE_ASSEMBLY_SYNC_DIR="$sync" CAS_RELEASE_GATE_LOG_DIR="$logs" \
        run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 --only "$row" \
        >"$tmp/sync-$row.log" 2>&1 &
    gate_pid=$!
    for ((attempt=0; attempt<200; attempt++)); do
        [[ ! -e "$sync/compiled-$row" ]] || break
        kill -0 "$gate_pid" 2>/dev/null || break
        sleep 0.05
    done
    if [[ -e "$sync/compiled-$row" ]] && ! grep -q -- '--no-fail-fast' "$tmp/cargo.log"; then
        ok "$row compiles while its test consumer waits for admission"
    else
        bad "$row failed to stop at test admission: $(cat "$tmp/sync-$row.log")"
    fi
    if [[ "$row" == nextest ]]; then
        printf '2' >"$sync/release-$row"
        if wait "$gate_pid" && grep -qF "PASS $row" "$tmp/sync-$row.log"; then
            ok 'native consumer starts only after its admitted test slot is released'
        else
            bad "native test slot did not release: $(cat "$tmp/sync-$row.log")"
        fi
    else
        touch "$sync/abort"
        if wait "$gate_pid"; then
            bad 'aborted archive consumer published PASS'
        elif grep -qF 'assembly test admission aborted' "$logs/archive-mode.log" \
            && ! grep -q -- '--no-fail-fast' "$tmp/cargo.log"; then
            ok 'archive consumer aborts without running tests or publishing PASS'
        else
            bad "archive admission abort failed: $(cat "$tmp/sync-$row.log")"
        fi
    fi
done

# The fixture has 64 GiB total, 60 available. A 44 GiB reserve admits one
# producer, not two: this is real serial dispatch through both shell rows.
repo="$(new_fixture assembly-serial-memory)"
CAS_RELEASE_GATE_ASSEMBLY_RESERVE_GIB=44 run_gate "$repo" '' \
    python3 "$repo/scripts/assembly-proof.py" prove "$repo" >"$tmp/serial-proof.log" 2>&1
if grep -qF '"mode": "serial"' "$tmp/serial-proof.log" \
    && grep -qF 'insufficient available memory' "$tmp/serial-proof.log" \
    && grep -qF 'PASS assembly receipt=' "$tmp/serial-proof.log"; then
    ok 'memory reserve selects the serial path and retains all proof legs'
else
    bad "serial memory fallback failed: $(cat "$tmp/serial-proof.log")"
fi

printf '\n%s passed, %s failed\n' "$pass" "$fail"
test "$fail" -eq 0
