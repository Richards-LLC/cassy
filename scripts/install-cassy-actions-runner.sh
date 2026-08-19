#!/usr/bin/env bash
# Install the pinned GitHub runner and register it in the pre-created,
# selected-repository/selected-workflow group. Run from a trusted checkout:
#   SCCACHE_SOURCE="$(command -v sccache)" \
#     sudo --preserve-env=RUNNER_TOKEN,SCCACHE_SOURCE scripts/install-cassy-actions-runner.sh
set -euo pipefail

runner_user=cassy-actions
runner_root=/var/lib/cassy-actions
runner_dir="$runner_root/runner"
runner_version=2.336.0
runner_archive="actions-runner-linux-x64-${runner_version}.tar.gz"
runner_url="https://github.com/actions/runner/releases/download/v${runner_version}/${runner_archive}"
runner_sha256=04cf0be1aff4c3ec3554466c39124ca250e3effd8873bb7e8d68535aa9505d5d
runner_group=cassy-public-trusted
unit_source="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/ops/systemd/cassy-actions-runner.service"
wrapper_source="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/ops/systemd/run-cassy-actions-runner.sh"

if [[ "$(id -u)" -ne 0 ]]; then
    echo "run as root (sudo --preserve-env=RUNNER_TOKEN $0)" >&2
    exit 1
fi
if [[ -z "${RUNNER_TOKEN:-}" ]]; then
    echo "RUNNER_TOKEN must contain a short-lived Richards-LLC organization runner registration token" >&2
    exit 1
fi
if [[ ! -f "$unit_source" || ! -f "$wrapper_source" ]]; then
    echo "systemd service files not found: $unit_source / $wrapper_source" >&2
    exit 1
fi

if ! id "$runner_user" >/dev/null 2>&1; then
    useradd --system --create-home --home-dir "$runner_root" --shell /usr/sbin/nologin "$runner_user"
fi
install -d -o "$runner_user" -g "$runner_user" -m 0750 \
    "$runner_dir" "$runner_root/cache" "$runner_root/cache/cargo-target" \
    "$runner_root/cache/sccache" "$runner_root/.cargo" \
    "$runner_root/.cargo/bin" "$runner_root/.rustup"
chown -R "$runner_user:$runner_user" \
    "$runner_root/cache" "$runner_root/.cargo" "$runner_root/.rustup"

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

sudo -u "$runner_user" env HOME="$runner_root" \
    "$runner_dir/config.sh" --unattended --replace \
    --url https://github.com/Richards-LLC \
    --token "$RUNNER_TOKEN" \
    --name soundwave-cas-ci \
    --runnergroup "$runner_group" \
    --labels cas-ci-32core,trusted-branches \
    --work _work

install -o "$runner_user" -g "$runner_user" -m 0755 \
    "$wrapper_source" "$runner_root/run-service.sh"
install -o root -g root -m 0644 "$unit_source" /etc/systemd/system/cassy-actions-runner.service
systemctl daemon-reload
systemctl enable --now cassy-actions-runner.service
systemctl --no-pager --full status cassy-actions-runner.service
