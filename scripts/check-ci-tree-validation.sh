#!/usr/bin/env bash
# Heavy main-push jobs consume the same exact-tree queue proof as Fast
# Validation. Missing, failed or ambiguous queue evidence always runs the work.
set -euo pipefail

output="${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"
printf 'run-heavy=true\n' >>"$output"

if [[ "${GITHUB_EVENT_NAME:-}" != push || "${GITHUB_REF:-}" != refs/heads/main ]]; then
    echo 'Tree dedupe applies only to main pushes; running heavy validation.'
    exit 0
fi

# Keep one receipt validator: the queue publishes merge-queue-validated-tree-*,
# and the validator checks that its workflow completed successfully. The old
# PR marker could never be published after heavy jobs moved off PR events.
proof="$(mktemp)"
trap 'rm -f "$proof"' EXIT
validator="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/check-ci-merge-queue-validation.sh"
if ! GITHUB_OUTPUT="$proof" "$validator"; then
    echo '::warning::Queue proof lookup failed; running heavy validation.'
    exit 0
fi

if grep -qx 'run-fast-validation=false' "$proof" && grep -qx 'reuse-source=merge-queue' "$proof"; then
    sed -n -e '/^tree-hash=/p' -e '/^validating-run-id=/p' -e '/^prior-run-url=/p' "$proof" >>"$output"
    printf 'run-heavy=false\n' >>"$output"
fi
