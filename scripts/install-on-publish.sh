#!/usr/bin/env bash
# Install each published CAS release on this host, with no agent hop (cas-f95b).
#
#   scripts/install-on-publish.sh --install     # install the user timer (once per host)
#   scripts/install-on-publish.sh --uninstall   # remove it
#   scripts/install-on-publish.sh --once        # one check; what the timer runs
#
# Trigger: a systemd user timer (Linux) or LaunchAgent (macOS) runs --once
# every 60 s. Each run makes one authenticated GitHub API call (latest release),
# 60 of gh's 5,000 calls an hour. It was chosen over the alternatives because it is the smallest thing
# that keeps working: a webhook cannot reach this host; the GitHub release
# event has nothing on this host to deliver to; and a watcher inside the cas
# daemon would be Rust that only a new release could ship, would stop when the
# daemon stops, and would update the binary it is running. The timer survives
# reboots and daemon restarts. Linux logs go to the systemd journal; macOS
# logs go to ~/.local/state/cassy/install-on-publish.log (or XDG_STATE_HOME).
#
# When the latest release is newer than `cas --version`, --once runs
# `cas update --yes --version <x.y.z>`, checks `cas --version`, and completes
# the release timer in the GitHub release notes: release.yml writes
# "PR #N opened -> published" with a marker; the first host to install adds
# "-> installed", the one timer from PR opened to installed. A failed install
# is retried after RELEASE_INSTALL_RETRY_SECONDS (default 600).
#
# Environment: CAS_RELEASE_REPO (default Richards-LLC/cassy), CAS_BIN (cas),
# GH_BIN (gh), XDG_STATE_HOME, RELEASE_INSTALL_NOW (epoch, tests),
# RELEASE_INSTALL_HOST (host label in the notes; default hostname -s).
set -euo pipefail

repo="${CAS_RELEASE_REPO:-Richards-LLC/cassy}"
cas_bin="${CAS_BIN:-cas}"
gh_bin="${GH_BIN:-gh}"
state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/cassy"
state_file="$state_dir/install-on-publish.last-failure"
retry_seconds="${RELEASE_INSTALL_RETRY_SECONDS:-600}"
unit_name=cassy-install-on-publish
agent_label=dev.cassy.install-on-publish
platform="$(uname -s)"
script_path="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"

log() { printf 'install-on-publish: %s\n' "$*"; }
now() { printf '%s\n' "${RELEASE_INSTALL_NOW:-$(date -u +%s)}"; }
iso() {
    if [[ "$platform" == Darwin ]]; then
        date -u -r "$1" +%Y-%m-%dT%H:%M:%SZ
    else
        date -u -d "@$1" +%Y-%m-%dT%H:%M:%SZ
    fi
}

xml() { printf '%s' "$1" | sed 's/\&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g; s/"/\&quot;/g'; }

install_agent() {
    local libexec="$HOME/.local/libexec/cassy" agents="$HOME/Library/LaunchAgents"
    local plist="$HOME/Library/LaunchAgents/$agent_label.plist" domain
    local resolved_cas resolved_gh
    domain="gui/$(id -u)"
    resolved_cas="$(command -v "$cas_bin")"
    resolved_gh="$(command -v "$gh_bin")"
    mkdir -p "$libexec" "$agents" "$state_dir"
    # Unload the old job before replacing its executable on a reinstall.
    launchctl bootout "$domain" "$plist" 2>/dev/null || true
    install -m 0755 "$script_path" "$libexec/install-on-publish.sh"
    cat >"$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
    <key>Label</key><string>$agent_label</string>
    <key>ProgramArguments</key><array>
        <string>$(xml "$libexec/install-on-publish.sh")</string><string>--once</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>StartInterval</key><integer>60</integer>
    <key>EnvironmentVariables</key><dict>
        <key>PATH</key><string>$(xml "$HOME/.local/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")</string>
        <key>CAS_BIN</key><string>$(xml "$resolved_cas")</string>
        <key>GH_BIN</key><string>$(xml "$resolved_gh")</string>
        <key>CAS_RELEASE_REPO</key><string>$(xml "$repo")</string>
        <key>XDG_STATE_HOME</key><string>$(xml "${XDG_STATE_HOME:-$HOME/.local/state}")</string>
    </dict>
    <key>StandardOutPath</key><string>$(xml "$state_dir/install-on-publish.log")</string>
    <key>StandardErrorPath</key><string>$(xml "$state_dir/install-on-publish.log")</string>
</dict></plist>
EOF
    chmod 0644 "$plist"
    launchctl enable "$domain/$agent_label"
    if ! launchctl bootstrap "$domain" "$plist"; then
        # bootout can return while the previous job is still shutting down.
        log "launchctl bootstrap failed; retrying once in 1 s"
        sleep 1
        launchctl bootstrap "$domain" "$plist"
    fi
    log "installed $agent_label LaunchAgent; logs: $state_dir/install-on-publish.log"
}

uninstall_agent() {
    local plist="$HOME/Library/LaunchAgents/$agent_label.plist"
    launchctl bootout "gui/$(id -u)" "$plist" 2>/dev/null || true
    rm -f "$plist" "$HOME/.local/libexec/cassy/install-on-publish.sh"
    log "removed $agent_label LaunchAgent"
}

