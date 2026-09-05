#!/usr/bin/env bash
# Cassy Installer — install the Cassy binary from GitHub Releases.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Richards-LLC/cassy/main/scripts/cas-install.sh | bash
#
# Options (via env vars):
#   CAS_INSTALL_DIR   Override install directory (default: ~/.local/bin — the canonical location)
#   CAS_VERSION       Install a specific version (default: latest)
#   CAS_REPO          Override GitHub repo (default: Richards-LLC/cassy)
#   CAS_WIRE_PATH     1 = wire PATH into the login shell's rc file without asking,
#                     0 = never edit an rc file (just print the line to add).
#                     Unset = ask on the terminal when one is available.
#
# Artifact trust model: the installer requires the selected asset's SHA-256
# from GitHub Release metadata and checks it before extraction. This detects
# corrupt or mismatched downloads, but it is not substitution-resistant: the
# archive and digest share the same GitHub/repository publishing authority.
# Cassy does not currently name an independent signing key or attestation root.

# --- POSIX-safe preamble: this installer requires bash -------------------------
# A piped script has no shebang: `curl ... | sh` runs THIS FILE under /bin/sh
# whatever the first line says. That matters because the body below uses bash
# syntax, and under a strict POSIX shell some of it does not fail — it
# MISPARSES. `command -v curl &>/dev/null` under dash is read as
# `command -v curl &` plus `>/dev/null`, which backgrounds the probe and prints
# to stdout instead of testing anything. Detect the wrong interpreter here, in
# syntax every shell agrees on, and say exactly what to run instead. Re-exec is
# not an option: when the script arrives on stdin there is no file to re-exec.
if [ -z "${BASH_VERSION:-}" ]; then
  echo "x The Cassy installer needs bash, but it is running under a different shell." >&2
  echo "  Re-run it with bash:" >&2
  echo "    curl -fsSL https://raw.githubusercontent.com/Richards-LLC/cassy/main/scripts/cas-install.sh | bash" >&2
  exit 1
fi

set -euo pipefail

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

REPO="${CAS_REPO:-Richards-LLC/cassy}"
INSTALL_DIR="${CAS_INSTALL_DIR:-}"
VERSION="${CAS_VERSION:-}"
BINARY_NAME="cas"
GITHUB_API="https://api.github.com"
# Captured before anything can extend it, so the fresh-login-shell check below
# measures what a NEW terminal sees rather than what this process arranged.
ORIGINAL_PATH="$PATH"

# ---------------------------------------------------------------------------
# Colors (disable if not a terminal)
# ---------------------------------------------------------------------------

if [ -t 1 ]; then
  BOLD='\033[1m'
  GREEN='\033[0;32m'
  YELLOW='\033[0;33m'
  RED='\033[0;31m'
  RESET='\033[0m'
else
  BOLD='' GREEN='' YELLOW='' RED='' RESET=''
fi

info()  { echo -e "${GREEN}>${RESET} $*"; }
warn()  { echo -e "${YELLOW}!${RESET} $*"; }
error() { echo -e "${RED}x${RESET} $*" >&2; }
bold()  { echo -e "${BOLD}$*${RESET}"; }

# ---------------------------------------------------------------------------
# Platform detection
# ---------------------------------------------------------------------------

detect_platform() {
  local os arch

  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Linux)  os="unknown-linux-gnu" ;;
    Darwin) os="apple-darwin" ;;
    *)
      error "Unsupported OS: $os"
      error "Cassy supports Linux x86_64 and macOS on Apple Silicon."
      exit 1
      ;;
  esac

  case "$arch" in
    x86_64|amd64) arch="x86_64" ;;
    arm64|aarch64)
      if [ "$os" = "apple-darwin" ]; then
        arch="aarch64"
      else
        error "Unsupported architecture: $arch"
        error "Cassy currently publishes Linux x86_64 and macOS Apple Silicon binaries."
        exit 1
      fi
      ;;
    *)
      error "Unsupported architecture: $arch"
      error "Cassy currently publishes Linux x86_64 and macOS Apple Silicon binaries."
      exit 1
      ;;
  esac

  if [ "$os" = "apple-darwin" ] && [ "$arch" != "aarch64" ]; then
    error "Unsupported architecture: $arch"
    error "Cassy publishes a macOS Apple Silicon binary only; Intel Macs must build from source."
    exit 1
  fi

  PLATFORM="${arch}-${os}"
}

