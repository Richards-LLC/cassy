#!/usr/bin/env bash
set -euo pipefail
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/cassy-actions-process-identity.sh
source "$script_dir/cassy-actions-process-identity.sh"
pass=0
check() {
    local expected="$1" label="$2" result=dead
    shift 2
    if process_identity_matches "$@"; then result=live; fi
    [[ "$result" == "$expected" ]] || { printf 'FAIL %s\n' "$label" >&2; exit 1; }
    printf 'ok   %s\n' "$label"
    pass=$((pass + 1))
}
check live 'same PID and start identity is live' 42 123 42 123 S
check dead 'reused PID with different start identity is dead' 42 123 42 124 S
check dead 'different PID with same start identity is dead' 42 123 43 123 S
check dead 'missing process is dead' 42 123 '' '' ''
check dead 'zombie cannot own a job' 42 123 42 123 Z
check dead 'exited process cannot own a job' 42 123 42 123 X
check dead 'invalid PID fails comparison' invalid 123 invalid 123 S
check dead 'missing recorded start fails comparison' 42 '' 42 '' S
printf 'PASS process owner identity: %s tests\n' "$pass"
