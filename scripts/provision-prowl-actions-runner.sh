#!/usr/bin/env bash
# Imported and adapted from mattpocock/skills wizard/template.sh, MIT © 2026 Matt Pocock.
# Edit only after STAGES.
set -euo pipefail

TOTAL_STAGES=0
_stage=0
ENV_FILE="${ENV_FILE:-.env}"
open_url() { if command -v xdg-open >/dev/null; then xdg-open "$1"; elif [ "$(uname)" = Darwin ] && command -v open >/dev/null; then open "$1"; else printf 'Open manually: %s\n' "$1"; fi; }
pause() { read -r -p "${1:-Press Enter to continue} " _ || true; }
confirm() { local answer; read -r -p "$1 [y/N] " answer || true; [[ "$answer" =~ ^[Yy]$ ]]; }
stage() { _stage=$((_stage + 1)); printf '\nStage %s/%s: %s\n' "$_stage" "$TOTAL_STAGES" "$1"; }
say() { printf '  %s\n' "$1"; }
ask() { read -r -p "$2 " "$1" || true; }
ask_secret() { read -r -s -p "$2 " "$1" || true; printf '\n'; }
write_env() { local key="$1" value="$2" tmp; touch "$ENV_FILE"; tmp=$(mktemp); grep -vE "^${key}=" "$ENV_FILE" >"$tmp" || true; printf '%s=%s\n' "$key" "$value" >>"$tmp"; mv "$tmp" "$ENV_FILE"; }
set_secret() { local name="$1" value="$2"; command -v gh >/dev/null && printf '%s' "$value" | gh secret set "$name" || printf 'Set GitHub secret manually: %s\n' "$name"; }
finish() { printf '\nWizard complete.\n'; }

# STAGES: operator-approved cas-96d4 setup; one confirmation per operation.
# Never run this wizard in agent checks. bash -n/static review only.
# GitHub service entry point: https://docs.github.com/en/actions/how-tos/manage-runners/self-hosted-runners/configure-the-application
# Registration/removal tokens arrive in RUNNER_TOKEN / RUNNER_REMOVE_TOKEN.

operation="${1:---install}"
confirmed="${2:-}"
[[ "$#" -le 2 && ( -z "$confirmed" || "$confirmed" == --yes ) ]] || { printf 'Use --install|--verify|--uninstall [--yes]\n' >&2; exit 1; }
runner_user=cassy-ci
runner_home=/var/lib/cassy-ci
runner_dir="$runner_home/runner"
runner_logs=/var/log/cassy-ci-runner
state=/var/db/cassy-ci-runner
label=io.petrastella.cassy-ci
plist="/Library/LaunchDaemons/$label.plist"
wrapper=/usr/local/libexec/cassy-ci-runner.sh
ace="user:$runner_user deny read,execute,readattr,readextattr,readsecurity"
runner_version=2.336.0
archive="actions-runner-osx-arm64-$runner_version.tar.gz"
archive_sha=8e8839c49b7060b6b2154f4931f815df330c27f167d53ef2239ee3dfce28b079
xcode=/Applications/Xcode_26.3.app/Contents/Developer

fail() { printf 'STOP: %s\n' "$*" >&2; exit 1; }
approve() { if [[ "$confirmed" == --yes ]]; then return 0; else confirm "$1"; fi; }
as_runner() {
    /usr/bin/sudo -n -H -u "$runner_user" /usr/bin/env -i \
        HOME="$runner_home" USER="$runner_user" LOGNAME="$runner_user" \
        PATH="$runner_home/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
        CARGO_HOME="$runner_home/.cargo" RUSTUP_HOME="$runner_home/.rustup" "$@"
}
owned_account() {
    [[ -f "$state/uid" && "$(/usr/bin/stat -f %u "$state")" == 0 ]] || fail "No root-owned installation receipt at $state"
    [[ "$(/usr/bin/id -u "$runner_user")" == "$(cat "$state/uid")" ]] || fail "Account UID differs from the installation receipt"
}
verify_isolation() {
    owned_account
    case " $(/usr/bin/id -Gn "$runner_user") " in *' admin '*|*' wheel '*) fail "Runner account is privileged" ;; esac
    if as_runner /usr/bin/sudo -n /usr/bin/true 2>/dev/null; then fail "Runner account can sudo"; fi
    [[ -s "$state/protected-homes" ]] || fail "No operator homes recorded"
    while IFS= read -r home; do
        [[ -d "$home" ]] || continue
        if as_runner /bin/test -r "$home" || as_runner /bin/test -x "$home"; then
            fail "Runner can read or traverse $home"
        fi
        if as_runner /bin/test -r "$home/Library/Keychains"; then fail "Runner can read an operator keychain"; fi
        printf 'DENIED to %s: %s (including its keychains and private files)\n' "$runner_user" "$home"
    done < "$state/protected-homes"
    [[ "$(as_runner /usr/bin/printenv HOME)" == "$runner_home" ]] || fail "Wrong runner HOME"
    say "Account is non-admin; operator homes are inaccessible. Service uses its own HOME, Cargo and keychain namespace."
}

