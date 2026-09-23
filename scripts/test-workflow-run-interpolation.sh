#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
checker="$repo_root/scripts/check-workflow-run-interpolation.py"
fixture="$(mktemp)"
trap 'rm -f "$fixture"' EXIT

python3 "$checker"

cat > "$fixture" <<'YAML'
jobs:
  example:
    steps:
      - env:
          SUBJECT: ${{ steps.notes.outputs.notes }}
        run: |
          printf '%s\n' "$SUBJECT"
YAML
python3 "$checker" "$fixture"

cat > "$fixture" <<'YAML'
jobs:
  example:
    steps:
      - run: echo "${{ steps.notes.outputs.notes }}"
YAML
if python3 "$checker" "$fixture" >/dev/null 2>&1; then
  echo 'FAIL: inline run interpolation passed' >&2
  exit 1
fi

cat > "$fixture" <<'YAML'
jobs:
  example:
    steps:
      - run: |
          echo "${{ github.event.head_commit.message }}"
YAML
if python3 "$checker" "$fixture" >/dev/null 2>&1; then
  echo 'FAIL: block run interpolation passed' >&2
  exit 1
fi

echo 'ok: safe env expansion accepted; inline and block injection rejected'
