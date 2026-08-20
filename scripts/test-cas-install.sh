#!/usr/bin/env bash
# Fixture test for the portable installer. No network access or real install.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
installer="$script_dir/cas-install.sh"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

mkdir -p "$tmpdir/bin" "$tmpdir/install" "$tmpdir/archive"
cat >"$tmpdir/archive/cas" <<'EOF'
#!/usr/bin/env bash
echo 'cas fixture version'
EOF
chmod +x "$tmpdir/archive/cas"
tar -czf "$tmpdir/release.tar.gz" -C "$tmpdir/archive" cas

cat >"$tmpdir/bin/uname" <<'EOF'
#!/usr/bin/env bash
case "$1" in
  -s) printf '%s\n' "${FIXTURE_OS:?}" ;;
  -m) printf '%s\n' "${FIXTURE_ARCH:?}" ;;
  *) exit 2 ;;
esac
EOF

cat >"$tmpdir/bin/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "${FIXTURE_CURL_LOG:?}"
output=''
while (($#)); do
  case "$1" in
    -o) output="$2"; shift 2 ;;
    *) shift ;;
  esac
done
if [[ -n "$output" ]]; then
  cp "${FIXTURE_ARCHIVE:?}" "$output"
else
  printf '%s\n' '{"tag_name":"v9.9.9"}'
fi
EOF

cat >"$tmpdir/bin/xattr" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${FIXTURE_XATTR_LOG:?}"
EOF
chmod +x "$tmpdir/bin/uname" "$tmpdir/bin/curl" "$tmpdir/bin/xattr"

darwin_output="$(
  PATH="$tmpdir/bin:/usr/bin:/bin" \
  FIXTURE_OS=Darwin \
  FIXTURE_ARCH=arm64 \
  FIXTURE_ARCHIVE="$tmpdir/release.tar.gz" \
  FIXTURE_CURL_LOG="$tmpdir/curl.log" \
  FIXTURE_XATTR_LOG="$tmpdir/xattr.log" \
  CAS_INSTALL_DIR="$tmpdir/install" \
  CAS_REPO=fixture/cassy \
  "$installer"
)"
grep -qF 'Platform: aarch64-apple-darwin' <<<"$darwin_output"
grep -qF 'cas fixture version' <<<"$darwin_output"
test -x "$tmpdir/install/cas"
grep -qF 'cas-aarch64-apple-darwin.tar.gz' "$tmpdir/curl.log"
grep -qF -- "-d com.apple.quarantine $tmpdir/install/cas" "$tmpdir/xattr.log"
echo 'ok   macOS Apple Silicon installs the published Darwin asset and clears quarantine'

set +e
intel_output="$(PATH="$tmpdir/bin:/usr/bin:/bin" FIXTURE_OS=Darwin FIXTURE_ARCH=x86_64 "$installer" 2>&1)"
intel_status=$?
set -e
test "$intel_status" -ne 0
grep -qF 'macOS Apple Silicon binary only; Intel Macs must build from source' <<<"$intel_output"
echo 'ok   macOS Intel names the source-build path instead of selecting a missing asset'

echo 'PASS: portable installer macOS behavior verified.'
