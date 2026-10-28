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
if [[ "$status" -eq 0 ]] && grep -qF 'PASS operator launch smoke: 4 entry points' <<<"$output" \
    && [[ "$(grep -c '^ok   agent refused: ' <<<"$output")" == 4 ]]; then
    ok 'exact-marker gate: the operator launches all four entry points and an agent is refused'
else
    bad "exact-marker gate did not pass: $output"
fi

output="$(bash "$script_dir/check-operator-launch.sh" "$(fake_cas v3510)" 2>&1)" && status=0 || status=$?
if [[ "$status" -ne 0 ]] && grep -qF 'FAIL operator: cas factory --check-launch' <<<"$output" \
    && grep -qF 'FAIL operator: cas claude smoke --check-launch' <<<"$output" \
    && grep -qF 'an operator could not launch' <<<"$output"; then
    ok '3.51.0 prefix gate: the smoke fails the release on the operator lockout'
else
    bad "3.51.0 prefix gate was not caught: $output"
fi

output="$(bash "$script_dir/check-operator-launch.sh" "$(fake_cas open)" 2>&1)" && status=0 || status=$?
if [[ "$status" -ne 0 ]] && [[ "$(grep -c '^FAIL agent not refused: ' <<<"$output")" == 4 ]]; then
    ok 'a gate that admits agents fails the smoke'
else
    bad "an open gate passed the smoke: $output"
fi

printf '\n%s passed, %s failed\n' "$pass" "$fail"
test "$fail" -eq 0
