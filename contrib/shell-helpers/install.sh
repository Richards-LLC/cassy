#!/usr/bin/env bash
# Install the tracked CAS developer helpers into ~/.local/bin.
# Override the destination with CAS_UPDATE_INSTALL_DIR.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
install_dir="${CAS_UPDATE_INSTALL_DIR:-$HOME/.local/bin}"

mkdir -p "$install_dir"
install -m 0755 "$script_dir/cas-update" "$install_dir/cas-update"
printf 'Installed cas-update from %s to %s\n' "$script_dir/cas-update" "$install_dir/cas-update"
