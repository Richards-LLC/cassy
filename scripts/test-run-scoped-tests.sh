#!/usr/bin/env bash
#
# Self-test for scripts/run-scoped-tests.sh (cas-a967 / GH #173).
#
# Proves the guard actually fails on each silent-success shape observed in
# GH #173, and does NOT fail a genuine green run — the second half being the
# one that makes the first half worth having.
#
# The shapes are driven through the REAL wrapper end-to-end by replacing the
# cargo binary with a stub that replays captured output and exits with the
# captured status. That is deliberate, not a shortcut:
#
#   - Shape 2 (relative $ZIG) would otherwise need a real ghostty_vt_sys
#     rebuild against a deliberately broken toolchain — minutes of wall-clock
#     and a poisoned build cache, to observe a failure whose text we already
#     have from the incident.
#   - Shape 3 needs a compiled 3929-test binary to reproduce honestly.
#
# Every assertion still runs the wrapper's real argument handling, real
# preflight, real parsing and real verdict. Only cargo is stubbed, and the
# stub output is copied from the GH #173 run logs.
#
# Usage: scripts/test-run-scoped-tests.sh
# Exit codes: 0 = all cases behaved, 1 = the guard is broken.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GUARD="${SCRIPT_DIR}/run-scoped-tests.sh"

tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT

pass_count=0
fail_count=0

# make_stub <name> <exit-status> <<<"output"
make_stub() {
    local name="$1" status="$2" stub="${tmpdir}/$1"
    {
        echo '#!/usr/bin/env bash'
        echo 'printf "argv: %s\n" "$*"'
        echo "cat <<'STUB_EOF'"
        cat
        echo 'STUB_EOF'
        echo "touch \"${tmpdir}/${name}.invoked\""
        echo "exit ${status}"
    } >"${stub}"
    chmod +x "${stub}"
    echo "${stub}"
}

# expect <pass|fail> <expected-output-substring, or "-"> <description> <command...>
#
# The substring matters as much as the status, and for the same reason this
# whole task exists: a guard that exits nonzero for the WRONG reason is a
# guard whose individual checks are not actually proven. Asserting only the
# exit code let two mutants survive during development — disabling the
# "no harness reported" check and disabling the relative-$ZIG preflight both
# left this suite fully green, because a sibling check happened to catch the
# same case with a misleading message.
expect() {
    local want="$1" match="$2" desc="$3"
    shift 3
    local out status
    out="$("$@" 2>&1)"
    status=$?

    local ok=1 why=""
    if [[ "${want}" == "pass" && "${status}" -ne 0 ]]; then
        ok=0
        why="expected exit 0"
    fi
    if [[ "${want}" == "fail" && "${status}" -eq 0 ]]; then
        ok=0
        why="expected a nonzero exit"
    fi
    if [[ "${ok}" -eq 1 && "${match}" != "-" ]] && ! grep -qF -- "${match}" <<<"${out}"; then
        ok=0
        why="right status, WRONG reason — output does not mention: ${match}"
    fi

    if [[ "${ok}" -eq 1 ]]; then
        pass_count=$((pass_count + 1))
        printf 'ok   %s (exit %s, expected %s)\n' "${desc}" "${status}" "${want}"
    else
        fail_count=$((fail_count + 1))
        printf 'FAIL %s (exit %s, %s)\n' "${desc}" "${status}" "${why}"
        echo "${out}" | sed 's/^/       | /'
    fi
}

echo "=== scoped-test guard self-test (GH #173) ==="
echo

# ---------------------------------------------------------------------------
# Shape 1 — wrong package name. The crate is `cas`, not `cas-cli`.
# ---------------------------------------------------------------------------
stub="$(make_stub cargo-wrong-pkg 101 <<'EOF'
error: package ID specification `cas-cli` did not match any packages
help: there is a similarly named package `cas`
EOF
)"
expect fail "The package name did not resolve" \
    "shape 1: unresolved package name (-p cas-cli)" \
    env CARGO="${stub}" "${GUARD}" -p cas-cli --lib some_module

