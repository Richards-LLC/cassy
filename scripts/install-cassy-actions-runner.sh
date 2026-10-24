#!/usr/bin/env bash
# Linux-only runner provisioning: the pinned Linux archive and GNU coreutils are required.
# Install the pinned GitHub runner and register it in the pre-created,
# selected-repository/selected-workflow group. Run from a trusted checkout:
#   RUNNER_SLOT=2 SCCACHE_SOURCE="$(command -v sccache)" \
#     sudo --preserve-env=RUNNER_TOKEN,RUNNER_SLOT,SCCACHE_SOURCE \
#       scripts/install-cassy-actions-runner.sh
# Slots 3 and 4 (cas-3b5e) are light lane slots: label cas-lane-light only,
# no Rust/sccache, CPU-capped units. RUNNER_REGISTRATION=repo uses a repository
# token and omits the organization runner group.
set -euo pipefail

runner_user=cassy-actions
runner_root=/var/lib/cassy-actions
runner_slot="${RUNNER_SLOT:-1}"
runner_version=2.336.0
runner_archive="actions-runner-linux-x64-${runner_version}.tar.gz"
runner_url_archive="https://github.com/actions/runner/releases/download/v${runner_version}/${runner_archive}"
runner_sha256=04cf0be1aff4c3ec3554466c39124ca250e3effd8873bb7e8d68535aa9505d5d
runner_group=cassy-public-trusted
runner_labels=cas-ci-32core,trusted-branches
registration="${RUNNER_REGISTRATION:-org}"
light=false
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/cassy-actions-runner-registration.sh
source "$repo_root/scripts/cassy-actions-runner-registration.sh"
pruner_source="$repo_root/scripts/prune-cassy-actions-cache.sh"
pruner_dest=/usr/local/sbin/cassy-actions-cache-prune
mount_guard_source="$repo_root/scripts/check-cassy-actions-cache-mount.sh"
mount_guard_dest="$runner_root/check-cache-mount.sh"
job_lock_source="$repo_root/scripts/cassy-actions-cache-job-lock.sh"
job_lock_dest="$runner_root/cache-job-lock.sh"
process_identity_source="$repo_root/scripts/cassy-actions-process-identity.sh"
process_identity_dest="$runner_root/cassy-actions-process-identity.sh"
job_started_dest="$runner_root/cache-job-started.sh"
job_completed_dest="$runner_root/cache-job-completed.sh"
job_lock_state="$runner_root/job-locks"
prune_service_source="$repo_root/ops/systemd/cassy-actions-cache-prune.service"
prune_timer_source="$repo_root/ops/systemd/cassy-actions-cache-prune.timer"

case "$runner_slot" in
    1)
        runner_name=soundwave-cas-ci
        runner_dir="$runner_root/runner"
        cargo_target_dir="$runner_root/cache/cargo-target"
        sccache_dir="$runner_root/cache/sccache"
        service_name=cassy-actions-runner.service
        wrapper_source="$repo_root/ops/systemd/run-cassy-actions-runner.sh"
        wrapper_dest="$runner_root/run-service.sh"
        ;;
    2)
        runner_name=soundwave-cas-ci-2
        runner_dir="$runner_root/runner-2"
        cargo_target_dir="$runner_root/cache/cargo-target-2"
        sccache_dir="$runner_root/cache/sccache-2"
        service_name=cassy-actions-runner-2.service
        wrapper_source="$repo_root/ops/systemd/run-cassy-actions-runner-2.sh"
        wrapper_dest="$runner_root/run-service-2.sh"
        ;;
    3|4)
        # Never cas-ci-32core: merge-queue and release Rust jobs stay off.
        runner_name="soundwave-cas-lane-$runner_slot"
        runner_dir="$runner_root/runner-$runner_slot"
        service_name="cassy-actions-runner-$runner_slot.service"
        runner_labels=cas-lane-light,trusted-branches
        light=true
        ;;
    *)
        echo "RUNNER_SLOT must be 1, 2, 3 or 4; got $runner_slot" >&2
        exit 1
        ;;
esac
case "$registration" in
    org) runner_url=https://github.com/Richards-LLC ;;
    repo) runner_url=https://github.com/Richards-LLC/cassy ;;
    *) echo "RUNNER_REGISTRATION must be org or repo; got $registration" >&2; exit 1 ;;
esac
group_args=()
set_runner_group_args "$registration" "$runner_group" group_args
unit_source="$repo_root/ops/systemd/$service_name"

if [[ "$(id -u)" -ne 0 ]]; then
    echo "run as root (sudo --preserve-env=RUNNER_TOKEN $0)" >&2
    exit 1
fi
if [[ -z "${RUNNER_TOKEN:-}" ]]; then
    echo "RUNNER_TOKEN must contain a short-lived Richards-LLC organization or repository runner registration token" >&2
    exit 1
fi
if "$light"; then
    if [[ ! -f "$unit_source" ]]; then
        echo "light runner unit $unit_source is missing" >&2
        exit 1
    fi
    if ! id "$runner_user" >/dev/null 2>&1; then
        echo "install slot 1 first: the $runner_user service account is missing" >&2
        exit 1
    fi
    install -d -o "$runner_user" -g "$runner_user" -m 0750 "$runner_dir"
    tmp_dir="$(mktemp -d)"
    chmod 0755 "$tmp_dir"
    trap 'rm -rf "$tmp_dir"' EXIT
    curl --fail --location --proto '=https' --tlsv1.2 "$runner_url_archive" -o "$tmp_dir/$runner_archive"
    printf '%s  %s\n' "$runner_sha256" "$tmp_dir/$runner_archive" | sha256sum --check --strict
    tar -xzf "$tmp_dir/$runner_archive" -C "$runner_dir"
    chown -R "$runner_user:$runner_user" "$runner_dir"
    (
        cd "$runner_dir"
        sudo -u "$runner_user" env HOME="$runner_root" \
            ./config.sh --unattended --replace \
            --url "$runner_url" \
            --token "$RUNNER_TOKEN" \
            --name "$runner_name" \
            "${group_args[@]}" \
            --labels "$runner_labels" \
            --work _work
    )
    install -o root -g root -m 0644 "$unit_source" "/etc/systemd/system/$service_name"
    systemctl daemon-reload
    systemctl enable --now "$service_name"
    systemctl --no-pager --full status "$service_name"
    exit 0