# ---------------------------------------------------------------------------
# Install directory resolution
# ---------------------------------------------------------------------------

resolve_install_dir() {
  if [ -n "$INSTALL_DIR" ]; then
    return
  fi

  # Canonical install location is $HOME/.local/bin. Installing system-wide to
  # /usr/local/bin creates silent duplicates that diverge from per-user dev
  # builds — see docs/requests/completed/BUG-stale-cas-binaries-on-path.md.
  INSTALL_DIR="$HOME/.local/bin"
}

ensure_install_dir() {
  if [ ! -d "$INSTALL_DIR" ]; then
    info "Creating $INSTALL_DIR"
    if [ "$INSTALL_DIR" = "/usr/local/bin" ] && [ ! -w /usr/local/bin ]; then
      sudo mkdir -p "$INSTALL_DIR"
    else
      mkdir -p "$INSTALL_DIR"
    fi
  fi

  # PATH is handled after the binary exists — see wire_path(). Warning about it
  # here, before there is anything to run, produced advice the user had to
  # remember through a download; now it is an offer to fix it.

  # Flag other cas binaries on PATH that will silently shadow (or be shadowed
  # by) the one we're about to install. Scan PATH directly rather than relying
  # on `which -a` — the -a flag is a GNU/macOS extension that busybox `which`
  # (common on Alpine/CI images) does not support.
  local others=""
  local IFS_BACKUP="$IFS"
  IFS=':'
  for dir in $PATH; do
    [ -z "$dir" ] && continue
    if [ -x "$dir/cas" ] && [ "$dir/cas" != "$INSTALL_DIR/cas" ]; then
      others="${others}${dir}/cas
"
    fi
  done
  IFS="$IFS_BACKUP"
  if [ -n "$others" ]; then
    warn "Other cas binaries on PATH (these will diverge from the canonical install):"
    printf '%s' "$others" | sed 's/^/  /' >&2
    warn "Remove them to avoid silent staleness — see cas-cli/docs/CONTRIBUTING.md (\"Canonical install path\")"
  fi
}

# ---------------------------------------------------------------------------
# Version resolution
# ---------------------------------------------------------------------------

resolve_version() {
  if [ -n "$VERSION" ]; then
    # Ensure version starts with 'v'
    case "$VERSION" in
      v*) ;;
      *)  VERSION="v${VERSION}" ;;
    esac
    return
  fi

  info "Fetching latest release..."
  local release_url="${GITHUB_API}/repos/${REPO}/releases/latest"
  local response

  if command -v curl &>/dev/null; then
    response="$(curl -fsSL "$release_url" 2>/dev/null)" || {
      error "Failed to fetch latest release from $release_url"
      error "Check your internet connection or set CAS_VERSION manually."
      exit 1
    }
  elif command -v wget &>/dev/null; then
    response="$(wget -qO- "$release_url" 2>/dev/null)" || {
      error "Failed to fetch latest release from $release_url"
      exit 1
    }
  else
    error "Neither curl nor wget found. Install one and try again."
    exit 1
  fi

  # Parse tag_name from JSON (works without jq)
  VERSION="$(echo "$response" | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')"

  if [ -z "$VERSION" ]; then
    error "Could not determine latest version from GitHub API."
    error "Set CAS_VERSION=v2.0.0 (or your target version) and try again."
    exit 1
  fi
}

# ---------------------------------------------------------------------------
# Download and install
# ---------------------------------------------------------------------------

release_asset_sha256() {
  local asset_name="$1"
  local asset_record digest_record

  # The installer deliberately has no jq dependency. Flatten the response,
  # split it at every asset `name` field, then inspect only the record beginning
  # with the exact selected asset. This prevents a missing digest from falling
  # through to a different asset's digest later in the response.
  asset_record="$(
    tr -d '\r\n' \
      | sed 's/"name"[[:space:]]*:[[:space:]]*/\
"name":/g' \
      | grep -F -m1 "\"name\":\"${asset_name}\""
  )" || return 1

  digest_record="$(
    printf '%s\n' "$asset_record" \
      | grep -o -m1 '"digest"[[:space:]]*:[[:space:]]*"sha256:[0-9a-f]\{64\}"'
  )" || return 1

  printf '%s\n' "$digest_record" \
    | sed 's/.*"sha256:\([0-9a-f]\{64\}\)"/\1/'
}

sha256_file() {
  local path="$1"

  if command -v sha256sum &>/dev/null; then
    sha256sum -- "$path" | awk '{print $1}'
  elif command -v shasum &>/dev/null; then
    shasum -a 256 "$path" | awk '{print $1}'
  else
    return 127
  fi
}