# ---------------------------------------------------------------------------
# Shape 2 — build-script panic from a relative $ZIG. Two variants, because the
# dangerous one is the second: the incident's wrapper swallowed the status.
# ---------------------------------------------------------------------------
read -r -d '' BUILD_PANIC <<'EOF'
   Compiling ghostty_vt_sys v0.1.0 (/repo/vendor/ghostty_vt_sys)
error: failed to run custom build command for `ghostty_vt_sys v0.1.0`

Caused by:
  process didn't exit successfully: `/repo/target/debug/build/ghostty_vt_sys-abc/build-script-build` (exit status: 101)
  --- stderr
  thread 'main' panicked at build.rs:64:9:
  zig compiler not found: .context/zig/zig
  note: run with `RUST_BACKTRACE=1` to display a backtrace
EOF

stub="$(make_stub cargo-build-panic 101 <<<"${BUILD_PANIC}")"
expect fail "A build script failed" \
    "shape 2: build-script panic (cargo reports nonzero)" \
    env CARGO="${stub}" "${GUARD}" -p cas --lib some_module

stub="$(make_stub cargo-build-panic-swallowed 0 <<<"${BUILD_PANIC}")"
expect fail "no test harness ever reported" \
    "shape 2b: build-script panic with the status SWALLOWED (exit 0)" \
    env CARGO="${stub}" "${GUARD}" -p cas --lib some_module

# ---------------------------------------------------------------------------
# Shape 3 — the filter matched nothing. Verbatim from the GH #173 run log:
# "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 3929 filtered out".
# cargo exits 0. This is the shape no exit-code check can ever catch.
# ---------------------------------------------------------------------------
stub="$(make_stub cargo-all-filtered 0 <<'EOF'
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.21s
     Running unittests src/lib.rs (target/debug/deps/cas-7f1c2a9d)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 3929 filtered out; finished in 0.00s
EOF
)"
expect fail "0 tests passed" \
    "shape 3: stale filter, 0 passed / 3929 filtered, 'test result: ok'" \
    env CARGO="${stub}" "${GUARD}" -p cas --lib some_module::tests::

# ---------------------------------------------------------------------------
# The control: a genuine scoped green run must still exit 0. Without this the
# guard could "pass" all of the above by simply always failing.
# ---------------------------------------------------------------------------
stub="$(make_stub cargo-genuine-green 0 <<'EOF'
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.31s
     Running unittests src/lib.rs (target/debug/deps/cas-7f1c2a9d)

running 524 tests
test store::tests::round_trips ... ok
test store::tests::rejects_bad_input ... ok

test result: ok. 524 passed; 0 failed; 0 ignored; 0 measured; 3405 filtered out; finished in 1.23s
EOF
)"
expect pass "PASS: 524 test(s) passed" \
    "control: genuine scoped green (524 passed)" \
    env CARGO="${stub}" "${GUARD}" -p cas --lib store::tests

# A real failing test must also fail, via cargo's own status.
stub="$(make_stub cargo-real-failure 101 <<'EOF'
running 3 tests
test store::tests::round_trips ... ok
test store::tests::rejects_bad_input ... FAILED

failures:
    store::tests::rejects_bad_input

test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 3926 filtered out; finished in 0.04s
EOF
)"
expect fail "the test run exited 101" \
    "control: a genuinely failing test still fails" \
    env CARGO="${stub}" "${GUARD}" -p cas --lib store::tests

# ---------------------------------------------------------------------------
# nextest format — discipline.md recommends it, so the guard must read it too.
# ---------------------------------------------------------------------------
stub="$(make_stub cargo-nextest-green 0 <<'EOF'
    Starting 524 tests across 1 binary (3405 skipped)
        PASS [   0.011s] cas store::tests::round_trips
------------
     Summary [   1.234s] 524 tests run: 524 passed, 3405 skipped
