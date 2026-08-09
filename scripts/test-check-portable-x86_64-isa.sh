#!/usr/bin/env bash
# Deterministic self-test for scripts/check-portable-x86_64-isa.sh.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$script_dir/check-portable-x86_64-isa.sh"
dependency_guard="$script_dir/check-portable-x86_64-dependencies.sh"
blake3_build_guard="$script_dir/check-blake3-no-avx512-build.sh"
compiler="${CC:-cc}"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

if [[ "$#" -gt 1 ]]; then
  echo "Usage: scripts/test-check-portable-x86_64-isa.sh [final-packaged-cas]" >&2
  exit 2
fi

cat >"$tmpdir/baseline.S" <<'EOF'
.text
.globl main
.type main, @function
main:
  xor %eax, %eax
  ret
EOF

cat >"$tmpdir/seeded-avx512.S" <<'EOF'
.text
.globl main
.type main, @function
main:
  # EVEX-encoded vcvttsd2usi %xmm0,%rcx, matching the incident instruction.
  .byte 0x62, 0xf1, 0xff, 0x08, 0x78, 0xc8
  xor %eax, %eax
  ret
EOF

"$compiler" "$tmpdir/baseline.S" -o "$tmpdir/baseline"
"$compiler" "$tmpdir/seeded-avx512.S" -o "$tmpdir/seeded-avx512"

baseline_output="$($guard "$tmpdir/baseline")"
grep -qF 'evex_avx512=absent' <<<"$baseline_output"
echo 'ok   baseline x86_64 artifact accepted'

set +e
seeded_output="$($guard "$tmpdir/seeded-avx512" 2>&1)"
seeded_status=$?
set -e
if [[ "$seeded_status" -ne 1 ]]; then
  echo "FAIL seeded AVX-512 artifact: expected exit 1, got $seeded_status" >&2
  echo "$seeded_output" >&2
  exit 1
fi
grep -qF 'forbidden EVEX/AVX-512' <<<"$seeded_output"
grep -qi 'vcvttsd2usi' <<<"$seeded_output"
echo 'ok   seeded AVX-512 artifact rejected'

set +e
wrong_arch_output="$($guard "$tmpdir/baseline.S" 2>&1)"
wrong_arch_status=$?
set -e
if [[ "$wrong_arch_status" -ne 2 ]]; then
  echo "FAIL non-ELF input: expected exit 2, got $wrong_arch_status" >&2
  echo "$wrong_arch_output" >&2
  exit 1
fi
grep -qF 'requires an x86_64 ELF object or archive' <<<"$wrong_arch_output"
echo 'ok   missing or wrong artifact fails closed'

cat >"$tmpdir/ring-tree.txt" <<'EOF'
cas v2.55.1
├── rustls feature "ring"
├── blake3 feature "no_avx512"
└── blake3 v1.8.6 (/repo/vendor/blake3-1.8.6)
EOF

dependency_output="$($dependency_guard --tree-file "$tmpdir/ring-tree.txt")"
grep -qF 'rustls_provider=ring aws_lc=absent blake3_patch=1.8.6 blake3_avx512=disabled' <<<"$dependency_output"
echo 'ok   ring-only release dependency graph accepted'

cat >"$tmpdir/aws-lc-tree.txt" <<'EOF'
cas v2.55.1
├── rustls feature "ring"
├── blake3 feature "no_avx512"
├── blake3 v1.8.6 (/repo/vendor/blake3-1.8.6)
└── aws-lc-sys v0.39.1
EOF

set +e
aws_lc_output="$($dependency_guard --tree-file "$tmpdir/aws-lc-tree.txt" 2>&1)"
aws_lc_status=$?
set -e
if [[ "$aws_lc_status" -ne 1 ]]; then
  echo "FAIL AWS-LC dependency graph: expected exit 1, got $aws_lc_status" >&2
  echo "$aws_lc_output" >&2
  exit 1
fi
grep -qF 'includes forbidden AWS-LC provider code' <<<"$aws_lc_output"
echo 'ok   seeded AWS-LC release dependency graph rejected'

cat >"$tmpdir/blake3-avx512-tree.txt" <<'EOF'
cas v2.55.1
├── rustls feature "ring"
└── blake3 v1.8.6 (/repo/vendor/blake3-1.8.6)
EOF

set +e
blake3_output="$($dependency_guard --tree-file "$tmpdir/blake3-avx512-tree.txt" 2>&1)"
blake3_status=$?
set -e
if [[ "$blake3_status" -ne 1 ]]; then
  echo "FAIL BLAKE3 AVX-512 dependency graph: expected exit 1, got $blake3_status" >&2
  echo "$blake3_output" >&2
  exit 1
fi
grep -qF 'does not disable BLAKE3 AVX-512 dispatch' <<<"$blake3_output"
echo 'ok   seeded BLAKE3 AVX-512 release feature graph rejected'

live_dependency_output="$($dependency_guard)"
grep -qF 'rustls_provider=ring aws_lc=absent blake3_patch=1.8.6 blake3_avx512=disabled' <<<"$live_dependency_output"
echo 'ok   locked Linux release dependency graph is ring-only and disables BLAKE3 AVX-512'

mkdir -p "$tmpdir/blake3-good/blake3-good/out"
printf '%s\n' \
  'cargo::rerun-if-env-changed=CARGO_FEATURE_NO_AVX512' \
  'cargo::rustc-cfg=blake3_sse2_ffi' \
  'cargo::rustc-cfg=blake3_sse41_ffi' \
  'cargo::rustc-cfg=blake3_avx2_ffi' \
  >"$tmpdir/blake3-good/blake3-good/output"
blake3_build_output="$($blake3_build_guard "$tmpdir/blake3-good")"
grep -qF 'avx512_inputs=absent' <<<"$blake3_build_output"
echo 'ok   BLAKE3 build without AVX-512 inputs accepted'

mkdir -p "$tmpdir/blake3-bad/blake3-bad/out"
printf '%s\n' \
  'cargo::rerun-if-env-changed=CARGO_FEATURE_NO_AVX512' \
  'cargo::rustc-cfg=blake3_avx512_ffi' \
  'cargo::rustc-link-lib=static=blake3_avx512_assembly' \
  >"$tmpdir/blake3-bad/blake3-bad/output"
touch "$tmpdir/blake3-bad/blake3-bad/out/libblake3_avx512_assembly.a"
set +e
blake3_bad_output="$($blake3_build_guard "$tmpdir/blake3-bad" 2>&1)"
blake3_bad_status=$?
set -e
if [[ "$blake3_bad_status" -ne 1 ]]; then
  echo "FAIL BLAKE3 AVX-512 build inputs: expected exit 1, got $blake3_bad_status" >&2
  echo "$blake3_bad_output" >&2
  exit 1
fi
grep -qF 'enables forbidden AVX-512 inputs' <<<"$blake3_bad_output"
echo 'ok   seeded BLAKE3 AVX-512 build inputs rejected'

if [[ "$#" -eq 1 ]]; then
  final_output="$($guard "$1")"
  grep -qF 'evex_avx512=absent' <<<"$final_output"
  echo "ok   exact final packaged executable accepted: $1"
fi

echo 'PASS: portable x86_64 ISA audit behavior verified.'
