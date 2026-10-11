#!/usr/bin/env bash
# Affected journeys, four workers. No arguments resolve the active task and
# its artifact directory; an explicit task artifact directory is recommended.
# Usage: journey-eval.sh [artifact-dir] [--affected <base>|--full] [--workers=1..4] [--task <id>]
# --full is reserved for the supervisor/CI at epic assembly and merge queue.
# Caller spec/grep filters are refused: the producer owns the exact selection.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
plan="$(python3 "$repo/scripts/journey-receipt.py" plan --repo "$repo" "$@")"
# A clean evaluated input set is necessary for an exact-tip receipt.
if [[ -n "$(git -C "$repo" status --porcelain -- hub-web docs/qa/journeys.md scripts/journey-eval.sh scripts/journey-receipt.py scripts/journey-bundles.py scripts/journeys-for-diff.py)" ]]; then
    printf 'journey-eval: commit product/catalog/runner inputs before recording an exact-tip receipt\n' >&2
    exit 2
fi
mapfile -t fields < <(python3 -c 'import json,sys; p=json.load(sys.stdin); print(p["artifacts"]); print(p["head_sha"]); print(p["workers"]); print(p["scope"]); print(p["grep"] or "")' <<<"$plan")
artifacts="${fields[0]}"
commit="${fields[1]}"
workers="${fields[2]}"
scope="${fields[3]}"
pattern="${fields[4]}"
hub="$repo/hub-web"
mkdir -p "$artifacts"
# Keep the original failure/control evidence on repeat runs. Each archive
# retains the same relative layout, including receipt/report and bundle refs.
python3 - "$artifacts" <<'PY'
import datetime, uuid, sys
from pathlib import Path
root = Path(sys.argv[1])
names = ('journeys', 'playwright', 'journey-receipt.json', 'journey-selection.json')
existing = [root / name for name in names if (root / name).exists()]
if existing:
    archive = root / 'journey-runs' / (datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ') + '-' + uuid.uuid4().hex[:8])
    archive.mkdir(parents=True)
    for path in existing:
        path.rename(archive / path.name)
PY
# cas-bb5e: a full run reuses the factory daemon's background evaluation of
# the same hub-web/dist tree, journey sources, catalog and runner. Set
# JOURNEY_EVAL_FRESH=1 to run the browsers anyway.
if [[ "$scope" == full ]] && [[ -f "$repo/scripts/journey-background.py" ]] \
    && python3 "$repo/scripts/journey-background.py" reuse --repo "$repo" --artifacts "$artifacts"; then
    exit 0
fi
printf '%s\n' "$plan" > "$artifacts/journey-selection.json"
mkdir -p "$artifacts/journeys" "$artifacts/playwright"
status=0
if [[ -n "$pattern" ]]; then
    [[ -d "$hub/node_modules/@playwright/test" ]] || {
        printf 'journey-eval: run `npm ci` in %s first\n' "$hub" >&2
        exit 2
    }
    tree="$(git -C "$repo" rev-parse HEAD:hub-web/dist)"
    pw_version="$(cd "$hub" && node -p 'require("@playwright/test/package.json").version')"
    args=("--workers=$workers")
    if [[ "$scope" == affected ]]; then args+=(--grep "$pattern"); fi
    (cd "$hub" && JOURNEY_RECEIPTS="$artifacts/journeys" JOURNEY_OUTPUT="$artifacts/playwright" \
        npm run journeys -- "${args[@]}") || status=$?
else
    printf 'journey-eval: no affected journeys; no browser run\n'
    pw_version="$(python3 - "$hub/package.json" <<'PY'
import json, sys
print('playwright ' + json.load(open(sys.argv[1]))['devDependencies']['@playwright/test'] + ' (not run: zero impact)')
PY
)"
fi
receipt_status=0
python3 "$repo/scripts/journey-receipt.py" write --plan "$artifacts/journey-selection.json" \
    --report "$artifacts/playwright/report.json" --suite-exit "$status" --tool-version "$pw_version" \
    --output "$artifacts/journey-receipt.json" || receipt_status=$?
if [[ -n "$pattern" ]]; then
    bundle_status=0
    python3 "$repo/scripts/journey-bundles.py" "$artifacts" "$tree" "$commit" "$status" "$hub" "$pw_version" || bundle_status=$?
    if (( status == 0 && receipt_status == 0 )); then receipt_status=$bundle_status; fi
fi
# Preserve the native exit in the receipt even if receipt/bundle validation
# fails separately. A zero native exit without selected results still refuses.
if (( status != 0 )); then exit "$status"; fi
exit "$receipt_status"
