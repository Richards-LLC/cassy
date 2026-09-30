#!/usr/bin/env bash
# Compatibility entry point retained for one release. New credentials use Violet names.
set -euo pipefail
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec "$script_dir/violet-credentials.sh" "$@"
