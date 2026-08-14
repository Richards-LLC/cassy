#!/usr/bin/env bash
#
# Mechanical guard against silent-success test runs (cas-a967 / GH #173).
#
# On 2026-08-07 three independent test invocations in this repo exited 0 while
# executing ZERO tests. A worker judging green by exit code shipped unverified
# work three times in one task:
#
#   1. `cargo test -p cas-cli --lib <filter>` — the crate is named `cas`, not
#      `cas-cli`. cargo errors, but a compound command wrapper swallowed it.
#   2. `cargo test --lib -p cas <filter>` with a RELATIVE $ZIG path from a
#      worktree — ghostty_vt_sys's build script panics. Build scripts run with
#      cwd set to the crate directory, so `.context/zig/zig` resolves against
#      the wrong root and is never found.
#   3. `cargo test --lib -p cas some_module::tests::` where the real module is
#      `some_module::additive_only_tests::` — cargo printed, verbatim,
#      "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 3929
#      filtered out" and exited 0.
#
# rule-173 covers the discipline half (trust the "test result:" line and its
# passed count, quote counts in close notes). This script is the mechanical
# half, so honesty does not depend on discipline alone.
#
# THE VERDICT. A run passes only if all three hold:
#
#   a. cargo itself exited 0, AND
#   b. at least one test-harness summary line was emitted (a run that never
#      reached the harness — unresolved package, build-script panic, link
#      failure — proves nothing, even if something upstream swallowed the
#      exit code), AND
#   c. the total passed count is greater than zero (an all-filtered run is a
#      failure to run, not a pass).
#
# (b) is the load-bearing one. (a) alone is exactly the check that failed
# three times; a wrapper, a `&&` chain or a backgrounded pipeline can drop a
# nonzero status, but none of them can invent a "test result:" line.
#
# Both harness formats are understood: `cargo test`'s "test result: ok. N
# passed" and `cargo nextest run`'s "Summary [...] N tests run: M passed".
#
# Usage — every argument other than `--proof` is passed through to cargo:
#
#   scripts/run-scoped-tests.sh -p cas --lib my_module
#   scripts/run-scoped-tests.sh --proof -p cas --lib my_module
#   scripts/run-scoped-tests.sh --proof -p cas --test cli_test
#   scripts/run-scoped-tests.sh --lib -- --nocapture
#   make -C cas-cli test-scoped SCOPED_ARGS='-p cas --lib my_module'
#
# Scope it, as always: a full `cargo test` here links ~64 test binaries.
#
# Environment:
#   CARGO         cargo binary to invoke (default: cargo)
#   CARGO_CMD     subcommand: "nextest run" (default) or "test"
#   SCOPED_TEST_LOG
#                 path to keep the captured run log (default: a temp file)
#
# Exit codes: 0 = genuinely green with a nonzero passed count,
#             1 = the run failed, executed nothing, or `--proof` missed a
#                 committed source/test surface.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO="${CARGO:-cargo}"
CARGO_CMD="${CARGO_CMD:-nextest run}"

# `--proof` is deliberately opt-in. Workers need fast, narrow filters while
# developing; the committed-diff check belongs to the final receipt they quote
# at handoff, where an incomplete surface must fail loudly instead of reading
# as a green proof.
proof_mode=0
proof_args=()
for arg in "$@"; do
    if [[ "$arg" == "--proof" ]]; then
        proof_mode=1
    else
        proof_args+=("$arg")
    fi
done
set -- "${proof_args[@]}"

