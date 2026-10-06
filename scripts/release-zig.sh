#!/usr/bin/env bash
# Shared publisher/probe Zig discovery. Source this file; call from any cwd.
release_zig_environment() {
    local root="$1" zig="${ZIG:-}"
    if [[ -x "$root/.context/zig/zig" ]]; then
        zig="$root/.context/zig/zig"
    elif [[ -z "$zig" || ! -x "$zig" ]]; then
        (cd "$root" && ./scripts/bootstrap-zig.sh) || return $?
        zig="$root/.context/zig/zig"
    fi
    [[ -x "$zig" ]] || { printf 'error: release Zig is unavailable: %s\n' "$zig" >&2; return 1; }
    # zigbuild discovers `zig` through PATH, whereas other release checks use ZIG.
    ZIG="$(cd "$(dirname "$zig")" && pwd)/$(basename "$zig")"
    export ZIG
    export PATH="$(dirname "$ZIG"):$PATH"
    printf 'Zig: %s\n' "$("$ZIG" version)"
}
