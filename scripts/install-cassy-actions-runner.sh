#!/usr/bin/env bash
# Install the pinned GitHub runner and register it in the pre-created,
# selected-repository/selected-workflow group. Run from a trusted checkout:
#   RUNNER_SLOT=2 SCCACHE_SOURCE="$(command -v sccache)" \
#     sudo --preserve-env=RUNNER_TOKEN,RUNNER_SLOT,SCCACHE_SOURCE \
#       scripts/install-cassy-actions-runner.sh
set -euo pipefail

runner_user=cassy-actions
runner_root=/var/lib/cassy-actions
runner_slot="${RUNNER_SLOT:-1}"
runner_version=2.336.0
runner_archive="actions-runner-linux-x64-${runner_version}.tar.gz"
runner_url="https://github.com/actions/runner/releases/download/v${runner_version}/${runner_archive}"
runner_sha256=04cf0be1aff4c3ec3554466c39124ca250e3effd8873bb7e8d68535aa9505d5d
runner_group=cassy-public-trusted
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
pruner_source="$repo_root/scripts/prune-cassy-actions-cache.sh"
pruner_dest="$runner_root/prune-cache.sh"
mount_guard_source="$repo_root/scripts/check-cassy-actions-cache-mount.sh"
mount_guard_dest="$runner_root/check-cache-mount.sh"

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
    *)
        echo "RUNNER_SLOT must be 1 or 2; got $runner_slot" >&2
        exit 1
        ;;
esac
unit_source="$repo_root/ops/systemd/$service_name"

if [[ "$(id -u)" -ne 0 ]]; then
    echo "run as root (sudo --preserve-env=RUNNER_TOKEN $0)" >&2
    exit 1
fi
if [[ -z "${RUNNER_TOKEN:-}" ]]; then
    echo "RUNNER_TOKEN must contain a short-lived Richards-LLC organization runner registration token" >&2
    exit 1
fi
if [[ ! -f "$unit_source" || ! -f "$wrapper_source" || ! -x "$pruner_source" || ! -x "$mount_guard_source" ]]; then
    echo "runner service/guard files not found: $unit_source / $wrapper_source / $pruner_source / $mount_guard_source" >&2
    exit 1
fi

if ! id "$runner_user" >/dev/null 2>&1; then
    useradd --system --create-home --home-dir "$runner_root" --shell /usr/sbin/nologin "$runner_user"
fi
install -d -o "$runner_user" -g "$runner_user" -m 0750 \
    "$runner_dir" "$runner_root/cache" "$cargo_target_dir" \
    "$sccache_dir" "$runner_root/.cargo" \
    "$runner_root/.cargo/bin" "$runner_root/.rustup"
chown -R "$runner_user:$runner_user" \
    "$runner_root/cache" "$runner_root/.cargo" "$runner_root/.rustup"
if ! mountpoint -q "$runner_root/cache"; then
    echo "$runner_root/cache must be a dedicated mounted cache volume; refusing root-filesystem fallback" >&2
    exit 1
fi

tmp_dir="$(mktemp -d)"
chmod 0755 "$tmp_dir"
trap 'rm -rf "$tmp_dir"' EXIT
curl --fail --location --proto '=https' --tlsv1.2 "$runner_url" -o "$tmp_dir/$runner_archive"
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
        --url https://github.com/Richards-LLC \
        --token "$RUNNER_TOKEN" \
        --name "$runner_name" \
        --runnergroup "$runner_group" \
        --labels cas-ci-32core,trusted-branches \
        --work _work
)

install -o "$runner_user" -g "$runner_user" -m 0755 \
    "$wrapper_source" "$wrapper_dest"
install -o root -g root -m 0755 "$pruner_source" "$pruner_dest"
install -o root -g root -m 0755 "$mount_guard_source" "$mount_guard_dest"
install -o root -g root -m 0644 "$unit_source" "/etc/systemd/system/$service_name"
systemctl daemon-reload
systemctl enable --now "$service_name"
systemctl --no-pager --full status "$service_name"