download_and_install() {
  local asset_name="cas-${PLATFORM}.tar.gz"
  local download_url="https://github.com/${REPO}/releases/download/${VERSION}/${asset_name}"
  local receipt_url="${GITHUB_API}/repos/${REPO}/releases/tags/${VERSION}"
  local release_receipt expected_sha256 actual_sha256

  info "Fetching the published GitHub release receipt..."
  if command -v curl &>/dev/null; then
    release_receipt="$(curl -fsSL "$receipt_url" 2>/dev/null)" || {
      error "Failed to fetch the published release receipt: $receipt_url"
      error "Refusing to install without a verifiable SHA-256 receipt."
      exit 1
    }
  elif command -v wget &>/dev/null; then
    release_receipt="$(wget -qO- "$receipt_url" 2>/dev/null)" || {
      error "Failed to fetch the published release receipt: $receipt_url"
      error "Refusing to install without a verifiable SHA-256 receipt."
      exit 1
    }
  else
    error "Neither curl nor wget found. Install one and try again."
    exit 1
  fi

  expected_sha256="$(printf '%s\n' "$release_receipt" | release_asset_sha256 "$asset_name")" || {
    error "Published GitHub release receipt has no valid SHA-256 for ${asset_name}."
    error "Refusing to extract or replace ${INSTALL_DIR}/${BINARY_NAME}."
    exit 1
  }

  info "Downloading Cassy ${VERSION} for ${PLATFORM}..."

  local tmp_dir
  tmp_dir="$(mktemp -d)"
  # Capture the path while the local exists; an EXIT trap that expands
  # `$tmp_dir` later fails under `set -u` after this function has returned.
  trap "rm -rf -- $(printf '%q' "$tmp_dir")" EXIT

  local archive_path="${tmp_dir}/${asset_name}"

  if command -v curl &>/dev/null; then
    curl -fSL --progress-bar "$download_url" -o "$archive_path" || {
      error "Download failed: $download_url"
      error "Check that version ${VERSION} exists and has a release asset."
      exit 1
    }
  elif command -v wget &>/dev/null; then
    wget -q --show-progress "$download_url" -O "$archive_path" || {
      error "Download failed: $download_url"
      exit 1
    }
  fi

  actual_sha256="$(sha256_file "$archive_path")" || {
    error "Cannot verify the archive: install sha256sum or shasum and try again."
    error "Refusing to extract or replace ${INSTALL_DIR}/${BINARY_NAME}."
    exit 1
  }
  if [ "$actual_sha256" != "$expected_sha256" ]; then
    error "Archive SHA-256 does not match the published GitHub release receipt."
    error "Expected: $expected_sha256"
    error "Actual:   $actual_sha256"
    error "Refusing to extract or replace ${INSTALL_DIR}/${BINARY_NAME}."
    exit 1
  fi
  info "Verified SHA-256 against the published GitHub release receipt."

  info "Extracting..."
  tar -xzf "$archive_path" -C "$tmp_dir"

  if [ ! -f "${tmp_dir}/${BINARY_NAME}" ]; then
    error "Archive did not contain '${BINARY_NAME}' binary."
    error "Contents: $(ls "$tmp_dir")"
    exit 1
  fi

  info "Installing to ${INSTALL_DIR}/${BINARY_NAME}"
  if [ "$INSTALL_DIR" = "/usr/local/bin" ] && [ ! -w "$INSTALL_DIR" ]; then
    sudo install -m 755 "${tmp_dir}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
  else
    install -m 755 "${tmp_dir}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
  fi

  # GitHub-downloaded binaries can retain the quarantine attribute on macOS.
  # Clearing it is harmless when the attribute is absent and lets the first
  # `cas` invocation proceed without a Gatekeeper rejection.
  if [ "$(uname -s)" = "Darwin" ] && command -v xattr &>/dev/null; then
    xattr -d com.apple.quarantine "${INSTALL_DIR}/${BINARY_NAME}" 2>/dev/null || true
  fi
}

# ---------------------------------------------------------------------------
# PATH wiring
#
# Installing a binary the user's shell cannot find is not an install. The
# offer below is the difference between "cas: command not found" on a fresh
# Mac and a working tool.
# ---------------------------------------------------------------------------

MARKER_BEGIN='# >>> cassy path >>>'
MARKER_END='# <<< cassy path <<<'

