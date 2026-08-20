#!/usr/bin/env bash
# Re-assert, at execution time on the machine itself, that a release lane which
# landed on the persistent self-hosted runner is genuinely trusted traffic
# (cas-6981 posture, cas-3b7c0 release routing).
#
# The workflow `if:` expression already gates runner ASSIGNMENT. This is the
# second, independent control that runs after assignment, so a future edit to
# an expression cannot silently hand the box to untrusted input. It also
# refuses to build inside a host layout where the runner's caches are not
# isolated from the operator's factory worktrees.
set -euo pipefail

fail() {
    echo "error: $1" >&2
    exit 1
}

test "${GITHUB_EVENT_NAME:?}" = push \
    || fail "release lane on the isolated runner requires a push event; got $GITHUB_EVENT_NAME"
test "${GITHUB_REPOSITORY:?}" = Richards-LLC/cassy \
    || fail "release lane pinned to the canonical repository; got $GITHUB_REPOSITORY"
test "${SELF_HOSTED_ENABLED:-}" = enabled \
    || fail "release self-hosted routing is not explicitly enabled"

# Only the two refs a release can legitimately travel on: the release-prep
# commit on main (prebuild) and the annotated release tag (publication).
case "${GITHUB_REF:?}" in
    refs/heads/main | refs/tags/v*) ;;
    *) fail "ref is outside the trusted release set: $GITHUB_REF" ;;
esac

"$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/check-cassy-actions-runner-isolation.sh"

echo "release runner trust contract satisfied: ref=$GITHUB_REF target=$CARGO_TARGET_DIR"
