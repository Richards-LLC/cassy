#!/usr/bin/env bash
# Deterministic self-test for scripts/check-portable-x86_64-isa.sh.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$script_dir/check-portable-x86_64-isa.sh"
compiler="${CC:-cc}"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

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

echo 'PASS: portable x86_64 ISA audit behavior verified.'