case "$operation" in
    --install|--uninstall|--verify) ;;
    *) fail "Usage: sudo $0 --install|--verify|--uninstall (tokens via environment, never command arguments)" ;;
esac
[[ "$(/usr/bin/uname -s)" == Darwin && "$(/usr/bin/uname -m)" == arm64 ]] || fail "Run on prowl (macOS ARM64)"
[[ "$(/usr/bin/id -u)" == 0 ]] || fail "Run with sudo"
# Suppress inherited tracing before touching any token.
set +x
umask 077

if [[ "$operation" == --verify ]]; then
    verify_isolation
    /bin/launchctl print "system/$label"
    exit 0
fi

if [[ "$operation" == --uninstall ]]; then
    TOTAL_STAGES=2
    owned_account
    if [[ -f "$runner_dir/.runner" ]]; then
        [[ -n "${RUNNER_REMOVE_TOKEN:-}" ]] || fail "Supervisor must provide a fresh repository runner removal token"
    fi
    say "Stop and unregister prowl-cas-mac, remove only this install's deny ACLs, then delete cassy-ci and its CI-only files."
    if ! approve "Uninstall the dedicated Cassy CI account and runner?"; then exit 0; fi
    stage "Stop and unregister"
    /bin/launchctl bootout "system/$label" 2>/dev/null || true
    # Token is piped to the service account, never placed in argv or a file.
    if [[ -f "$runner_dir/.runner" ]]; then
        printf '%s\n' "$RUNNER_REMOVE_TOKEN" | as_runner /bin/bash -c '
            read -r ACTIONS_RUNNER_INPUT_TOKEN
            export ACTIONS_RUNNER_INPUT_TOKEN
            cd "$HOME/runner"
            exec ./config.sh remove --unattended
        '
    fi
    unset RUNNER_REMOVE_TOKEN
    # Never remove home deny ACLs while any process still has this UID.
    if /usr/bin/pgrep -u "$(cat "$state/uid")" >/dev/null; then
        fail "CI processes remain after service stop; keep isolation in place and inspect them before retrying uninstall"
    fi
    stage "Remove this installation"
    while IFS= read -r home; do
        [[ ! -d "$home" ]] || /bin/chmod -a "$ace" "$home"
    done < "$state/protected-homes"
    /bin/rm -f "$plist" "$wrapper"
    /usr/bin/dscl . -delete "/Users/$runner_user"
    /usr/bin/dscl . -delete "/Groups/$runner_user"
    /bin/rm -rf "$runner_home" "$runner_logs" "$state"
    finish
    exit 0
fi

TOTAL_STAGES=4
stage "Check prerequisites"
[[ -n "${RUNNER_TOKEN:-}" ]] || fail "Supervisor must provide a fresh repository runner registration token"
[[ -x "$xcode/usr/bin/xcodebuild" ]] || fail "Install and license Xcode 26.3 at $xcode before provisioning"
[[ ! -e "$state" && ! -e "$runner_home" && ! -e "$runner_logs" && ! -e "$plist" && ! -e "$wrapper" ]] || fail "Existing installation: verify or uninstall it first"
if /usr/bin/id "$runner_user" >/dev/null 2>&1 || /usr/bin/dscl . -read "/Groups/$runner_user" >/dev/null 2>&1; then
    fail "Refusing to adopt an existing account or group"
fi
say "Create a hidden non-admin cassy-ci account, deny it access to existing human homes, install the pinned runner and Rust in its own home, then start a system launchd service."
say "No operator files are copied. One deny ACL is added to each existing human home; uninstall removes those exact entries."
if ! approve "Provision this isolated runner on prowl?"; then exit 0; fi

stage "Create the service account and isolate operator homes"
/bin/mkdir -m 0700 "$state"
# Allocate a UID/GID unused by both local users and groups, never reuse a human.
used_users="$(/usr/bin/dscl . -list /Users UniqueID)" || fail "Cannot enumerate local users"
used_groups="$(/usr/bin/dscl . -list /Groups PrimaryGroupID)" || fail "Cannot enumerate local groups"
used_ids="$used_users
$used_groups"
ci_uid=550
while printf '%s\n' "$used_ids" | /usr/bin/awk -v candidate="$ci_uid" '$NF == candidate {found=1} END {exit !found}'; do
    ci_uid=$((ci_uid + 1))
    [[ "$ci_uid" -lt 1000 ]] || fail "No free service UID below 1000"
