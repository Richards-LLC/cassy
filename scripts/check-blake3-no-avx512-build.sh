#!/usr/bin/env bash
# Reject a BLAKE3 release build that compiled AVX-512 inputs despite the
# locked no_avx512 feature. The final executable ISA scan remains authoritative.

set -euo pipefail

usage() {
  echo "Usage: scripts/check-blake3-no-avx512-build.sh <release-build-directory>" >&2
}

if [[ "$#" -ne 1 || ! -d "$1" ]]; then
  usage
  exit 2
fi

build_dir="$1"
mapfile -t outputs < <(find "$build_dir" -mindepth 2 -maxdepth 2 -type f -path '*/blake3-*/output' -print)
if [[ "${#outputs[@]}" -ne 1 ]]; then
  echo "error: expected exactly one BLAKE3 build output under $build_dir; found ${#outputs[@]}" >&2
  exit 2
fi

blake3_dir="$(dirname "${outputs[0]}")"
if grep -Eq '^cargo::rustc-cfg=blake3_avx512_ffi$|^cargo::rustc-link-lib=static=blake3_avx512_(assembly|intrinsics)$' "${outputs[0]}"; then
  echo "error: BLAKE3 build output enables forbidden AVX-512 inputs: ${outputs[0]}" >&2
  grep -E '^cargo::rustc-cfg=blake3_avx512_ffi$|^cargo::rustc-link-lib=static=blake3_avx512_(assembly|intrinsics)$' "${outputs[0]}" >&2
  exit 1
fi

mapfile -t avx512_inputs < <(find "$blake3_dir/out" -maxdepth 1 -type f \
  \( -iname '*avx512*' -o -iname '*avx-512*' \) -print 2>/dev/null)
if [[ "${#avx512_inputs[@]}" -ne 0 ]]; then
  echo "error: BLAKE3 build produced forbidden AVX-512 archive/object inputs" >&2
  printf '%s\n' "${avx512_inputs[@]}" >&2
  exit 1
fi

if ! grep -qF 'cargo::rerun-if-env-changed=CARGO_FEATURE_NO_AVX512' "${outputs[0]}"; then
  echo "error: BLAKE3 build output does not prove the patched no_avx512 build decision ran" >&2
  exit 1
fi

echo "portable BLAKE3 build audit passed: avx512_inputs=absent output=${outputs[0]}"
