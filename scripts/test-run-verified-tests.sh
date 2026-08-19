#!/usr/bin/env bash
# Self-test for scripts/run-verified-tests.sh (GH #499).
set -uo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="${script_dir}/run-verified-tests.sh"
tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT

pass=0
fail=0

make_stub() {
    local name="$1" status="$2"
    local stub="${tmpdir}/${name}"
    {
        echo '#!/usr/bin/env bash'
        echo "cat <<'STUB_EOF'"
        cat
        echo 'STUB_EOF'
        echo "exit ${status}"
    } >"${stub}"
    chmod +x "${stub}"
    echo "${stub}"
}

expect() {
    local wanted="$1" needle="$2" label="$3"
    shift 3
    local output status
    output="$("$@" 2>&1)"
    status=$?
    local ok=1
    [[ "${wanted}" == pass && "${status}" -ne 0 ]] && ok=0
    [[ "${wanted}" == fail && "${status}" -eq 0 ]] && ok=0
    [[ "${ok}" -eq 1 && "${needle}" != - ]] && ! grep -qF -- "${needle}" <<<"${output}" && ok=0
    if [[ "${ok}" -eq 1 ]]; then
        pass=$((pass + 1))
        echo "ok   ${label}"
    else
        fail=$((fail + 1))
        echo "FAIL ${label} (exit ${status})" >&2
        echo "${output}" >&2
    fi
}

stub="$(make_stub green-nextest 0 <<'EOF'
    Starting 2 tests across 1 binary (0 skipped)
------------
     Summary [   0.012s] 2 tests run: 2 passed, 0 skipped
EOF
)"
expect pass 'PASS: 2 test(s) passed' 'nextest green receipt passes' \
    env CARGO="${stub}" "${guard}" nextest run -p cas --lib module

stub="$(make_stub zero-cargo 0 <<'EOF'
running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.00s
EOF
)"
expect fail '0 tests passed' 'zero-match Cargo result fails loudly' \
    env CARGO="${stub}" "${guard}" test -p cas --lib stale_filter

stub="$(make_stub zero-nextest 0 <<'EOF'
    Starting 0 tests across 1 binary (4 skipped)
------------
     Summary [   0.001s] 0 tests run: 0 passed, 4 skipped
EOF
)"
expect fail '0 tests passed' 'zero-match nextest result fails loudly' \
    env CARGO="${stub}" "${guard}" nextest run -p cas --lib stale_filter

stub="$(make_stub no-harness 0 <<'EOF'
Finished `test` profile after a swallowed build failure
EOF
)"
expect fail 'no test harness ever reported' 'swallowed pre-harness failure fails loudly' \
    env CARGO="${stub}" "${guard}" test -p cas --doc

stub="$(make_stub failing 42 <<'EOF'
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
EOF
)"
expect fail 'the test run exited 42' 'real Cargo failure propagates' \
    env CARGO="${stub}" "${guard}" test -p cas --doc

if [[ "${fail}" -ne 0 ]]; then
    echo "FAIL: ${fail} verified-test guard self-test(s) failed" >&2
    exit 1
fi
echo "PASS: ${pass} verified-test guard self-test(s) passed."
