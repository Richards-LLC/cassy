#!/usr/bin/env bash
# Pin the Linux Cassy hub to a release-built binary and isolate it from factory
# load (cas-93356). The operator runs this; agents never do.
#
#   scripts/hub-pin.sh install [--dry-run] [--restart]
#       Idempotent. Install the host-isolation drop-ins and point the hub unit at
#       ~/.local/lib/cassy/hub/current, which links to the installed released cas.
#       Reloads systemd and checks the effective values. The running hub keeps
#       running unless --restart is given; running factories are not moved.
#   scripts/hub-pin.sh inspect
#       Print the pin state. Changes nothing.
#   scripts/hub-pin.sh check-binary <path>
#       Report whether a binary may be pinned. Changes nothing.
#   scripts/hub-pin.sh stage --run-id <id> --sha <full-sha>
#       Fetch and verify a hotfix from a successful Release Prebuild run into an
#       immutable slot. Inert: the hub is not touched. There is no way to stage a
#       local file, so a `cas qa candidate` or other debug build cannot get in.
#   scripts/hub-pin.sh activate --sha <full-sha> --expected-current <target> [--dry-run]
#       Point the hub at a staged slot and restart it once. If it does not come up
#       on those bytes, restore the previous target, restart once more and stop.
#   scripts/hub-pin.sh rollback --expected-current <target> [--dry-run]
#       Return the hub to the released binary it last ran, and restart it once.
#
# `cas update` clears a hotfix pin when a new release installs: the new binary
# repoints `current` before it restarts the hub (hub_service.rs). Releases are
# built, audited and timed as before; this adds no build, job or timer.
#
# Environment: CAS_RELEASE_REPO (default Richards-LLC/cassy), CAS_BIN (cas),
# GH_BIN (gh), SYSTEMCTL_BIN (systemctl), XDG_CONFIG_HOME, XDG_STATE_HOME.
set -euo pipefail

repo="${CAS_RELEASE_REPO:-Richards-LLC/cassy}"
cas_bin="${CAS_BIN:-cas}"
gh_bin="${GH_BIN:-gh}"
systemctl_bin="${SYSTEMCTL_BIN:-systemctl}"
prebuild_workflow=.github/workflows/release-prebuild.yml
prebuild_job="Prebuild Linux x86_64"
prebuild_audit_step="Audit build outputs and packaged Linux executable"
artifact=cas-x86_64-unknown-linux-gnu
dropin_name=90-cassy-host-isolation.conf

lib_dir="$HOME/.local/lib/cassy/hub"
current="$lib_dir/current"
versions="$lib_dir/versions"
state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/cassy"
receipt="$state_dir/hub-pin.json"
units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
unit="$units/cas-hub.service"
script_path="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
ops_dir="$(dirname "$(dirname "$script_path")")/ops/systemd"

dry_run=false

log() { printf 'hub-pin: %s\n' "$*"; }
die() { printf 'hub-pin: REFUSED: %s\n' "$*" >&2; exit 1; }
usage() { sed -n '2,/^set -euo/p' "$script_path" | sed '$d; s/^# \{0,1\}//' >&2; exit 2; }
sha256() { sha256sum "$1" | cut -d' ' -f1; }
systemctl_user() { "$systemctl_bin" --user "$@"; }

# Perform a host change, or only describe it in a dry run.
act() {
    if "$dry_run"; then log "would: $*"; else "$@"; fi
}

take_lock() {
    "$dry_run" && return 0
    mkdir -p "$state_dir"
    exec 9>"$state_dir/hub-pin.lock"
    flock -n 9 || die "another hub pin operation is running"
}

