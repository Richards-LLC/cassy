#!/usr/bin/env bash
# cas-4ce5 (independent QA): prove the "Upload Commander journey evidence on
# failure" step in .github/workflows/ci.yml really uploads files from the
# hidden hub-web/e2e/.results directory, not just that its YAML looks right.
#
# actions/upload-artifact@v4 resolves `path` with @actions/glob and passes
# `excludeHiddenFiles: !include-hidden-files`. This runs that same library,
# with the step's own `path` and `include-hidden-files`, over a fixture tree
# shaped like a failed Playwright run. A startup failure has native reports
# but no recovery receipt: retain those and hidden traces in one upload.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# @actions/glob is a locked devDependency of hub-web (the line actions/
# upload-artifact@v4 depends on), installed by the web step's normal npm ci.
# Nothing is fetched here; without that install the test fails, never skips.
glob_module="hub-web/node_modules/@actions/glob/package.json"
if [[ ! -f "$glob_module" ]]; then
  echo "test-ci-journey-evidence-upload: $glob_module is missing; run npm ci in hub-web first" >&2
  exit 1
fi

step_json="$(python3 - <<'PY'
import json, yaml
workflow = yaml.safe_load(open(".github/workflows/ci.yml"))
uploads = [step for step in workflow["jobs"]["fast-validation-journeys"]["steps"]
           if step.get("uses", "").startswith("actions/upload-artifact@")]
assert len(uploads) == 1, f"journey evidence must upload once per shard, found {len(uploads)} uploads"
step = uploads[0]
assert step.get("if") == "always()", "preserve evidence even when startup fails before a recovery receipt"
inputs = step["with"]
assert inputs["name"] == "queue-flakes-playwright-${{ matrix.shard }}", "retain the queue fan-in artifact contract"
print(json.dumps({"path": str(inputs["path"]).replace("${{ matrix.shard }}", "3").strip(),
                  "includeHidden": inputs.get("include-hidden-files") is True}))
PY
)"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/tree/hub-web/e2e/.results/reconnect.journey.ts-HUB-J11-journeys"
printf 'PK' > "$work/tree/hub-web/e2e/.results/reconnect.journey.ts-HUB-J11-journeys/trace.zip"
printf '# Instructions\n' > "$work/tree/hub-web/e2e/.results/reconnect.journey.ts-HUB-J11-journeys/error-context.md"
mkdir -p "$work/tree/target/queue-flakes/playwright-3/initial/playwright/.diagnostics"
printf '{"errors":["webServer startup"]}\n' > "$work/tree/target/queue-flakes/playwright-3/initial/report.json"
printf 'webServer startup failed\n' > "$work/tree/target/queue-flakes/playwright-3/initial/console.log"
printf 'PK' > "$work/tree/target/queue-flakes/playwright-3/initial/playwright/.diagnostics/trace.zip"

probe="hub-web/e2e/.ci-upload-probe.mjs"   # inside hub-web so the import resolves its node_modules
trap 'rm -rf "$work" "$repo_root/$probe"' EXIT
cat > "$probe" <<'JS'
import { create } from "@actions/glob";
const [pattern, includeHidden] = [process.argv[2], process.argv[3] === "true"];
const files = async (exclude) => (await (await create(pattern, { excludeHiddenFiles: exclude })).glob());
const asConfigured = await files(!includeHidden);
const byDefault = await files(true);
console.log(JSON.stringify({ asConfigured, byDefault }));
JS

pattern="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["path"])' "$step_json")"
include_hidden="$(python3 -c 'import json,sys; print(str(json.loads(sys.argv[1])["includeHidden"]).lower())' "$step_json")"
result="$(cd "$work/tree" && node "$repo_root/$probe" "$pattern" "$include_hidden" | tail -n 1)"   # @actions/core prints ::debug:: lines first

python3 - "$result" <<'PY'
import json, sys
counts = json.loads(sys.argv[1])
from pathlib import Path
matched = [Path(path) for path in counts["asConfigured"] if Path(path).is_file()]
required = {"initial/report.json", "initial/console.log", ".diagnostics/trace.zip",
            "reconnect.journey.ts-HUB-J11-journeys/trace.zip",
            "reconnect.journey.ts-HUB-J11-journeys/error-context.md"}
missing = [name for name in required if not any(path.as_posix().endswith(name) for path in matched)]
assert not missing, f"single journey evidence upload loses startup diagnostics/hidden traces: {missing}"
assert not any("/.results/" in path or "/.diagnostics/" in path for path in counts["byDefault"]), "control must exclude hidden evidence"
print(f"journey evidence upload: {len(matched)} evidence files retained in one artifact without a recovery receipt")
PY
