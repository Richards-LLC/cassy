#!/usr/bin/env bash
# Verify the staged Commander distribution before a Vercel preview or go-live.
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
dist_dir=${DIST_DIR:-"$script_dir/../hub-web/dist"}
origin_index=${ORIGIN_INDEX:-"$dist_dir/index.html"}

if [[ ! -d "$dist_dir" ]]; then
  printf 'error: dist directory not found: %s\n' "$dist_dir" >&2
  exit 1
fi

stale_assets=$(find "$dist_dir" \
  \( -name 'ghostty-vt.wasm' -o -name 'ghostty-write-pty.wasm' -o -name 'symbols.woff2' \) \
  -print)
if [[ -n "$stale_assets" ]]; then
  printf 'error: stale dist contains removed Commander assets:\n%s\n' "$stale_assets" >&2
  exit 1
fi

if ! cmp --silent "$dist_dir/index.html" "$origin_index"; then
  printf 'error: root index must exactly match the pinned Commander index: %s\n' "$origin_index" >&2
  exit 1
fi
printf 'verified root index matches Commander distribution\n'

dist_digest=$(
  cd "$dist_dir" &&
    find . -type f -print0 |
      LC_ALL=C sort -z |
      while IFS= read -r -d '' path; do
        shasum -a 256 -- "${path#./}"
      done |
      shasum -a 256 |
      awk '{print $1}'
)
printf 'dist_digest %s\n' "$dist_digest"
