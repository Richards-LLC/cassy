#!/usr/bin/env bash
# Probe the release report's PDF renderer the way `cas release report --pdf`
# runs it (cas-be3a). The renderer installs the current Playwright into a
# disposable workspace and launches headless Chromium, which needs that
# Playwright release's browser build in the host cache. When that build was
# missing, the report stage stopped after publication; this probe lets
# preflight catch it before anything publishes.
#
# Prints `playwright-version=<v>` and exits 0 when the browser launches.
set -euo pipefail

for tool in node npm; do
    command -v "$tool" >/dev/null 2>&1 || {
        printf 'report renderer: %s is not installed\n' "$tool" >&2
        exit 1
    }
done

work="$(mktemp -d "${TMPDIR:-/tmp}/cas-report-renderer.XXXXXX")"
trap 'rm -rf "$work"' EXIT

if ! (cd "$work" && npm install --no-save --no-audit --no-fund --silent playwright) \
    >"$work/npm.log" 2>&1; then
    cat "$work/npm.log" >&2
    printf 'report renderer: npm install playwright failed\n' >&2
    exit 1
fi

node - "$work/node_modules/playwright" <<'NODE'
const modulePath = process.argv[2];
const playwright = require(modulePath);
console.log(`playwright-version=${require(`${modulePath}/package.json`).version}`);
playwright.chromium
  .launch({ headless: true })
  .then(browser => browser.close())
  .catch(error => {
    console.error(error.message);
    process.exit(1);
  });
NODE