# A debug build is never pinned. The section check inspects the bytes, so it does
# not depend on what the file is called; it is a floor, not proof of provenance.
check_release_binary() {
    local path="$1" sections
    [[ -f "$path" && -x "$path" ]] || die "$path is not an executable file"
    case "$path" in
        */target/debug/*|*/.cas/worktrees/*) die "$path is a development build path, not a release" ;;
    esac
    sections="$(readelf -S "$path" 2>/dev/null)" || die "cannot read $path as an ELF executable (readelf is required)"
    if grep -q '\.debug_info' <<<"$sections"; then
        die "$path is a debug build (.debug_info present); the hub only runs release builds"
    fi
}

# Pin metadata is written below, one field per line, so this reads it back.
receipt_field() {
    sed -n "s/^  \"$2\": \"\(.*\)\",\{0,1\}\$/\1/p" "$1"
}

write_atomic_file() {
    local path="$1" content="$2" tmp
    tmp="$path.tmp.$$"
    printf '%s\n' "$content" >"$tmp"
    chmod 0600 "$tmp"
    mv -f "$tmp" "$path"
}

# Atomically retarget the managed path: a sibling link renamed over it.
point_current_at() {
    local target="$1" tmp="$current.tmp.$$"
    ln -sfn "$target" "$tmp"
    mv -Tf "$tmp" "$current"
}

base_exec_binary() {
    sed -n 's/^ExecStart=\([^ ]*\).*/\1/p' "$unit" | head -n 1
}

