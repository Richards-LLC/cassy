#!/usr/bin/env bash
# Fixture tests for the release-publication state machine. No network access.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
receipt="$script_dir/release-published-receipt.sh"
template="$script_dir/../docs/release-notes/runtime-release-template.md"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

fixture="$tmpdir/fixture"
mkdir -p "$fixture/assets"
printf 'linux published bytes\n' >"$fixture/assets/cas-x86_64-unknown-linux-gnu.tar.gz"
printf 'macos published bytes\n' >"$fixture/assets/cas-aarch64-apple-darwin.tar.gz"
linux_sha="$(sha256sum "$fixture/assets/cas-x86_64-unknown-linux-gnu.tar.gz" | awk '{print $1}')"
macos_sha="$(sha256sum "$fixture/assets/cas-aarch64-apple-darwin.tar.gz" | awk '{print $1}')"

fake_gh="$tmpdir/gh"
cat >"$fake_gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$1 $2" in
  "release view") cat "$FIXTURE/release.json" ;;
  "release download")
    pattern=""
    directory=""
    while [[ "$#" -gt 0 ]]; do
      case "$1" in
        --pattern) pattern="$2"; shift 2 ;;
        --dir) directory="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    cp "$FIXTURE/assets/$pattern" "$directory/$pattern"
    ;;
  *) echo "unexpected fake gh invocation: $*" >&2; exit 2 ;;
esac
EOF
chmod +x "$fake_gh"

write_release() {
  cat >"$fixture/release.json" <<EOF
{"isDraft":false,"publishedAt":"2026-08-15T00:57:26Z","assets":$1}
EOF
}

# A release object alone is insufficient: this pins the exact half-uploaded
# state that used to let a local digest reach the announcement.
write_release "[{\"name\":\"cas-x86_64-unknown-linux-gnu.tar.gz\",\"digest\":\"sha256:$linux_sha\"}]"
set +e
partial_output="$(FIXTURE="$fixture" GH_BIN="$fake_gh" "$receipt" v2.69.1 2>&1)"
partial_status=$?
set -e
test "$partial_status" -ne 0
grep -qF 'release v2.69.1 is partial; required asset cas-aarch64-apple-darwin.tar.gz' <<<"$partial_output"
echo 'ok   partial published release fails closed before announcement fields'

write_release "[{\"name\":\"cas-x86_64-unknown-linux-gnu.tar.gz\",\"digest\":\"sha256:$(printf '0%.0s' {1..64})\"},{\"name\":\"cas-aarch64-apple-darwin.tar.gz\",\"digest\":\"sha256:$macos_sha\"}]"
set +e
mismatch_output="$(FIXTURE="$fixture" GH_BIN="$fake_gh" "$receipt" v2.69.1 2>&1)"
mismatch_status=$?
set -e
test "$mismatch_status" -ne 0
grep -qF 'does not match GitHub digest' <<<"$mismatch_output"
echo 'ok   mismatched downloaded bytes fail closed'

write_release "[{\"name\":\"cas-x86_64-unknown-linux-gnu.tar.gz\",\"digest\":\"sha256:$linux_sha\"},{\"name\":\"cas-aarch64-apple-darwin.tar.gz\",\"digest\":\"sha256:$macos_sha\"}]"
draft="$fixture/draft.md"
printf 'Linux {{LINUX_SHA256}} / {{LINUX_SHA256}}; macOS {{MACOS_SHA256}} / {{MACOS_SHA256}}\n' >"$draft"
success_output="$(FIXTURE="$fixture" GH_BIN="$fake_gh" "$receipt" v2.69.1 --write-draft "$draft")"
grep -qFx 'TAG=v2.69.1' <<<"$success_output"
grep -qFx "LINUX_SHA256=$linux_sha" <<<"$success_output"
grep -qFx "MACOS_SHA256=$macos_sha" <<<"$success_output"
grep -qF "$linux_sha" "$draft"
grep -qF "$macos_sha" "$draft"
test "$(rg -oF "$linux_sha" "$draft" | wc -l)" -eq 2
test "$(rg -oF "$macos_sha" "$draft" | wc -l)" -eq 2
if rg -qF '{{LINUX_SHA256}}|{{MACOS_SHA256}}' "$draft"; then
    echo 'FAIL receipt left a digest placeholder in the draft' >&2
    exit 1
fi
echo 'ok   complete published release emits and mechanically writes fresh digests'

test "$(rg -oF '{{LINUX_SHA256}}' "$template" | wc -l)" -eq 2
test "$(rg -oF '{{MACOS_SHA256}}' "$template" | wc -l)" -eq 2
echo 'ok   runtime draft template exposes only receipt-fillable digest placeholders'
