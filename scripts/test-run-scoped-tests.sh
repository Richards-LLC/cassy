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

stub="$(make_stub cargo-nextest-empty 0 <<'EOF'
    Starting 0 tests across 1 binary (3929 skipped)
------------
     Summary [   0.004s] 0 tests run: 0 passed, 3929 skipped
EOF
)"
expect fail "0 tests passed" \
    "nextest: filter matched nothing (0 tests run)" \
    env CARGO="${stub}" CARGO_CMD="nextest run" "${GUARD}" -p cas --lib bogus::filter

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

echo
echo "test result: ${pass_count} passed; ${fail_count} failed"
if [[ "${fail_count}" -ne 0 ]]; then
    echo "The guard itself is broken — do not trust it."
    exit 1
fi
echo "PASS: the guard fails every GH #173 shape and passes a genuine green run."
