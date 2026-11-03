#!/usr/bin/env bash
# cas-0c988, cas-fb11: rebuild committed generated output after a merge
# instead of hand-merging it.
#
# Usage: scripts/regenerate-generated-artifacts.sh <pre-merge-head> <merged-head>
# Run in the merged checkout (Cassy's merge does this; so can a supervisor after
# a manual `git merge`). .gitattributes marks the outputs `merge=cas-generated`,
# so the merge kept the target's copy without a conflict. This rebuilds each
# output whose inputs changed and commits it:
# - hub-web/dist from the merged hub-web sources;
# - cas-cli/src/builtins/reference-history.json (and the generated Codex
#   verifier projection) from the merged history, via
#   scripts/gen-builtin-reference-history.sh.
# Each step runs on its own: one failing step never leaves the other's output
# stale. A failure prints one summary line first with retained log paths
# (Cassy's merge warning quotes the first line), then the logs, and exits non-zero.
set -euo pipefail
old=${1:?pre-merge head}
new=${2:?merged head}

regenerate_hub_web_dist() {
  if git diff --quiet "$old" "$new" -- hub-web ':(exclude)hub-web/dist'; then
    return 0
  fi
  (
    cd hub-web
    # Detach legacy shared links so npm cannot change another checkout.
    if [ -L node_modules ]; then
      rm node_modules
    fi
    # npm's hidden lock omits the root package and uninstalled optional packages.
    # Compare package metadata, not file bytes; stale or unreadable locks reinstall.
    if ! node <<'JS'
const fs = require('node:fs');
const {isDeepStrictEqual} = require('node:util');
try {
  const wanted = JSON.parse(fs.readFileSync('package-lock.json')).packages;
  const installed = JSON.parse(fs.readFileSync('node_modules/.package-lock.json')).packages;
  for (const [path, entry] of Object.entries(wanted)) {
    if (!path || (!installed[path] && entry.optional)) continue;
    if (!isDeepStrictEqual(entry, installed[path]) || !fs.existsSync(path)) process.exit(1);
  }
  if (Object.keys(installed).some(path => !Object.hasOwn(wanted, path))) process.exit(1);
} catch {
  process.exit(1);
}
JS
    then
      npm ci --prefer-offline --no-audit --no-fund
    fi
    npm run build
  )
  if [ -n "$(git status --porcelain -- hub-web/dist)" ]; then
    git add -A hub-web/dist
    git commit -q \
      -m "build(hub-web): regenerate dist from merged sources" \
      -m "Rebuilt by scripts/regenerate-generated-artifacts.sh after merging ${new:0:9} (cas-0c988)."
    echo "regenerated hub-web/dist"
  fi
}

regenerate_reference_history() {
  local ledger=cas-cli/src/builtins/reference-history.json
  local -a outputs=("$ledger" cas-cli/src/builtins/codex/agents/task-verifier.toml .codex/agents/task-verifier.toml)
  # Only builtin content the ledger hashes (skill files in every flavour and the
  # task-verifier body and projections) or the generator itself can change it.
  # A full run walks all history (~30s), so other builtin changes skip it.
  local -a inputs=(':(glob)cas-cli/src/builtins/**/skills/**'
    ':(glob)cas-cli/src/builtins/**/agents/task-verifier.*' scripts/gen-builtin-reference-history.sh)
  if [ ! -x scripts/gen-builtin-reference-history.sh ]; then
    return 0
  fi
  # Endpoint trees can match after an edit is reverted. The ledger still
  # includes that revision. Walk every merged parent so TREESAME merge
  # simplification cannot hide a side branch's intermediate builtin edits.
  local changed
  changed=$(git log --full-history -m -1 --format=%H "$old..$new" -- "${inputs[@]}")
  if [ -z "$changed" ]; then
    return 0
  fi
  # The ledger hashes every committed version of each builtin file, so its own
  # commit can add one (a regenerated verifier projection). Only then does a
  # second pass fold that in; the committed ledger then equals a fresh run.
  local pass
  for pass in 1 2; do
    scripts/gen-builtin-reference-history.sh >/dev/null
    if [ -z "$(git status --porcelain --untracked-files=no -- "${outputs[@]}")" ]; then
      return 0
    fi
    git add -u -- "${outputs[@]}"
    if [ "$pass" = 1 ]; then
      git commit -q \
        -m "build(builtins): regenerate reference-history from merged history" \
        -m "Rebuilt by scripts/regenerate-generated-artifacts.sh after merging ${new:0:9} (cas-fb11)."
      echo "regenerated $ledger"
      if git diff --quiet HEAD^ HEAD -- "${inputs[@]}"; then
        return 0
      fi
    else
      git commit -q --amend --no-edit
    fi
  done
}

logs=$(mktemp -d)
steps=(hub-web/dist reference-history)
failed=()
failed_steps=()
trap 'if [ "${#failed[@]}" = 0 ]; then rm -rf "$logs"; fi' EXIT
for step in "${steps[@]}"; do
  log=$logs/${step//\//-}
  # errexit holds inside the subshell only outside a conditional context.
  set +e
  case $step in
    hub-web/dist) (set -e; regenerate_hub_web_dist) >"$log" 2>&1 ;;
    reference-history) (set -e; regenerate_reference_history) >"$log" 2>&1 ;;
  esac
  status=$?
  set -e
  if [ "$status" != 0 ]; then
    failed+=("$step failed (exit $status, log: $log)")
    failed_steps+=("$step")
  else
    cat "$log"
  fi
done
if [ "${#failed[@]}" != 0 ]; then
  summary=$(IFS=';'; echo "${failed[*]}")
  echo "regenerate-generated-artifacts: ${summary//;/; }" >&2
  for step in "${failed_steps[@]}"; do
    log=$logs/${step//\//-}
    if [ -s "$log" ]; then
      echo "--- $step" >&2
      cat "$log" >&2
    fi
  done
  exit 1
fi
