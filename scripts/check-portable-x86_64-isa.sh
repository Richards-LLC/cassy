#!/usr/bin/env bash
# Reject x86_64 release artifacts containing EVEX-encoded instructions.
#
# EVEX uses opcode-map prefix 0x62 and is the encoding used by AVX-512. CAS's
# distributed x86_64 baseline does not require AVX-512, so an EVEX instruction
# in executable code is a release-blocking portability defect.

set -euo pipefail

usage() {
  echo "Usage: scripts/check-portable-x86_64-isa.sh <x86_64-elf>" >&2
}

if [[ "$#" -ne 1 ]]; then
  usage
  exit 2
fi

artifact="$1"
if [[ ! -f "$artifact" ]]; then
  echo "error: ISA audit input is not a file: $artifact" >&2
  exit 2
fi

for tool in file objdump awk; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: required ISA audit tool is unavailable: $tool" >&2
    exit 2
  fi
done

file_description="$(file -Lb -- "$artifact")"
if [[ "$file_description" != *"ELF 64-bit"* || "$file_description" != *"x86-64"* ]]; then
  echo "error: ISA audit requires an x86_64 ELF artifact; got: $file_description" >&2
  exit 2
fi

disassembly="$(mktemp)"
offenders="$(mktemp)"
trap 'rm -f "$disassembly" "$offenders"' EXIT

if ! LC_ALL=C objdump -d -- "$artifact" >"$disassembly"; then
  echo "error: objdump could not disassemble ISA audit input: $artifact" >&2
  exit 2
fi

# GNU objdump prints one instruction as ADDRESS: BYTE... MNEMONIC. Walk only
# leading instruction bytes and allow legacy prefixes before 0x62; never scan
# immediates or data bytes, which would create false positives.
awk '
  /^[[:space:]]*[[:xdigit:]]+:/ {
    line = $0
    sub(/^[[:space:]]*[[:xdigit:]]+:[[:space:]]*/, "", line)
    count = split(line, field, /[[:space:]]+/)
    for (i = 1; i <= count && field[i] ~ /^[[:xdigit:]][[:xdigit:]]$/; i++) {
      byte = tolower(field[i])
      if (byte == "62") {
        print $0
        break
      }
      if (byte !~ /^(f0|f2|f3|2e|36|3e|26|64|65|66|67|4[0-9a-f])$/) {
        break
      }
    }
  }
' "$disassembly" >"$offenders"

if [[ -s "$offenders" ]]; then
  echo "error: x86_64 artifact contains forbidden EVEX/AVX-512 instruction encoding: $artifact" >&2
  echo "The portable release baseline must run without AVX-512. First findings:" >&2
  head -n 10 "$offenders" >&2
  exit 1
fi

checksum="$(sha256sum -- "$artifact" | awk '{print $1}')"
echo "portable x86_64 ISA audit passed: evex_avx512=absent sha256=$checksum artifact=$artifact"
