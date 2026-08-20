#!/usr/bin/env bash
# Verify that a trusted self-hosted lane received one complete, isolated slot
# tuple. Accepting the target, cache, and port independently could combine
# slot 1 and slot 2 and silently reintroduce a shared Cargo lock or cache
# server.
set -euo pipefail

fail() {
    echo "error: $1" >&2
    exit 1
}

case "${CARGO_TARGET_DIR:?}:${SCCACHE_DIR:?}:${SCCACHE_SERVER_PORT:?}" in
    /var/lib/cassy-actions/cache/cargo-target:/var/lib/cassy-actions/cache/sccache:4227)
        slot=1
        ;;
    /var/lib/cassy-actions/cache/cargo-target-2:/var/lib/cassy-actions/cache/sccache-2:4228)
        slot=2
        ;;
    *)
        fail "runner target/cache/port is not an approved isolated slot tuple"
        ;;
esac

test "$(id -u)" -ne 0 || fail "trusted lane must not run as root"
echo "runner isolation contract satisfied: slot=$slot target=$CARGO_TARGET_DIR"