# The LOGIN shell, which is not the shell running this script. `curl | bash`
# runs the installer under bash even on a Mac whose login shell is zsh, so
# reading $0 or $BASH would confidently wire the wrong file.
detect_login_shell() {
  local shell_path="${SHELL:-}"
  if [ -z "$shell_path" ] && command -v getent >/dev/null 2>&1; then
    shell_path="$(getent passwd "$(id -un)" 2>/dev/null | awk -F: '{print $7}')"
  fi
  LOGIN_SHELL="$shell_path"
  LOGIN_SHELL_NAME="$(basename -- "${shell_path:-unknown}")"
}

# Which file the guard belongs in.
#
# zsh -> .zshenv, deliberately, and NOT .zshrc: zsh reads .zshrc only for
# interactive shells, so a PATH exported there is invisible to the
# non-interactive shells that MCP clients use to spawn `cas serve`. That exact
# failure is documented in docs/onboarding/macbook-from-zero.md.
#
# bash -> .bashrc when it exists (every mainstream Linux distro sources it from
# the login path), otherwise .profile.
#
# Anything else -> no guess. Wiring an unknown shell's startup file is how an
# installer corrupts someone's environment; print the line instead.
resolve_rc_file() {
  RC_FILE=""
  case "$LOGIN_SHELL_NAME" in
    zsh)
      RC_FILE="${ZDOTDIR:-$HOME}/.zshenv"
      ;;
    bash)
      if [ -f "$HOME/.bashrc" ]; then
        RC_FILE="$HOME/.bashrc"
      else
        RC_FILE="$HOME/.profile"
      fi
      ;;
    *)
      RC_FILE=""
      ;;
  esac
}

manual_path_line() {
  printf 'export PATH="%s:$PATH"\n' "$INSTALL_DIR"
}

print_manual_instructions() {
  warn "Add this line to your shell startup file, then open a new terminal:"
  echo "    $(manual_path_line)"
}

rc_already_wired() {
  [ -n "$RC_FILE" ] && [ -f "$RC_FILE" ] && grep -qF "$MARKER_BEGIN" "$RC_FILE"
}

# The block is idempotent twice over: the markers stop this installer from
# appending it again, and the `case` inside stops the block itself from
# re-prepending if the file is sourced more than once in a session.
append_path_guard() {
  local dir="$1" file="$2"
  [ -e "$file" ] || : >"$file"
  {
    printf '\n%s\n' "$MARKER_BEGIN"
    printf '%s\n' '# Added by the Cassy installer (scripts/cas-install.sh).'
    printf '%s\n' '# Keeps the Cassy install directory on PATH for every shell, including the'
    printf '%s\n' '# non-interactive ones MCP clients use to spawn `cas serve`.'
    printf '%s\n' 'case ":$PATH:" in'
    printf '  *":%s:"*) ;;\n' "$dir"
    printf '  *) export PATH="%s:$PATH" ;;\n' "$dir"
    printf '%s\n' 'esac'
    printf '%s\n' "$MARKER_END"
  } >>"$file"
}

# Ask on the controlling terminal, never on stdin: under `curl | bash` stdin IS
# the script, and reading from it would consume the installer's own body.
prompt_yes_no() {
  local question="$1" answer=""
  # `[ -r /dev/tty ]` is not the test you want: the device node exists and is
  # readable by mode even when this process has no controlling terminal, and
  # the open then fails with ENXIO mid-install. Try the open itself.
  if ! { : >/dev/tty; } 2>/dev/null; then
    return 2
  fi
  printf '%b %s [Y/n] ' "${YELLOW}?${RESET}" "$question" >/dev/tty 2>/dev/null || return 2
  read -r answer </dev/tty 2>/dev/null || return 2
  case "$answer" in
    ""|y|Y|yes|YES|Yes) return 0 ;;
    *) return 1 ;;
  esac
}