execstart_overrides() {
    local dir="$unit.d" file
    [[ -d "$dir" ]] || return 0
    for file in "$dir"/*.conf; do
        [[ -f "$file" ]] && grep -l '^[[:space:]]*ExecStart=' "$file" || true
    done
}

require_pinned_unit() {
    [[ -f "$unit" ]] || die "no hub service at $unit; run \`cas hub service install\`, then \`hub-pin.sh install\`"
    [[ "$(base_exec_binary)" == "$current" ]] || die "the hub unit does not launch $current; run \`hub-pin.sh install\` first"
    local overrides
    overrides="$(execstart_overrides)"
    [[ -z "$overrides" ]] || die "a drop-in overrides the hub's ExecStart, so a pin would not take effect: $overrides"
}

# Run the hub's own restart once and prove the manager runs the pinned bytes.
restart_and_verify() {
    local pid exe want
    # Close the lock: a hub that outlives this script must not keep holding it.
    "$cas_bin" hub restart >/dev/null 9>&- || return 1
    pid="$(systemctl_user show -p MainPID --value cas-hub.service)" || return 1
    [[ "$pid" =~ ^[1-9][0-9]*$ ]] || return 1
    exe="$(readlink -f "/proc/$pid/exe")" || return 1
    want="$(readlink -f "$current")"
    [[ "$exe" == "$want" ]]
}

install_if_changed() {
    local source="$1" target="$2" mode="${3:-0644}"
    if [[ -f "$target" ]] && cmp -s "$source" "$target"; then
        return 1
    fi
    act mkdir -p "$(dirname "$target")"
    act install -m "$mode" "$source" "$target"
    return 0
}

cmd_install() {
    local restart=false changed=false installed existing backup effective
    while (($#)); do
        case "$1" in
            --dry-run) dry_run=true ;;
            --restart) restart=true ;;
            *) usage ;;
        esac
        shift
    done
    [[ "$(uname -s)" == Linux ]] || die "the hub pin is Linux systemd only"
    [[ -f "$unit" ]] || die "no hub service at $unit; run \`cas hub service install\` first"
    installed="$(command -v "$cas_bin")" || die "cannot find $cas_bin"
    check_release_binary "$installed"
    local overrides
    overrides="$(execstart_overrides)"
    [[ -z "$overrides" ]] || die "a drop-in overrides the hub's ExecStart; move it aside first, keeping a copy: $overrides"
    existing="$(base_exec_binary)"
    [[ "$existing" == /* ]] || die "the hub unit has no absolute ExecStart binary"
    if [[ "$existing" != "$current" ]] && [[ "$(readlink -f "$existing")" != "$(readlink -f "$installed")" ]]; then
        die "the hub unit launches $existing, not the installed $installed; run \`cas hub service install\` first"
    fi
    take_lock

    if [[ -L "$current" ]]; then
        log "pin path exists: $current -> $(readlink "$current")"
    else
        [[ -e "$current" ]] && die "$current exists and is not a symlink"
        act mkdir -p "$versions"
        act chmod 0700 "$lib_dir" "$versions"
        act point_current_at "$installed"
        log "pin path: $current -> $installed"
    fi

    if [[ "$existing" != "$current" ]]; then
        backup="$state_dir/hub-pin-backup/cas-hub.service.$(date -u +%Y%m%dT%H%M%SZ)"
        act mkdir -p "$(dirname "$backup")"
        act cp -p "$unit" "$backup"
        act sed -i "s|^ExecStart=$existing |ExecStart=$current |" "$unit"
        log "hub unit: ExecStart $existing -> $current (previous unit kept at $backup)"
        changed=true
    fi

    # Drop-ins layer over a base unit or slice another owner may manage.
    local name
    for name in cas-hub.service agent.slice agent-builds.slice; do
        if install_if_changed "$ops_dir/$name.d/$dropin_name" "$units/$name.d/$dropin_name"; then
            log "drop-in: $units/$name.d/$dropin_name"
            changed=true
        fi
    done
    if [[ ! -f "$units/agent.slice" ]]; then
        act install -m 0644 "$ops_dir/agent.slice" "$units/agent.slice"
        log "slice: $units/agent.slice"
        changed=true
    fi
    if install_if_changed "$script_path" "$HOME/.local/libexec/cassy/hub-pin.sh" 0755; then
        log "helper: $HOME/.local/libexec/cassy/hub-pin.sh"
    fi

    if "$dry_run"; then
        log "dry run: nothing was changed and systemctl was not run"
        if "$restart"; then log "would: $cas_bin hub restart"; fi
        return 0
    fi
    if "$changed"; then
        systemctl_user daemon-reload
    else
        log "already installed; nothing changed"
    fi

    effective="$(systemctl_user show -p CPUWeight -p MemoryLow -p ExecStart cas-hub.service)"
    grep -q '^CPUWeight=1000$' <<<"$effective" || die "effective hub CPUWeight is not 1000: $effective"
    grep -q '^MemoryLow=1073741824$' <<<"$effective" || die "effective hub MemoryLow is not 1G: $effective"
    grep -q "path=$current " <<<"$effective" || die "effective hub ExecStart is not $current: $effective"
    [[ "$(systemctl_user show -p CPUWeight --value agent.slice)" == 40 ]] || die "effective agent.slice CPUWeight is not 40"
    [[ "$(systemctl_user show -p CPUWeight --value agent-builds.slice)" == 25 ]] || die "effective agent-builds.slice CPUWeight is not 25"
    log "effective: hub CPUWeight=1000 MemoryLow=1G ExecStart=$current; agent.slice 40; agent-builds.slice 25"

    if "$restart"; then
        restart_and_verify || die "the hub did not come back on $current; check \`cas hub status\`"
        log "hub restarted on $(readlink -f "$current")"
    else
        log "the running hub was not restarted; ExecStart applies at its next restart"
    fi
}

cmd_inspect() {
    if [[ -L "$current" ]]; then
        log "current: $current -> $(readlink "$current")"
    else
        log "current: not installed"
    fi
    if [[ -f "$receipt" ]]; then
        log "hotfix pin active:"
        cat "$receipt"
    else
        log "no hotfix pin"
    fi
    if [[ -f "$unit" ]]; then log "unit ExecStart binary: $(base_exec_binary)"; else log "no hub unit at $unit"; fi
    local overrides
    overrides="$(execstart_overrides)"
    if [[ -n "$overrides" ]]; then log "ExecStart overrides: $overrides"; fi
}

cmd_check_binary() {
    (($# == 1)) || usage
    check_release_binary "$1"
    log "$1 may be pinned (no debug sections); stage still requires Release Prebuild provenance"
}

cmd_stage() {
    local run_id="" sha=""
    while (($#)); do
        case "$1" in
            --run-id) run_id="${2:-}"; shift ;;
            --sha) sha="${2:-}"; shift ;;
            *) usage ;;
        esac
        shift
    done
    [[ "$run_id" =~ ^[0-9]+$ ]] || die "--run-id must name a Release Prebuild run"
    [[ "$sha" =~ ^[0-9a-f]{40}$ ]] || die "--sha must be a full commit SHA"
    take_lock

    local conclusion head path full_name audit
    IFS=$'\t' read -r conclusion head path full_name < <(
        "$gh_bin" api "repos/$repo/actions/runs/$run_id" \
            --jq '[.conclusion, .head_sha, .path, .repository.full_name] | @tsv'
    ) || die "cannot read run $run_id"
    [[ "$path" == "$prebuild_workflow" ]] || die "run $run_id is $path, not $prebuild_workflow"
    [[ "$full_name" == "$repo" ]] || die "run $run_id belongs to $full_name, not $repo"
    [[ "$conclusion" == success ]] || die "run $run_id concluded '$conclusion', not success"
    [[ "$head" == "$sha" ]] || die "run $run_id built $head, not $sha"
    audit="$("$gh_bin" api "repos/$repo/actions/runs/$run_id/jobs?per_page=100" \
        --jq ".jobs[] | select(.name == \"$prebuild_job\") | .steps[] | select(.name == \"$prebuild_audit_step\") | .conclusion")" \
        || die "cannot read run $run_id's jobs"
    [[ "$audit" == success ]] || die "run $run_id has no passing packaged audit step (got '$audit')"

    mkdir -p "$versions"
    chmod 0700 "$lib_dir" "$versions"
    local work
    work="$(mktemp -d "$lib_dir/.stage.XXXXXX")"
    # shellcheck disable=SC2064 # Expand now: this path is local to the function.
    trap "rm -rf '$work'" EXIT
    "$gh_bin" run download "$run_id" --repo "$repo" --name "$artifact" --dir "$work/download" \
        || die "cannot download $artifact from run $run_id (it may have expired); use the normal release path"
    local archive="$work/download/$artifact.tar.gz"
    [[ -f "$archive" ]] || die "run $run_id's artifact has no $artifact.tar.gz"
    [[ "$(tar -tzf "$archive" | LC_ALL=C sort | tr '\n' ' ')" == "LICENSE cas " ]] \
        || die "unexpected archive members in $artifact.tar.gz"
    mkdir "$work/slot"
    tar -xzf "$archive" -C "$work/slot" cas
    [[ -f "$work/slot/cas" && ! -L "$work/slot/cas" ]] || die "the archive's cas is not a regular file"
    check_release_binary "$work/slot/cas"

    local digest slot="$versions/$sha"
    digest="$(sha256 "$work/slot/cas")"
    if [[ -d "$slot" ]]; then
        [[ "$(receipt_field "$slot/receipt.json" digest)" == "$digest" ]] || die "$slot already holds different bytes"
        log "already staged: $slot"
        return 0
    fi
    chmod 0555 "$work/slot/cas"
    cat >"$work/slot/receipt.json" <<EOF
{
  "source_sha": "$sha",
  "run_id": "$run_id",
  "workflow": "$prebuild_workflow",
  "repository": "$repo",
  "artifact": "$artifact",
  "digest": "$digest",
  "staged_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF
    chmod 0444 "$work/slot/receipt.json"
    # A directory cannot move between parents once it is read-only.
    mv -T "$work/slot" "$slot"
    chmod 0555 "$slot"
    log "staged $sha from run $run_id: $slot/cas (sha256 $digest)"
}

# Keep an immutable copy of the released bytes being left behind.
preserve_released() {
    local source="$1" digest="$2" slot="$versions/released-$digest"
    if [[ ! -d "$slot" ]]; then
        act mkdir -p "$slot"
        act install -m 0555 "$source" "$slot/cas"
        act chmod 0555 "$slot"
    fi
    printf '%s/cas\n' "$slot"
}

cmd_activate() {
    local sha="" expected=""
    while (($#)); do
        case "$1" in
            --sha) sha="${2:-}"; shift ;;
            --expected-current) expected="${2:-}"; shift ;;
            --dry-run) dry_run=true ;;
            *) usage ;;
        esac
        shift
    done
    [[ "$sha" =~ ^[0-9a-f]{40}$ ]] || die "--sha must be a full commit SHA"
    [[ -n "$expected" ]] || die "--expected-current is required; see \`hub-pin.sh inspect\`"
    require_pinned_unit
    take_lock

    local slot="$versions/$sha"
    [[ -f "$slot/receipt.json" && -f "$slot/cas" ]] || die "$sha is not staged; run \`hub-pin.sh stage\`"
    local digest
    digest="$(receipt_field "$slot/receipt.json" digest)"
    [[ "$(sha256 "$slot/cas")" == "$digest" ]] || die "$slot/cas does not match its staged digest"
    [[ "$(receipt_field "$slot/receipt.json" workflow)" == "$prebuild_workflow" ]] || die "$slot has no Release Prebuild receipt"
    check_release_binary "$slot/cas"

    local previous prior_receipt="" released_path released_digest preserved
    previous="$(readlink "$current")"
    [[ "$previous" == "$expected" ]] || die "current is $previous, not the expected $expected; re-read \`hub-pin.sh inspect\`"
    [[ "$previous" != "$slot/cas" ]] || { log "already pinned to $sha"; return 0; }
    if [[ -f "$receipt" ]]; then
        prior_receipt="$(cat "$receipt")"
        released_path="$(receipt_field "$receipt" released_path)"
        released_digest="$(receipt_field "$receipt" released_digest)"
        preserved="$(receipt_field "$receipt" preserved)"
    else
        released_path="$previous"
        released_digest="$(sha256 "$previous")"
        preserved="$(preserve_released "$previous" "$released_digest")"
    fi
    if "$dry_run"; then
        log "would: point $current at $slot/cas, write $receipt and run \`$cas_bin hub restart\` once"
        return 0
    fi

    mkdir -p "$state_dir"
    write_atomic_file "$receipt" "$(cat <<EOF
{
  "slot": "$slot/cas",
  "source_sha": "$sha",
  "run_id": "$(receipt_field "$slot/receipt.json" run_id)",
  "digest": "$digest",
  "previous": "$previous",
  "released_path": "$released_path",
  "released_digest": "$released_digest",
  "preserved": "$preserved",
  "activated_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF
)"
    point_current_at "$slot/cas"
    if restart_and_verify; then
        log "hub pinned to $sha"
        return 0
    fi
    log "the hub did not come up on $sha; restoring $previous"
    point_current_at "$previous"
    if [[ -n "$prior_receipt" ]]; then
        write_atomic_file "$receipt" "$prior_receipt"
    else
        rm -f "$receipt"
    fi
    if restart_and_verify; then
        die "activation of $sha failed and was rolled back; the hub is on $previous"
    fi
    die "activation of $sha failed and the rollback restart failed too; current is $previous; check \`cas hub status\`"
}

cmd_rollback() {
    local expected=""
    while (($#)); do
        case "$1" in
            --expected-current) expected="${2:-}"; shift ;;
            --dry-run) dry_run=true ;;
            *) usage ;;
        esac
        shift
    done
    [[ -n "$expected" ]] || die "--expected-current is required; see \`hub-pin.sh inspect\`"
    require_pinned_unit
    [[ -f "$receipt" ]] || die "no hotfix pin to roll back"
    take_lock

    local now target released_path released_digest preserved
    now="$(readlink "$current")"
    [[ "$now" == "$expected" ]] || die "current is $now, not the expected $expected; re-read \`hub-pin.sh inspect\`"
    released_path="$(receipt_field "$receipt" released_path)"
    released_digest="$(receipt_field "$receipt" released_digest)"
    preserved="$(receipt_field "$receipt" preserved)"
    # The installed binary may have moved on; the preserved copy cannot have.
    if [[ -f "$released_path" && "$(sha256 "$released_path")" == "$released_digest" ]]; then
        target="$released_path"
    else
        target="$preserved"
    fi
    [[ "$(sha256 "$target")" == "$released_digest" ]] || die "$target does not match the released digest"
    check_release_binary "$target"
    if "$dry_run"; then
        log "would: point $current at $target and run \`$cas_bin hub restart\` once"
        return 0
    fi
    point_current_at "$target"
    if [[ "$target" == "$released_path" ]]; then
        rm -f "$receipt"
    else
        # A slot target keeps a receipt: `cas update` only advances a pin it can prove.
        write_atomic_file "$receipt" "$(sed "s|^  \"slot\": \".*\",\$|  \"slot\": \"$target\",|" "$receipt")"
    fi
    restart_and_verify || die "rolled back to $target, but the hub did not come up on it; check \`cas hub status\`"
    log "hub rolled back to $target"
}

command="${1:-}"
(($#)) && shift
case "$command" in
    install) cmd_install "$@" ;;
    inspect) cmd_inspect "$@" ;;
    check-binary) cmd_check_binary "$@" ;;
    stage) cmd_stage "$@" ;;
    activate) cmd_activate "$@" ;;
    rollback) cmd_rollback "$@" ;;
    *) usage ;;
esac
