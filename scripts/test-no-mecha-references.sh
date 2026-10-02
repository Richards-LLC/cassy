#!/usr/bin/env bash
# Enforce the reviewed Violet retirement boundary, including unstaged edits.
set -euo pipefail
exec python3 "$(dirname "$0")/check-violet-references.py" "$@"