EOF
)"
expect pass "PASS: 524 test(s) passed" \
    "nextest: genuine green (524 passed)" \
    env CARGO="${stub}" CARGO_CMD="nextest run" "${GUARD}" -p cas --lib store::tests

# CI enables color even when its output is captured. These strings deliberately
# contain literal ESC bytes, mirroring the nextest and cargo summaries that
# originally made a 4378/4378 green run look like it had no harness result.
ansi_nextest_summary=$'\033[32;1m     Summary \033[0m [   1.234s] 524 tests run: \033[32m524 passed\033[0m, 3405 skipped'
stub="$(make_stub cargo-nextest-ansi-green 0 <<EOF
    Starting 524 tests across 1 binary (3405 skipped)
${ansi_nextest_summary}
EOF
)"
expect pass "PASS: 524 test(s) passed" \
    "nextest: ANSI-colored green summary (524 passed)" \
    env CARGO="${stub}" CARGO_CMD="nextest run" "${GUARD}" -p cas --lib store::tests

ansi_cargo_summary=$'\033[32;1mtest result:\033[0m ok. \033[32m7 passed\033[0m; 0 failed; 0 ignored; 0 measured; 12 filtered out; finished in 0.01s'
stub="$(make_stub cargo-test-ansi-green 0 <<EOF
running 7 tests
${ansi_cargo_summary}
EOF
)"
expect pass "PASS: 7 test(s) passed" \
    "cargo test: ANSI-colored green summary (7 passed)" \
    env CARGO="${stub}" CARGO_CMD="test" "${GUARD}" -p cas --lib store::tests

stub="$(make_stub cargo-ansi-no-summary 0 <<'EOF'
[32mFinished[0m `test` profile [unoptimized + debuginfo] target(s) in 0.31s
[31merror:[0m link failed before any test binary ran
EOF
)"
expect fail "no test harness ever reported" \
    "ANSI-colored output without a harness summary still fails" \
    env CARGO="${stub}" CARGO_CMD="test" "${GUARD}" -p cas --lib store::tests

stub="$(make_stub cargo-nextest-empty 0 <<'EOF'
    Starting 0 tests across 1 binary (3929 skipped)
------------
     Summary [   0.004s] 0 tests run: 0 passed, 3929 skipped
EOF
)"
expect fail "0 tests passed" \
    "nextest: filter matched nothing (0 tests run)" \
    env CARGO="${stub}" CARGO_CMD="nextest run" "${GUARD}" -p cas --lib bogus::filter

# The wrapper itself defaults to nextest; callers should not need to remember
# CARGO_CMD on every scoped proof run.
stub="$(make_stub cargo-nextest-default 0 <<'EOF'
    Starting 7 tests across 1 binary (12 skipped)
------------
     Summary [   0.104s] 7 tests run: 7 passed, 12 skipped
EOF
)"
expect pass "argv: nextest run -p cas --lib store::tests" \
    "nextest is the default runner" \
    env CARGO="${stub}" "${GUARD}" -p cas --lib store::tests

# ---------------------------------------------------------------------------
# Preflight — a relative $ZIG must be rejected BEFORE cargo is invoked.
# ---------------------------------------------------------------------------
stub="$(make_stub cargo-should-not-run 0 <<'EOF'
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
EOF
)"
# The path is relative AND executable from the caller's cwd, so ONLY the
# relative-path branch can reject it. Using a relative path that also happens
# not to exist would let the "not executable" branch mask a broken relative
# check — which is exactly how this case passed against a mutant that had the
# relative check disabled.
printf '#!/bin/sh\nexit 0\n' >"${tmpdir}/zigstub"
chmod +x "${tmpdir}/zigstub"
expect fail "is a relative path" \
    "preflight: relative \$ZIG path is rejected (even when it resolves)" \
    bash -c "cd '${tmpdir}' && CARGO='${stub}' ZIG='./zigstub' '${GUARD}' -p cas --lib store::tests"

