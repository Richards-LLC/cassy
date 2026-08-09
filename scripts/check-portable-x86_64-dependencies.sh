#!/usr/bin/env bash
# Reject Linux release graphs that compile an AVX-512-capable TLS provider into
# CAS even when another provider is selected at runtime.

set -euo pipefail

usage() {
  echo "Usage: scripts/check-portable-x86_64-dependencies.sh [--tree-file <cargo-tree-output>]" >&2
}

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
tree_file="$tmpdir/cargo-tree.txt"

case "$#" in
  0)
    for tool in cargo grep; do
      if ! command -v "$tool" >/dev/null 2>&1; then
        echo "error: required portable dependency audit tool is unavailable: $tool" >&2
        exit 2
      fi
    done
    if ! cargo tree --locked -p cas \
      --target x86_64-unknown-linux-gnu \
      --edges normal,build,features >"$tree_file"; then
      echo "error: could not resolve the locked Linux x86_64 release dependency graph" >&2
      exit 2
    fi
    ;;
  2)
    if [[ "$1" != "--tree-file" || ! -f "$2" ]]; then
      usage
      exit 2
    fi
    cp -- "$2" "$tree_file"
    ;;
  *)
    usage
    exit 2
    ;;
esac

if grep -Eq 'aws-lc-(rs|sys)( feature)?[[:space:]]' "$tree_file"; then
  echo "error: portable Linux release graph includes forbidden AWS-LC provider code" >&2
  grep -E 'aws-lc-(rs|sys)( feature)?[[:space:]]' "$tree_file" | head -n 10 >&2
  exit 1
fi

if grep -Eq 'rustls feature "aws(_lc_rs|-lc-rs)"' "$tree_file"; then
  echo "error: portable Linux release graph enables rustls AWS-LC provider features" >&2
  exit 1
fi

if ! grep -qF 'rustls feature "ring"' "$tree_file"; then
  echo "error: portable Linux release graph does not enable the required rustls ring provider" >&2
  exit 1
fi

if ! grep -qF 'blake3 feature "no_avx512"' "$tree_file"; then
  echo "error: portable Linux release graph does not disable BLAKE3 AVX-512 dispatch" >&2
  exit 1
fi

if ! grep -Eq 'blake3 v1\.8\.6 \(.*[/]vendor/blake3-1\.8\.6\)' "$tree_file"; then
  echo "error: portable Linux release graph does not use the audited BLAKE3 1.8.6 build override" >&2
  exit 1
fi

echo "portable x86_64 dependency audit passed: rustls_provider=ring aws_lc=absent blake3_patch=1.8.6 blake3_avx512=disabled"
