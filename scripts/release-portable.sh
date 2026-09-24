#!/usr/bin/env bash
# shellcheck disable=SC2034,SC2329 # RELEASE_PORTABLE_* are outputs read by callers; sha256sum is defined for callers.
# Portable host helpers for the release train, gate and ISA audit (cas-fed5).
#
# The train was written on Linux. Cutting v3.29.0 from macOS needed GNU
# coreutils first on PATH (stat -c, sha256sum), a hand-made setsid shim, a
# manual ~/.cargo/bin PATH entry and Homebrew's GNU objdump. Each helper here
# keeps the Linux behaviour exactly (the GNU tool is tried first) and adds the
# stock-macOS equivalent, so no manual shims are needed.
#
# Source this file; it defines functions only and never exits.
#
# Test seams (the self-test uses them to exercise the fallbacks on Linux):
#   CAS_RELEASE_PORTABLE_STAT      stat binary (default: stat)
#   CAS_RELEASE_PORTABLE_SETSID    setsid binary (default: setsid)
#   CAS_RELEASE_PORTABLE_SHA256SUM sha256sum binary (default: sha256sum)

# Device number of a path: GNU `stat -c %d`, then BSD/macOS `stat -f %d`.
# Prints nothing and returns 1 when neither works. (GNU `stat -f` means
# "file system status", so the GNU form must be tried first.)
release_portable_stat_device() {
    local path="$1" stat_bin="${CAS_RELEASE_PORTABLE_STAT:-stat}" device
    device="$("$stat_bin" -c %d "$path" 2>/dev/null)" && [[ "$device" =~ ^[0-9]+$ ]] && {
        printf '%s\n' "$device"
        return 0
    }
    device="$("$stat_bin" -f %d "$path" 2>/dev/null)" && [[ "$device" =~ ^[0-9]+$ ]] && {
        printf '%s\n' "$device"
        return 0
    }
    return 1
}

# Sets RELEASE_PORTABLE_SETSID to the command prefix that runs its arguments
# in a new session (so the recorded pid is also the process-group id that
# `--stop` signals): util-linux setsid, else Perl's POSIX::setsid (stock on
# macOS), else Python's os.setsid. Returns 1 and names what is missing when
# none is available.
release_portable_setsid_prefix() {
    local setsid_bin="${CAS_RELEASE_PORTABLE_SETSID:-setsid}"
    if command -v "$setsid_bin" >/dev/null 2>&1; then
        RELEASE_PORTABLE_SETSID=("$setsid_bin")
    elif command -v perl >/dev/null 2>&1; then
        # shellcheck disable=SC2016 # Perl code, expanded by Perl.
        RELEASE_PORTABLE_SETSID=(perl -MPOSIX=setsid -e
            'defined(setsid()) or die "setsid: $!\n"; exec { $ARGV[0] } @ARGV or die "exec $ARGV[0]: $!\n"')
    elif command -v python3 >/dev/null 2>&1; then
        RELEASE_PORTABLE_SETSID=(python3 -c
            'import os, sys; os.setsid(); os.execvp(sys.argv[1], sys.argv[1:])')
    else
        printf 'no way to start a new session: install util-linux setsid, or make perl or python3 available\n' >&2
        return 1
    fi
}

# SHA-256 of files or stdin in `sha256sum` format ("<hex>  <name>"):
# GNU sha256sum, else `shasum -a 256` (stock on macOS).
release_portable_sha256sum() {
    local sha_bin="${CAS_RELEASE_PORTABLE_SHA256SUM:-sha256sum}"
    if command -v "$sha_bin" >/dev/null 2>&1; then
        "$sha_bin" "$@"
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$@"
    else
        printf 'no SHA-256 tool: install coreutils sha256sum or perl shasum\n' >&2
        return 127
    fi
}

# Scripts written against GNU names call `sha256sum` directly. Where the
# binary is absent, define a function of that name so those call sites keep
# working unchanged; where it exists this does nothing.
release_portable_define_sha256sum() {
    if ! command -v sha256sum >/dev/null 2>&1; then
        sha256sum() { release_portable_sha256sum "$@"; }
    fi
}

# Append the Cargo bin directory to PATH when it exists and is missing, so a
# train started from a non-login shell (an agent, launchd, cron) still finds
# cargo, cargo-nextest and cargo-zigbuild. Appended, never prepended: a PATH
# that already resolves these tools resolves them exactly as before.
release_portable_path_add_cargo_bin() {
    local cargo_bin_dir="${CARGO_HOME:-${HOME:-}/.cargo}/bin"
    [[ -d "$cargo_bin_dir" ]] || return 0
    case ":$PATH:" in
        *":$cargo_bin_dir:"*) ;;
        *) PATH="${PATH:+$PATH:}$cargo_bin_dir"; export PATH ;;
    esac
}

# Path of a GNU objdump (the ISA audit parses its output format). macOS's
# objdump is LLVM's; Homebrew's binutils installs GNU objdump as `gobjdump`
# or keg-only under opt/binutils. Returns 1 when none is found.
release_portable_gnu_objdump() {
    local candidate prefix
    local -a candidates=("${OBJDUMP:-}" objdump gobjdump x86_64-linux-gnu-objdump)
    for prefix in /opt/homebrew/opt/binutils /usr/local/opt/binutils; do
        candidates+=("$prefix/bin/objdump" "$prefix/bin/gobjdump")
    done
    for candidate in "${candidates[@]}"; do
        [[ -n "$candidate" ]] || continue
        command -v "$candidate" >/dev/null 2>&1 || continue
        if "$candidate" --version 2>/dev/null | head -n1 | grep -q 'GNU objdump'; then
            command -v "$candidate"
            return 0
        fi
    done
    return 1
}

# Sets RELEASE_PORTABLE_X86_64_CC to a compiler command that produces an
# x86_64 Linux ELF (the ISA self-test builds two tiny fixtures): $CC when
# set, the native cc on an x86_64 Linux host, else `zig cc -target
# x86_64-linux-gnu` (ZIG, then zig on PATH). Returns 1 when none exists.
release_portable_x86_64_linux_cc() {
    local zig_bin
    if [[ -n "${CC:-}" ]]; then
        # shellcheck disable=SC2206 # CC may carry flags, as make allows.
        RELEASE_PORTABLE_X86_64_CC=($CC)
        return 0
    fi
    if [[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] && command -v cc >/dev/null 2>&1; then
        RELEASE_PORTABLE_X86_64_CC=(cc)
        return 0
    fi
    zig_bin="${ZIG:-}"
    [[ -n "$zig_bin" && -x "$zig_bin" ]] || zig_bin="$(command -v zig 2>/dev/null || true)"
    if [[ -n "$zig_bin" ]]; then
        RELEASE_PORTABLE_X86_64_CC=("$zig_bin" cc -target x86_64-linux-gnu)
        return 0
    fi
    return 1
}

# Default scratch base for the gate's out-of-checkout rows. Linux keeps
# /var/tmp/cas-release-gate. macOS uses /Users/Shared/cas-release-gate: /tmp
# and /var/tmp are Cassy disposable roots there (a cas child whose cwd or
# TMPDIR sits under them is treated as a throwaway copy), and /Users/Shared is
# on the same APFS data volume as checkouts under /Users with no .cas
# ancestor, which the gate's device and ancestry checks require.
release_portable_default_scratch_base() {
    if [[ "$(uname -s)" == Darwin ]]; then
        printf '/Users/Shared/cas-release-gate\n'
    else
        printf '/var/tmp/cas-release-gate\n'
    fi
}

