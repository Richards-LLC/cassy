#!/usr/bin/env bash
# cas-4ce5 (independent QA): prove the "Upload Commander journey evidence on
# failure" step in .github/workflows/ci.yml really uploads files from the
# hidden hub-web/e2e/.results directory, not just that its YAML looks right.
#
# actions/upload-artifact@v4 resolves `path` with @actions/glob and passes
# `excludeHiddenFiles: !include-hidden-files`. This runs that same library,
# with the step's own `path` and `include-hidden-files`, over a fixture tree
# shaped like a failed Playwright run, and requires at least the trace and
# error context to be matched. It also checks the default (hidden files
# excluded) matches nothing, so the test can tell the two apart.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

GLOB_VERSION="0.5.0"   # the @actions/glob line actions/upload-artifact@v4 depends on

step_json="$(python3 - <<'PY'
import json, yaml
workflow = yaml.safe_load(open(".github/workflows/ci.yml"))
for job in workflow["jobs"].values():
    for step in job.get("steps", []):
        uses = step.get("uses", "")
        inputs = step.get("with") or {}
        if uses.startswith("actions/upload-artifact@") and str(inputs.get("path", "")).strip().startswith("hub-web/e2e/.results"):
            print(json.dumps({"path": str(inputs["path"]).strip(), "includeHidden": inputs.get("include-hidden-files") is True}))
            raise SystemExit(0)
raise SystemExit("no upload-artifact step uploads hub-web/e2e/.results")
PY
)"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/tree/hub-web/e2e/.results/reconnect.journey.ts-HUB-J11-journeys" "$work/glob"
printf 'PK' > "$work/tree/hub-web/e2e/.results/reconnect.journey.ts-HUB-J11-journeys/trace.zip"
printf '# Instructions\n' > "$work/tree/hub-web/e2e/.results/reconnect.journey.ts-HUB-J11-journeys/error-context.md"

(cd "$work/glob" && npm init -y >/dev/null && npm install --no-audit --no-fund --silent "@actions/glob@${GLOB_VERSION}" >/dev/null)

cat > "$work/glob/probe.mjs" <<'JS'
import { create } from "@actions/glob";
const [pattern, includeHidden] = [process.argv[2], process.argv[3] === "true"];
const files = async (exclude) => (await (await create(pattern, { excludeHiddenFiles: exclude })).glob());
const asConfigured = await files(!includeHidden);
const byDefault = await files(true);
console.log(JSON.stringify({ asConfigured: asConfigured.length, byDefault: byDefault.length }));
JS

pattern="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["path"])' "$step_json")"
include_hidden="$(python3 -c 'import json,sys; print(str(json.loads(sys.argv[1])["includeHidden"]).lower())' "$step_json")"
result="$(cd "$work/tree" && NODE_PATH="$work/glob/node_modules" node "$work/glob/probe.mjs" "$pattern" "$include_hidden" | tail -n 1)"   # @actions/core prints ::debug:: lines first

python3 - "$result" <<'PY'
import json, sys
counts = json.loads(sys.argv[1])
if counts["byDefault"] != 0:
    raise SystemExit(f"fixture is not hidden-only: the default glob matched {counts['byDefault']} files")
if counts["asConfigured"] < 2:
    raise SystemExit(f"the journey-evidence upload step matches {counts['asConfigured']} files under hub-web/e2e/.results; set include-hidden-files: true")
print(f"journey evidence upload: {counts['asConfigured']} files matched as configured, 0 with hidden files excluded")
PY
