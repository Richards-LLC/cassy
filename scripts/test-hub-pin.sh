#!/usr/bin/env bash
# Deterministic self-test for scripts/hub-pin.sh (cas-93356).
#
# Proves: install is a true dry run, then idempotent; only a successful, audited
# Release Prebuild artifact can be staged, so a debug binary is refused on every
# route; activation restarts once and restores the previous target on failure;
# rollback returns to the released bytes. systemctl, gh and cas are fakes on PATH
# and HOME is disposable, so this never touches a real service. Filesystem moves,
# readelf and /proc identity checks are real.
# shellcheck disable=SC2016 # check evaluates assertion strings in the test shell.
set -euo pipefail

if [[ "$(uname -s)" != Linux ]]; then
    printf 'skip: the hub pin is Linux systemd only\n'
    exit 0
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
pin="$script_dir/hub-pin.sh"
tmp="$(mktemp -d)"
hub_pids="$tmp/hub-pids"
cleanup() {
    if [[ -f "$hub_pids" ]]; then
        while read -r pid; do kill "$pid" 2>/dev/null || true; done <"$hub_pids"
    fi
    chmod -R u+w "$tmp" 2>/dev/null || true
    rm -rf "$tmp"
}
trap cleanup EXIT

pass=0
fail=0
ok() { printf 'ok   %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL %s\n' "$1"; fail=$((fail + 1)); }
check() { if eval "$1"; then ok "$2"; else bad "$2"; fi; }
# The command must exit non-zero and say why.
refused() {
    local want="$1" name="$2" output
    shift 2
    if output="$("$@" 2>&1)"; then
        bad "$name (it succeeded)"
    elif grep -q -- "$want" <<<"$output"; then
        ok "$name"
    else
        bad "$name: $output"
    fi
}

# Only this self-test may run the script from inside a factory agent. HOME below is
# disposable and every host command is a fake.
export CAS_HUB_PIN_TEST_ALLOW_AGENT=1
export HOME="$tmp/home"
export XDG_CONFIG_HOME="$HOME/.config"
export XDG_STATE_HOME="$HOME/.local/state"
export CAS_RELEASE_REPO=example/repo
export PATH="$tmp/bin:$PATH"
export FAKE_LOG="$tmp/calls.log"
export FAKE_HUB_PIDS="$hub_pids"
export FAKE_FIFO="$tmp/hub-fifo"
units="$XDG_CONFIG_HOME/systemd/user"
unit="$units/cas-hub.service"
current="$HOME/.local/lib/cassy/hub/current"
receipt="$XDG_STATE_HOME/cassy/hub-pin.json"
sha_a=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
sha_b=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
sha_debug=dddddddddddddddddddddddddddddddddddddddd
mkdir -p "$tmp/bin" "$HOME/.local/bin" "$units" "$tmp/fixtures"
mkfifo "$FAKE_FIFO"

# A release fixture is a real ELF with its debug sections removed; the debug
# fixture is the same file with a real .debug_info section added.
cp "$(readlink -f "$(command -v bash)")" "$tmp/fixtures/release"
chmod u+w "$tmp/fixtures/release"
objcopy --strip-debug "$tmp/fixtures/release"
printf 'x' >"$tmp/fixtures/section"
cp "$tmp/fixtures/release" "$tmp/fixtures/debug"
objcopy --add-section ".debug_info=$tmp/fixtures/section" "$tmp/fixtures/debug"
cp "$tmp/fixtures/release" "$tmp/fixtures/other-release"
printf 'different bytes' >>"$tmp/fixtures/other-release"

archive_of() {
    local dir="$tmp/fixtures/archive-$1"
    mkdir -p "$dir"
    cp "$tmp/fixtures/$1" "$dir/cas"
    printf 'license\n' >"$dir/LICENSE"
    tar -czf "$tmp/fixtures/$1.tar.gz" -C "$dir" cas LICENSE
}
archive_of release
archive_of other-release
archive_of debug

