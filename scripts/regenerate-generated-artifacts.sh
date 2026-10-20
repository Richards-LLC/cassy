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
set -euo pipefail
old=${1:?pre-merge head}
new=${2:?merged head}

regenerate_hub_web_dist() {
  if git diff --quiet "$old" "$new" -- hub-web ':(exclude)hub-web/dist'; then
    return 0
  fi
  (
    cd hub-web
    # A merge may run in an ephemeral worktree with no node_modules: reuse the
    # main checkout's when its lockfile matches, otherwise install from the lockfile.
    if [ ! -e node_modules ]; then
      main=$(git worktree list --porcelain | sed -n '1s/^worktree //p')
      if [ -n "$main" ] && [ -d "$main/hub-web/node_modules" ] \
        && cmp -s package-lock.json "$main/hub-web/package-lock.json"; then
        ln -s "$main/hub-web/node_modules" node_modules
      else
        npm ci --prefer-offline --no-audit --no-fund >/dev/null
      fi
    fi
    npm run build >/dev/null
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
  if [ ! -x scripts/gen-builtin-reference-history.sh ] \
    || git diff --quiet "$old" "$new" -- cas-cli/src/builtins ":(exclude)$ledger"; then
    return 0
  fi
  # The ledger hashes every committed version of each builtin file, so its own
  # commit can add one (a regenerated verifier projection). A second pass
  # folds that in; the committed ledger then equals a fresh run.
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
    else
      git commit -q --amend --no-edit
    fi
  done
}

regenerate_hub_web_dist
regenerate_reference_history
