#!/usr/bin/env bash
# Build the real previous hub releases for the isolated update recovery matrix.
set -euo pipefail

root=$(git rev-parse --show-toplevel)
source_root="$root/target/old-hub"
build_root="$root/target/old-hub-build/v3.27.8"
binary_root="$root/target/old-hub-binaries"
mkdir -p "$source_root" "$binary_root"

for tag in v3.27.8 v3.28.1 v3.28.2; do
    source="$source_root/$tag"
    binary="$binary_root/$tag/cas"
    if [[ -x "$binary" ]] && "$binary" --version | grep -q "cas ${tag#v} "; then
        continue
    fi
    if [[ ! -d "$source/.git" && ! -f "$source/.git" ]]; then
        git worktree add --detach "$source" "$tag"
    fi
    CARGO_TARGET_DIR="$build_root" cargo build --manifest-path "$source/Cargo.toml" -p cas --bin cas
    mkdir -p "$(dirname "$binary")"
    cp "$build_root/debug/cas" "$binary"
    "$binary" --version
done