cat >"$tmp/bin/gh" <<'EOF'
#!/usr/bin/env bash
printf 'gh %s\n' "$*" >>"$FAKE_LOG"
case "$*" in
  *"/jobs?per_page=100"*) printf '%s\n' "${FAKE_AUDIT:-success}" ;;
  api\ repos/example/repo/actions/runs/*)
    printf '%s\t%s\t%s\t%s\n' "${FAKE_CONCLUSION:-success}" "${FAKE_HEAD:?}" \
      "${FAKE_WORKFLOW:-.github/workflows/release-prebuild.yml}" "${FAKE_REPO:-example/repo}" ;;
  run\ download*)
    [[ "${FAKE_NO_ARTIFACT:-}" == 1 ]] && exit 1
    while (($#)); do [[ "$1" == --dir ]] && dir="$2"; shift; done
    mkdir -p "$dir"
    cp "$FAKE_ARCHIVE" "$dir/cas-x86_64-unknown-linux-gnu.tar.gz" ;;
  *) exit 1 ;;
esac
EOF
cat >"$tmp/bin/systemctl" <<'EOF'
#!/usr/bin/env bash
printf 'systemctl %s\n' "$*" >>"$FAKE_LOG"
case "$*" in
  "--user daemon-reload") ;;
  "--user show -p MainPID --value cas-hub.service") tail -n 1 "$FAKE_HUB_PIDS" ;;
  "--user show -p CPUWeight -p MemoryLow -p ExecStart cas-hub.service")
    printf 'CPUWeight=%s\nMemoryLow=1073741824\nExecStart={ path=%s ; argv[]=x ; }\n' \
      "${FAKE_CPU_WEIGHT:-1000}" "$(sed -n 's/^ExecStart=\([^ ]*\).*/\1/p' "$XDG_CONFIG_HOME/systemd/user/cas-hub.service")" ;;
  "--user show -p CPUWeight --value agent.slice") echo 40 ;;
  "--user show -p CPUWeight --value agent-builds.slice") echo 25 ;;
  *) exit 1 ;;
esac
EOF
# The fake `cas hub restart` launches whatever the managed path names, as the
# manager would. FAKE_BAD_TARGET simulates bytes that never come up.
cat >"$tmp/bin/cas" <<'EOF'
#!/usr/bin/env bash
printf 'cas %s\n' "$*" >>"$FAKE_LOG"
[[ "$*" == "hub restart" ]] || exit 1
target="$(readlink -f "$HOME/.local/lib/cassy/hub/current")"
if [[ -n "${FAKE_BAD_TARGET:-}" && "$target" == *"$FAKE_BAD_TARGET"* ]]; then exit 1; fi
# A builtin read leaves no child behind when the fixture kills this process.
"$target" -c 'read -r -t 300 _ <>"$FAKE_FIFO"; :' >/dev/null 2>&1 &
printf '%s\n' "$!" >>"$FAKE_HUB_PIDS"
sleep 0.2 # Let the fork exec: the real restart returns once the hub answers.
EOF
chmod +x "$tmp/bin/gh" "$tmp/bin/systemctl" "$tmp/bin/cas"

# Install checks the bytes of the CLI it pins, so it gets a release-shaped file
# through CAS_BIN. Everything else calls the fake `cas` above.
install_cli="$HOME/.local/bin/cas"
cp "$tmp/fixtures/release" "$install_cli"

cat >"$unit" <<EOF
[Unit]
Description=Cassy Commander hub

[Service]
ExecStart=$install_cli hub serve --bind 127.0.0.1 --port 4173 --tailscale-serve
EOF
mkdir -p "$unit.d"
printf '[Service]\nMemoryLow=256M\n' >"$unit.d/60-interactive-reserve.conf"

snapshot() { (cd "$HOME" && find . -type f -o -type l | LC_ALL=C sort | xargs -r sha256sum 2>/dev/null); }
run_install() { CAS_BIN="$install_cli" "$pin" install "$@"; }

