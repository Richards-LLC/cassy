#!/usr/bin/env bash
#
# Mechanical receipt for every Cargo test command whose success is consumed by
# a worker, CI lane, Make target, or release gate (GH #499).  Cargo and
# nextest both exit zero when a filter selects no tests.  Exit status alone is
# therefore not evidence that the intended suite ran.
#
# Usage:
#   scripts/run-verified-tests.sh nextest run -p cas --lib module
#   scripts/run-verified-tests.sh test -p cas --doc
#
# CARGO may replace the cargo binary in self-tests.  VERIFIED_TEST_LOG keeps
# the raw captured output when a caller needs a durable receipt.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO="${CARGO:-cargo}"

if [[ $# -eq 0 ]]; then
    echo "error: pass a Cargo test command, e.g. nextest run -p cas --lib module" >&2
    exit 1
fi

if [[ -n "${VERIFIED_TEST_LOG:-}" ]]; then
    log="${VERIFIED_TEST_LOG}"
    : >"${log}"
    clean_log="$(mktemp)"
    trap 'rm -f "${clean_log}"' EXIT
else
    log="$(mktemp)"
    clean_log="$(mktemp)"
    trap 'rm -f "${log}" "${clean_log}"' EXIT
fi

echo "Running: ${CARGO} $*"
echo
(cd "${REPO_ROOT}" && "${CARGO}" "$@" 2>&1) | tee "${log}"
cargo_status="${PIPESTATUS[0]}"

# Cargo and nextest color their summary lines in CI.  Preserve the requested
# raw log while parsing an ANSI-free copy.
sed -E $'s/\x1b\\[[0-9;]*[A-Za-z]//g' "${log}" >"${clean_log}"

cargo_test_summaries="$(grep -c '^test result:' "${clean_log}" 2>/dev/null || true)"
cargo_test_passed="$(
    sed -n 's/^test result: [a-zA-Z]*\. \([0-9]\{1,\}\) passed.*/\1/p' "${clean_log}" 2>/dev/null |
        awk '{ total += $1 } END { print total + 0 }'
)"
nextest_summaries="$(grep -cE '^ *Summary +\[.*\] +[0-9]+ tests? run:' "${clean_log}" 2>/dev/null || true)"
nextest_passed="$(
    sed -n 's/^ *Summary \{1,\}\[.*\] *[0-9]\{1,\} tests\{0,1\} run: \([0-9]\{1,\}\) passed.*/\1/p' "${clean_log}" 2>/dev/null |
        awk '{ total += $1 } END { print total + 0 }'
)"

summaries=$((cargo_test_summaries + nextest_summaries))
passed=$((cargo_test_passed + nextest_passed))

fail() {
    echo "FAIL: $1" >&2
    shift
    for line in "$@"; do
        echo "      ${line}" >&2
    done
    exit 1
}

echo
echo "--- verified-test guard (GH #499) ---"
if [[ "${cargo_status}" -ne 0 ]]; then
    echo "FAIL: the test run exited ${cargo_status}." >&2
    exit "${cargo_status}"
fi
if [[ "${summaries}" -eq 0 ]]; then
    fail "cargo exited 0 but no test harness ever reported." \
        "No Cargo or nextest summary was emitted, so this command proved no test completed."
fi
if [[ "${passed}" -eq 0 ]]; then
    fail "0 tests passed across ${summaries} harness summary line(s)." \
        "A zero-test result is a failed verification, even when Cargo reports success."
fi

echo "PASS: ${passed} test(s) passed across ${summaries} harness summary line(s)."
