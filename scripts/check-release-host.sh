#!/usr/bin/env bash
# Fail early when a local release host cannot build one of its requested targets.

set -euo pipefail

usage() {
    echo "Usage: scripts/check-release-host.sh <host-os> <target> [...]" >&2
}

if [[ "$#" -lt 2 ]]; then
    usage
    exit 2
fi

host_os="$1"
shift

for target in "$@"; do
    case "$target" in
        x86_64-unknown-linux-gnu)
            # cargo-zigbuild cross-compiles this target from supported hosts.
            ;;
        aarch64-apple-darwin)
            if [[ "$host_os" != "Darwin" ]]; then
                echo "error: $target requires a macOS host for the native release build; run the macOS release job or a Mac checkout" >&2
                exit 1
            fi
            ;;
        *)
            echo "error: unsupported local release target: $target" >&2
            exit 2
            ;;
    esac
done

echo "release host preflight passed: host=$host_os targets=$*"