wire_path() {
  PATH_WIRED_FILE=""

  case ":$PATH:" in
    *":$INSTALL_DIR:"*)
      # Already visible to this shell. Say nothing: the fresh-login-shell check
      # in verify_install() is the claim that actually matters.
      return 0
      ;;
  esac

  detect_login_shell
  resolve_rc_file

  echo ""
  warn "$INSTALL_DIR is not on your PATH, so \`cas\` will not be found yet."

  if [ -z "$RC_FILE" ]; then
    warn "Your login shell (${LOGIN_SHELL_NAME}) is not one this installer edits automatically."
    print_manual_instructions
    return 0
  fi

  if rc_already_wired; then
    info "$RC_FILE already has the Cassy PATH guard — leaving it alone."
    PATH_WIRED_FILE="$RC_FILE"
    return 0
  fi

  local decision=""
  case "${CAS_WIRE_PATH:-}" in
    1) decision=yes ;;
    0) decision=no ;;
    *)
      if prompt_yes_no "Add it to $RC_FILE?"; then
        decision=yes
      else
        # Exit status 2 means there was no terminal to ask on. An unattended
        # install must not silently edit a startup file nobody consented to.
        decision=no
      fi
      ;;
  esac

  if [ "$decision" = yes ]; then
    append_path_guard "$INSTALL_DIR" "$RC_FILE"
    info "Added the Cassy PATH guard to $RC_FILE"
    PATH_WIRED_FILE="$RC_FILE"
  else
    print_manual_instructions
  fi
}

# ---------------------------------------------------------------------------
# Verification
# ---------------------------------------------------------------------------

# The honest question is not "did a file land on disk" but "can a new terminal
# run it". Ask a fresh LOGIN shell, which re-reads the startup files, so a
# successful answer proves the wiring above actually worked.
fresh_login_shell_sees_cas() {
  [ -n "${LOGIN_SHELL:-}" ] || return 1
  [ -x "$LOGIN_SHELL" ] || return 1
  case "$LOGIN_SHELL_NAME" in
    zsh|bash|sh|dash|ksh) ;;
    *) return 1 ;;
  esac
  # Start the child from the PATH this process had BEFORE any wiring. If it
  # inherited a PATH the installer had already extended, this check would pass
  # by construction and prove nothing about a new terminal — the one thing it
  # exists to prove.
  #
  # A startup file can also be slow or interactive; a verification step must
  # never be the thing that hangs an install.
  if command -v timeout >/dev/null 2>&1; then
    env PATH="$ORIGINAL_PATH" timeout 15 "$LOGIN_SHELL" -lc 'command -v cas >/dev/null 2>&1' >/dev/null 2>&1
  else
    env PATH="$ORIGINAL_PATH" "$LOGIN_SHELL" -lc 'command -v cas >/dev/null 2>&1' >/dev/null 2>&1
  fi
}

verify_install() {
  local installed_version

  if [ ! -x "${INSTALL_DIR}/${BINARY_NAME}" ]; then
    error "Installation failed — ${BINARY_NAME} not found at ${INSTALL_DIR}/${BINARY_NAME}."
    exit 1
  fi
  installed_version="$("${INSTALL_DIR}/${BINARY_NAME}" --version 2>/dev/null || echo "unknown")"

  detect_login_shell

  echo ""
  if fresh_login_shell_sees_cas; then
    bold "Cassy installed successfully!"
    echo ""
    info "Version:  $installed_version"
    info "Location: ${INSTALL_DIR}/${BINARY_NAME}"
    info "A new terminal can run \`cas\`."
  else
    # Deliberately not "installed successfully". The binary is there, but the
    # thing the user is about to try — typing `cas` in a new window — does not
    # work yet, and saying otherwise is how someone ends up stranded.
    bold "Cassy is installed, but a new terminal cannot run \`cas\` yet."
    echo ""
    info "Version:  $installed_version"
    info "Location: ${INSTALL_DIR}/${BINARY_NAME}"
    if [ -n "${PATH_WIRED_FILE:-}" ]; then
      warn "$PATH_WIRED_FILE was updated — open a new terminal (or run: source $PATH_WIRED_FILE)."
    else
      print_manual_instructions
    fi
    warn "Until then, run it by full path: ${INSTALL_DIR}/${BINARY_NAME}"
  fi

  echo ""
  bold "Next steps:"
  echo "  1. Initialize a project:  cd your-project && cas init"
  echo "  2. Start the hub service:  cas hub service install"
  echo "  3. Refresh all projects:  cas update --all-projects"
  echo "  4. Start a session:        cas factory"
  echo "  5. Check the docs:         cas --help"
  echo ""
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

main() {
  echo ""
  bold "Cassy Installer"
  echo ""

  detect_platform
  resolve_install_dir
  ensure_install_dir
  resolve_version

  info "Version:  ${VERSION}"
  info "Platform: ${PLATFORM}"
  info "Install:  ${INSTALL_DIR}"
  echo ""

  download_and_install
  wire_path
  verify_install
}

main "$@"
