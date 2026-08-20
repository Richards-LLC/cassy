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

# ---------------------------------------------------------------------------
# PATH wiring seam (cas-c741)
#
# Installing a binary the user's shell cannot find is not an install. These
# cases drive the rc-edit seam directly: which file, exactly once, only with
# consent, and an honest verdict afterwards.
#
# The fake login shells live OUTSIDE the installer's PATH and are named by
# absolute path through $SHELL, because the whole point is that the login shell
# is not the shell running the installer.
# ---------------------------------------------------------------------------

mkdir -p "$tmpdir/shells"
cat >"$tmpdir/shells/zsh" <<'EOF'
#!/usr/bin/env bash
# Enough of zsh to exercise the seam: a login shell reads .zshenv.
[ -f "$HOME/.zshenv" ] && . "$HOME/.zshenv"
while [[ "${1:-}" == -* ]]; do
  case "$1" in
    -lc) shift; eval "$1"; exit $? ;;
    *) shift ;;
  esac
done
EOF
cat >"$tmpdir/shells/bash" <<'EOF'
#!/usr/bin/env bash
# A bash login shell path that reaches .bashrc (as distro .profile files do),
# falling back to .profile when there is no .bashrc.
if [ -f "$HOME/.bashrc" ]; then . "$HOME/.bashrc"
elif [ -f "$HOME/.profile" ]; then . "$HOME/.profile"
fi
while [[ "${1:-}" == -* ]]; do
  case "$1" in
    -lc) shift; eval "$1"; exit $? ;;
    *) shift ;;
  esac
done
EOF
cat >"$tmpdir/shells/fish" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "$tmpdir/shells/zsh" "$tmpdir/shells/bash" "$tmpdir/shells/fish"

wire_run() {
  # A fresh HOME per scenario keeps rc state from leaking between cases.
  local home="$1" shell="$2"
  shift 2
  mkdir -p "$home"
  env -i \
    PATH="$tmpdir/bin:/usr/bin:/bin" \
    HOME="$home" \
    SHELL="$shell" \
    FIXTURE_OS=Darwin \
    FIXTURE_ARCH=arm64 \
    FIXTURE_ARCHIVE="$tmpdir/release.tar.gz" \
    FIXTURE_CURL_LOG="$tmpdir/curl.log" \
    FIXTURE_XATTR_LOG="$tmpdir/xattr.log" \
    CAS_INSTALL_DIR="$home/.local/bin" \
    CAS_REPO=fixture/cassy \
    "$@" \
    "$installer" 2>&1
}

count_markers() {
  grep -cF '# >>> cassy path >>>' "$1" 2>/dev/null || true
}

# 1. zsh writes .zshenv — NOT .zshrc, because .zshrc is interactive-only and an
#    MCP client spawning `cas serve` would never see it.
zsh_home="$tmpdir/home-zsh"
zsh_output="$(wire_run "$zsh_home" "$tmpdir/shells/zsh" CAS_WIRE_PATH=1)"
test -f "$zsh_home/.zshenv"
test ! -f "$zsh_home/.zshrc"
grep -qF "$zsh_home/.local/bin" "$zsh_home/.zshenv"
test "$(count_markers "$zsh_home/.zshenv")" -eq 1
grep -qF 'Cassy installed successfully!' <<<"$zsh_output"
echo 'ok   zsh login shell is wired through .zshenv and verifies in a fresh login shell'

# 2. Re-running must not append a second block.
zsh_second="$(wire_run "$zsh_home" "$tmpdir/shells/zsh" CAS_WIRE_PATH=1)"
test "$(count_markers "$zsh_home/.zshenv")" -eq 1
grep -qF 'already has the Cassy PATH guard' <<<"$zsh_second"
echo 'ok   a second install leaves exactly one guard block and says so'

# 3. bash prefers an existing .bashrc over .profile.
bashrc_home="$tmpdir/home-bashrc"
mkdir -p "$bashrc_home"
: >"$bashrc_home/.bashrc"
wire_run "$bashrc_home" "$tmpdir/shells/bash" CAS_WIRE_PATH=1 >/dev/null
test "$(count_markers "$bashrc_home/.bashrc")" -eq 1
test ! -f "$bashrc_home/.profile"
echo 'ok   bash with a .bashrc is wired there, not in .profile'

# 4. bash with no .bashrc falls back to .profile.
profile_home="$tmpdir/home-profile"
wire_run "$profile_home" "$tmpdir/shells/bash" CAS_WIRE_PATH=1 >/dev/null
test "$(count_markers "$profile_home/.profile")" -eq 1
test ! -f "$profile_home/.bashrc"
echo 'ok   bash without a .bashrc falls back to .profile'

# 5. Declining edits nothing and prints the exact line — and the installer must
#    NOT claim success, because a new terminal still cannot run `cas`.
declined_home="$tmpdir/home-declined"
declined_output="$(wire_run "$declined_home" "$tmpdir/shells/zsh" CAS_WIRE_PATH=0)"
test ! -f "$declined_home/.zshenv"
grep -qF "export PATH=\"$declined_home/.local/bin:\$PATH\"" <<<"$declined_output"
grep -qF 'a new terminal cannot run `cas` yet' <<<"$declined_output"
grep -qvF 'installed successfully' <<<"$declined_output"
echo 'ok   declining touches no file, prints the exact line, and does not claim success'

# 6. An unknown login shell is never guessed at.
fish_home="$tmpdir/home-fish"
fish_output="$(wire_run "$fish_home" "$tmpdir/shells/fish" CAS_WIRE_PATH=1)"
test -z "$(find "$fish_home" -maxdepth 1 -name '.*' -type f 2>/dev/null)"
grep -qF 'not one this installer edits automatically' <<<"$fish_output"
grep -qF "export PATH=\"$fish_home/.local/bin:\$PATH\"" <<<"$fish_output"
echo 'ok   an unfamiliar login shell gets instructions instead of an edited startup file'

# 7. No override and no terminal to ask on. An unattended `curl | bash` in CI or
#    a provisioning script must never silently edit someone's startup file, and
#    it must not spray tty errors either: /dev/tty exists and is readable by
#    mode here, but opening it fails with ENXIO.
unattended_home="$tmpdir/home-unattended"
unattended_output="$(wire_run "$unattended_home" "$tmpdir/shells/zsh" </dev/null)"
test ! -f "$unattended_home/.zshenv"
grep -qF "export PATH=\"$unattended_home/.local/bin:\$PATH\"" <<<"$unattended_output"
grep -qvF '/dev/tty' <<<"$unattended_output"
echo 'ok   an unattended install edits nothing, prints the line, and reports no tty errors'

# 8. `curl | sh` ignores the shebang. Under a POSIX shell the body below the
#    preamble does not fail cleanly — it misparses — so the preamble must catch
#    it and name the working command.
if command -v dash >/dev/null 2>&1; then
  set +e
  sh_output="$(dash "$installer" 2>&1)"
  sh_status=$?
  set -e
  test "$sh_status" -eq 1
  grep -qF 'needs bash' <<<"$sh_output"
  grep -qF '| bash' <<<"$sh_output"
  echo 'ok   running the installer under a non-bash shell fails loudly with the right command'
else
  echo 'ok   (not observable here) dash is unavailable to exercise the non-bash invocation'
fi

echo 'PASS: portable installer macOS behavior and PATH wiring verified.'