if [[ -e "${tmpdir}/cargo-should-not-run.invoked" ]]; then
    fail_count=$((fail_count + 1))
    echo "FAIL preflight ran cargo anyway — it must fail before the build"
else
    pass_count=$((pass_count + 1))
    echo "ok   preflight rejected before invoking cargo (no build wasted)"
fi

expect fail "not an executable file" \
    "preflight: \$ZIG pointing at a nonexistent file is rejected" \
    env CARGO="${stub}" ZIG="/nonexistent/zig" "${GUARD}" -p cas --lib store::tests

# An absolute, executable $ZIG must not trip the preflight.
expect pass "PASS: 7 test(s) passed" \
    "preflight: absolute executable \$ZIG is accepted" \
    env CARGO="$(make_stub cargo-zig-ok 0 <<'EOF'
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out; finished in 0.01s
EOF
)" ZIG="${SCRIPT_DIR}/run-scoped-tests.sh" "${GUARD}" -p cas --lib store::tests

# ---------------------------------------------------------------------------
# Refusing an unscoped run.
# ---------------------------------------------------------------------------
expect fail "refusing to run unscoped" \
    "refuses to run with no scope arguments" \
    env CARGO="${stub}" "${GUARD}"

# ---------------------------------------------------------------------------
# Proof surface — GH #329 / cas-8fd4. The runner needs a final-receipt mode
# that notices a test name is narrower than the test MODULE changed by the
# committed diff, while leaving ordinary development filters unblocked.
# ---------------------------------------------------------------------------
SURFACE_GUARD="${SCRIPT_DIR}/check-scoped-test-surface.sh"
surface_repo="${tmpdir}/surface-repo"
mkdir -p "${surface_repo}/cas-cli/src/hooks" "${surface_repo}/cas-cli/tests"
git -C "${surface_repo}" init -q -b main
{
    printf 'mod worker_commit_guard_tests {\n'
    for test_number in $(seq 1 40); do
        printf '    #[test] fn established_contract_%s() {}\n' "${test_number}"
    done
    printf '    #[test] fn cas_8fd4_added_one() {}\n'
    printf '    #[test] fn cas_8fd4_added_two() {}\n'
    printf '}\n'
} >"${surface_repo}/cas-cli/src/hooks/pre_tool.rs"
printf '// factory integration target\n' \
    >"${surface_repo}/cas-cli/tests/factory_mcp_ops_test.rs"
# This file is a decoy: the path is test data, not a Rust module declaration.
# The proof checker must still resolve the nested module to mcp_tools_test.
printf 'const DECOY_PATH: &str = "mcp_tools_test/task_tools/operations.rs";\n' \
    >"${surface_repo}/cas-cli/tests/builtin_archive_portability_test.rs"
mkdir -p "${surface_repo}/cas-cli/tests/mcp_tools_test/task_tools"
printf '#[path = "mcp_tools_test/task_tools/mod.rs"]\nmod task_tools;\n' \
    >"${surface_repo}/cas-cli/tests/mcp_tools_test.rs"
printf 'mod operations;\nmod verification_flow;\n' \
    >"${surface_repo}/cas-cli/tests/mcp_tools_test/task_tools/mod.rs"
printf '// nested operations module\n' \
    >"${surface_repo}/cas-cli/tests/mcp_tools_test/task_tools/operations.rs"
printf '// nested verification module\n' \
    >"${surface_repo}/cas-cli/tests/mcp_tools_test/task_tools/verification_flow.rs"
git -C "${surface_repo}" add .
git -C "${surface_repo}" -c user.name=scoped-test-fixture -c user.email=scoped-test-fixture@example.invalid \
    commit -qm base
git -C "${surface_repo}" checkout -qb proof
printf '\nfn fix_the_guard() {}\n' >>"${surface_repo}/cas-cli/src/hooks/pre_tool.rs"
git -C "${surface_repo}" add .
git -C "${surface_repo}" -c user.name=scoped-test-fixture -c user.email=scoped-test-fixture@example.invalid \
    commit -qm source-change

