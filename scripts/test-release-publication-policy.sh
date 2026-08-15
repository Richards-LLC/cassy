#!/usr/bin/env bash
# Static contract for the normal CI-authoritative release lane.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
release="$script_dir/release.sh"

help_output="$("$release" --help)"
grep -qF 'GitHub Release workflow creates the normal published release.' <<<"$help_output"
grep -qF -- '--manual-publish' <<<"$help_output"
grep -qF -- '--acknowledge-workflow-conflict' <<<"$help_output"
echo 'ok   help distinguishes CI authority from explicit manual failover'

set +e
missing_ack_output="$("$release" --manual-publish 2>&1)"
missing_ack_status=$?
set -e
test "$missing_ack_status" -eq 2
grep -qF -- '--manual-publish requires --acknowledge-workflow-conflict' <<<"$missing_ack_output"
echo 'ok   manual publishing cannot begin without conflict acknowledgement'

push_line="$(rg -nF 'git push origin "$TAG"' "$release" | cut -d: -f1)"
manual_create_line="$(rg -nF 'gh release create "$TAG"' "$release" | cut -d: -f1)"
test -n "$push_line"
test -n "$manual_create_line"
test "$push_line" -lt "$manual_create_line"
rg -qF 'dist/local-audit' "$release"
rg -qF 'never the shipped bytes' "$release"
echo 'ok   tag starts CI before the exceptional competing manual publisher'