install_units() {
    if [[ "$platform" == Darwin ]]; then install_agent; return; fi
    local libexec="$HOME/.local/libexec/cassy" units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
    local source_dir
    source_dir="$(cd "$(dirname "$script_path")/.." && pwd)/ops/systemd"
    mkdir -p "$libexec" "$units"
    install -m 0755 "$script_path" "$libexec/install-on-publish.sh"
    install -m 0644 "$source_dir/$unit_name.service" "$units/$unit_name.service"
    install -m 0644 "$source_dir/$unit_name.timer" "$units/$unit_name.timer"
    systemctl --user daemon-reload
    systemctl --user enable --now "$unit_name.timer"
    log "installed $unit_name.timer; logs: journalctl --user -u $unit_name"
}

uninstall_units() {
    if [[ "$platform" == Darwin ]]; then uninstall_agent; return; fi
    local units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
    systemctl --user disable --now "$unit_name.timer" 2>/dev/null || true
    rm -f "$units/$unit_name.service" "$units/$unit_name.timer" "$HOME/.local/libexec/cassy/install-on-publish.sh"
    systemctl --user daemon-reload
    log "removed $unit_name.timer"
}

version_greater() {
    [[ "$1" != "$2" && "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n 1)" == "$1" ]]
}

installed_version() {
    "$cas_bin" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -n 1
}

# Replace "Installed: pending." with the install time and the full timer.
complete_timer() {
    local tag="$1" installed_at="$2" body notes marker pr opened minutes host replacement
    body="$("$gh_bin" api "repos/$repo/releases/tags/$tag" --jq .body 2>/dev/null)" || {
        log "could not read the $tag release notes; timer not completed"
        return 0
    }
    if ! grep -q 'Installed: pending\.' <<<"$body"; then
        return 0
    fi
    marker="$(grep -oE '<!-- cas-release-timer [^>]*-->' <<<"$body" | head -n 1 || true)"
    pr="$(sed -n 's/.* pr=\([0-9]*\) .*/\1/p' <<<"$marker")"
    opened="$(sed -n 's/.* opened=\([0-9]*\) .*/\1/p' <<<"$marker")"
    host="${RELEASE_INSTALL_HOST:-$(hostname -s 2>/dev/null || echo host)}"
    if [[ -n "$pr" && -n "$opened" ]]; then
        minutes=$(( (installed_at - opened + 59) / 60 ))
        replacement="Installed on $host $(iso "$installed_at"). **PR #$pr opened → installed: $minutes min.**"
    else
        replacement="Installed on $host $(iso "$installed_at")."
    fi
    notes="$(mktemp)"
    REPLACEMENT="$replacement" INSTALLED_AT="$installed_at" perl -pe '
        s/Installed: pending\./$ENV{REPLACEMENT}/;
        s/(<!-- cas-release-timer [^>]*?) -->/$1 installed=$ENV{INSTALLED_AT} -->/;
    ' <<<"$body" >"$notes"
    if "$gh_bin" release edit "$tag" --repo "$repo" --notes-file "$notes" >/dev/null; then
        log "release timer for $tag: $replacement"
    else
        log "could not update the $tag release notes; timer not completed"
    fi
    rm -f "$notes"
}

update_release() {
    if [[ "$platform" == Darwin ]]; then
        # Stock macOS has Perl but no GNU timeout. The alarm survives exec.
        perl -e 'alarm 900; exec @ARGV or die "exec: $!\n"' "$cas_bin" update --yes --version "$1"
    else
        timeout 900 "$cas_bin" update --yes --version "$1"
    fi
}

run_once() {
    local tag latest current started last_version last_at
    tag="$("$gh_bin" api "repos/$repo/releases/latest" --jq .tag_name 2>/dev/null)" || {
        log "latest release lookup failed; retrying on the next tick"
        return 0
    }
    latest="${tag#v}"
    [[ "$latest" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { log "latest release tag is not v<x.y.z>: $tag"; return 0; }
    current="$(installed_version || true)"
    if [[ -n "$current" ]] && ! version_greater "$latest" "$current"; then
        return 0
    fi
    if [[ -f "$state_file" ]]; then
        read -r last_version last_at <"$state_file" || true
        if [[ "$last_version" == "$latest" && -n "$last_at" ]] && (( $(now) - last_at < retry_seconds )); then
            return 0
        fi
    fi
    started="$(now)"
    log "installing $tag over ${current:-nothing}"
    if update_release "$latest" && [[ "$(installed_version)" == "$latest" ]]; then
        rm -f "$state_file"
        log "installed $tag in $(( $(now) - started )) s"
        complete_timer "$tag" "$(now)"
    else
        mkdir -p "$state_dir"
        printf '%s %s\n' "$latest" "$(now)" >"$state_file"
        log "install of $tag failed; next attempt in ${retry_seconds} s"
        return 1
    fi
}

case "${1:---once}" in
    --install) install_units ;;
    --uninstall) uninstall_units ;;
    --once)
        mkdir -p "$state_dir"
        if [[ "$platform" == Darwin ]]; then
            # shlock atomically claims a PID lock and recovers a dead owner's lock.
            shlock -p "$$" -f "$state_dir/install-on-publish.lock" || { log "another install check is running"; exit 0; }
            trap 'rm -f "$state_dir/install-on-publish.lock"' EXIT
            trap 'exit 130' INT
            trap 'exit 143' TERM
        else
            exec 9>"$state_dir/install-on-publish.lock"
            flock -n 9 || { log "another install check is running"; exit 0; }
        fi
        run_once
        ;;
    -h|--help) sed -n '2,27p' "$0" | sed 's/^# \{0,1\}//' ;;
    *) printf 'usage: %s [--install | --uninstall | --once]\n' "$0" >&2; exit 2 ;;
esac
