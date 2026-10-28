#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
verify_script="$script_dir/verify-hub-dist.sh"
scratch_dir=$(mktemp -d "${TMPDIR:-/tmp}/verify-dist.XXXXXX")
trap 'rm -rf "$scratch_dir"' EXIT

origin_index="$scratch_dir/origin-index.html"
printf '<!doctype html><title>fixture</title>\n' > "$origin_index"

prepare_fixture() {
  local fixture_dir=$1
  mkdir -p "$fixture_dir"
  cp "$origin_index" "$fixture_dir/index.html"
}

clean_fixture="$scratch_dir/clean"
prepare_fixture "$clean_fixture"
if DIST_DIR="$clean_fixture" ORIGIN_INDEX="$origin_index" "$verify_script" > "$scratch_dir/clean.log" 2>&1; then
  printf 'PASS: dist without removed assets is accepted\n'
else
  cat "$scratch_dir/clean.log" >&2
  printf 'FAIL: clean dist was rejected\n' >&2
  exit 1
fi

for stale_asset in ghostty-vt.wasm ghostty-write-pty.wasm symbols.woff2; do
  stale_fixture="$scratch_dir/stale-${stale_asset}"
  prepare_fixture "$stale_fixture"
  : > "$stale_fixture/$stale_asset"

  if DIST_DIR="$stale_fixture" ORIGIN_INDEX="$origin_index" "$verify_script" > "$scratch_dir/stale.log" 2>&1; then
    printf 'FAIL: stale dist containing %s was accepted\n' "$stale_asset" >&2
    exit 1
  fi
  if ! grep -Fq 'error: stale dist' "$scratch_dir/stale.log" || ! grep -Fq "$stale_asset" "$scratch_dir/stale.log"; then
    cat "$scratch_dir/stale.log" >&2
    printf 'FAIL: stale dist rejection did not identify %s\n' "$stale_asset" >&2
    exit 1
  fi
  printf 'PASS: stale dist containing %s is rejected\n' "$stale_asset"
done

# The root entry must remain a byte copy; a successful digest alone is insufficient.
printf 'different root\n' > "$origin_index"
if DIST_DIR="$clean_fixture" ORIGIN_INDEX="$origin_index" "$verify_script" > "$scratch_dir/root.log" 2>&1; then
  printf 'FAIL: divergent root index was accepted\n' >&2
  exit 1
fi
grep -Fq 'root index must exactly match' "$scratch_dir/root.log"
printf 'PASS: divergent root index is rejected\n'