done
/usr/bin/dscl . -create "/Groups/$runner_user"
/usr/bin/dscl . -create "/Groups/$runner_user" PrimaryGroupID "$ci_uid"
/usr/bin/dscl . -create "/Groups/$runner_user" GeneratedUID "$(/usr/bin/uuidgen)"
/usr/bin/dscl . -create "/Users/$runner_user"
/usr/bin/dscl . -create "/Users/$runner_user" UniqueID "$ci_uid"
/usr/bin/dscl . -create "/Users/$runner_user" GeneratedUID "$(/usr/bin/uuidgen)"
/usr/bin/dscl . -create "/Users/$runner_user" RealName 'Cassy CI service'
/usr/bin/dscl . -create "/Users/$runner_user" PrimaryGroupID "$ci_uid"
/usr/bin/dscl . -create "/Users/$runner_user" NFSHomeDirectory "$runner_home"
/usr/bin/dscl . -create "/Users/$runner_user" UserShell /usr/bin/false
/usr/bin/dscl . -create "/Users/$runner_user" IsHidden 1
/usr/bin/dscl . -create "/Users/$runner_user" AuthenticationAuthority ';DisabledUser;'
printf '%s\n' "$ci_uid" > "$state/uid"
/bin/mkdir -p "$runner_dir" "$runner_home/Library/Keychains"
/usr/sbin/chown -R "$runner_user:$runner_user" "$runner_home"
/bin/chmod 0700 "$runner_home"
# Root owns the log directory so CI cannot replace launchd log paths with symlinks.
/bin/mkdir -m 0750 "$runner_logs"
/usr/sbin/chown "root:$runner_user" "$runner_logs"
: > "$runner_logs/service.log"
: > "$runner_logs/service-error.log"
/usr/sbin/chown "$runner_user:$runner_user" "$runner_logs/"*.log
/bin/chmod 0640 "$runner_logs/"*.log
: > "$state/protected-homes"
# Directory Services covers homes outside /Users as well as the sudo owner.
while read -r account uid; do
    [[ "$uid" =~ ^[0-9]+$ && "$uid" -ge 501 && "$account" != "$runner_user" ]] || continue
    home="$(/usr/bin/dscl . -read "/Users/$account" NFSHomeDirectory | /usr/bin/sed 's/^NFSHomeDirectory: //')"
    [[ -d "$home" && "$home" != / && "$home" != "$runner_home" && "$home" != /var/empty ]] || continue
    /bin/chmod +a# 0 "$ace" "$home"
    printf '%s\n' "$home" >> "$state/protected-homes"
done < <(printf '%s\n' "$used_users")
verify_isolation

stage "Install runner and CI-owned Rust"
/usr/bin/curl --fail --location --proto '=https' --tlsv1.2 \
    "https://github.com/actions/runner/releases/download/v$runner_version/$archive" -o "$state/$archive"
printf '%s  %s\n' "$archive_sha" "$state/$archive" | /usr/bin/shasum -a 256 -c -
/usr/bin/tar -xzf "$state/$archive" -C "$runner_dir"
/usr/sbin/chown -R "$runner_user:$runner_user" "$runner_dir"
# Execute bootstrap as the isolated user. No sudo/admin tools or operator HOME.
/usr/bin/curl --fail --location --proto '=https' --tlsv1.2 https://sh.rustup.rs \
    | as_runner /bin/sh -s -- -y --profile minimal --default-toolchain stable
printf '%s\n' "$RUNNER_TOKEN" | as_runner /bin/bash -c '
    read -r ACTIONS_RUNNER_INPUT_TOKEN
    export ACTIONS_RUNNER_INPUT_TOKEN
    cd "$HOME/runner"
    exec ./config.sh --unattended --url https://github.com/Richards-LLC/cassy \
        --name prowl-cas-mac --labels cas-mac,trusted-branches --work _work
'
unset RUNNER_TOKEN

stage "Install launchd service and verify"
/bin/mkdir -p /usr/local/libexec
cat > "$wrapper" <<'WRAPPER'
#!/bin/bash
set -euo pipefail
cd /var/lib/cassy-ci/runner
exec ./runsvc.sh
WRAPPER
/usr/sbin/chown root:wheel "$wrapper"
/bin/chmod 0755 "$wrapper"
cat > "$plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>io.petrastella.cassy-ci</string>
<key>ProgramArguments</key><array><string>/usr/local/libexec/cassy-ci-runner.sh</string></array>
<key>UserName</key><string>cassy-ci</string><key>GroupName</key><string>cassy-ci</string>
<key>WorkingDirectory</key><string>/var/lib/cassy-ci/runner</string>
<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
<key>ProcessType</key><string>Background</string><key>Umask</key><integer>63</integer>
<key>EnvironmentVariables</key><dict>
<key>HOME</key><string>/var/lib/cassy-ci</string>
<key>CARGO_HOME</key><string>/var/lib/cassy-ci/.cargo</string>
<key>RUSTUP_HOME</key><string>/var/lib/cassy-ci/.rustup</string>
<key>PATH</key><string>/var/lib/cassy-ci/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
<key>DEVELOPER_DIR</key><string>/Applications/Xcode_26.3.app/Contents/Developer</string>
</dict>
<key>StandardOutPath</key><string>/var/log/cassy-ci-runner/service.log</string>
<key>StandardErrorPath</key><string>/var/log/cassy-ci-runner/service-error.log</string>
</dict></plist>
PLIST
/usr/sbin/chown root:wheel "$plist"
/bin/chmod 0644 "$plist"
/usr/bin/plutil -lint "$plist"
verify_isolation
/bin/launchctl bootstrap system "$plist"
/bin/launchctl print "system/$label"
say "Runner registered as prowl-cas-mac. Supervisor must record live online/offline routing runs and elapsed minutes."
finish
