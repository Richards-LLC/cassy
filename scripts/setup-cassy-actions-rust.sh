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

incomplete_toolchain() {
    echo "::error::shared stable Rust toolchain is incomplete: $1. Stop jobs and repair stable only when all runner slots are idle (RUSTUP_HOME=$rustup_home)." >&2
    exit 1
}

toolchains="$("$rustup_bin" toolchain list)" || incomplete_toolchain 'cannot read the toolchain registry'
if printf '%s\n' "$toolchains" | awk '$1 == "stable" || $1 ~ /^stable-/ { found = 1 } END { exit found ? 0 : 1 }'; then
    echo 'stable Rust toolchain is already installed; skipped rustup mutation'
else
    echo 'stable Rust toolchain is missing; installing under the shared rustup lock'
    "$rustup_bin" toolchain install stable --profile minimal
fi

# A rustc binary alone does not prove that a raced rustup install left the
# registry and standard library intact. Do not repair an existing toolchain
# here: another slot may be compiling against it after releasing this lock.
rustc_info="$("$rustup_bin" run stable rustc -vV)" || incomplete_toolchain 'rustc cannot run'
host="$(printf '%s\n' "$rustc_info" | awk '$1 == "host:" { print $2 }')"
[[ -n "$host" ]] || incomplete_toolchain 'rustc did not report its host target'
installed_targets="$("$rustup_bin" target list --toolchain stable --installed)" || incomplete_toolchain 'cannot read installed targets'
printf '%s\n' "$installed_targets" | grep -Fxq -- "$host" || incomplete_toolchain "rust-std for $host is absent from the registry"
target_libdir="$("$rustup_bin" run stable rustc --print target-libdir)" || incomplete_toolchain 'cannot locate the host standard library'
std_libraries=("$target_libdir"/libstd-*.rlib)
[[ -f "${std_libraries[0]}" ]] || incomplete_toolchain "rust-std files for $host are missing"
"$rustup_bin" run stable cargo --version || incomplete_toolchain 'cargo cannot run'

# Use an explicit toolchain rather than changing rustup's shared default file.
export RUSTUP_TOOLCHAIN=stable
if [[ -n "${GITHUB_ENV:-}" ]]; then
    printf '%s\n' 'RUSTUP_TOOLCHAIN=stable' >>"$GITHUB_ENV"
fi
"$rustup_bin" run stable rustc --version
flock -u 9
