#!/usr/bin/env bash
# Fixture-driven self-test for scripts/release-gate.sh.
#
# The fixtures deliberately keep Cargo and nextest fake: the gate's contract is
# about dispatching every release check and failing closed with a named reason,
# not about spending a release's build time in its own unit test.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
gate="$script_dir/release-gate.sh"
tmp="$(mktemp -d)"
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
    mkdir -p "$repo/scripts" "$repo/cas-cli/src" "$repo/cas-cli/tests" "$repo/crates" \
        "$repo/.context/zig"
    cp "$gate" "$repo/scripts/release-gate.sh"
    cat >"$repo/.gitignore" <<'EOF'
.context/zig/
EOF
    printf '%s\n' '#!/usr/bin/env bash' 'exit 0' >"$repo/.context/zig/zig"
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
  : > cas-cli/src/builtins/reference-history.json
fi
EOF
    chmod +x "$repo/scripts"/*.sh
    mkdir -p "$repo/cas-cli/src/builtins"
    : >"$repo/cas-cli/src/builtins/reference-history.json"
    for mirror in \
        "$repo/cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md" \
        "$repo/cas-cli/src/builtins/codex/skills/cas-cut-release/references/failure-log.md" \
        "$repo/cas-cli/src/builtins/grok/skills/cas-cut-release/references/failure-log.md"; do
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
# cas-c0411: every gate child must see the raised `cas init` watchdog budget,
# because the child that hit the 300s default was a test's `cas init`, several
# processes below the gate.
printf 'CAS_INIT_TIMEOUT_SECS=%s :: %s\n' "${CAS_INIT_TIMEOUT_SECS:-unset}" "$*" \
  >>"${GATE_FIXTURE_ENV_LOG:-/dev/null}"
printf 'ZIG=%s :: %s\n' "${ZIG:-unset}" "$*" \
  >>"${GATE_FIXTURE_ZIG_LOG:-/dev/null}"
if [[ "$*" == 'check --workspace --tests' && "${GATE_FIXTURE_CHECK_FAIL:-}" == 1 ]]; then exit 1; fi
if [[ "$*" == 'nextest run -p cas'* && "${GATE_FIXTURE_NEXTEST_FAIL:-}" == 1 ]]; then exit 1; fi
if [[ "$*" == *'builtin_archive_portability_test'* && "${GATE_FIXTURE_FIXTURE_PATHS_FAIL:-}" == 1 ]]; then exit 1; fi
if [[ "$*" == 'test -p cas --doc' && "${GATE_FIXTURE_DOCTEST_FAIL:-}" == 1 ]]; then exit 1; fi
if [[ "${GATE_FIXTURE_SNAPSHOT_FAIL:-}" == 1 ]]; then
  case "$*" in *component_output_test*) exit 1;; esac
fi
if [[ "${GATE_FIXTURE_DRIFT_FAIL:-}" == 1 ]]; then
  case "$*" in *builtin_flavor_drift_test*) exit 1;; esac
fi
if [[ "$*" == 'nextest archive -p cas'* ]]; then
  archive_file=''
  for arg in "$@"; do [[ "$arg" == *.tar.zst ]] && archive_file="$arg"; done
  [[ -n "$archive_file" ]] && printf archive >"$archive_file"
  exit 0
fi
if [[ "$*" == 'nextest run --archive-file '* && "${GATE_FIXTURE_ARCHIVE_FAIL:-}" == 1 ]]; then exit 1; fi
EOF
    chmod +x "$repo/scripts/cargo-stub"
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
          CARGO="$repo/scripts/cargo-stub" \
          RELEASE_GATE_GEN_REFERENCE_HISTORY="$repo/scripts/gen-builtin-reference-history.sh" \
          "$@")
    else
        (cd "$repo" && \
          env -u ZIG -u CAS_RELEASE_EPIC_REF -u CAS_RELEASE_TRAIN_BRANCH \
          GATE_FIXTURE_CARGO_LOG="$tmp/cargo.log" \
          CARGO="$repo/scripts/cargo-stub" \
          RELEASE_GATE_GEN_REFERENCE_HISTORY="$repo/scripts/gen-builtin-reference-history.sh" \
          "$@")
    fi
}

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
    for name in scratch-base epic-worktree-fresh epic-worktree-zig failure-log ancestor-proxy-config \
        version-literals fixture-paths workspace-tests nextest doctests archive-mode snapshot-portability \
        builtin-projections changelog-and-versions release-script procedure-guardrails working-tree; do
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

# 1-7. Each mechanical or command-backed failure is isolated in its own repo.
repo="$(new_fixture version-literal)"
printf 'const VERSION: &str = "9.99.7-rc.1";\n' >"$repo/cas-cli/src/version.rs"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.99.7 2>&1 || true)"
assert_named_failure version-literals "$output"

run_scenario workspace-check GATE_FIXTURE_CHECK_FAIL workspace-tests
run_scenario nextest-run GATE_FIXTURE_NEXTEST_FAIL nextest
run_scenario doctest-run GATE_FIXTURE_DOCTEST_FAIL doctests
run_scenario archive-run GATE_FIXTURE_ARCHIVE_FAIL archive-mode
run_scenario snapshot-run GATE_FIXTURE_SNAPSHOT_FAIL snapshot-portability
run_scenario projection-run GATE_FIXTURE_DRIFT_FAIL builtin-projections
run_scenario fixture-paths-run GATE_FIXTURE_FIXTURE_PATHS_FAIL fixture-paths

# 8. Ledger regeneration must be compared to the committed file.
run_scenario reference-ledger GATE_FIXTURE_REFERENCE_FAIL builtin-projections

# 9. Changelog/version contract and clean-tree contract are independent.
repo="$(new_fixture changelog-failure)"
sed -i '/Fixture release/d' "$repo/CHANGELOG.md"
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
    GATE_FIXTURE_ZIG_LOG="$zig_log" \
    CARGO="$epic_worktree/scripts/cargo-stub" \
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
grep -qF 'Learned release failure in all three mirrors' <<<"$learn_output"
grep -qF 'Regenerated builtin reference history after --learn' <<<"$learn_output"
grep -qF 'changed ledger' "$repo/cas-cli/src/builtins/reference-history.json"
grep -qF 'new release symptom' "$repo/cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md"
cmp "$repo/cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md" \
    "$repo/cas-cli/src/builtins/codex/skills/cas-cut-release/references/failure-log.md"
cmp "$repo/cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md" \
    "$repo/cas-cli/src/builtins/grok/skills/cas-cut-release/references/failure-log.md"
ok '--learn appends and mirrors a dated failure entry'

# cas-4ccc. A populated .cas/proxy.toml ABOVE the worktree is readable by any
# test that resolves project config by walking up from its cwd. The gate must
# neutralize it and name it — never refuse, because blocking a release on the
# operator's own MCP configuration is how the original hour was lost.
repo="$(new_fixture ancestor-proxy)"
mkdir -p "$(dirname "$repo")/.cas"
printf '[servers.mecha-cassy]\ntype = "http"\nurl = "https://example.invalid/mcp"\n' \
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
      CARGO="$repo/scripts/cargo-stub" \
      RELEASE_GATE_GEN_REFERENCE_HISTORY="$repo/scripts/gen-builtin-reference-history.sh" \
      "$repo/scripts/release-gate.sh" 9.99.7)
}

repo="$(new_fixture default-scratch-base)"
output="$(run_gate_unset_home "$repo" 2>&1 || true)"
if grep -qF 'scratch base: /var/tmp/cas-release-gate (from default)' <<<"$output" \
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
    CARGO="$repo/scripts/cargo-stub" \
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
output="$(cd "$repo" && GATE_FIXTURE_CARGO_LOG="$tmp/cargo.log" \
    CARGO="$repo/scripts/cargo-stub" CAS_RELEASE_GATE_ARCHIVE_SIZE_FILE="$archive_receipt" \
    "$repo/scripts/release-gate.sh" 9.99.7 --only archive-mode 2>&1)"
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
      CARGO="$repo/scripts/cargo-stub" \
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

printf '\n%s passed, %s failed\n' "$pass" "$fail"
test "$fail" -eq 0
