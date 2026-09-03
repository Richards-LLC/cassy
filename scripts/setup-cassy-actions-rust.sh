#!/usr/bin/env bash
# Prepare the pre-provisioned Rust toolchain on a trusted self-hosted runner.
# The runner slots intentionally share RUSTUP_HOME, so any exceptional install
# is serialized instead of allowing rustup rollback to race another lane.
set -euo pipefail

rustup_bin="${RUSTUP:-rustup}"
rustup_home="${RUSTUP_HOME:?RUSTUP_HOME must point at the shared runner toolchain}"
lock_file="${CASSY_RUSTUP_LOCK_FILE:-$rustup_home/cassy-rustup.lock}"

if [[ "$rustup_bin" == */* ]]; then
    [[ -x "$rustup_bin" ]] || {
        echo "rustup executable is not available: $rustup_bin" >&2
        exit 1
    }
else
    command -v "$rustup_bin" >/dev/null || {
        echo "rustup executable is not available: $rustup_bin" >&2
        exit 1
    }
fi
command -v flock >/dev/null || {
    echo 'flock is required to protect the shared rustup home' >&2
    exit 1
}

mkdir -p "$rustup_home"
exec 9>"$lock_file"
flock -x 9

if "$rustup_bin" toolchain list | awk '$1 == "stable" || $1 ~ /^stable-/ { found = 1 } END { exit found ? 0 : 1 }'; then
    echo 'stable Rust toolchain is already installed; skipped rustup mutation'
else
    echo 'stable Rust toolchain is missing; installing under the shared rustup lock'
    "$rustup_bin" toolchain install stable --profile minimal
fi

# Use an explicit toolchain rather than changing rustup's shared default file.
export RUSTUP_TOOLCHAIN=stable
if [[ -n "${GITHUB_ENV:-}" ]]; then
    printf '%s\n' 'RUSTUP_TOOLCHAIN=stable' >>"$GITHUB_ENV"
fi
"$rustup_bin" run stable rustc --version
flock -u 9