if [[ $# -eq 0 ]]; then
    echo "error: refusing to run unscoped." >&2
    echo "Pass a scope, e.g.: scripts/run-scoped-tests.sh -p cas --lib my_module" >&2
    echo "A full suite here links ~64 test binaries — see CLAUDE.md." >&2
    exit 1
fi

# nextest's positional filter is a literal substring match, not a regex. A
# regex-looking final argument therefore costs a full build only to hit the
# zero-test guard below. Do not warn for `-E`: that is nextest's explicit
# expression syntax and is the suggested escape hatch for real regexes.
scope_filter="${!#}"
regex_filter=0
if [[ " $* " != *" -E "* && " $* " != *" --filter-expr "* ]]; then
    for metachar in '(' ')' '|' '[' ']' '+' '*' '?' '^' '$' '\\'; do
        if [[ "${scope_filter}" == *"${metachar}"* ]]; then
            regex_filter=1
            break
        fi
    done
fi

if [[ "${regex_filter}" -eq 1 ]]; then
    echo "WARNING: nextest filters are substring matches, not regexes — your pattern contains regex syntax: ${scope_filter}" >&2
    echo "         Use a shared literal substring, or nextest's -E expression syntax for regex matching." >&2
    echo >&2
fi

# ---------------------------------------------------------------------------
# Preflight: shape 2, caught before the build rather than after it.
#
# The build script resolves $ZIG with cwd = the crate directory, so a relative
# path that works from the repo root silently fails to resolve from a
# worktree. Failing here costs a second; failing in the build script costs a
# compile and produces a panic buried in cargo's "Caused by:" block.
# ---------------------------------------------------------------------------
if [[ -n "${ZIG:-}" ]]; then
    if [[ "${ZIG}" != /* ]]; then
        echo "FAIL: \$ZIG is a relative path: ${ZIG}" >&2
        echo >&2
        echo "Build scripts run with cwd set to the crate directory, not the repo root," >&2
        echo "so a relative ZIG resolves against the wrong directory and ghostty_vt_sys's" >&2
        echo "build script panics. This is GH #173 shape 2. Export an absolute path:" >&2
        echo "  export ZIG=\"${REPO_ROOT}/.context/zig/zig\"" >&2
        exit 1
    fi
    if [[ ! -x "${ZIG}" ]]; then
        echo "FAIL: \$ZIG is set to ${ZIG}, which is not an executable file." >&2
        echo "Run ./scripts/bootstrap-zig.sh, or unset ZIG to fall back to PATH." >&2
        exit 1
    fi
fi

if [[ -n "${SCOPED_TEST_LOG:-}" ]]; then
    log="${SCOPED_TEST_LOG}"
    : >"${log}"
    clean_log="$(mktemp)"
    trap 'rm -f "${clean_log}"' EXIT
else
    log="$(mktemp)"
    clean_log="$(mktemp)"
    trap 'rm -f "${log}" "${clean_log}"' EXIT
fi

echo "Running: ${CARGO} ${CARGO_CMD} $*"
echo

# Interleave stderr into the captured log: the build-script panic and the
# package-spec error both arrive on stderr, and the verdict below reads them.
(cd "${REPO_ROOT}" && "${CARGO}" ${CARGO_CMD} "$@" 2>&1) | tee "${log}"
cargo_status="${PIPESTATUS[0]}"

# Cargo and nextest color their summary text in CI. Keep the requested raw log
# intact, but make every verdict parse against one ANSI-free view so a green
# colorized run is judged the same way as a green plain-text run.
sed -E $'s/\x1b\\[[0-9;]*[A-Za-z]//g' "${log}" >"${clean_log}"

# ---------------------------------------------------------------------------
# Verdict
# ---------------------------------------------------------------------------

# `cargo test`: "test result: ok. 524 passed; 0 failed; ..."
cargo_test_summaries="$(grep -c '^test result:' "${clean_log}" 2>/dev/null || true)"
cargo_test_passed="$(
    sed -n 's/^test result: [a-zA-Z]*\. \([0-9]\{1,\}\) passed.*/\1/p' "${clean_log}" 2>/dev/null |
        awk '{ total += $1 } END { print total + 0 }'
)"

# `cargo nextest run`: "Summary [ 1.234s] 524 tests run: 524 passed, 0 skipped"
nextest_summaries="$(grep -cE '^ *Summary +\[.*\] +[0-9]+ tests? run:' "${clean_log}" 2>/dev/null || true)"
nextest_passed="$(
    sed -n 's/^ *Summary \{1,\}\[.*\] *[0-9]\{1,\} tests\{0,1\} run: \([0-9]\{1,\}\) passed.*/\1/p' "${clean_log}" 2>/dev/null |
        awk '{ total += $1 } END { print total + 0 }'
)"

summaries=$((cargo_test_summaries + nextest_summaries))
passed=$((cargo_test_passed + nextest_passed))

filtered="$(
    sed -n 's/^test result:.* \([0-9]\{1,\}\) filtered out.*/\1/p' "${clean_log}" 2>/dev/null |
        awk '{ total += $1 } END { print total + 0 }'
)"

echo
echo "--- scoped-test guard (GH #173) ---"

fail() {
    echo "FAIL: $1" >&2
    shift
    for line in "$@"; do
        echo "      ${line}" >&2
    done
    exit 1
}

# (a) cargo's own status. Diagnose the two shapes that produce it, because the
#     real error is buried far above the tail a reader usually sees.
if [[ "${cargo_status}" -ne 0 ]]; then
    detail=()
    if grep -q 'did not match any packages\|package ID specification' "${clean_log}" 2>/dev/null; then
        detail+=("The package name did not resolve. In this workspace the binary crate")
        detail+=("is \`cas\` (directory cas-cli/) — \`-p cas-cli\` matches nothing. GH #173 shape 1.")
    fi
    if grep -q 'failed to run custom build command' "${clean_log}" 2>/dev/null; then
        detail+=("A build script failed — no test binary was ever produced, so nothing ran.")
        detail+=("If it is ghostty_vt_sys, check \$ZIG is an absolute path. GH #173 shape 2.")
    fi
    if [[ "${cargo_status}" -eq 4 && "${summaries}" -gt 0 && "${passed}" -eq 0 && "${regex_filter}" -eq 1 ]]; then
        detail+=("nextest filters are substring matches, not regexes — your pattern contains regex syntax: \`${scope_filter}\`.")
        detail+=("Use a shared literal substring, or nextest's -E expression syntax for regex matching.")
    fi
    fail "the test run exited ${cargo_status}." "${detail[@]}"
fi

# (b) Did a test harness actually report? An exit code can be swallowed by a
#     wrapper or a pipeline; a summary line cannot be faked into existence.
if [[ "${summaries}" -eq 0 ]]; then
    fail "cargo exited 0 but no test harness ever reported." \
        "Not one \"test result:\" or nextest \"Summary\" line was emitted, so no test" \
        "binary ran to completion. An exit code of 0 here means the failure was" \
        "swallowed upstream (build-script panic, unresolved package, link error)" \
        "— it does not mean the tests passed. GH #173 shapes 1 and 2."
fi

# (c) Did anything actually execute?
if [[ "${passed}" -eq 0 ]]; then
    detail=(
        "${filtered} test(s) were filtered out — the filter matched nothing, so this run" \
        "verified nothing. \"test result: ok\" with a passed count of 0 is a" \
        "failure to run, not a pass. Check the module path: a stale filter like" \
        "\`mod::tests::\` silently matches zero tests when the module was renamed" \
        "(e.g. to \`mod::additive_only_tests::\`). GH #173 shape 3."
    )
    if [[ "${regex_filter}" -eq 1 ]]; then
        detail+=(
            "nextest filters are substring matches, not regexes — your pattern contains regex syntax: \`${scope_filter}\`." \
            "Use a shared literal substring, or nextest's -E expression syntax for regex matching."
        )
    fi
    fail "0 tests passed across ${summaries} harness summary line(s)." "${detail[@]}"
fi

echo "PASS: ${passed} test(s) passed across ${summaries} harness summary line(s)."
if [[ "${filtered}" -gt 0 ]]; then
    echo "      (${filtered} filtered out by the scope — expected for a scoped run.)"
fi
echo "Quote that passed count in your close note (rule-173)."

if [[ "${proof_mode}" -eq 1 ]]; then
    "${REPO_ROOT}/scripts/check-scoped-test-surface.sh" -- "$@"
else
    echo "      Iteration receipt only. Add --proof for committed-diff surface validation at handoff."
fi
