#!/usr/bin/env bash
# Run the hub-web user-journey suite against the committed production bundle
# and lay out one evidence bundle per journey for evaluation.
#
# Usage: journey-eval.sh <artifact-dir> [extra playwright args, e.g. --grep HUB-J4]
# <artifact-dir> is normally <artifacts_root>/<task-id>.
#
# Output (cas-qa-craft evidence bundle shape, producer "journey"):
#   <artifact-dir>/journeys/<ID>/  bundle.json, trace.zip, trace-actions.txt, receipt.webm,
#                                  J01.png…, final.aria.yml, final.aria.json, result.json
#   <artifact-dir>/journeys/JOURNEYS.md  dist tree, commit, per-journey result, stage timings
#   <artifact-dir>/playwright/           raw Playwright output
# Exit status is the suite's. See docs/qa/journey-evaluation.md.
set -euo pipefail

artifacts="${1:?usage: journey-eval.sh <artifact-dir> [playwright args...]}"
shift
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mkdir -p "$artifacts"
artifacts="$(cd "$artifacts" && pwd)"
hub="$repo/hub-web"

[[ -d "$hub/node_modules/@playwright/test" ]] || {
    printf 'journey-eval: run `npm ci` in %s first\n' "$hub" >&2
    exit 2
}
if ! git -C "$repo" diff --quiet -- hub-web/dist; then
    printf 'journey-eval: hub-web/dist has uncommitted changes; evaluate the committed bundle\n' >&2
    exit 2
fi

tree="$(git -C "$repo" rev-parse HEAD:hub-web/dist)"
commit="$(git -C "$repo" rev-parse HEAD)"
rm -rf "$artifacts/journeys" "$artifacts/playwright"
mkdir -p "$artifacts/journeys"

status=0
(cd "$hub" && JOURNEY_RECEIPTS="$artifacts/journeys" JOURNEY_OUTPUT="$artifacts/playwright" \
    npx playwright test --project=journeys "$@") || status=$?

pw_version="$(cd "$hub" && node -p 'require("@playwright/test/package.json").version')"
python3 "$repo/scripts/journey-bundles.py" "$artifacts" "$tree" "$commit" "$status" "$hub" "$pw_version"
exit "$status"
