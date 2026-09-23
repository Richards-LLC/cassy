#!/usr/bin/env bash
# Run the hub-web user-journey suite against the committed production bundle
# and lay out one receipt directory per journey for evaluation.
#
# Usage: journey-eval.sh <artifact-dir> [extra playwright args, e.g. --grep HUB-J4]
#
# Output:
#   <artifact-dir>/journeys/<ID>/{trace.zip,journey.webm,NN-<stage>.png,final.aria.yml,result.json}
#   <artifact-dir>/SUMMARY.md   tree hash, commit, per-journey result and stage timings
#   <artifact-dir>/playwright/  raw Playwright output and HTML report
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

tree="$(git -C "$repo" rev-parse HEAD:hub-web)"
commit="$(git -C "$repo" rev-parse HEAD)"
rm -rf "$artifacts/journeys" "$artifacts/playwright"
mkdir -p "$artifacts/journeys"

status=0
(cd "$hub" && JOURNEY_RECEIPTS="$artifacts/journeys" JOURNEY_OUTPUT="$artifacts/playwright" \
    npx playwright test --project=journeys "$@") || status=$?

python3 - "$artifacts" "$tree" "$commit" "$status" <<'EOF'
import json, sys
from pathlib import Path
artifacts, tree, commit, status = Path(sys.argv[1]), sys.argv[2], sys.argv[3], int(sys.argv[4])
rows = []
for result in sorted((artifacts / "journeys").glob("*/result.json")):
    data = json.loads(result.read_text())
    stages = data.get("stages", [])
    total = sum(s.get("ms", 0) for s in stages)
    slow = max(stages, key=lambda s: s.get("ms", 0)) if stages else {"title": "-", "ms": 0}
    rows.append((data["id"], data["title"], data["status"], total, slow))
lines = [
    f"# Journey suite run — hub-web {tree[:8]}", "",
    f"- hub_web_tree: {tree}",
    f"- evaluated_commit: {commit}",
    f"- suite_exit: {status}",
    f"- journeys: {len(rows)}, PASS {sum(1 for r in rows if r[2] == 'PASS')}",
    "- label: real-bundle, protocol-double", "",
    "| ID | Journey | Run | Total | Slowest stage |", "|---|---|---|---|---|",
]
for ident, title, run, total, slow in rows:
    lines.append(f"| {ident} | {title} | {run} | {total / 1000:.1f}s | {slow['title']} ({slow['ms'] / 1000:.1f}s) |")
(artifacts / "SUMMARY.md").write_text("\n".join(lines) + "\n")
print("\n".join(lines))
EOF
exit "$status"
