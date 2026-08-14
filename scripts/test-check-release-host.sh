#!/usr/bin/env bash
# Self-test for scripts/check-release-host.sh without building release artifacts.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$script_dir/check-release-host.sh"

linux_output="$($guard Linux x86_64-unknown-linux-gnu)"
grep -qF 'host=Linux targets=x86_64-unknown-linux-gnu' <<<"$linux_output"
echo 'ok   Linux host accepts the Zig-built Linux artifact'

darwin_output="$($guard Darwin aarch64-apple-darwin x86_64-unknown-linux-gnu)"
grep -qF 'host=Darwin targets=aarch64-apple-darwin x86_64-unknown-linux-gnu' <<<"$darwin_output"
echo 'ok   macOS host accepts native Darwin plus Zig-built Linux artifacts'

set +e
wrong_host_output="$($guard Linux aarch64-apple-darwin 2>&1)"
wrong_host_status=$?
set -e
if [[ "$wrong_host_status" -ne 1 ]]; then
    echo "FAIL non-macOS Darwin target: expected exit 1, got $wrong_host_status" >&2
    echo "$wrong_host_output" >&2
    exit 1
fi
grep -qF 'requires a macOS host for the native release build' <<<"$wrong_host_output"
echo 'ok   non-macOS Darwin request fails before build'

echo 'PASS: release host preflight behavior verified.'
