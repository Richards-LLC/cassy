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

set -euo pipefail

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

REPO="${CAS_REPO:-Richards-LLC/cassy}"
INSTALL_DIR="${CAS_INSTALL_DIR:-}"
VERSION="${CAS_VERSION:-}"
BINARY_NAME="cas"
GITHUB_API="https://api.github.com"

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
    *)
      error "Unsupported OS: $os"
      error "Cassy currently only supports Linux. macOS/Windows support is planned."
      exit 1
      ;;
  esac

  case "$arch" in
    x86_64|amd64) arch="x86_64" ;;
    *)
      error "Unsupported architecture: $arch"
      error "Cassy currently only supports x86_64. ARM64 support is planned."
      exit 1
      ;;
  esac

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

  # Check if install dir is in PATH
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
      warn "$INSTALL_DIR is not in your PATH"
      warn "Add this to your shell profile:"
      warn "  export PATH=\"$INSTALL_DIR:\$PATH\""
      ;;
  esac

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

download_and_install() {
  local asset_name="cas-${PLATFORM}.tar.gz"
  local download_url="https://github.com/${REPO}/releases/download/${VERSION}/${asset_name}"

  info "Downloading Cassy ${VERSION} for ${PLATFORM}..."

  local tmp_dir
  tmp_dir="$(mktemp -d)"
  trap 'rm -rf "$tmp_dir"' EXIT

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
}

# ---------------------------------------------------------------------------
# Verification
# ---------------------------------------------------------------------------

verify_install() {
  local installed_version

  if ! command -v "$BINARY_NAME" &>/dev/null; then
    # Try the explicit path
    if [ -x "${INSTALL_DIR}/${BINARY_NAME}" ]; then
      installed_version="$("${INSTALL_DIR}/${BINARY_NAME}" --version 2>/dev/null || echo "unknown")"
    else
      error "Installation failed — ${BINARY_NAME} not found."
      exit 1
    fi
  else
    installed_version="$("$BINARY_NAME" --version 2>/dev/null || echo "unknown")"
  fi

  echo ""
  bold "Cassy installed successfully!"
  echo ""
  info "Version:  $installed_version"
  info "Location: ${INSTALL_DIR}/${BINARY_NAME}"
  echo ""
  bold "Next steps:"
  echo "  1. Initialize a project:  cd your-project && cas init"
  echo "  2. Start a session:       cas factory"
  echo "  3. Check the docs:        cas --help"
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
  verify_install
}

main "$@"