# --- who may run it ----------------------------------------------------------
refused 'operator runs this' 'a worker is refused' env -u CAS_HUB_PIN_TEST_ALLOW_AGENT CAS_AGENT_ROLE=worker "$pin" inspect
refused 'operator runs this' 'any other agent role is refused' env -u CAS_HUB_PIN_TEST_ALLOW_AGENT CAS_AGENT_ROLE=director "$pin" inspect
refused 'operator runs this' 'the test override is refused outside a disposable HOME' env HOME=/home/not-disposable CAS_AGENT_ROLE=worker "$pin" inspect
check 'env -u CAS_HUB_PIN_TEST_ALLOW_AGENT -u CAS_AGENT_ROLE "$pin" inspect >/dev/null' 'an operator shell runs it'
check 'env -u CAS_HUB_PIN_TEST_ALLOW_AGENT CAS_AGENT_ROLE=supervisor "$pin" inspect >/dev/null' 'an operator command in a supervisor pane runs it'

# An installed copy has no drop-ins beside it, and must say so before changing anything.
cp "$pin" "$tmp/fixtures/installed-hub-pin.sh"
refused 'from a checkout' 'install refuses without its drop-ins' env CAS_BIN="$install_cli" "$tmp/fixtures/installed-hub-pin.sh" install
check '[[ ! -e "$current" ]] && grep -q "^ExecStart=$install_cli " "$unit"' 'that refusal changes nothing'

# --- install ---------------------------------------------------------------
before="$(snapshot)"
: >"$FAKE_LOG"
dry="$(run_install --dry-run)"
check '[[ "$(snapshot)" == "$before" ]]' 'a dry run changes no file'
check '[[ ! -s "$FAKE_LOG" ]]' 'a dry run never runs systemctl'
check 'grep -q "would: .*sed -i" <<<"$dry" && grep -q "90-cassy-host-isolation.conf" <<<"$dry"' 'a dry run names the unit and drop-in changes'

refused 'debug build' 'install refuses a debug CLI' env CAS_BIN="$tmp/fixtures/debug" "$pin" install --dry-run
printf '[Service]\nExecStart=\nExecStart=/tmp/candidate hub serve\n' >"$unit.d/70-hotfix.conf"
refused 'overrides the hub' 'install refuses an ExecStart override' run_install
rm "$unit.d/70-hotfix.conf"
check '[[ ! -e "$current" ]]' 'a refused install leaves no pin path'

: >"$FAKE_LOG"
run_install >/dev/null
check '[[ "$(readlink "$current")" == "$install_cli" ]]' 'the pin path links to the installed release'
check 'grep -q "^ExecStart=$current hub serve --bind 127.0.0.1 --port 4173 --tailscale-serve$" "$unit"' 'the unit launches the pin path and keeps its arguments'
check 'grep -q "^CPUWeight=1000$" "$unit.d/90-cassy-host-isolation.conf" && grep -q "^MemoryLow=1G$" "$unit.d/90-cassy-host-isolation.conf"' 'the hub drop-in carries the weights'
check 'grep -q "^CPUWeight=40$" "$units/agent.slice.d/90-cassy-host-isolation.conf" && grep -q "^CPUWeight=25$" "$units/agent-builds.slice.d/90-cassy-host-isolation.conf"' 'the slice drop-ins carry the weights'
check '[[ -f "$unit.d/60-interactive-reserve.conf" ]]' 'an existing drop-in is left alone'
check 'ls "$XDG_STATE_HOME/cassy/hub-pin-backup/"cas-hub.service.* >/dev/null' 'the previous unit is kept'
check '[[ -x "$HOME/.local/libexec/cassy/hub-pin.sh" ]]' 'the helper is installed for hotfixes without a checkout'
check 'grep -q "daemon-reload" "$FAKE_LOG" && ! grep -q "hub restart" "$FAKE_LOG"' 'install reloads systemd and leaves the hub running'

before="$(snapshot)"
: >"$FAKE_LOG"
again="$(run_install)"
check '[[ "$(snapshot)" == "$before" ]]' 'a second install changes nothing'
check '! grep -q daemon-reload "$FAKE_LOG" && grep -q "already installed" <<<"$again"' 'a second install does not reload'
refused 'CPUWeight is not 1000' 'install checks the effective weight' env FAKE_CPU_WEIGHT=100 CAS_BIN="$install_cli" "$pin" install