expect fail "missing library module 'worker_commit_guard_tests'" \
    "proof: narrow two-test filter is refused for a changed 42-test module" \
    bash -c "cd '${surface_repo}' && '${SURFACE_GUARD}' --base main -- -p cas --lib cas_8fd4"

expect pass "SCOPED PROOF SURFACE: covered committed diff" \
    "proof: complete changed-module filter is accepted" \
    bash -c "cd '${surface_repo}' && '${SURFACE_GUARD}' --base main -- -p cas --lib worker_commit_guard_tests --test builtin_archive_portability_test --test hook_schema"

printf '// contract changed with the implementation\n' >>"${surface_repo}/cas-cli/tests/factory_mcp_ops_test.rs"
git -C "${surface_repo}" add .
git -C "${surface_repo}" -c user.name=scoped-test-fixture -c user.email=scoped-test-fixture@example.invalid \
    commit -qm integration-change

expect fail "missing integration target 'factory_mcp_ops_test'" \
    "proof: changed integration binary cannot be omitted" \
    bash -c "cd '${surface_repo}' && '${SURFACE_GUARD}' --base main -- -p cas --lib worker_commit_guard_tests"

expect fail "Run scripts/run-scoped-tests.sh --proof" \
    "proof: missing target names the complete rerun command" \
    bash -c "cd '${surface_repo}' && '${SURFACE_GUARD}' --base main -- -p cas --lib worker_commit_guard_tests"

expect pass "SCOPED PROOF SURFACE: covered committed diff" \
    "proof: changed module plus integration target is accepted" \
    bash -c "cd '${surface_repo}' && '${SURFACE_GUARD}' --base main -- -p cas --lib worker_commit_guard_tests --test factory_mcp_ops_test --test builtin_archive_portability_test --test hook_schema"

printf '// nested operation changed\n' >>"${surface_repo}/cas-cli/tests/mcp_tools_test/task_tools/operations.rs"
printf '// nested verification flow changed\n' >>"${surface_repo}/cas-cli/tests/mcp_tools_test/task_tools/verification_flow.rs"
git -C "${surface_repo}" add .
git -C "${surface_repo}" -c user.name=scoped-test-fixture -c user.email=scoped-test-fixture@example.invalid \
    commit -qm nested-integration-change

expect fail "missing integration target 'mcp_tools_test'" \
    "proof: changed nested integration module cannot be omitted" \
    bash -c "cd '${surface_repo}' && '${SURFACE_GUARD}' --base main -- -p cas --lib worker_commit_guard_tests --test factory_mcp_ops_test"

expect pass "SCOPED_PROOF: targets=" \
    "proof: nested integration module resolves to its owning binary" \
    bash -c "cd '${surface_repo}' && '${SURFACE_GUARD}' --base main -- -p cas --lib worker_commit_guard_tests --test factory_mcp_ops_test --test mcp_tools_test --test builtin_archive_portability_test --test hook_schema"

# Builtin skill/reference changes must include the cross-flavor, agent-contract,
# and path-specific guardrail binaries. The latter is discovered from the
# guardrail's literal builtin path, so a compact-reference edit cannot claim
# coverage from flavor drift alone (cas-7715).
mkdir -p "${surface_repo}/cas-cli/src/builtins/skills/cas-supervisor/references"
printf 'compact reference\n' \
    >"${surface_repo}/cas-cli/src/builtins/skills/cas-supervisor/references/epic-driving.md"
cat >"${surface_repo}/cas-cli/tests/factory_codex_skill_guardrails.rs" <<'EOF'
fn supervisor_epic_driving_reference_is_compact() {
    let _path = "cas-cli/src/builtins/skills/cas-supervisor/references/epic-driving.md";
}
EOF
git -C "${surface_repo}" add .
git -C "${surface_repo}" -c user.name=scoped-test-fixture -c user.email=scoped-test-fixture@example.invalid \
    commit -qm builtin-reference-base
