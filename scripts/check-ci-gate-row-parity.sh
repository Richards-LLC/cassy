#!/usr/bin/env bash
# Check that every protected-default CI lane has a release-gate row.
set -euo pipefail

workflow=${1:?CI workflow path}
release_gate=${2:?release-gate.sh path}
exemptions=${3:?gate-exemptions.md path}

# These are the required work-producing jobs behind the protected Fast
# Validation/macOS contexts. Rollups and the preflight/build dependencies are
# checked separately by test-ci-test-tiers.sh.
required_jobs=(
    fast-validation-suite-shards
    fast-validation-suite
    fast-validation-docs
    macos-check
)
declare -A gate_rows=(
    [fast-validation-suite-shards]=archive-mode
    [fast-validation-suite]=nextest
    [fast-validation-docs]=doctests
    [macos-check]=macos-check
)

job_block() {
    local job=$1
    awk -v header="  ${job}:" '
        $0 == header { inside = 1; next }
        inside && /^  [A-Za-z0-9_-]+:$/ { exit }
        inside { print }
    ' "$workflow"
}

gate_text=$(<"$release_gate")
exemption_text=$(<"$exemptions")
failures=0
for job in "${required_jobs[@]}"; do
    block=$(job_block "$job")
    if [[ -z "$block" ]]; then
        printf 'FAIL CI gate parity: required job %s is absent from workflow\n' "$job"
        failures=$((failures + 1))
        continue
    fi
    row=${gate_rows[$job]-}
    if [[ -n "$row" ]]; then
        if grep -Eq "(^|[[:space:]])${row}([[:space:]]|$)" <<<"$gate_text"; then
            printf 'ok   CI gate parity: %s -> %s\n' "$job" "$row"
        else
            printf 'FAIL CI gate parity: %s maps to missing release row %s\n' "$job" "$row"
            failures=$((failures + 1))
        fi
    elif ! grep -Eq "^[[:space:]]*${job}[[:space:]]*\\|[[:space:]]+[^[:space:]#].*" <<<"$exemption_text"; then
        printf 'FAIL CI gate parity: %s has no gate row or reasoned exemption\n' "$job"
        failures=$((failures + 1))
    else
        printf 'ok   CI gate parity: %s has a reasoned exemption\n' "$job"
    fi
done

if ((failures)); then
    exit 1
fi