# --- a debug binary never reaches a slot -------------------------------------
stage() { FAKE_HEAD="${FAKE_HEAD:-$sha_a}" FAKE_ARCHIVE="${FAKE_ARCHIVE:-$tmp/fixtures/release.tar.gz}" "$pin" stage "$@"; }
if "$pin" stage --binary "$tmp/fixtures/debug" --sha "$sha_debug" --run-id 1 >/dev/null 2>&1; then
    bad 'stage has no route for a local file'
else
    ok 'stage has no route for a local file'
fi
refused 'debug build' 'check-binary refuses real debug sections' "$pin" check-binary "$tmp/fixtures/debug"
refused 'development build path' 'check-binary refuses a target/debug path' bash -c 'mkdir -p "$1/target/debug" && cp "$2" "$1/target/debug/cas" && "$3" check-binary "$1/target/debug/cas"' _ "$tmp" "$tmp/fixtures/release" "$pin"
check '"$pin" check-binary "$tmp/fixtures/release" >/dev/null' 'check-binary accepts a stripped release'
refused 'debug build' 'a debug artifact from a good run is refused' env FAKE_ARCHIVE="$tmp/fixtures/debug.tar.gz" FAKE_HEAD="$sha_debug" "$pin" stage --run-id 7 --sha "$sha_debug"
check '[[ ! -e "$HOME/.local/lib/cassy/hub/versions/$sha_debug" ]]' 'a refused artifact leaves no slot'
refused 'not .github/workflows/release-prebuild.yml' 'another workflow is refused' env FAKE_WORKFLOW=.github/workflows/ci.yml FAKE_HEAD="$sha_a" "$pin" stage --run-id 7 --sha "$sha_a"
refused "concluded 'failure'" 'a failed run is refused' env FAKE_CONCLUSION=failure FAKE_HEAD="$sha_a" "$pin" stage --run-id 7 --sha "$sha_a"
refused 'built' 'a run of another commit is refused' env FAKE_HEAD="$sha_b" "$pin" stage --run-id 7 --sha "$sha_a"
refused 'belongs to' 'a run from another repository is refused' env FAKE_REPO=fork/repo FAKE_HEAD="$sha_a" "$pin" stage --run-id 7 --sha "$sha_a"
refused 'no passing packaged audit' 'a run without the audit is refused' env FAKE_AUDIT=skipped FAKE_HEAD="$sha_a" "$pin" stage --run-id 7 --sha "$sha_a"
refused 'may have expired' 'an expired artifact is refused' env FAKE_NO_ARTIFACT=1 FAKE_HEAD="$sha_a" "$pin" stage --run-id 7 --sha "$sha_a"

# A debug binary dropped straight into a slot, with a forged receipt, is refused.
forged="$HOME/.local/lib/cassy/hub/versions/$sha_debug"
mkdir -p "$forged"
cp "$tmp/fixtures/debug" "$forged/cas"
printf '{\n  "workflow": ".github/workflows/release-prebuild.yml",\n  "digest": "%s"\n}\n' "$(sha256sum "$forged/cas" | cut -d' ' -f1)" >"$forged/receipt.json"
refused 'debug build' 'activate refuses a debug binary in a forged slot' "$pin" activate --sha "$sha_debug" --expected-current "$install_cli"
check '[[ "$(readlink "$current")" == "$install_cli" ]]' 'a refused activation leaves the pin path alone'

# --- stage, activate, rollback ----------------------------------------------
: >"$FAKE_LOG"
stage --run-id 7 --sha "$sha_a" >/dev/null
slot_a="$HOME/.local/lib/cassy/hub/versions/$sha_a"
check '[[ -x "$slot_a/cas" && ! -w "$slot_a/cas" ]]' 'a staged slot is immutable'
check '[[ "$(readlink "$current")" == "$install_cli" ]] && ! grep -q "hub restart" "$FAKE_LOG"' 'stage is inert'
check 'stage --run-id 7 --sha "$sha_a" | grep -q "already staged"' 'staging the same bytes again is a no-op'
refused 'different bytes' 'different bytes cannot replace a slot' env FAKE_ARCHIVE="$tmp/fixtures/other-release.tar.gz" FAKE_HEAD="$sha_a" "$pin" stage --run-id 8 --sha "$sha_a"