printf 'new guidance\n' \
    >>"${surface_repo}/cas-cli/src/builtins/skills/cas-supervisor/references/epic-driving.md"
git -C "${surface_repo}" add .
git -C "${surface_repo}" -c user.name=scoped-test-fixture -c user.email=scoped-test-fixture@example.invalid \
    commit -qm builtin-reference-change

expect fail "missing integration target 'factory_codex_skill_guardrails'" \
    "proof: builtin reference guardrail cannot be omitted" \
    bash -c "cd '${surface_repo}' && '${SURFACE_GUARD}' --base main -- -p cas --test builtin_flavor_drift_test --test agent_definition_contract_test"

expect pass "SCOPED PROOF SURFACE: covered committed diff" \
    "proof: builtin reference includes its literal-path guardrail" \
    bash -c "cd '${surface_repo}' && '${SURFACE_GUARD}' --base main -- -p cas --lib worker_commit_guard_tests --test builtin_flavor_drift_test --test agent_definition_contract_test --test factory_codex_skill_guardrails --test factory_mcp_ops_test --test mcp_tools_test --test builtin_archive_portability_test --test hook_schema"

# Installed catalogs flatten skill bodies to skills/<name>/SKILL.md. The
# issue-intake directive guard reads that path rather than the source-tree
# spelling, so a supervisor-body-only change must name its binary too.
printf 'supervisor body\n' \
    >"${surface_repo}/cas-cli/src/builtins/skills/cas-supervisor.md"
cat >"${surface_repo}/cas-cli/tests/issue_intake_directive_test.rs" <<'EOF'
const SUPERVISOR_BODY: &str = "skills/cas-supervisor/SKILL.md";
EOF
git -C "${surface_repo}" add .
git -C "${surface_repo}" -c user.name=scoped-test-fixture -c user.email=scoped-test-fixture@example.invalid \
    commit -qm builtin-body-base
printf 'body-only change\n' \
    >>"${surface_repo}/cas-cli/src/builtins/skills/cas-supervisor.md"
git -C "${surface_repo}" add .
git -C "${surface_repo}" -c user.name=scoped-test-fixture -c user.email=scoped-test-fixture@example.invalid \
    commit -qm builtin-body-change

expect fail "missing integration target 'issue_intake_directive_test'" \
    "proof: installed catalog guardrail cannot be omitted" \
    bash -c "cd '${surface_repo}' && '${SURFACE_GUARD}' --base main -- -p cas --lib worker_commit_guard_tests --test builtin_flavor_drift_test --test agent_definition_contract_test --test factory_codex_skill_guardrails --test factory_mcp_ops_test --test mcp_tools_test"

expect pass "SCOPED PROOF SURFACE: covered committed diff" \
    "proof: supervisor body includes its installed-catalog guardrail" \
    bash -c "cd '${surface_repo}' && '${SURFACE_GUARD}' --base main -- -p cas --lib worker_commit_guard_tests --test builtin_flavor_drift_test --test agent_definition_contract_test --test factory_codex_skill_guardrails --test issue_intake_directive_test --test factory_mcp_ops_test --test mcp_tools_test --test builtin_archive_portability_test --test hook_schema"

mapping_repo="${tmpdir}/mapping-repo"
mkdir -p "${mapping_repo}/cas-cli/src/mcp/tools/service" "${mapping_repo}/cas-cli/tests"
git -C "${mapping_repo}" init -q -b main
printf 'pub(super) async fn factory_worker_status() {}\n' \
    >"${mapping_repo}/cas-cli/src/mcp/tools/service/factory_ops.rs"
printf 'async fn test_worker_status() {}\n' \
    >"${mapping_repo}/cas-cli/tests/factory_mcp_ops_test.rs"
git -C "${mapping_repo}" add .
git -C "${mapping_repo}" -c user.name=scoped-test-fixture -c user.email=scoped-test-fixture@example.invalid \
    commit -qm base
