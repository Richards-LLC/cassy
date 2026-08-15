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

audit_guard_line="$(grep -nF 'if ! "$PUBLISH_TAG"; then' "$release" | tail -n1 | cut -d: -f1)"
push_line="$(grep -nF 'git push origin "$TAG"' "$release" | cut -d: -f1)"
manual_create_line="$(grep -nF 'gh release create "$TAG"' "$release" | cut -d: -f1)"
test -n "$audit_guard_line"
test -n "$push_line"
test -n "$manual_create_line"
test "$audit_guard_line" -lt "$push_line"
test "$push_line" -lt "$manual_create_line"
grep -qF 'dist/local-audit' "$release"
grep -qF 'never the shipped bytes' "$release"
echo 'ok   audit exits before the explicit tag push and manual publisher'

# The pre-push preflight runs before `git push origin "$TAG"`, so it must use
# the local lane; the CI-side remote tag re-fetch cannot pass before the push.
preflight_line="$(grep -nF 'check-release-preflight.sh' "$release" | cut -d: -f1)"
test -n "$preflight_line"
test "$preflight_line" -lt "$push_line"
grep -qF 'check-release-preflight.sh --local "$TAG"' "$release"
echo 'ok   pre-push preflight uses the local lane instead of the CI remote-tag lane'

# CI keeps the full lane, including the remote tag re-fetch.
release_workflow="$(cd "$script_dir/.." && pwd)/.github/workflows/release.yml"
grep -qE 'check-release-preflight\.sh "\$\{GITHUB_REF_NAME\}"' "$release_workflow"
! grep -qF 'check-release-preflight.sh --local' "$release_workflow"
echo 'ok   CI release workflow keeps the remote-tag preflight lane'