: >"$FAKE_LOG"
dry="$("$pin" activate --sha "$sha_a" --expected-current "$install_cli" --dry-run)"
check 'grep -q "would: point" <<<"$dry" && [[ "$(readlink "$current")" == "$install_cli" ]] && [[ ! -e "$receipt" ]]' 'a dry-run activation changes nothing'
refused 'not the expected' 'a stale operator decision is refused' "$pin" activate --sha "$sha_a" --expected-current /somewhere/else

"$pin" activate --sha "$sha_a" --expected-current "$install_cli" >/dev/null
check '[[ "$(readlink "$current")" == "$slot_a/cas" ]]' 'activate points the hub at the slot'
check '[[ "$(grep -c "hub restart" "$FAKE_LOG")" == 1 ]]' 'activate restarts the hub once'
check '[[ "$(readlink -f "/proc/$(tail -n 1 "$hub_pids")/exe")" == "$(readlink -f "$slot_a/cas")" ]]' 'the restarted process runs the pinned bytes'
check 'grep -q "\"released_path\": \"$install_cli\"" "$receipt"' 'the receipt names the released binary'
check 'python3 -m json.tool "$receipt" >/dev/null' 'the receipt is valid JSON for cas update'
check '[[ -x "$(sed -n "s/^  \"preserved\": \"\(.*\)\",\$/\1/p" "$receipt")" ]]' 'the released bytes are preserved'

# A hotfix that never comes up restores the previous target once and stops.
FAKE_HEAD="$sha_b" FAKE_ARCHIVE="$tmp/fixtures/other-release.tar.gz" "$pin" stage --run-id 9 --sha "$sha_b" >/dev/null
before_receipt="$(cat "$receipt")"
: >"$FAKE_LOG"
refused 'failed and was rolled back' 'a hub that stays down is rolled back' env FAKE_BAD_TARGET="$sha_b" "$pin" activate --sha "$sha_b" --expected-current "$slot_a/cas"
check '[[ "$(readlink "$current")" == "$slot_a/cas" ]]' 'failed readiness restores the previous target'
check '[[ "$(cat "$receipt")" == "$before_receipt" ]]' 'failed readiness restores the previous receipt'
check '[[ "$(grep -c "hub restart" "$FAKE_LOG")" == 2 ]]' 'failed readiness restarts once more, with no retry loop'

: >"$FAKE_LOG"
"$pin" rollback --expected-current "$slot_a/cas" >/dev/null
check '[[ "$(readlink "$current")" == "$install_cli" ]] && [[ ! -e "$receipt" ]]' 'rollback returns to the released binary and clears the pin'
check '[[ "$(grep -c "hub restart" "$FAKE_LOG")" == 1 ]]' 'rollback restarts the hub once'
refused 'no hotfix pin' 'rollback needs a pin' "$pin" rollback --expected-current "$install_cli"

# If the installed binary moved on, rollback uses the preserved released bytes.
"$pin" activate --sha "$sha_a" --expected-current "$install_cli" >/dev/null
preserved="$(sed -n 's/^  "preserved": "\(.*\)",$/\1/p' "$receipt")"
cp -f "$tmp/fixtures/other-release" "$install_cli"
"$pin" rollback --expected-current "$slot_a/cas" >/dev/null
check '[[ "$(readlink "$current")" == "$preserved" ]]' 'rollback falls back to the preserved bytes'
check 'grep -q "\"slot\": \"$preserved\"" "$receipt"' 'a preserved-bytes rollback keeps a receipt cas update can trust'

printf '\n%d passed, %d failed\n' "$pass" "$fail"
((fail == 0))