git -C "${mapping_repo}" checkout -qb proof
printf '// worker status implementation changed\n' \
    >>"${mapping_repo}/cas-cli/src/mcp/tools/service/factory_ops.rs"
git -C "${mapping_repo}" add .
git -C "${mapping_repo}" -c user.name=scoped-test-fixture -c user.email=scoped-test-fixture@example.invalid \
    commit -qm factory-ops-change

expect fail "missing integration target 'factory_mcp_ops_test'" \
    "proof: changed factory_ops module cannot omit its public-surface binary" \
    bash -c "cd '${mapping_repo}' && '${SURFACE_GUARD}' --base main -- -p cas --lib factory_ops"

expect pass "SCOPED_PROOF: targets=lib:factory_ops,test:factory_mcp_ops_test result=PASS" \
    "proof: factory_ops public symbol maps to factory_mcp_ops_test" \
    bash -c "cd '${mapping_repo}' && '${SURFACE_GUARD}' --base main -- -p cas --lib factory_ops --test factory_mcp_ops_test"

# The end-to-end runner must propagate a rejected surface check instead of
# printing a green receipt after the checker reports missing targets. It also
# forwards an explicit baseline for release-assembly proof runs.
mkdir -p "${mapping_repo}/scripts"
cp "${SURFACE_GUARD}" "${mapping_repo}/scripts/check-scoped-test-surface.sh"
cp "${GUARD}" "${mapping_repo}/scripts/run-scoped-tests.sh"
runner_stub="$(make_stub cargo-proof-runner 0 <<'EOF'
    Summary [   0.001s] 1 tests run: 1 passed, 0 skipped
EOF
)"
expect fail "SCOPED PROOF INCOMPLETE" \
    "proof runner: missing integration target is a nonzero delivery result" \
    env CARGO="${runner_stub}" SCOPED_PROOF_BASE=main \
    "${mapping_repo}/scripts/run-scoped-tests.sh" --proof -p cas --lib factory_ops

expect fail "cannot find a merge-base" \
    "proof runner: explicit baseline is forwarded to the surface checker" \
    env CARGO="${runner_stub}" SCOPED_PROOF_BASE=missing-baseline \
    "${mapping_repo}/scripts/run-scoped-tests.sh" --proof -p cas --lib factory_ops

expect pass "SCOPED_PROOF: command=scripts/run-scoped-tests.sh --proof" \
    "proof runner: complete mapped receipt is green" \
    env CARGO="${runner_stub}" SCOPED_PROOF_BASE=main \
    "${mapping_repo}/scripts/run-scoped-tests.sh" --proof -p cas --lib factory_ops --test factory_mcp_ops_test

docs_repo="${tmpdir}/docs-repo"
mkdir -p "${docs_repo}/docs"
git -C "${docs_repo}" init -q -b main
printf 'base\n' >"${docs_repo}/docs/readme.md"
git -C "${docs_repo}" add .
git -C "${docs_repo}" -c user.name=scoped-test-fixture -c user.email=scoped-test-fixture@example.invalid \
    commit -qm base
git -C "${docs_repo}" checkout -qb proof
printf 'non-test documentation only\n' >>"${docs_repo}/docs/readme.md"
git -C "${docs_repo}" add .
git -C "${docs_repo}" -c user.name=scoped-test-fixture -c user.email=scoped-test-fixture@example.invalid \
    commit -qm docs-only

expect pass "SCOPED PROOF SURFACE: covered committed diff" \
    "proof: non-test-only diff does not invent a required target" \
    bash -c "cd '${docs_repo}' && '${SURFACE_GUARD}' --base main -- -p cas --lib"

echo
echo "test result: ${pass_count} passed; ${fail_count} failed"
if [[ "${fail_count}" -ne 0 ]]; then
    echo "The guard itself is broken — do not trust it."
    exit 1
fi
echo "PASS: the guard fails every GH #173 shape and passes a genuine green run."