fi
if [[ ! -f "$unit_source" || ! -f "$wrapper_source" || ! -x "$pruner_source" ||
      ! -x "$mount_guard_source" || ! -x "$job_lock_source" || ! -f "$process_identity_source" ||
      ! -f "$prune_service_source" ||
      ! -f "$prune_timer_source" ]]; then
    echo "runner service, cache guard, or pruning files are missing" >&2
    exit 1
fi

if ! id "$runner_user" >/dev/null 2>&1; then
    useradd --system --create-home --home-dir "$runner_root" --shell /usr/sbin/nologin "$runner_user"
fi
install -d -o "$runner_user" -g "$runner_user" -m 0750 \
    "$runner_dir" "$runner_root/cache" "$cargo_target_dir" \
    "$sccache_dir" "$runner_root/.cargo" \
    "$runner_root/.cargo/bin" "$runner_root/.rustup" "$job_lock_state"
chown -R "$runner_user:$runner_user" \
    "$runner_root/cache" "$runner_root/.cargo" "$runner_root/.rustup"
if ! mountpoint -q "$runner_root/cache"; then
    echo "$runner_root/cache must be the dedicated mounted cache volume" >&2
    exit 1
fi
"$mount_guard_source"

tmp_dir="$(mktemp -d)"
chmod 0755 "$tmp_dir"
trap 'rm -rf "$tmp_dir"' EXIT
curl --fail --location --proto '=https' --tlsv1.2 "$runner_url_archive" -o "$tmp_dir/$runner_archive"
printf '%s  %s\n' "$runner_sha256" "$tmp_dir/$runner_archive" | sha256sum --check --strict
tar -xzf "$tmp_dir/$runner_archive" -C "$runner_dir"
chown -R "$runner_user:$runner_user" "$runner_dir"

# The host already carries the distro dependencies used by CI. Install Rust in
# the isolated service account rather than exposing the operator's home.
if [[ ! -x "$runner_root/.cargo/bin/rustup" ]]; then
    curl --fail --location --proto '=https' --tlsv1.2 https://sh.rustup.rs -o "$tmp_dir/rustup-init.sh"
    chmod 0755 "$tmp_dir/rustup-init.sh"
    sudo -u "$runner_user" env HOME="$runner_root" CARGO_HOME="$runner_root/.cargo" \
        RUSTUP_HOME="$runner_root/.rustup" \
        "$tmp_dir/rustup-init.sh" -y --profile minimal --default-toolchain stable --no-modify-path
fi
sudo -u "$runner_user" env HOME="$runner_root" CARGO_HOME="$runner_root/.cargo" \
    RUSTUP_HOME="$runner_root/.rustup" \
    "$runner_root/.cargo/bin/rustup" toolchain install stable --profile minimal
sudo -u "$runner_user" env HOME="$runner_root" CARGO_HOME="$runner_root/.cargo" \
    RUSTUP_HOME="$runner_root/.rustup" \
    "$runner_root/.cargo/bin/rustup" default stable
sccache_source="${SCCACHE_SOURCE:-$(command -v sccache 2>/dev/null || true)}"
if [[ -n "$sccache_source" && -x "$sccache_source" ]]; then
    install -o "$runner_user" -g "$runner_user" -m 0755 \
        "$sccache_source" "$runner_root/.cargo/bin/sccache"
else
    echo "sccache is required; set SCCACHE_SOURCE to its absolute executable path" >&2
    exit 1
fi

(
    cd "$runner_dir"
    sudo -u "$runner_user" env HOME="$runner_root" \
        PATH="$runner_root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
        ./config.sh --unattended --replace \
        --url "$runner_url" \
        --token "$RUNNER_TOKEN" \
        --name "$runner_name" \
        "${group_args[@]}" \
        --labels "$runner_labels" \
        --work _work
)

install -o "$runner_user" -g "$runner_user" -m 0755 \
    "$wrapper_source" "$wrapper_dest"
install -o root -g root -m 0755 "$mount_guard_source" "$mount_guard_dest"
install -o root -g root -m 0644 "$process_identity_source" "$process_identity_dest"
install -o root -g root -m 0755 "$job_lock_source" "$job_lock_dest"
ln -sfn "$(basename -- "$job_lock_dest")" "$job_started_dest"
ln -sfn "$(basename -- "$job_lock_dest")" "$job_completed_dest"
install -o root -g root -m 0755 "$pruner_source" "$pruner_dest"
install -o root -g root -m 0644 "$prune_service_source" \
    /etc/systemd/system/cassy-actions-cache-prune.service
install -o root -g root -m 0644 "$prune_timer_source" \
    /etc/systemd/system/cassy-actions-cache-prune.timer
install -o root -g root -m 0644 "$unit_source" "/etc/systemd/system/$service_name"
systemctl daemon-reload
systemctl enable --now cassy-actions-cache-prune.timer
systemctl enable --now "$service_name"
systemctl --no-pager --full status "$service_name"
