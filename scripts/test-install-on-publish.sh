#!/usr/bin/env bash
# Deterministic self-test for scripts/install-on-publish.sh (cas-f95b).
#
# Proves: a newer published release is installed with `cas update --yes
# --version`; the release notes' timer is completed once with the install
# time and the PR-opened -> installed minutes; an installed or older release
# is a no-op; a failed install is retried only after the back-off; installation
# configures and removes the native per-user scheduler.
# shellcheck disable=SC2016 # check evaluates assertion strings in the test shell.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
installer="$script_dir/install-on-publish.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

pass=0
fail=0
ok() { printf 'ok   %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL %s\n' "$1"; fail=$((fail + 1)); }
check() { if eval "$1"; then ok "$2"; else bad "$2"; fi; }

mkdir -p "$tmp/bin"
cat >"$tmp/bin/gh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$FAKE_LOG"
case "$1 $2" in
  "api repos/example/repo/releases/latest") printf '{"tag_name":"%s","published_at":"2026-11-05T23:00:00Z","draft":false}\n' "$FAKE_LATEST" ;;
  "api repos/example/repo/releases/tags/"*) cat "$FAKE_BODY" ;;
  "release edit")
    while (($#)); do [[ "$1" == --notes-file ]] && cp "$2" "$FAKE_BODY"; shift; done ;;
  *) echo "unexpected fake gh invocation: $*" >&2; exit 2 ;;
esac
EOF
cat >"$tmp/bin/cas" <<'EOF'
#!/usr/bin/env bash
if [[ "$1" == --version ]]; then
    version="$(cat "$FAKE_INSTALLED")"
    printf 'version=%s\n' "$version" >>"$FAKE_LOG"
    echo "cas $version (abc 2026-10-15)"
    exit 0
fi
printf 'cas %s\n' "$*" >>"$FAKE_LOG"
if [[ "$1 $2" == "release published" ]]; then
    publication_tag="$(perl -MJSON::PP -0777 -ne 'print decode_json($_)->{tag_name}' "$4")"
    printf 'publish=%s@%s\n' "$publication_tag" "$(cat "$FAKE_INSTALLED")" >>"$FAKE_LOG"
    [[ "${FAKE_PUBLISH_FAIL_TAG:-}" == "$publication_tag" ]] && exit 1
    exit 0
fi
if [[ "$1 $2" == "release installed" ]]; then
    notice_tag="$(perl -MJSON::PP -0777 -ne 'print decode_json($_)->{tag_name}' "$4")"
    if [[ "${6:-}" == example/repo && "${7:-}" == --failed-attempts ]]; then
        [[ "${FAKE_FAILURE_REPORT_FAILS:-}" == 1 ]] && exit 1
        printf 'failed-notice=%s:%s\n' "$notice_tag" "$8" >>"$FAKE_LOG"
        exit 0
    fi
    printf 'notice=%s\n' "$notice_tag" >>"$FAKE_LOG"
    if [[ "${FAKE_NOTICE_HANGS:-}" == "$notice_tag" ]]; then
        exec perl -MPOSIX=setpgid -e '
            my $parent = $$;
            my $child = fork(); defined $child or die $!;
            if (!$child) {
                setpgid(0, 0) == 0 or die $!;
                $SIG{TERM} = "IGNORE";
                open my $f, ">", $ENV{FAKE_NOTICE_PID} or die $!;
                print $f "$parent $$\n"; close $f;
                select undef, undef, undef, 60; exit 1;
            }
            waitpid($child, 0); exit 1;
        '
    fi
    [[ "${FAKE_NOTICE_FAIL_TAG:-}" == "$notice_tag" ]] && exit 1
    [[ "${FAKE_NOTICE_FAILS:-}" == 1 ]] && exit 1
    exit 0
fi
[[ "${FAKE_UPDATE_FAILS:-}" == 1 ]] && exit 1
[[ "$1 $2 $3" == "update --yes --version" ]] && printf '%s\n' "$4" >"$FAKE_INSTALLED"
# The binary is current but a project refresh failed.
[[ "${FAKE_REFRESH_FAILS:-}" == 1 ]] && exit 1
exit 0
EOF
cat >"$tmp/bin/uname" <<'EOF'
#!/usr/bin/env bash
if [[ -n "${FAKE_OS:-}" ]]; then echo "$FAKE_OS"; else /usr/bin/uname "$@"; fi
EOF
cat >"$tmp/bin/launchctl" <<'EOF'
#!/usr/bin/env bash
printf 'launchctl %s\n' "$*" >>"$FAKE_LOG"
if [[ "$1" == bootstrap ]]; then
    if [[ "${FAKE_BOOTSTRAP_FAILS:-}" == always ]] ||
       [[ "${FAKE_BOOTSTRAP_FAILS:-}" == once && ! -e "$FAKE_LOG.bootstrap-failed" ]]; then
        touch "$FAKE_LOG.bootstrap-failed"
        echo 'bootstrap: job is still shutting down' >&2
        exit 5
    fi
fi
EOF
cat >"$tmp/bin/sleep" <<'EOF'
#!/usr/bin/env bash
printf 'sleep %s\n' "$*" >>"$FAKE_LOG"
EOF
cat >"$tmp/bin/systemctl" <<'EOF'
#!/usr/bin/env bash
printf 'systemctl %s\n' "$*" >>"$FAKE_LOG"
EOF
chmod +x "$tmp/bin/"*
export PATH="$tmp/bin:$PATH"

export GH_BIN="$tmp/bin/gh" CAS_BIN="$tmp/bin/cas" CAS_RELEASE_REPO=example/repo
export FAKE_LOG="$tmp/log" FAKE_INSTALLED="$tmp/installed" FAKE_BODY="$tmp/body.md"
export XDG_STATE_HOME="$tmp/state" RELEASE_INSTALL_HOST=testhost
opened=1760500000
published=$((opened + 20 * 60))
installed=$((opened + 24 * 60))
cat >"$FAKE_BODY" <<EOF
- One shipped change.

---

**Release timer:** PR #4242 opened x → published y: 20 min. Installed: pending.
<!-- cas-release-timer pr=4242 opened=$opened published=$published -->
EOF

# 1. A newer release installs and completes the timer, even when a project
# refresh failed after the binary became current.
echo 9.99.1 >"$FAKE_INSTALLED"
: >"$FAKE_LOG"
FAKE_LATEST=v9.99.2 FAKE_REFRESH_FAILS=1 RELEASE_INSTALL_NOW="$installed" "$installer" --once >"$tmp/out" 2>&1
check 'grep -q "^cas update --yes --version 9.99.2$" "$FAKE_LOG"' 'a newer release runs cas update for that version'
check '[[ "$(cat "$FAKE_INSTALLED")" == 9.99.2 ]]' 'the host converges on the published version'
check 'grep -q "<!-- cas-release-timer pr=4242 opened=$opened published=$published installed=$installed -->" "$FAKE_BODY"' 'the timer marker records the install time'
check '! grep -q "Installed: pending" "$FAKE_BODY"' 'the pending timer is replaced'

check '[[ "$(grep -c "^cas release installed --receipt " "$FAKE_LOG")" == 1 ]]' 'cas-61e0: fake publication and verified install notify the owning supervisor once'
if python3 - "$FAKE_LOG" <<'PY_INSTALL_ORDER'
import pathlib, sys
lines = pathlib.Path(sys.argv[1]).read_text().splitlines()
assert lines.count("publish=v9.99.2@9.99.2") == 1
assert lines.index("cas update --yes --version 9.99.2") < lines.index("version=9.99.2") < lines.index("publish=v9.99.2@9.99.2")
PY_INSTALL_ORDER
then ok 'the installed version is verified before its publication reconciles'; else bad 'the installed version is verified before its publication reconciles'; fi

# 2. An installed release is a no-op: no update, no notes edit.
: >"$FAKE_LOG"
FAKE_LATEST=v9.99.2 "$installer" --once >/dev/null
check 'grep -q "^api repos/example/repo/releases/latest$" "$FAKE_LOG" && ! grep -q "update\|release edit" "$FAKE_LOG"' 'an installed release does nothing'
check '! grep -q "release published" "$FAKE_LOG"' 'cas-6429: a reconciled publication is not triggered again'

# Failed notice delivery is retried on the current-version path, without reinstall.
FAKE_LATEST=v9.99.2 FAKE_NOTICE_FAILS=1 "$installer" --once >/dev/null
# Force a new publication episode by clearing only the fixture notification receipt.
rm -f "$XDG_STATE_HOME/cassy/install-on-publish.notified"
rm -f "$XDG_STATE_HOME/cassy/install-on-publish.notified.d/v9.99.2"
: >"$FAKE_LOG"
FAKE_LATEST=v9.99.2 FAKE_NOTICE_FAILS=1 "$installer" --once >/dev/null
FAKE_LATEST=v9.99.2 "$installer" --once >/dev/null
FAKE_LATEST=v9.99.2 "$installer" --once >/dev/null
check '[[ "$(grep -c "^cas release installed --receipt " "$FAKE_LOG")" == 2 ]] && ! grep -q "cas update" "$FAKE_LOG"' 'cas-61e0: a pending notice retries once successfully without reinstalling'

# cas-a00be: a pending older notice survives a newer installation, and is
# attempted only after the update. All calls use the fixture binary.
(
    export XDG_STATE_HOME="$tmp/ordering-state"
    mkdir -p "$XDG_STATE_HOME/cassy"
    printf '{"tag_name":"v9.99.2","published_at":"2026-11-05T23:00:00Z","draft":false}\n' >"$XDG_STATE_HOME/cassy/install-on-publish.pending.json"
    : >"$FAKE_LOG"
    FAKE_LATEST=v9.99.3 FAKE_NOTICE_FAIL_TAG=v9.99.2 "$installer" --once >/dev/null
    python3 - "$FAKE_LOG" "$FAKE_INSTALLED" "$XDG_STATE_HOME/cassy" <<'PY_ORDER'
import json
import pathlib
import sys
log = pathlib.Path(sys.argv[1]).read_text().splitlines()
state = pathlib.Path(sys.argv[3])
assert pathlib.Path(sys.argv[2]).read_text().strip() == "9.99.3"
assert log.index("cas update --yes --version 9.99.3") < log.index("notice=v9.99.2")
assert json.loads((state / "install-on-publish.pending.d/v9.99.2.json").read_text())["tag_name"] == "v9.99.2"
PY_ORDER
    [[ $? == 0 ]] || exit 1
) && ok 'a newer install precedes retries and retains the older pending notice' || bad 'a newer install precedes retries and retains the older pending notice'

# An always-failing older notice retires once, including when the legacy
# receipt keeps rediscovering it. Subsequent ticks must not recreate it.
(
    export XDG_STATE_HOME="$tmp/exhausted-state" RELEASE_NOTICE_MAX_ATTEMPTS=3
    echo 9.99.3 >"$FAKE_INSTALLED"
    mkdir -p "$XDG_STATE_HOME/cassy"
    printf '{"tag_name":"v9.99.2","published_at":"2026-11-05T23:00:00Z","draft":false}\n' >"$XDG_STATE_HOME/cassy/install-on-publish.pending.json"
    cp "$XDG_STATE_HOME/cassy/install-on-publish.pending.json" "$tmp/legacy-publication.json"
    : >"$FAKE_LOG"
    for tick in 1 2 3 4 5; do
        # Replay legacy discovery across timer restarts as well as ordinary retries.
        cp "$tmp/legacy-publication.json" "$XDG_STATE_HOME/cassy/install-on-publish.pending.json"
        FAKE_LATEST=v9.99.3 FAKE_NOTICE_FAIL_TAG=v9.99.2 FAKE_FAILURE_REPORT_FAILS=$((tick == 3)) "$installer" --once >/dev/null
    done
    python3 - "$FAKE_LOG" "$XDG_STATE_HOME/cassy" <<'PY_EXHAUSTED'
import json, pathlib, sys
lines = pathlib.Path(sys.argv[1]).read_text().splitlines()
state = pathlib.Path(sys.argv[2])
assert lines.count("notice=v9.99.2") == 3, lines
assert lines.count("failed-notice=v9.99.2:3") == 1, lines
assert not (state / "install-on-publish.pending.d/v9.99.2.json").exists()
failed = list((state / "install-on-publish.failed.d").glob("*.json"))
assert len(failed) == 1, failed
assert json.loads(failed[0].read_text())["tag_name"] == "v9.99.2"
assert json.loads(failed[0].read_text())["installer_notice_attempts"] == 3
PY_EXHAUSTED
) && ok 'an exhausted older notice stops at the cap with one failure report' || bad 'an exhausted older notice stops at the cap with one failure report'

# A failed older publication stays pending even if its install notice succeeds.
# A newer install must happen before either publication is reconciled again.
(
    export XDG_STATE_HOME="$tmp/published-order-state"
    echo 9.99.2 >"$FAKE_INSTALLED"
    : >"$FAKE_LOG"
    FAKE_LATEST=v9.99.2 FAKE_PUBLISH_FAIL_TAG=v9.99.2 "$installer" --once >/dev/null
    : >"$FAKE_LOG"
    FAKE_LATEST=v9.99.3 "$installer" --once >/dev/null
    python3 - "$FAKE_LOG" <<'PY_PUBLICATION_ORDER'
import pathlib, sys
lines = pathlib.Path(sys.argv[1]).read_text().splitlines()
installed = lines.index("cas update --yes --version 9.99.3")
verified = lines.index("version=9.99.3")
for tag in ["v9.99.2", "v9.99.3"]:
    assert lines.count(f"publish={tag}@9.99.3") == 1
    assert installed < verified < lines.index(f"publish={tag}@9.99.3")
PY_PUBLICATION_ORDER
    [[ $? == 0 ]] || exit 1
    : >"$FAKE_LOG"
    FAKE_LATEST=v9.99.3 "$installer" --once >/dev/null
    ! grep -q '^publish=' "$FAKE_LOG"
) && ok 'a newer install precedes both pending publications and successful replays stay deduped' || bad 'a newer install precedes both pending publications and successful replays stay deduped'

# Exercise a hung notice with a TERM-ignoring child in its own process group.
# A separate test deadline
# bounds the red run too; kill only the process group this fixture started.
(
    export XDG_STATE_HOME="$tmp/timeout-state" FAKE_NOTICE_PID="$tmp/notice.pid"
    : >"$FAKE_LOG"
    FAKE_LATEST=v9.99.4 FAKE_NOTICE_HANGS=v9.99.4 RELEASE_NOTICE_TIMEOUT_SECONDS=1 python3 - "$installer" <<'PY_TIMEOUT'
import os
import pathlib
import signal
import subprocess
import sys
process = subprocess.Popen([sys.argv[1], "--once"], start_new_session=True, stdout=subprocess.DEVNULL)
try:
    assert process.wait(timeout=6) == 0
except BaseException:
    pid_file = pathlib.Path(os.environ["FAKE_NOTICE_PID"])
    if pid_file.exists():
        for owned in map(int, pid_file.read_text().split()):
            try:
                os.kill(owned, signal.SIGKILL)
            except ProcessLookupError:
                pass
    os.killpg(process.pid, signal.SIGKILL)
    process.wait()
    raise
pid, child = map(int, pathlib.Path(os.environ["FAKE_NOTICE_PID"]).read_text().split())
try:
    os.kill(pid, 0)
except ProcessLookupError:
    pass
else:
    raise AssertionError("timed-out notice survived")
# An orphaned child may await init reaping; it must already be terminated.
state = subprocess.run(["/bin/ps", "-o", "stat=", "-p", str(child)], capture_output=True, text=True).stdout.strip()
assert not state or state.startswith("Z"), state
state = pathlib.Path(os.environ["XDG_STATE_HOME"]) / "cassy"
assert (state / "install-on-publish.pending.d/v9.99.4.json").exists()
assert not (state / "install-on-publish.notified.d/v9.99.4").exists()
PY_TIMEOUT
    [[ $? == 0 ]] || exit 1
    FAKE_LATEST=v9.99.4 "$installer" --once >/dev/null
    FAKE_LATEST=v9.99.4 "$installer" --once >/dev/null
    [[ "$(grep -c '^notice=v9.99.4$' "$FAKE_LOG")" == 2 ]]
    [[ "$(grep -c '^cas update ' "$FAKE_LOG")" == 1 ]]
    [[ ! -e "$XDG_STATE_HOME/cassy/install-on-publish.pending.d/v9.99.4.json" ]]
) && ok 'a hung notice releases the lock, stays pending and retries once successfully' || bad 'a hung notice releases the lock, stays pending and retries once successfully'
echo 9.99.2 >"$FAKE_INSTALLED"

# 3. A second host leaves the completed timer alone.
echo 9.99.1 >"$FAKE_INSTALLED"
cp "$FAKE_BODY" "$tmp/before.md"
: >"$FAKE_LOG"
FAKE_LATEST=v9.99.2 RELEASE_INSTALL_HOST=other "$installer" --once >/dev/null
check 'cmp -s "$FAKE_BODY" "$tmp/before.md" && ! grep -q "release edit" "$FAKE_LOG"' 'a completed timer is not overwritten'

# 4. A failed install is retried only after the back-off.
echo 9.99.2 >"$FAKE_INSTALLED"
: >"$FAKE_LOG"
set +e
FAKE_LATEST=v9.99.3 FAKE_UPDATE_FAILS=1 RELEASE_INSTALL_NOW=2000 "$installer" --once >/dev/null
# shellcheck disable=SC2034 # Read by the assertion passed to check below.
status=$?
set -e
check '[[ $status -ne 0 ]] && grep -q "^cas update --yes --version 9.99.3$" "$FAKE_LOG"' 'a failed install exits non-zero'
check '! grep -q "^publish=" "$FAKE_LOG" && [[ -r "$XDG_STATE_HOME/cassy/install-on-publish.published.pending.d/v9.99.3.json" ]]' 'an unverified install keeps its publication pending without reconciling'
: >"$FAKE_LOG"
FAKE_LATEST=v9.99.3 FAKE_UPDATE_FAILS=1 RELEASE_INSTALL_NOW=2100 "$installer" --once >/dev/null || true
check '! grep -q "update" "$FAKE_LOG"' 'a failed version is not retried inside the back-off'
: >"$FAKE_LOG"
FAKE_LATEST=v9.99.3 RELEASE_INSTALL_NOW=2700 "$installer" --once >/dev/null
check 'grep -q "^cas update --yes --version 9.99.3$" "$FAKE_LOG" && [[ "$(cat "$FAKE_INSTALLED")" == 9.99.3 ]]' 'the install is retried after the back-off'

# 5. A lookup failure is quiet and harmless.
: >"$FAKE_LOG"
FAKE_LATEST=not-a-version "$installer" --once >/dev/null
check '! grep -q "update" "$FAKE_LOG"' 'a malformed latest tag installs nothing'

# 6. A second check cannot enter while the native lock is held.
: >"$FAKE_LOG"
if [[ "$(uname -s)" == Darwin ]]; then
    shlock -p "$$" -f "$XDG_STATE_HOME/cassy/install-on-publish.lock"
else
    exec 8>"$XDG_STATE_HOME/cassy/install-on-publish.lock"
    flock -n 8
fi
FAKE_LATEST=v9.99.4 "$installer" --once >"$tmp/locked-out"
check '[[ ! -s "$FAKE_LOG" ]]' 'a concurrent check exits before querying or updating a release'
if [[ "$(uname -s)" == Darwin ]]; then
    rm "$XDG_STATE_HOME/cassy/install-on-publish.lock"
else
    exec 8>&-
fi

# 7. --install produces a runnable LaunchAgent, survives reinstall and uninstalls.
mac_home="$tmp/mac home & user"
plist="$mac_home/Library/LaunchAgents/dev.cassy.install-on-publish.plist"
: >"$FAKE_LOG"
if HOME="$mac_home" FAKE_OS=Darwin "$installer" --install >"$tmp/install-out" 2>&1 &&
   HOME="$mac_home" FAKE_OS=Darwin FAKE_BOOTSTRAP_FAILS=once "$installer" --install >>"$tmp/install-out" 2>&1; then
    check '[[ "$(grep -c "^launchctl bootstrap " "$FAKE_LOG")" == 3 ]] && grep -q "^sleep 1$" "$FAKE_LOG" && grep -q "retrying once" "$tmp/install-out"' 'macOS reinstall logs its retry and recovers after the old job finishes shutting down'
    if ! python3 -c 'import plistlib' >/dev/null 2>&1; then
        printf 'skip macOS LaunchAgent plist validation: python3 is unavailable\n'
    elif ! python3 - "$plist" "$mac_home" "$CAS_BIN" "$GH_BIN" "$XDG_STATE_HOME/cassy/install-on-publish.log" <<'PY_PLIST'
import plistlib
import sys
with open(sys.argv[1], "rb") as f:
    agent = plistlib.load(f)
assert agent["Label"] == "dev.cassy.install-on-publish"
assert agent["ProgramArguments"] == [sys.argv[2] + "/.local/libexec/cassy/install-on-publish.sh", "--once"]
assert agent["StartInterval"] == 60
assert agent["RunAtLoad"] is True
assert agent["EnvironmentVariables"]["CAS_BIN"] == sys.argv[3]
assert agent["EnvironmentVariables"]["GH_BIN"] == sys.argv[4]
assert agent["EnvironmentVariables"]["CAS_RELEASE_REPO"] == "example/repo"
assert "/opt/homebrew/bin" in agent["EnvironmentVariables"]["PATH"].split(":")
assert agent["StandardOutPath"] == sys.argv[5]
assert agent["StandardErrorPath"] == sys.argv[5]
PY_PLIST
    then
        bad 'macOS LaunchAgent has the required execution and environment contract'
    fi
    # The installed copy must work without the source checkout, even without Python.
    echo 9.99.3 >"$FAKE_INSTALLED"
    FAKE_LATEST=v9.99.3 "$mac_home/.local/libexec/cassy/install-on-publish.sh" --once >/dev/null
    check '! grep -q "systemctl\|cas update\|release edit" "$FAKE_LOG" && grep -q "launchctl bootstrap gui/$(id -u) $plist" "$FAKE_LOG"' 'macOS installs a runnable LaunchAgent, including paths with XML characters'
else
    cat "$tmp/install-out"
    bad 'macOS installs and reloads its LaunchAgent'
fi
HOME="$mac_home" FAKE_OS=Darwin "$installer" --uninstall >"$tmp/uninstall-out" 2>&1 || true
check '[[ ! -e "$plist" && ! -e "$mac_home/.local/libexec/cassy/install-on-publish.sh" ]] && grep -q "launchctl bootout gui/$(id -u) $plist" "$FAKE_LOG"' 'macOS uninstall unloads the agent and removes installed files'

# A persistent load error is still reported after one bounded retry.
: >"$FAKE_LOG"
set +e
HOME="$mac_home" FAKE_OS=Darwin FAKE_BOOTSTRAP_FAILS=always "$installer" --install >"$tmp/install-out" 2>&1
# shellcheck disable=SC2034 # Read by the assertion passed to check below.
status=$?
set -e
check '[[ $status == 5 && "$(grep -c "^launchctl bootstrap " "$FAKE_LOG")" == 2 && "$(grep -c "^sleep " "$FAKE_LOG")" == 1 ]] && grep -q "bootstrap: job is still shutting down" "$tmp/install-out" && ! grep -q "installed .*LaunchAgent" "$tmp/install-out"' 'macOS reports a persistent load failure after one retry'

# 8. Linux keeps the existing systemd units and timer lifecycle.
linux_home="$tmp/linux-home"
linux_config="$tmp/linux-config"
: >"$FAKE_LOG"
HOME="$linux_home" XDG_CONFIG_HOME="$linux_config" FAKE_OS=Linux "$installer" --install >/dev/null
check 'cmp -s "$script_dir/../ops/systemd/cassy-install-on-publish.service" "$linux_config/systemd/user/cassy-install-on-publish.service" && cmp -s "$script_dir/../ops/systemd/cassy-install-on-publish.timer" "$linux_config/systemd/user/cassy-install-on-publish.timer" && grep -q "systemctl --user enable --now cassy-install-on-publish.timer" "$FAKE_LOG" && ! grep -q launchctl "$FAKE_LOG"' 'Linux installs the unchanged systemd units'
HOME="$linux_home" XDG_CONFIG_HOME="$linux_config" FAKE_OS=Linux "$installer" --uninstall >/dev/null
check '[[ ! -e "$linux_config/systemd/user/cassy-install-on-publish.timer" && ! -e "$linux_home/.local/libexec/cassy/install-on-publish.sh" ]] && grep -q "systemctl --user disable --now cassy-install-on-publish.timer" "$FAKE_LOG"' 'Linux uninstall keeps its existing lifecycle'

printf '\n%s passed, %s failed\n'  "$pass" "$fail"
test "$fail" -eq 0
