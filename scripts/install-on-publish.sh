#!/usr/bin/env bash
# Install each published CAS release on this host, with no agent hop (cas-f95b).
#
#   scripts/install-on-publish.sh --install     # install the user timer (once per host)
#   scripts/install-on-publish.sh --uninstall   # remove it
#   scripts/install-on-publish.sh --once        # one check; what the timer runs
#
# Trigger: a systemd user timer runs --once every 60 s. Each run makes one
# authenticated GitHub API call (latest release), 60 of gh's 5,000 calls an
# hour. It was chosen over the alternatives because it is the smallest thing
# that keeps working: a webhook cannot reach this host; the GitHub release
# event has nothing on this host to deliver to; and a watcher inside the cas
# daemon would be Rust that only a new release could ship, would stop when the
# daemon stops, and would update the binary it is running. The timer survives
# reboots and daemon restarts, and its journal is the install log
# (`journalctl --user -u cassy-install-on-publish`).
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
script_path="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"

log() { printf 'install-on-publish: %s\n' "$*"; }
now() { printf '%s\n' "${RELEASE_INSTALL_NOW:-$(date -u +%s)}"; }
iso() { date -u -d "@$1" +%Y-%m-%dT%H:%M:%SZ; }

install_units() {
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
    local tag="$1" installed_at="$2" body notes marker pr opened minutes host
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
    if timeout 900 "$cas_bin" update --yes --version "$latest" && [[ "$(installed_version)" == "$latest" ]]; then
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
        exec 9>"$state_dir/install-on-publish.lock"
        flock -n 9 || { log "another install check is running"; exit 0; }
        run_once
        ;;
    -h|--help) sed -n '2,27p' "$0" | sed 's/^# \{0,1\}//' ;;
    *) printf 'usage: %s [--install | --uninstall | --once]\n' "$0" >&2; exit 2 ;;
esac
