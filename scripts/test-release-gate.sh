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

pass=0
fail=0

ok() { printf 'ok   %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL %s\n' "$1"; fail=$((fail + 1)); }

new_fixture() {
    local name="$1" repo
    repo="$tmp/$name"
    mkdir -p "$repo/scripts" "$repo/cas-cli/src" "$repo/cas-cli/tests" "$repo/crates"
    cp "$gate" "$repo/scripts/release-gate.sh"
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
        printf '[package]\nname = "%s"\nversion = "9.8.7"\n' "$crate" >"$file"
    done
    cat >"$repo/CHANGELOG.md" <<'EOF'
## [Unreleased]

## [9.8.7] - 2026-09-02

- Fixture release.
EOF
    cat >"$repo/Cargo.lock" <<'EOF'
# fixture lockfile
EOF
    cat >"$repo/scripts/cargo-stub" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${GATE_FIXTURE_CARGO_LOG:?}"
if [[ "$*" == 'check --workspace --tests' && "${GATE_FIXTURE_CHECK_FAIL:-}" == 1 ]]; then exit 1; fi
if [[ "$*" == 'nextest run -p cas'* && "${GATE_FIXTURE_NEXTEST_FAIL:-}" == 1 ]]; then exit 1; fi
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
    printf '%s' "$repo"
}

run_gate() {
    local repo="$1" failure_variable="${2:-}"
    shift 2
    if [[ -n "$failure_variable" ]]; then
        (cd "$repo" && \
          env "$failure_variable=1" \
          GATE_FIXTURE_CARGO_LOG="$tmp/cargo.log" \
          CARGO="$repo/scripts/cargo-stub" \
          RELEASE_GATE_GEN_REFERENCE_HISTORY="$repo/scripts/gen-builtin-reference-history.sh" \
          "$@")
    else
        (cd "$repo" && \
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
    for name in failure-log version-literals workspace-tests nextest doctests archive-mode \
        snapshot-portability builtin-projections changelog-and-versions release-script \
        procedure-guardrails working-tree; do
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
    output="$(run_gate "$repo" "$variable" "$repo/scripts/release-gate.sh" 9.8.7 2>&1 || true)"
    assert_named_failure "$3" "$output"
}

# 1-7. Each mechanical or command-backed failure is isolated in its own repo.
repo="$(new_fixture version-literal)"
printf 'const VERSION: &str = "9.8.7";\n' >"$repo/cas-cli/src/version.rs"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.8.7 2>&1 || true)"
assert_named_failure version-literals "$output"

run_scenario workspace-check GATE_FIXTURE_CHECK_FAIL workspace-tests
run_scenario nextest-run GATE_FIXTURE_NEXTEST_FAIL nextest
run_scenario doctest-run GATE_FIXTURE_DOCTEST_FAIL doctests
run_scenario archive-run GATE_FIXTURE_ARCHIVE_FAIL archive-mode
run_scenario snapshot-run GATE_FIXTURE_SNAPSHOT_FAIL snapshot-portability
run_scenario projection-run GATE_FIXTURE_DRIFT_FAIL builtin-projections

# 8. Ledger regeneration must be compared to the committed file.
run_scenario reference-ledger GATE_FIXTURE_REFERENCE_FAIL builtin-projections

# 9. Changelog/version contract and clean-tree contract are independent.
repo="$(new_fixture changelog-failure)"
sed -i '/Fixture release/d' "$repo/CHANGELOG.md"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.8.7 2>&1 || true)"
assert_named_failure changelog-and-versions "$output"

repo="$(new_fixture dirty-tree)"
printf 'untracked\n' >"$repo/untracked.txt"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.8.7 2>&1 || true)"
assert_named_failure working-tree "$output"

repo="$(new_fixture invalid-failure-log)"
printf '%s\n' '- 2026-09-02 — **not-a-gate-check** — Symptom: unparseable. Root cause: fixture. Release: fixture.' >>"$repo/cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.8.7 2>&1 || true)"
assert_named_failure failure-log "$output"

repo="$(new_fixture learn-mode)"
learn_output="$(cd "$repo" && "$repo/scripts/release-gate.sh" --learn 'new release symptom' 'new release cause' 'procedure-guardrails' 2>&1)"
grep -qF 'Learned release failure in all three mirrors' <<<"$learn_output"
grep -qF 'new release symptom' "$repo/cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md"
cmp "$repo/cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md" \
    "$repo/cas-cli/src/builtins/codex/skills/cas-cut-release/references/failure-log.md"
cmp "$repo/cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md" \
    "$repo/cas-cli/src/builtins/grok/skills/cas-cut-release/references/failure-log.md"
ok '--learn appends and mirrors a dated failure entry'

# cas-4ccc. A populated .cas/proxy.toml ABOVE the worktree is readable by any
# test that resolves project config by walking up from its cwd. The gate must
# name that file rather than let three unrelated proxy tests fail as if the
# release broke them.
repo="$(new_fixture ancestor-proxy)"
mkdir -p "$(dirname "$repo")/.cas"
printf '[servers.mecha-cassy]\ntype = "http"\nurl = "https://example.invalid/mcp"\n' \
    >"$(dirname "$repo")/.cas/proxy.toml"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.8.7 2>&1 || true)"
assert_named_failure ancestor-proxy-config "$output"
if grep -qF '.cas/proxy.toml' <<<"$output"; then
    ok 'ancestor proxy.toml failure names the exact leaking file'
else
    bad 'ancestor proxy.toml failure did not name the file'
fi
# Remove the whole directory, not just the file: later scenarios assert that no
# ancestor of their scratch base holds a .cas store at all, and an empty
# leftover would fail them for this fixture's reason.
rm -rf "$(dirname "$repo")/.cas"

# The repository's OWN .cas/proxy.toml is where a project config belongs and
# must not trip the check — only a store above the worktree is the leak.
repo="$(new_fixture own-proxy)"
mkdir -p "$repo/.cas"
printf '[servers.local]\ntype = "http"\nurl = "https://example.invalid/mcp"\n' \
    >"$repo/.cas/proxy.toml"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.8.7 2>&1 || true)"
if grep -qF 'FAIL ancestor-proxy-config' <<<"$output"; then
    bad "the repository's own .cas/proxy.toml must not trip the ancestor check"
else
    ok "the repository's own .cas/proxy.toml is not treated as an ancestor leak"
fi

repo="$(new_fixture passing)"
output="$(run_gate "$repo" '' "$repo/scripts/release-gate.sh" 9.8.7 2>&1)"
assert_all_pass "$output"

printf '\n%s passed, %s failed\n' "$pass" "$fail"
test "$fail" -eq 0
