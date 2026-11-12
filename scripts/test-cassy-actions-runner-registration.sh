#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/cassy-actions-runner-registration.sh
source "$script_dir/cassy-actions-runner-registration.sh"

assert_group_args() {
    local registration="$1"
    shift
    local -a expected=("$@") actual=()
    set_runner_group_args "$registration" cassy-public-trusted actual
    if (( ${#actual[@]} != ${#expected[@]} )); then
        printf 'FAIL %s registration expected %d runner-group args, got %d\n' \
            "$registration" "${#expected[@]}" "${#actual[@]}" >&2
        exit 1
    fi
    for index in "${!expected[@]}"; do
        if [[ "${actual[index]}" != "${expected[index]}" ]]; then
            printf 'FAIL %s registration runner-group arg %d: expected %q, got %q\n' \
                "$registration" "$index" "${expected[index]}" "${actual[index]}" >&2
            exit 1
        fi
    done
}

assert_group_args org --runnergroup cassy-public-trusted
assert_group_args repo
printf 'PASS repository runner registration omits the organization-only runner group\n'

fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT
cat >"$fixture/id" <<'EOF'
#!/usr/bin/env bash
: >"$TEST_ID_CALLED"
printf '1234\n'
EOF
chmod +x "$fixture/id"
for slot in {1..8} 0 9 -1 invalid; do
    rm -f "$fixture/id-called"
    # Stop at the root check, before provisioning can touch the host.
    if PATH="$fixture:$PATH" TEST_ID_CALLED="$fixture/id-called" RUNNER_SLOT="$slot" \
        bash "$script_dir/install-cassy-actions-runner.sh" >"$fixture/output" 2>&1; then
        printf 'FAIL slot %s unexpectedly reached provisioning\n' "$slot" >&2
        exit 1
    fi
    case "$slot" in
        [1-8])
            test -f "$fixture/id-called" || { printf 'FAIL slot %s rejected before privilege check\n' "$slot" >&2; exit 1; } ;;
        *) test ! -f "$fixture/id-called" ;;
    esac
done
printf 'PASS installer validates slots 1-8 before privilege or host changes\n'

# Exercise the operator entry through fake privilege/network/host boundaries.
mkdir -p "$fixture/repo/ops" "$fixture/repo/scripts"
cp "$script_dir/../ops/install-light-lanes-5-8.sh" "$fixture/repo/ops/"
cat >"$fixture/id" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "${TEST_PRIVILEGED:-1234}"
STUB
cat >"$fixture/sudo" <<'STUB'
#!/usr/bin/env bash
printf 'sudo\n' >>"$TEST_ROOT/sudo-calls"
[[ "$1" == -- ]] && shift
TEST_PRIVILEGED=0 SUDO_USER=fixture exec "$@"
STUB
cat >"$fixture/runuser" <<'STUB'
#!/usr/bin/env bash
[[ "$1" == -u && "$2" == fixture && "$3" == -- ]]
shift 3
exec "$@"
STUB
cat >"$fixture/gh" <<'STUB'
#!/usr/bin/env bash
[[ -z "${GH_DEBUG:-}" ]]
case "$*" in
    'api -X POST repos/Richards-LLC/cassy/actions/runners/registration-token -q .token')
        printf 'fixture-token\n' ;;
    *'repos/Richards-LLC/cassy/actions/runners?per_page=100'*)
        for slot in {3..8}; do printf 'soundwave-cas-lane-%s\tonline\tfalse\n' "$slot"; done ;;
    *) exit 2 ;;
esac
STUB
cat >"$fixture/repo/scripts/install-cassy-actions-runner.sh" <<'STUB'
#!/usr/bin/env bash
[[ "$RUNNER_REGISTRATION" == repo && "$RUNNER_TOKEN" == fixture-token ]]
printf '%s\n' "$RUNNER_SLOT" >>"$TEST_ROOT/installed"
STUB
for command in install systemctl; do
    printf '#!/usr/bin/env bash\nexit 0\n' >"$fixture/$command"
done
chmod +x "$fixture/"{id,sudo,runuser,gh,install,systemctl} "$fixture/repo/scripts/install-cassy-actions-runner.sh"
PATH="$fixture:$PATH" TEST_ROOT="$fixture" GH_DEBUG=api \
    bash -x "$fixture/repo/ops/install-light-lanes-5-8.sh" >"$fixture/operator-output" 2>&1
test "$(wc -l <"$fixture/sudo-calls")" -eq 1
test "$(cat "$fixture/installed")" = $'5\n6\n7\n8'
test "$(awk '$2 == "online" {n++} END {print n+0}' "$fixture/operator-output")" -eq 6
! grep -Fq fixture-token "$fixture/operator-output"
printf 'PASS one elevation installs four repo runners, hides tokens under tracing, and reports six online\n'

# Run the real installer light path, substituting host/network boundaries only.
mkdir -p "$fixture/runner"
cat >"$fixture/runner/config.sh" <<'STUB'
#!/usr/bin/env bash
while (($#)); do
    case "$1" in
        --name) name="$2"; shift 2 ;;
        --labels) labels="$2"; shift 2 ;;
        --url) url="$2"; shift 2 ;;
        --token) test "$2" = fixture-token; shift 2 ;;
        --runnergroup) exit 1 ;;
        *) shift ;;
    esac
done
test "$name" = "soundwave-cas-lane-$RUNNER_SLOT"
test "$labels" = cas-lane-light,trusted-branches
test "$url" = https://github.com/Richards-LLC/cassy
printf '%s\n' "$RUNNER_SLOT" >>"$TEST_ROOT/configured"
STUB
chmod +x "$fixture/runner/config.sh"
cat >"$fixture/host-boundaries" <<'STUB'
id() { [[ "$1" != -u ]] || printf '0\n'; }
install() { printf '%s\n' "$*" >>"$TEST_ROOT/install-calls"; }
curl() { :; }
sha256sum() { cat >/dev/null; }
tar() { :; }
chown() { :; }
sudo() {
    for arg in "$@"; do
        [[ "$arg" != fixture-token ]] || { echo "FAIL credential would enter sudo argument log" >&2; return 1; }
    done
    [[ "$1" == -u && "$2" == cassy-actions ]]; shift 2
    [[ "$1" != --preserve-env=RUNNER_TOKEN ]] || shift
    "$@"
}
systemctl() { printf '%s\n' "$*" >>"$TEST_ROOT/unit-calls"; }
cd() {
    case "$1" in
        /var/lib/cassy-actions/runner-*) builtin cd "$TEST_ROOT/runner" ;;
        *) builtin cd "$@" ;;
    esac
}
STUB
for slot in {3..8}; do
    if ! BASH_ENV="$fixture/host-boundaries" TEST_ROOT="$fixture" RUNNER_TOKEN=fixture-token \
        RUNNER_SLOT="$slot" RUNNER_REGISTRATION=repo \
        bash "$script_dir/install-cassy-actions-runner.sh" >"$fixture/light-output" 2>&1; then
        cat "$fixture/light-output" >&2
        exit 1
    fi
    grep -Fxq "enable --now cassy-actions-runner-$slot.service" "$fixture/unit-calls"
    ! grep -Fq fixture-token "$fixture/light-output"
done
test "$(cat "$fixture/configured")" = $'3\n4\n5\n6\n7\n8'
printf 'PASS real installer configures and starts slots 3-8 with light-only labels and repository registration\n'
