#!/usr/bin/env bash
# Self-test for scripts/check-operator-launch.sh (cas-e719).
#
# A fake `cas` reproduces each launch gate, and refuses an unknown account name
# like cas does since 3.84: the cas-31ec gate (exact agent
# markers), 3.51.0's prefix gate (its own CAS_FACTORY_NICE_WORKER and the
# operator's CLAUDE_CODE_* preferences read as agent markers), and a gate that
# never refuses. The smoke must pass only the first. The real binary runs it
# in factory_preflight_test::operator_launch_smoke_passes_the_operator_and_refuses_an_agent_cas_e719.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
pass=0
fail=0
ok() { printf 'ok   %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL %s\n' "$1"; fail=$((fail + 1)); }

fake_cas() {
    local gate="$1" path="$tmp/$1/cas"
    mkdir -p "$tmp/$1"
    cat >"$path" <<EOF
#!/usr/bin/env bash
gate=$gate
EOF
    cat >>"$path" <<'EOF'
# No caller/launch-manager factory identity may enter even cas init. The
# CAS_AGENT_ROLE negative control is intentionally supplied by the smoke.
for name in CAS_AGENT_NAME CAS_AGENT_ID CAS_SESSION_ID CAS_CLONE_PATH CAS_SUPERVISOR_NAME \
    CAS_FACTORY_MODE CAS_FACTORY_SESSION CAS_FACTORY_WORKER_NAMES CLAUDECODE \
    CLAUDE_CODE_ENTRYPOINT CODEX_SANDBOX CODEX_SANDBOX_NETWORK_DISABLED CODEX_THREAD_ID; do
    [[ -z "${!name:-}" ]] || exit 91
done
if [[ "$1" == init ]]; then
    mkdir -p .cas && printf '[factory]\nnice_cargo = true\n' >.cas/config.toml
    exit 0
fi
# Like cas, the default nice_cargo exports a build knob before any gate runs.
[[ "$1" == hub ]] || export CAS_FACTORY_NICE_WORKER=1
[[ -t 0 && -t 1 ]] || { echo 'refused: needs an interactive terminal'; exit 1; }
for name in $(compgen -e); do
    case "$gate:$name" in
        fixed:CAS_AGENT_ROLE|v3510:CAS_AGENT_*|v3510:CAS_FACTORY_*|v3510:CLAUDE_CODE_*)
            echo "refused: $name is set, so this runs inside an agent session."; exit 1 ;;
    esac
done
# Like cas since 3.84, an unknown account name is refused before launch.
if [[ "$1" == claude && ! -d "$HOME/.claude-$2" ]]; then
    echo "[ERROR] unknown claude account \"$2\""; exit 1
fi
echo 'launch check: the operator may proceed'
EOF
    chmod +x "$path"
    printf '%s\n' "$path"
}

output="$(bash "$script_dir/check-operator-launch.sh" "$(fake_cas fixed)" 2>&1)" && status=0 || status=$?
if [[ "$status" -eq 0 ]]; then
    ok 'exact-marker gate: the operator launches all four entry points and an agent is refused'
else
    bad "exact-marker gate did not pass: $output"
fi

output="$(bash "$script_dir/check-operator-launch.sh" "$(fake_cas v3510)" 2>&1)" && status=0 || status=$?
if [[ "$status" -ne 0 ]]; then
    ok '3.51.0 prefix gate: the smoke fails the release on the operator lockout'
else
    bad "3.51.0 prefix gate was not caught: $output"
fi

output="$(bash "$script_dir/check-operator-launch.sh" "$(fake_cas open)" 2>&1)" && status=0 || status=$?
if [[ "$status" -ne 0 ]]; then
    ok 'a gate that admits agents fails the smoke'
else
    bad "an open gate passed the smoke: $output"
fi

# On Linux, exercise the Darwin transport and the real env scrub with shims
# for native tools. This proves submission/exit/cleanup, not launchd ancestry.
if [[ "$(uname -s)" == Linux ]]; then
    mkdir -p "$tmp/native"
    printf '#!/bin/bash\necho Darwin\n' >"$tmp/native/uname"
    cat >"$tmp/native/script" <<'EOF'
#!/bin/bash
[[ "$1" == -q && "$2" == /dev/null && "$3" == /bin/bash && "$4" == -lc ]] || exit 92
printf -v command_line '%q ' "$3" "$4" "$5"
exec /usr/bin/script -qec "$command_line" /dev/null
EOF
    cat >"$tmp/native/launchctl" <<'EOF'
#!/bin/bash
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
action="$1"; shift
if [[ "$action" == remove ]]; then
    echo remove >>"$root/launchctl.calls"
    exit 0
fi
[[ "$action" == submit && "$1" == -l && "$3" == -o && "$5" == -e && "$7" == -- ]] || exit 93
output="$4"; error="$6"; shift 7
[[ "$1" == /usr/bin/env && "$2" == -i ]] || exit 94
args=()
for arg in "$@"; do
    [[ "$arg" != PATH=* ]] || arg="PATH=$root/native:${arg#PATH=}"
    args+=("$arg")
done
echo submit >>"$root/launchctl.calls"
# A launch manager may itself have factory variables. The job must scrub them.
CAS_FACTORY_MODE=manager CAS_AGENT_NAME=manager "${args[@]}" >"$output" 2>"$error" &
EOF
    chmod +x "$tmp/native/"*
    output="$(PATH="$tmp/native:$PATH" CAS_AGENT_NAME=caller CAS_FACTORY_MODE=caller \
        CODEX_THREAD_ID=caller bash "$script_dir/check-operator-launch.sh" "$(fake_cas fixed)" 2>&1)" && status=0 || status=$?
    if [[ "$status" -eq 0 && -f "$tmp/launchctl.calls" \
        && "$(grep -c '^submit$' "$tmp/launchctl.calls")" -eq 9 \
        && "$(grep -c '^remove$' "$tmp/launchctl.calls")" -eq 9 ]]; then
        ok 'Darwin submits all nine clean commands and removes their transient jobs'
    else
        bad "Darwin launch transport or factory environment scrub failed: $output"
    fi
fi

printf '\n%s passed, %s failed\n' "$pass" "$fail"
test "$fail" -eq 0
