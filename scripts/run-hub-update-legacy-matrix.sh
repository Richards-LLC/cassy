#!/usr/bin/env bash
# Build tagged hub processes, then run the new post-swap recovery matrix.
set -euo pipefail

root=$(git rev-parse --show-toplevel)
"$root/scripts/build-hub-legacy-binaries.sh"
cargo build --manifest-path "$root/Cargo.toml" -p cas --bin cas
export CAS_TEST_OLD_HUB_BIN_DIR="$root/target/old-hub-binaries"
"$root/scripts/run-scoped-tests.sh" -p cas --test hub_clean_home_test \
    -E 'test(real_legacy_hubs_recover_through_new_post_swap_step)' --run-ignored all
