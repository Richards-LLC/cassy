#!/usr/bin/env bash
# Static contract for the normal CI-authoritative release lane.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
release="$script_dir/release.sh"

help_output="$("$release" --help)"
grep -qF 'normal published release.' <<<"$help_output"
grep -qF -- '--publish-tag' <<<"$help_output"
grep -qF -- '--manual-publish' <<<"$help_output"
grep -qF -- '--acknowledge-workflow-conflict' <<<"$help_output"
echo 'ok   help distinguishes safe audit from explicit CI and manual publication'

set +e
missing_ack_output="$("$release" --publish-tag --manual-publish 2>&1)"
missing_ack_status=$?
set -e
test "$missing_ack_status" -eq 2
grep -qF -- '--manual-publish requires --acknowledge-workflow-conflict' <<<"$missing_ack_output"
echo 'ok   manual publishing cannot begin without conflict acknowledgement'

set +e
missing_publish_output="$("$release" --manual-publish --acknowledge-workflow-conflict 2>&1)"
missing_publish_status=$?
set -e
test "$missing_publish_status" -eq 2
grep -qF -- '--manual-publish requires --publish-tag' <<<"$missing_publish_output"
echo 'ok   manual publication cannot bypass explicit tag publication'

audit_guard_line="$(rg -nF 'if ! "$PUBLISH_TAG"; then' "$release" | tail -n1 | cut -d: -f1)"
push_line="$(rg -nF 'git push origin "$TAG"' "$release" | cut -d: -f1)"
manual_create_line="$(rg -nF 'gh release create "$TAG"' "$release" | cut -d: -f1)"
test -n "$audit_guard_line"
test -n "$push_line"
test -n "$manual_create_line"
test "$audit_guard_line" -lt "$push_line"
test "$push_line" -lt "$manual_create_line"
rg -qF 'dist/local-audit' "$release"
rg -qF 'never the shipped bytes' "$release"
echo 'ok   audit exits before the explicit tag push and manual publisher'
